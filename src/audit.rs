use crate::{
    document::Entry,
    rules::{Encoder, Profile, is_han, is_shape, is_sound, matches_encoding, profile},
    workspace::Workspace,
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Review,
    Info,
}

#[derive(Debug, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub rule: String,
    pub file: PathBuf,
    pub line: usize,
    pub text: Option<String>,
    pub actual: Option<String>,
    pub expected: Vec<String>,
    pub reason: String,
}

#[derive(Default, Debug, Serialize)]
pub struct Coverage {
    pub files: usize,
    pub records: usize,
    pub matched: usize,
    pub errors: usize,
    pub review: usize,
    pub not_applicable: usize,
}

#[derive(Debug, Serialize)]
pub struct FileCoverage {
    pub file: PathBuf,
    pub active: bool,
    pub profile: Profile,
    pub records: usize,
}

#[derive(Debug, Serialize)]
pub struct NodeAnalysis {
    pub code: String,
    pub words: Vec<String>,
    pub entries: Vec<Entry>,
    pub descendant_entries: usize,
    pub active_entries: usize,
}

#[derive(Debug, Serialize)]
pub struct AuditReport {
    pub character_coverage: crate::coverage::PhraseCoverage,
    pub root: PathBuf,
    pub schema_dictionary: String,
    pub all: bool,
    pub files: Vec<FileCoverage>,
    pub records: usize,
    pub coverage: BTreeMap<Profile, Coverage>,
    pub diagnostics: Vec<Diagnostic>,
    pub shared_nodes: Vec<NodeAnalysis>,
    pub duplicate_nodes: usize,
    pub occupied_prefix_nodes: usize,
    pub limitations: Vec<String>,
}

impl AuditReport {
    pub fn count(&self, severity: Severity) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == severity)
            .count()
    }
    pub fn markdown(&self) -> String {
        let mut out = format!(
            "# 键道词库全量检查\n\n目录：`{}`\n\n扫描 {} 个文件、{} 条记录；确定问题 {} 项，待核对 {} 项，信息 {} 项。\n\n",
            self.root.display(),
            self.files.len(),
            self.records,
            self.count(Severity::Error),
            self.count(Severity::Review),
            self.count(Severity::Info)
        );
        out.push_str("## 覆盖范围\n\n|规则|文件|记录|匹配|问题记录|待核对记录|不适用|\n|---|---:|---:|---:|---:|---:|---:|\n");
        for (p, c) in &self.coverage {
            out.push_str(&format!(
                "|{p:?}|{}|{}|{}|{}|{}|{}|\n",
                c.files, c.records, c.matched, c.errors, c.review, c.not_applicable
            ));
        }
        out.push_str("\n## 文件清单\n\n|文件|启用|规则|记录|\n|---|---|---|---:|\n");
        for f in &self.files {
            out.push_str(&format!(
                "|{}|{}|{:?}|{}|\n",
                f.file.file_name().unwrap_or_default().to_string_lossy(),
                f.active,
                f.profile,
                f.records
            ));
        }
        out.push_str(&format!("\n## 节点\n\n同码多词节点 {} 个，重复词条节点 {} 个，有后继词条的终结节点 {} 个。全部范围的同码包含不同方案及未启用词库，不代表实际候选冲突；完整记录见 JSON。\n\n",self.shared_nodes.len(),self.duplicate_nodes,self.occupied_prefix_nodes));
        out.push_str("## 证据与限制\n\n");
        for note in &self.limitations {
            out.push_str(&format!("- {note}\n"));
        }
        out.push_str(&self.character_coverage.markdown());
        out.push_str("\n## 诊断明细\n\n|级别|规则|文件:行|词条|实际编码|预期候选|原因|\n|---|---|---|---|---|---|---|\n");
        fn cell(s: &str) -> String {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('|', "&#124;")
                .replace('\n', "<br>")
                .replace('\r', " ")
        }
        for d in &self.diagnostics {
            out.push_str(&format!(
                "|{:?}|{}|{}:{}|{}|{}|{}|{}|\n",
                d.severity,
                cell(&d.rule),
                cell(&d.file.file_name().unwrap_or_default().to_string_lossy()),
                d.line,
                cell(d.text.as_deref().unwrap_or("")),
                cell(d.actual.as_deref().unwrap_or("")),
                cell(&d.expected.join(", ")),
                cell(&d.reason)
            ));
        }
        out
    }
}

fn diagnostic(
    entry: &Entry,
    severity: Severity,
    rule: &str,
    reason: &str,
    expected: Vec<String>,
) -> Diagnostic {
    Diagnostic {
        severity,
        rule: rule.into(),
        file: entry.file.clone(),
        line: entry.line,
        text: Some(entry.text.clone()),
        actual: Some(entry.code.clone()),
        expected,
        reason: reason.into(),
    }
}

fn check_entry(e: &Entry, p: Profile, encoder: &Encoder) -> Option<Diagnostic> {
    use Severity::{Error, Review};
    if e.text.is_empty() || e.code.is_empty() {
        return Some(diagnostic(
            e,
            Error,
            "empty_field",
            "词条或编码为空",
            vec![],
        ));
    }
    if p == Profile::Foreign {
        return None;
    }
    if !e.code.chars().all(|c| c.is_ascii_lowercase() || c == ';') {
        return Some(diagnostic(
            e,
            Error,
            "code_alphabet",
            "键道编码包含小写字母及分号以外的字符",
            vec![],
        ));
    }
    let structural = match p {
        Profile::Shape => e.code.chars().all(is_shape),
        Profile::Css => {
            (2..=3).contains(&e.code.len())
                && e.code.chars().take(1).all(is_sound)
                && e.code.chars().skip(1).all(is_shape)
        }
        Profile::CssSingle => {
            (2..=4).contains(&e.code.len())
                && e.code.chars().take(1).all(is_sound)
                && e.code.chars().skip(1).all(is_shape)
        }
        Profile::Initials => e.code.chars().all(is_sound) && e.code.len() == e.text.chars().count(),
        Profile::Syxb => {
            e.text.chars().count() == 1
                && e.code.len() == 4
                && e.code.chars().take(3).all(is_sound)
                && e.code.chars().skip(3).all(is_shape)
        }
        Profile::Guided => {
            e.code.starts_with('i')
                && e.code.len() >= 2
                && e.code.chars().all(|c| c.is_ascii_lowercase())
        }
        Profile::Standard | Profile::Supplement => {
            e.code.len() <= 6 && e.code.chars().all(|c| c.is_ascii_lowercase())
        }
        Profile::English => e.code.len() <= 20,
        Profile::Symbol | Profile::Custom => e.code.len() <= 12,
        Profile::Foreign => true,
    };
    if !structural {
        return Some(diagnostic(
            e,
            Error,
            "code_structure",
            "编码不满足该词库类型的字符结构或码长",
            vec![],
        ));
    }
    match p {
        Profile::Custom | Profile::Symbol => {
            return Some(diagnostic(
                e,
                Review,
                "mnemonic",
                "自定义助记/符号码，仅确认字符集和码长；语义映射需人工确认",
                vec![],
            ));
        }
        Profile::Guided => {
            return Some(diagnostic(
                e,
                Review,
                "guided_mapping",
                "已检查 i 引导和长度；诗词专用截取规则缺少独立映射依据",
                vec![],
            ));
        }
        Profile::Syxb => {
            return Some(diagnostic(
                e,
                Review,
                "component_reading",
                "声音形笔结构合法；缺少部件读音及笔形数据，不能验证部件取码",
                vec![],
            ));
        }
        Profile::Shape => {
            return Some(diagnostic(
                e,
                Review,
                "root_shape",
                "纯形码结构合法；未引入独立字根/部件表验证具体映射",
                vec![],
            ));
        }
        Profile::English => {
            return Some(diagnostic(
                e,
                Review,
                "english_mnemonic",
                "旧版英文库包含原拼、字母转码及中英混合助记码，未强制套用新版规则",
                vec![],
            ));
        }
        _ => {}
    }
    if !e.text.chars().all(is_han) {
        return Some(diagnostic(
            e,
            Review,
            "mixed_text",
            "含非汉字内容，不能按普通汉字组词规则确定读音及取字位置",
            vec![],
        ));
    }
    let rule = if p == Profile::Supplement {
        Profile::Standard
    } else {
        p
    };
    let generated = encoder.encode(&e.text, rule);
    if !generated.missing.is_empty() {
        return Some(diagnostic(
            e,
            Review,
            "missing_reference",
            &format!(
                "本地单字参考缺失：{}",
                generated.missing.iter().collect::<String>()
            ),
            vec![],
        ));
    }
    if generated.truncated {
        return Some(diagnostic(
            e,
            Review,
            "candidate_limit",
            "多音/飞键组合超出上限，不能据不完整候选判定错误",
            generated.candidates,
        ));
    }
    if matches_encoding(e, &generated) {
        return None;
    }
    // Supplement dictionaries intentionally hold manually assigned shortcuts.
    if p == Profile::Supplement {
        return Some(diagnostic(
            e,
            Review,
            "manual_shortcut",
            "补充库中的人工短码不匹配普通规则，保留并待核对",
            generated.candidates,
        ));
    }
    let sound_len = match rule {
        Profile::Css | Profile::CssSingle => 1,
        Profile::Initials => e.text.chars().count(),
        _ => match e.text.chars().count() {
            1 => 2,
            2 => 4,
            3 => 3,
            _ => 4,
        },
    };
    let observed_sound = &e.code[..e.code.len().min(sound_len)];
    let reading_matches = generated
        .candidates
        .iter()
        .any(|c| c.starts_with(observed_sound));
    let severity = if reading_matches { Error } else { Review };
    Some(diagnostic(
        e,
        severity,
        if reading_matches {
            "shape_or_prefix_mismatch"
        } else {
            "reading_mismatch"
        },
        if reading_matches {
            "音码可匹配，但形码或简码长度不符合本地参考编码前缀"
        } else {
            "音码不匹配本地参考；可能为错码、特殊读音或参考缺漏，需核对"
        },
        generated.candidates,
    ))
}

pub fn check(ws: &Workspace, encoder: &Encoder, all: bool) -> AuditReport {
    let mut report = AuditReport { character_coverage:crate::coverage::check(ws,all), root: ws.root.clone(), schema_dictionary: ws.dictionary.clone(), all, files: vec![], records: 0,
        coverage: BTreeMap::new(), diagnostics: vec![], shared_nodes: vec![], duplicate_nodes: 0, occupied_prefix_nodes: 0,
        limitations: vec![
            "音形匹配基于本地 xkjd6dz 与生僻字库的每读音最长编码，不是独立汉字拆分数据库；参考表自身的系统性错码无法据此发现。".into(),
            "Error 表示可确定的格式/结构错误或与本地参考矛盾；后者不等于已独立确认汉字读音笔顺。Review 不应自动修复。".into(),
            "多音与飞键采用参考表已有编码；未收录读音记待核对。匹配某个读音只表示存在合法编码，不确认该词的语境读音。".into(),
            "特殊助记、诗词映射、字根与声音形笔仅验证明确结构，未验证部分逐条列为 Review；拼音/日语只检查文件格式。".into(),
            "未应用 Rime custom patch、Lua、动态用户词频或编译排序；码位和前缀分析不是实际候选/顶功模拟。".into(),
        ] };
    for issue in &ws.import_issues {
        if all || issue.active {
            report.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                rule: "import".into(),
                file: issue.file.clone(),
                line: 1,
                text: None,
                actual: None,
                expected: vec![],
                reason: issue.message.clone(),
            });
        }
    }
    for doc in &ws.documents {
        if !all && !ws.active_files.contains(&doc.path) {
            continue;
        }
        let p = profile(&doc.path);
        report.files.push(FileCoverage {
            file: doc.path.clone(),
            active: ws.active_files.contains(&doc.path),
            profile: p,
            records: doc.entries.len(),
        });
        report.coverage.entry(p).or_default().files += 1;
        let mut header_diagnostic = |rule: &str, reason: String| {
            report.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                rule: rule.into(),
                file: doc.path.clone(),
                line: if rule == "dictionary_name" {
                    doc.raw
                        .lines()
                        .position(|line| {
                            line.split_once(':')
                                .is_some_and(|(key, _)| key.trim() == "name")
                        })
                        .map_or(1, |n| n + 1)
                } else {
                    1
                },
                text: None,
                actual: None,
                expected: vec![],
                reason,
            })
        };
        if let Some(error) = &doc.header_error {
            header_diagnostic("header", error.clone());
        }
        if let Some(header) = &doc.header {
            let expected = doc
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .trim_end_matches(".dict.yaml")
                .to_owned();
            if header.name.as_deref() != Some(&expected) {
                header_diagnostic(
                    "dictionary_name",
                    format!(
                        "name 应与文件名一致：预期 {expected}，实际 {:?}",
                        header.name
                    ),
                );
            }
            if let Some(columns) = &header.columns
                && (!columns.iter().any(|c| c == "text")
                    || !columns.iter().any(|c| c == "code")
                    || columns.iter().collect::<BTreeSet<_>>().len() != columns.len())
            {
                header_diagnostic(
                    "columns",
                    "columns 必须含唯一 text/code 列，且列名不得重复".into(),
                );
            }
        }
        if !doc.terminated_header
            && doc
                .header
                .as_ref()
                .is_none_or(|h| h.import_tables.is_empty())
        {
            header_diagnostic("body_delimiter", "缺少词条区起始标记 ...".into());
        }
        for line in &doc.malformed_lines {
            report.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                rule: "columns".into(),
                file: doc.path.clone(),
                line: *line,
                text: None,
                actual: None,
                expected: vec![],
                reason: "词条缺少制表符分隔的 text/code 字段".into(),
            });
        }
        for entry in &doc.entries {
            report.records += 1;
            let coverage = report.coverage.entry(p).or_default();
            coverage.records += 1;
            let mut diagnostics = Vec::new();
            if let Some(d) = check_entry(entry, p, encoder) {
                diagnostics.push(d);
            }
            let declared = doc.header.as_ref().and_then(|h| h.columns.as_ref());
            let weight_index = declared.map_or(Some(2), |columns| {
                columns.iter().position(|c| c == "weight")
            });
            let invalid_weight = weight_index
                .and_then(|i| entry.fields.get(i))
                .is_some_and(|w| {
                    !w.is_empty()
                        && w.strip_suffix('%')
                            .unwrap_or(w)
                            .parse::<f64>()
                            .map_or(true, |n| !n.is_finite() || n < 0.0)
                });
            let field_error = if let Some(columns) = declared {
                entry.fields.len() > columns.len() || invalid_weight
            } else {
                entry.fields.len() > 4 || invalid_weight
            };
            if field_error {
                diagnostics.push(diagnostic(
                    entry,
                    Severity::Error,
                    "extra_fields",
                    "额外字段不符合声明列数或默认 weight（数值/百分比）列，原始字段已保留",
                    vec![],
                ));
            }
            if diagnostics.iter().any(|d| d.severity == Severity::Error) {
                coverage.errors += 1;
            } else if !diagnostics.is_empty() {
                coverage.review += 1;
            } else if p == Profile::Foreign {
                coverage.not_applicable += 1;
            } else {
                coverage.matched += 1;
            }
            report.diagnostics.extend(diagnostics);
        }
    }
    let mut prefix_counts: BTreeMap<String, usize> = BTreeMap::new();
    for e in ws.entries.iter().filter(|e| ws.selected(e, all)) {
        for (end, _) in e.code.char_indices().skip(1) {
            *prefix_counts.entry(e.code[..end].into()).or_default() += 1;
        }
    }
    for (code, ids) in &ws.by_code {
        let entries: Vec<_> = ids
            .iter()
            .map(|&id| &ws.entries[id])
            .filter(|e| ws.selected(e, all))
            .collect();
        if entries.is_empty() {
            continue;
        }
        let words: BTreeSet<_> = entries.iter().map(|e| e.text.clone()).collect();
        let descendants = prefix_counts.get(code).copied().unwrap_or(0);
        if descendants > 0 {
            report.occupied_prefix_nodes += 1;
        }
        if entries.len() > words.len() {
            report.duplicate_nodes += 1;
            let mut seen = BTreeSet::new();
            for e in &entries {
                if !seen.insert(&e.text) {
                    report.diagnostics.push(diagnostic(
                        e,
                        Severity::Info,
                        "duplicate_pair",
                        "相同词条与编码重复出现；跨方案重复可能是有意的",
                        vec![],
                    ));
                }
            }
        }
        if words.len() > 1 {
            report.shared_nodes.push(NodeAnalysis {
                code: code.clone(),
                words: words.into_iter().collect(),
                active_entries: entries
                    .iter()
                    .filter(|e| ws.active_files.contains(&e.file))
                    .count(),
                entries: entries.into_iter().cloned().collect(),
                descendant_entries: descendants,
            });
        }
    }
    report
        .diagnostics
        .sort_by(|a, b| (&a.file, a.line, &a.rule).cmp(&(&b.file, b.line, &b.rule)));
    report
}
