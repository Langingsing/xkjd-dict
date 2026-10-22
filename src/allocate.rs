//! One guarded plan for keeping one phrase's code and extending another's.
use crate::{
    document::{Document, Entry},
    edit::{EditPlan, dictionary_path},
    rules::{self, Encoder, Encoding, Profile, Suggestion},
    workspace::Workspace,
};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

#[derive(Debug, Default)]
pub struct AllocationRequest {
    pub keep: String,
    pub extend: String,
    pub code: Option<String>,
    pub file: Option<PathBuf>,
    pub line: Option<usize>,
    pub full_code: Option<String>,
    pub character_codes: Vec<String>,
    pub all: bool,
}

#[derive(Debug, Default)]
pub struct PromotionRequest {
    pub text: String,
    pub code: String,
    /// Select one existing target code when the word has aliases.
    pub from_code: Option<String>,
    pub full_code: Option<String>,
    pub owner_full_code: Option<String>,
    pub character_codes: Vec<String>,
    pub all: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ready,
    Blocked,
    Applied,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    InvalidRequest,
    IncompleteWorkspace,
    RecordSelection,
    SharedCodeSelection,
    OtherCollision,
    UnsupportedDictionary,
    MissingReference,
    AmbiguousEncoding,
    TruncatedEncoding,
    InvalidFullCode,
    InvalidPrefix,
    NoFreeCode,
    Ordering,
    AtomicFile,
}

#[derive(Debug, Serialize)]
pub struct Issue {
    pub reason: Reason,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct AllocationReport {
    pub status: Status,
    pub keep: String,
    pub extend: String,
    pub shared_code: Option<String>,
    pub shared_candidates: Vec<String>,
    pub retained: Vec<Entry>,
    pub moving_candidates: Vec<Entry>,
    pub suggestion: Option<Suggestion>,
    pub full_code: Option<String>,
    pub assigned_code: Option<String>,
    pub file: Option<PathBuf>,
    pub diff: String,
    pub issue: Option<Issue>,
    pub promoted_from: Option<String>,
    pub promoted_full_code: Option<String>,
    pub promotion_encoding: Option<Encoding>,
}

#[derive(Debug)]
struct Snapshot {
    root: PathBuf,
    dictionaries: Vec<PathBuf>,
    fingerprints: BTreeMap<PathBuf, [u8; 32]>,
}

impl Snapshot {
    fn capture(ws: &Workspace) -> Self {
        let mut fingerprints: BTreeMap<_, _> = ws
            .documents
            .iter()
            .map(|doc| (doc.path.clone(), Sha256::digest(doc.raw.as_bytes()).into()))
            .collect();
        fingerprints.insert(
            ws.schema_path.clone(),
            Sha256::digest(ws.schema_text.as_bytes()).into(),
        );
        Self {
            root: ws.root.clone(),
            dictionaries: ws.documents.iter().map(|doc| doc.path.clone()).collect(),
            fingerprints,
        }
    }

    fn verify(&self) -> Result<()> {
        let mut paths = Vec::new();
        for item in fs::read_dir(&self.root)? {
            let path = item?.path();
            if path.is_file()
                && path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(".dict.yaml"))
            {
                paths.push(path);
            }
        }
        paths.sort();
        if paths != self.dictionaries {
            bail!(
                "词库文件集合已变化，拒绝使用旧分配计划：{}",
                self.root.display()
            );
        }
        for (path, expected) in &self.fingerprints {
            let bytes = fs::read(path).with_context(|| {
                format!("复核分配输入 {}：文件不可读取或已变化", path.display())
            })?;
            let actual: [u8; 32] = Sha256::digest(bytes).into();
            if &actual != expected {
                bail!("文件已变化，拒绝使用旧分配计划：{}", path.display());
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct AllocationPlan {
    pub report: AllocationReport,
    edit: EditPlan,
    snapshot: Snapshot,
}

impl AllocationPlan {
    pub fn apply(&mut self) -> Result<()> {
        self.snapshot.verify()?;
        self.edit.apply()?;
        if fs::read_to_string(&self.edit.path)? != self.edit.updated {
            bail!("写入后内容验证失败：{}", self.edit.path.display());
        }
        self.report.status = Status::Applied;
        Ok(())
    }
}

#[derive(Debug)]
pub enum AllocationDecision {
    Ready(AllocationPlan),
    Blocked(AllocationReport),
}

impl AllocationReport {
    fn blocked(mut self, reason: Reason, message: impl Into<String>) -> AllocationDecision {
        self.status = Status::Blocked;
        self.issue = Some(Issue {
            reason,
            message: message.into(),
        });
        AllocationDecision::Blocked(self)
    }
}

pub fn plan(ws: &Workspace, request: &AllocationRequest) -> Result<AllocationDecision> {
    plan_inner(ws, request, None)
}

/// The old owner is discovered from the exact short code; it is never guessed.
pub fn promote(ws: &Workspace, request: &PromotionRequest) -> Result<AllocationDecision> {
    let allocation = AllocationRequest {
        keep: request.text.clone(),
        code: Some(request.code.clone()),
        full_code: request.owner_full_code.clone(),
        character_codes: request.character_codes.clone(),
        all: request.all,
        ..Default::default()
    };
    plan_inner(ws, &allocation, Some(request))
}

fn plan_inner(
    ws: &Workspace,
    request: &AllocationRequest,
    promotion: Option<&PromotionRequest>,
) -> Result<AllocationDecision> {
    let mut report = AllocationReport {
        status: Status::Ready,
        keep: request.keep.clone(),
        extend: request.extend.clone(),
        shared_code: request.code.clone(),
        shared_candidates: vec![],
        retained: vec![],
        moving_candidates: vec![],
        suggestion: None,
        full_code: None,
        assigned_code: None,
        file: None,
        diff: String::new(),
        issue: None,
        promoted_from: None,
        promoted_full_code: None,
        promotion_encoding: None,
    };
    let owner: String;
    let extend = if let Some(promotion) = promotion {
        let owners: BTreeSet<_> = ws
            .query(None, Some(&promotion.code), None, request.all)
            .into_iter()
            .filter(|entry| entry.text != promotion.text)
            .map(|entry| entry.text.clone())
            .collect();
        if owners.len() != 1 {
            return Ok(report.blocked(
                Reason::RecordSelection,
                format!(
                    "目标短码没有唯一原占词组（候选 {:?}）；已在该码、空码或多个占词均不自动调换",
                    owners
                ),
            ));
        }
        owner = owners.into_iter().next().unwrap();
        owner.as_str()
    } else {
        request.extend.as_str()
    };
    report.extend = extend.into();
    if request.keep == extend
        || [request.keep.as_str(), extend]
            .iter()
            .any(|text| text.chars().count() < 2 || !text.chars().all(rules::is_han))
        || (request.line.is_some() && request.file.is_none())
    {
        return Ok(report.blocked(
            Reason::InvalidRequest,
            "必须指定两个不同的纯汉字词组；--line 需要同时指定 --file",
        ));
    }
    if ws
        .import_issues
        .iter()
        .any(|issue| request.all || issue.active)
        || ws.documents.iter().any(|doc| {
            (request.all
                || ws.active_files.contains(&doc.path)
                || rules::is_character_reference(&doc.path))
                && (doc.header_error.is_some()
                    || !doc.malformed_lines.is_empty()
                    || doc.validate_columns().is_err())
        })
    {
        return Ok(report.blocked(
            Reason::IncompleteWorkspace,
            "导入关系或所选词库存在格式问题，无法可信判断码位占用",
        ));
    }
    let file = request
        .file
        .as_ref()
        .map(|path| dictionary_path(&ws.root, path))
        .transpose()?;
    report.retained = ws
        .query(Some(&request.keep), None, None, request.all)
        .into_iter()
        .cloned()
        .collect();
    report.moving_candidates = ws
        .query(Some(extend), None, None, request.all)
        .into_iter()
        .filter(|entry| {
            file.as_ref().is_none_or(|path| *path == entry.file)
                && request.line.is_none_or(|line| entry.line == line)
        })
        .cloned()
        .collect();
    if report.retained.is_empty() || report.moving_candidates.is_empty() {
        return Ok(report.blocked(
            Reason::RecordSelection,
            "未找到保留词组或符合 --file/--line 的后者记录；只处理已存在的词条",
        ));
    }
    let keep_codes: BTreeSet<_> = report.retained.iter().map(|e| e.code.clone()).collect();
    report.shared_candidates = report
        .moving_candidates
        .iter()
        .map(|e| e.code.clone())
        .filter(|code| keep_codes.contains(code))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let code = if let Some(promotion) = promotion {
        if report
            .retained
            .iter()
            .any(|entry| entry.code == promotion.code)
        {
            return Ok(report.blocked(Reason::InvalidRequest, "目标词组已拥有此码；若要清理既有同码，请使用 allocate 保留现有码，避免新增重复记录"));
        }
        report.retained.retain(|entry| {
            promotion
                .from_code
                .as_ref()
                .is_none_or(|code| entry.code == *code)
        });
        if report.retained.len() != 1 {
            return Ok(report.blocked(
                Reason::RecordSelection,
                "目标词组存在多条记录；用 --from-code 选择，重复的同码来源需先处理",
            ));
        }
        promotion.code.clone()
    } else {
        match &request.code {
            Some(code) if report.shared_candidates.contains(code) => code.clone(),
            None if report.shared_candidates.len() == 1 => report.shared_candidates[0].clone(),
            _ => return Ok(report.blocked(
                Reason::SharedCodeSelection,
                "没有唯一共享编码；请用 --code 指定双方现有码，必要时用 --file/--line 选择后者记录",
            )),
        }
    };
    report.shared_code = Some(code.clone());
    if promotion.is_none() {
        report.retained.retain(|entry| entry.code == code);
    }
    report.moving_candidates.retain(|entry| entry.code == code);
    if report.moving_candidates.len() != 1 {
        return Ok(report.blocked(
            Reason::RecordSelection,
            "后者存在多条同码记录；请用 --file/--line 唯一指定，不自动修改其他记录",
        ));
    }
    let moving = &report.moving_candidates[0];
    if rules::profile(&moving.file) != Profile::Standard {
        return Ok(report.blocked(
            Reason::UnsupportedDictionary,
            "只支持普通键道词组库；特殊规则词库不自动分配",
        ));
    }
    if ws
        .query(None, Some(&code), None, request.all)
        .iter()
        .any(|entry| {
            entry.text != request.keep && (entry.file != moving.file || entry.line != moving.line)
        })
    {
        return Ok(report.blocked(
            Reason::OtherCollision,
            "原码还有其他词条占用，拒绝隐式处理第三方冲突",
        ));
    }
    dictionary_path(&ws.root, &moving.file)?;
    let encoder = Encoder::from_workspace(ws).with_overrides(&request.character_codes)?;
    if let Some(promotion) = promotion {
        let target = &report.retained[0];
        if target.file != moving.file {
            return Ok(report.blocked(
                Reason::AtomicFile,
                "两个词组不在同一文件，拒绝可能只写入一半的跨文件让码",
            ));
        }
        if rules::profile(&target.file) != Profile::Standard {
            return Ok(report.blocked(Reason::UnsupportedDictionary, "目标词组不属于普通音形词库"));
        }
        report.promotion_encoding = Some(encoder.encode(&request.keep, Profile::Standard));
        let full = match choose_full_code(
            report.promotion_encoding.as_ref().unwrap(),
            promotion.full_code.as_deref(),
        ) {
            Ok(full) => full,
            Err(issue) => {
                return Ok(report.blocked(issue.reason, format!("目标词组：{}", issue.message)));
            }
        };
        if code.len() < rules::minimum_prefix(&request.keep, Profile::Standard)
            || !full.starts_with(&code)
            || !full.starts_with(&target.code)
            || code.len() >= target.code.len()
        {
            return Ok(report.blocked(
                Reason::InvalidPrefix,
                "目标短码须为合法全码前缀，且短于目标现有码；不自动改动助记码",
            ));
        }
        report.promoted_from = Some(target.code.clone());
        report.promoted_full_code = Some(full);
    }
    report.suggestion = Some(rules::suggest(
        ws,
        &encoder,
        extend,
        Profile::Standard,
        request.all,
    ));
    let suggestion = report.suggestion.as_ref().unwrap();
    let full_code = match choose_full_code(&suggestion.encoding, request.full_code.as_deref()) {
        Ok(full) => full,
        Err(issue) => return Ok(report.blocked(issue.reason, issue.message)),
    };
    report.full_code = Some(full_code.clone());
    if !full_code.starts_with(&code)
        || code.len() < rules::minimum_prefix(extend, Profile::Standard)
    {
        return Ok(report.blocked(
            Reason::InvalidPrefix,
            "后者现有码不是所选全码的合法简码前缀",
        ));
    }
    let choice = suggestion
        .choices
        .iter()
        .find(|choice| choice.full_code == full_code)
        .unwrap();
    let Some(slot) = choice.slots.iter().find(|slot| {
        slot.code.len() > code.len()
            && slot.same_code.iter().all(|entry| {
                promotion.is_some()
                    && entry.file == report.retained[0].file
                    && entry.line == report.retained[0].line
            })
    }) else {
        return Ok(report.blocked(
            Reason::NoFreeCode,
            "从现有码延长至全码均被占用，没有空闲合法码；请显式选择另一合法全码或单字参考",
        ));
    };
    let new_code = slot.code.clone();
    let moving = &report.moving_candidates[0];
    let doc = ws
        .documents
        .iter()
        .find(|doc| doc.path == moving.file)
        .unwrap();
    let mut changes = vec![(moving.line, new_code.as_str())];
    if promotion.is_some() {
        changes.push((report.retained[0].line, code.as_str()));
    }
    let edit = match EditPlan::update_codes_ordered(doc, &changes) {
        Ok(edit) => edit,
        Err(error) => return Ok(report.blocked(Reason::Ordering, error.to_string())),
    };
    let after = Document::parse(doc.path.clone(), edit.updated.clone());
    let mut expected: Vec<_> = doc
        .entries
        .iter()
        .map(|entry| {
            let mut fields = entry.fields.clone();
            if let Some((_, code)) = changes.iter().find(|(line, _)| *line == entry.line) {
                fields[doc.columns().1] = (*code).into();
            }
            fields
        })
        .collect();
    let mut actual: Vec<_> = after
        .entries
        .iter()
        .map(|entry| entry.fields.clone())
        .collect();
    expected.sort();
    actual.sort();
    if expected != actual || !after.malformed_lines.is_empty() {
        bail!("分配计划验证失败：存在预期外的记录变化");
    }
    report.file = Some(doc.path.clone());
    report.assigned_code = Some(new_code);
    report.diff = edit.diff();
    let snapshot = Snapshot::capture(ws);
    snapshot.verify()?;
    Ok(AllocationDecision::Ready(AllocationPlan {
        report,
        edit,
        snapshot,
    }))
}

fn choose_full_code(
    encoding: &Encoding,
    explicit: Option<&str>,
) -> std::result::Result<String, Issue> {
    let issue = |reason, message: &str| Issue {
        reason,
        message: message.into(),
    };
    if !encoding.missing.is_empty() || !encoding.issues.is_empty() {
        return Err(issue(
            Reason::MissingReference,
            "单字参考缺失或规则不适用；用 --char-code 字=完整编码 显式补充",
        ));
    }
    if encoding.truncated {
        return Err(issue(
            Reason::TruncatedEncoding,
            "取码候选被截断；请用 --char-code 明确单字编码",
        ));
    }
    match explicit {
        Some(full) if encoding.candidates.iter().any(|code| code == full) => Ok(full.into()),
        Some(_) => Err(issue(
            Reason::InvalidFullCode,
            "指定全码不属于合法候选；核对或用 --char-code 明确单字参考",
        )),
        None if encoding.candidates.len() == 1 && encoding.ambiguous.is_empty() => {
            Ok(encoding.candidates[0].clone())
        }
        _ => Err(issue(
            Reason::AmbiguousEncoding,
            "读音、飞键或取码有歧义；请显式选择全码，或用 --char-code 明确单字编码",
        )),
    }
}
