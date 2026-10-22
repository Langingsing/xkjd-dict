//! Reverse lookup and explicit-space segmentation, independent of Rime ranking.
use crate::{
    document::Entry,
    rules::{Profile, profile},
    workspace::{ImportIssue, Workspace},
};
use anyhow::{Result, bail};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

pub const MAX_TEXT_CHARACTERS: usize = 128;
pub const MAX_PLANS: usize = 20;

#[derive(Clone, Debug, Serialize)]
pub struct Source {
    pub file: PathBuf,
    pub line: usize,
    pub active: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub text: String,
    pub sources: Vec<Source>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Certainty {
    /// Unique among exact dictionary matches, not a runtime guarantee.
    UniqueExact,
    /// User reports this text is the first candidate for this code.
    ConfirmedFirst,
    /// Several exact matches; runtime first candidate is unknown.
    UnverifiedRank,
    /// The user confirmed another text as first; selection is required.
    NeedsSelection,
}

#[derive(Clone, Debug, Serialize)]
pub struct CodeOption {
    pub text: String,
    pub code: String,
    pub certainty: Certainty,
    pub candidates: Vec<Candidate>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Burden {
    pub code_keys: usize,
    pub confirmation_keys: usize,
    pub typed_keys: usize,
    pub boundaries: usize,
    pub unresolved_selections: usize,
    pub unknown_rank_segments: usize,
    /// Counts only known non-first segments, not merely ambiguous dictionaries.
    pub selection_actions_lower_bound: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Plan {
    pub segments: Vec<CodeOption>,
    /// A template with a space per segment and explicit placeholders for
    /// unverified/known non-first candidates. Placeholders are not literal keys.
    pub input: String,
    pub burden: Burden,
}

impl Plan {
    fn prepend(option: &CodeOption, suffix: &Self) -> Self {
        let mut segments = vec![option.clone()];
        segments.extend(suffix.segments.iter().cloned());
        let mut burden = suffix.burden.clone();
        burden.code_keys += option.code.len();
        burden.confirmation_keys += 1;
        burden.typed_keys = burden.code_keys + burden.confirmation_keys;
        burden.boundaries = segments.len() - 1;
        burden.unresolved_selections += usize::from(matches!(
            option.certainty,
            Certainty::NeedsSelection | Certainty::UnverifiedRank
        ));
        burden.unknown_rank_segments += usize::from(option.certainty == Certainty::UnverifiedRank);
        burden.selection_actions_lower_bound +=
            usize::from(option.certainty == Certainty::NeedsSelection);
        let selection = match option.certainty {
            Certainty::UnverifiedRank => format!(" [核对候选:{}]", option.text),
            Certainty::NeedsSelection => format!(" [选择:{}]", option.text),
            _ => String::new(),
        };
        Self {
            input: format!("{}{selection} {}", option.code, suffix.input),
            segments,
            burden,
        }
    }

    fn empty() -> Self {
        Self {
            segments: vec![],
            input: String::new(),
            burden: Burden::default(),
        }
    }
}

#[derive(Clone, Copy)]
enum Ranking {
    Ease,
    Keys,
}

fn retain_best(plans: &mut Vec<Plan>, ranking: Ranking, limit: usize) {
    plans.sort_by(|a, b| {
        let key = |p: &Plan| match ranking {
            Ranking::Ease => (
                p.burden.unresolved_selections,
                p.burden.typed_keys,
                p.burden.boundaries,
            ),
            Ranking::Keys => (
                p.burden.typed_keys,
                p.burden.unresolved_selections,
                p.burden.boundaries,
            ),
        };
        key(a).cmp(&key(b)).then_with(|| {
            a.segments
                .iter()
                .map(|s| (&s.text, &s.code))
                .cmp(b.segments.iter().map(|s| (&s.text, &s.code)))
        })
    });
    plans.dedup_by(|a, b| {
        a.segments
            .iter()
            .map(|s| (&s.text, &s.code))
            .eq(b.segments.iter().map(|s| (&s.text, &s.code)))
    });
    plans.truncate(limit);
}

#[derive(Debug, Serialize)]
pub struct Alternatives {
    pub easier: Vec<Plan>,
    pub fewest_keys: Vec<Plan>,
    pub confirmed_first: Vec<Plan>,
}

#[derive(Debug, Serialize)]
pub struct ReverseResult {
    pub text: String,
    pub existing: Vec<CodeOption>,
    pub shortest_existing: Vec<String>,
    pub alternatives: Alternatives,
    /// Excludes every whole-text entry, so the plan survives loss of its short code.
    pub segmented: Alternatives,
    pub scope: Scope,
    pub import_issues: Vec<ImportIssue>,
    pub note: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    ActiveImports,
    AllDictionaries,
}

pub const LIMITATIONS: &str = "静态词库分析，非 Rime 排名或顶功模拟。每段显式按一次空格；typed_keys 不含未知候选选择键数。唯一精确匹配仍未验证补全、用户词典、custom patch、过滤器与运行时配置。confirmed_first 仅为用户提供的首选证据。省心排序依次比较未确定选择次数、编码加确认键数、分段边界；键数排序交换前两项。只使用已有小写字母码，不猜测未收录字或自动修改词库。";

/// Build once and reuse across lookups or conflict nodes. Duplicate sources are
/// retained, but the same text/code is a single candidate and segmentation edge.
pub struct ReverseIndex<'a> {
    ws: &'a Workspace,
    words: BTreeMap<&'a str, BTreeSet<&'a str>>,
    codes: BTreeMap<&'a str, BTreeMap<&'a str, Vec<&'a Entry>>>,
    confirmed_first: BTreeMap<String, String>,
    scope: Scope,
}

impl<'a> ReverseIndex<'a> {
    pub fn new(ws: &'a Workspace, all: bool, confirmed: &[String]) -> Result<Self> {
        let mut index = Self {
            ws,
            words: BTreeMap::new(),
            codes: BTreeMap::new(),
            confirmed_first: BTreeMap::new(),
            scope: if all {
                Scope::AllDictionaries
            } else {
                Scope::ActiveImports
            },
        };
        for entry in &ws.entries {
            if !ws.selected(entry, all) || !is_code(&entry.code) {
                continue;
            }
            // Foreign dictionaries must not supply Jiandao segmentation edges.
            // Their same-code entries still count as competitors if imported.
            if profile(&entry.file) != Profile::Foreign {
                index
                    .words
                    .entry(&entry.text)
                    .or_default()
                    .insert(&entry.code);
            }
            index
                .codes
                .entry(&entry.code)
                .or_default()
                .entry(&entry.text)
                .or_default()
                .push(entry);
        }
        for value in confirmed {
            let Some((text, code)) = value.split_once('=') else {
                bail!("首选证据应为 词语=编码：{value}");
            };
            if !index
                .words
                .get(text)
                .is_some_and(|codes| codes.contains(code))
            {
                bail!("首选证据没有对应的已选词库记录：{value}");
            }
            if index
                .confirmed_first
                .get(code)
                .is_some_and(|previous| previous != text)
            {
                bail!("同一码不能确认两个首选：{code}");
            }
            index.confirmed_first.insert(code.into(), text.into());
        }
        Ok(index)
    }

    fn option(&self, text: &str, code: &str) -> CodeOption {
        let candidates: Vec<_> = self.codes[code]
            .iter()
            .map(|(text, entries)| Candidate {
                text: (*text).into(),
                sources: entries
                    .iter()
                    .map(|e| Source {
                        file: e.file.clone(),
                        line: e.line,
                        active: self.ws.active_files.contains(&e.file),
                    })
                    .collect(),
            })
            .collect();
        let certainty = match self.confirmed_first.get(code) {
            Some(first) if first == text => Certainty::ConfirmedFirst,
            Some(_) => Certainty::NeedsSelection,
            None if candidates.len() == 1 => Certainty::UniqueExact,
            None => Certainty::UnverifiedRank,
        };
        CodeOption {
            text: text.into(),
            code: code.into(),
            certainty,
            candidates,
        }
    }

    fn edges(&self, text: &str) -> Vec<Vec<(usize, CodeOption)>> {
        let mut offsets: Vec<_> = text.char_indices().map(|(offset, _)| offset).collect();
        offsets.push(text.len());
        (0..offsets.len() - 1)
            .map(|start| {
                let mut edges = vec![];
                for end in start + 1..offsets.len() {
                    let part = &text[offsets[start]..offsets[end]];
                    if let Some(codes) = self.words.get(part) {
                        edges.extend(codes.iter().map(|code| (end, self.option(part, code))));
                    }
                }
                edges
            })
            .collect()
    }

    fn plans(
        edges: &[Vec<(usize, CodeOption)>],
        segmented: bool,
        ranking: Ranking,
        limit: usize,
    ) -> Vec<Plan> {
        let end = edges.len();
        let mut best: Vec<Vec<Plan>> = vec![vec![]; end + 1];
        best[end] = vec![Plan::empty()];
        for start in (0..end).rev() {
            let mut plans = vec![];
            for (next, option) in &edges[start] {
                if segmented && start == 0 && *next == end {
                    continue;
                }
                for suffix in &best[*next] {
                    plans.push(Plan::prepend(option, suffix));
                }
            }
            retain_best(&mut plans, ranking, limit);
            best[start] = plans;
        }
        std::mem::take(&mut best[0])
    }

    pub fn lookup(&self, text: &str, limit: usize) -> Result<ReverseResult> {
        let count = text.chars().count();
        if count == 0 || count > MAX_TEXT_CHARACTERS {
            bail!("反查文本需为 1..={MAX_TEXT_CHARACTERS} 个 Unicode 字符，实际 {count}");
        }
        validate_limit(limit)?;
        let mut existing: Vec<_> = self
            .words
            .get(text)
            .into_iter()
            .flatten()
            .map(|code| self.option(text, code))
            .collect();
        existing.sort_by(|a, b| (a.code.len(), &a.code).cmp(&(b.code.len(), &b.code)));
        let shortest_existing = existing
            .iter()
            .take_while(|option| option.code.len() == existing[0].code.len())
            .map(|option| option.code.clone())
            .collect();
        let edges = self.edges(text);
        let confirmed_edges: Vec<_> = edges
            .iter()
            .map(|items| {
                items
                    .iter()
                    .filter(|(_, option)| option.certainty == Certainty::ConfirmedFirst)
                    .cloned()
                    .collect()
            })
            .collect();
        let alternatives = |segmented| Alternatives {
            easier: Self::plans(&edges, segmented, Ranking::Ease, limit),
            fewest_keys: Self::plans(&edges, segmented, Ranking::Keys, limit),
            confirmed_first: Self::plans(&confirmed_edges, segmented, Ranking::Keys, limit),
        };
        Ok(ReverseResult {
            text: text.into(),
            existing,
            shortest_existing,
            alternatives: alternatives(false),
            segmented: alternatives(true),
            scope: self.scope,
            import_issues: self
                .ws
                .import_issues
                .iter()
                .filter(|issue| issue.active)
                .cloned()
                .collect(),
            note: LIMITATIONS,
        })
    }

    /// Analyze only the occupants of one proposed short code, without editing.
    pub fn reclaim(&self, code: &str, limit: usize, max_extra_keys: u32) -> Result<ReclaimResult> {
        validate_limit(limit)?;
        if !is_code(code) {
            bail!("让码分析只接受非空小写字母编码：{code}");
        }
        let mut candidates = vec![];
        for text in self
            .codes
            .get(code)
            .into_iter()
            .flat_map(|items| items.keys())
        {
            if text.chars().count() < 2 || !self.words.contains_key(text) {
                continue;
            }
            let analysis = self.lookup(text, limit)?;
            let best = analysis
                .segmented
                .confirmed_first
                .first()
                .filter(|p| {
                    p.burden.typed_keys as i64 - (code.len() + 1) as i64
                        <= i64::from(max_extra_keys)
                })
                .or_else(|| analysis.segmented.easier.first());
            let extra_keys = best.map(|p| p.burden.typed_keys as i64 - (code.len() + 1) as i64);
            let assessment = match best {
                None => ReclaimAssessment::NoSegmentation,
                Some(_) if self.scope == Scope::AllDictionaries => ReclaimAssessment::InactiveScope,
                Some(_) if self.ws.import_issues.iter().any(|issue| issue.active) => {
                    ReclaimAssessment::IncompleteImports
                }
                Some(p) if p.burden.unresolved_selections > 0 => ReclaimAssessment::NeedsSelection,
                Some(_) if extra_keys.is_some_and(|extra| extra > i64::from(max_extra_keys)) => {
                    ReclaimAssessment::ExtraKeys
                }
                Some(p)
                    if p.segments
                        .iter()
                        .all(|s| s.certainty == Certainty::ConfirmedFirst) =>
                {
                    ReclaimAssessment::ConfirmedCandidate
                }
                Some(_) => ReclaimAssessment::StaticCandidate,
            };
            candidates.push(ReclaimCandidate {
                text: (*text).into(),
                assessment,
                extra_keys,
                recommended: best.cloned(),
                segmented: analysis.segmented,
            });
        }
        candidates.sort_by_key(|c| (c.assessment, c.extra_keys, c.text.clone()));
        Ok(ReclaimResult {
            code: code.into(),
            max_extra_keys,
            candidates,
            scope: self.scope,
            import_issues: self
                .ws
                .import_issues
                .iter()
                .filter(|issue| issue.active)
                .cloned()
                .collect(),
            note: LIMITATIONS,
        })
    }
}

fn is_code(code: &str) -> bool {
    !code.is_empty() && code.bytes().all(|b| b.is_ascii_lowercase())
}

fn validate_limit(limit: usize) -> Result<()> {
    if !(1..=MAX_PLANS).contains(&limit) {
        bail!("方案数需为 1..={MAX_PLANS}，实际 {limit}");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReclaimAssessment {
    ConfirmedCandidate,
    StaticCandidate,
    NeedsSelection,
    ExtraKeys,
    NoSegmentation,
    InactiveScope,
    IncompleteImports,
}

#[derive(Debug, Serialize)]
pub struct ReclaimCandidate {
    pub text: String,
    pub assessment: ReclaimAssessment,
    pub extra_keys: Option<i64>,
    pub recommended: Option<Plan>,
    pub segmented: Alternatives,
}

#[derive(Debug, Serialize)]
pub struct ReclaimResult {
    pub code: String,
    pub max_extra_keys: u32,
    pub candidates: Vec<ReclaimCandidate>,
    pub scope: Scope,
    pub import_issues: Vec<ImportIssue>,
    pub note: &'static str,
}
