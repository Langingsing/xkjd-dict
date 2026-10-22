use crate::{document::Entry, workspace::Workspace};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub const SHAPES: &str = "aiouv";
pub const MAX_CANDIDATES: usize = 4096;
pub fn is_character_reference(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    name == "xkjd6dz.dict.yaml" || name.starts_with("shengpizi.")
}
pub fn is_shape(c: char) -> bool {
    SHAPES.contains(c)
}
pub fn is_sound(c: char) -> bool {
    c.is_ascii_lowercase() && !is_shape(c)
}
pub fn is_han(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x323af)
}
pub fn sound_shape(code: &str) -> bool {
    code.is_ascii()
        && (3..=6).contains(&code.len())
        && code.chars().take(2).all(is_sound)
        && code.chars().skip(2).all(is_shape)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Standard,
    Supplement,
    Initials,
    Css,
    CssSingle,
    Shape,
    Syxb,
    Symbol,
    English,
    Guided,
    Custom,
    Foreign,
}

pub fn profile(path: &Path) -> Profile {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if name.starts_with("japanese") || name == "pinyin_simp.dict.yaml" {
        Profile::Foreign
    } else if name.starts_with("abbr.") {
        Profile::Shape
    } else if name.starts_with("dup") {
        Profile::Supplement
    } else if name.starts_with("shengsheng.") {
        Profile::Initials
    } else if name == "xkjd6.wxw.dict.yaml" {
        Profile::Css
    } else if name == "xkjd6.wxwdanzi.dict.yaml" {
        Profile::CssSingle
    } else if name.starts_with("syxb.") {
        Profile::Syxb
    } else if name.starts_with("symbols.") {
        Profile::Symbol
    } else if name == "xkjd6.yingwen.dict.yaml" {
        Profile::English
    } else if name == "xkjd6.iboot.dict.yaml" {
        Profile::Guided
    } else if name.starts_with("private.")
        || name.starts_with("math.")
        || name == "xkjd6.lianjie.dict.yaml"
    {
        Profile::Custom
    } else if name.starts_with("xkjd6.")
        || name.ends_with(".xkjd6.dict.yaml")
        || name.starts_with("xkjd6dz.")
        || name.starts_with("shengpizi.")
        || name.starts_with("liuxing.")
    {
        Profile::Standard
    } else {
        Profile::Custom
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CharacterCode {
    pub code: String,
    pub source: CharacterSource,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CharacterSource {
    Dictionary { file: PathBuf, line: usize },
    ExplicitOverride,
}

#[derive(Debug)]
pub struct Encoder {
    pub characters: BTreeMap<char, Vec<CharacterCode>>,
}

#[derive(Debug, Serialize)]
pub struct Encoding {
    pub text: String,
    pub profile: Profile,
    pub candidates: Vec<String>,
    pub missing: Vec<char>,
    pub ambiguous: Vec<char>,
    pub truncated: bool,
    pub evidence: BTreeMap<char, Vec<CharacterCode>>,
    pub issues: Vec<String>,
}

impl Encoder {
    pub fn from_workspace(ws: &Workspace) -> Self {
        let mut characters: BTreeMap<char, Vec<CharacterCode>> = BTreeMap::new();
        // A longest code per reading is an existing reference, not a generated decomposition.
        for doc in &ws.documents {
            if !is_character_reference(&doc.path) {
                continue;
            }
            for e in &doc.entries {
                if e.text.chars().count() != 1 || !sound_shape(&e.code) {
                    continue;
                }
                let ch = e.text.chars().next().unwrap();
                let values = characters.entry(ch).or_default();
                let max_len = values
                    .iter()
                    .filter(|v| v.code[..2] == e.code[..2])
                    .map(|v| v.code.len())
                    .max()
                    .unwrap_or(0);
                if e.code.len() < max_len {
                    continue;
                }
                if e.code.len() > max_len {
                    values.retain(|v| v.code[..2] != e.code[..2]);
                }
                if !values.iter().any(|v| v.code == e.code) {
                    values.push(CharacterCode {
                        code: e.code.clone(),
                        source: CharacterSource::Dictionary {
                            file: e.file.clone(),
                            line: e.line,
                        },
                    });
                }
            }
        }
        for values in characters.values_mut() {
            values.sort_by(|a, b| a.code.cmp(&b.code));
        }
        Self { characters }
    }

    pub fn with_overrides(mut self, overrides: &[String]) -> Result<Self> {
        for item in overrides {
            let (text, code) = item
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("单字覆盖须为 字=完整编码: {item}"))?;
            if text.chars().count() != 1 || !text.chars().all(is_han) || !sound_shape(code) {
                bail!("无效单字编码覆盖 {item}");
            }
            self.characters.insert(
                text.chars().next().unwrap(),
                vec![CharacterCode {
                    code: code.into(),
                    source: CharacterSource::ExplicitOverride,
                }],
            );
        }
        Ok(self)
    }

    pub fn encode(&self, text: &str, rule: Profile) -> Encoding {
        let chars: Vec<_> = text.chars().collect();
        let mut out = Encoding {
            text: text.into(),
            profile: rule,
            candidates: vec![],
            missing: vec![],
            ambiguous: vec![],
            truncated: false,
            evidence: BTreeMap::new(),
            issues: vec![],
        };
        if chars.is_empty() {
            out.issues.push("词条不能为空".into());
            return out;
        }
        let supported = match rule {
            Profile::Standard => chars.iter().all(|c| is_han(*c)),
            Profile::Initials => chars.len() >= 2,
            Profile::Css => chars.len() == 2,
            Profile::CssSingle => chars.len() == 1,
            _ => false,
        };
        if !supported {
            out.issues.push("该规则不适用于此词条：普通规则仅支持纯汉字，声笔笔需二字，声笔笔单字需一字，声声需至少二字".into());
            return out;
        }
        let selected: BTreeSet<usize> = match rule {
            Profile::Css if chars.len() >= 2 => [0, 1].into_iter().collect(),
            Profile::Standard if chars.len() >= 4 => {
                [0, 1, 2, chars.len() - 1].into_iter().collect()
            }
            _ => (0..chars.len()).collect(),
        };
        for &i in &selected {
            let ch = chars[i];
            match self.characters.get(&ch) {
                None => {
                    if !out.missing.contains(&ch) {
                        out.missing.push(ch);
                    }
                }
                Some(codes) => {
                    if codes.len() > 1 && !out.ambiguous.contains(&ch) {
                        out.ambiguous.push(ch);
                    }
                    out.evidence.insert(ch, codes.clone());
                }
            }
        }
        if !out.missing.is_empty() {
            return out;
        }
        let mut combinations: Vec<Vec<&str>> = vec![vec![""; chars.len()]];
        for &i in &selected {
            let mut next = Vec::new();
            for combination in &combinations {
                for value in &self.characters[&chars[i]] {
                    if next.len() >= MAX_CANDIDATES {
                        out.truncated = true;
                        break;
                    }
                    let mut c = combination.clone();
                    c[i] = &value.code;
                    next.push(c);
                }
                if out.truncated {
                    break;
                }
            }
            combinations = next;
        }
        let mut candidates = BTreeSet::new();
        for c in combinations {
            let code = match rule {
                Profile::Standard if c.len() == 1 => c[0].to_owned(),
                Profile::Standard if c.len() == 2 => {
                    format!("{}{}{}{}", &c[0][..2], &c[1][..2], &c[0][2..3], &c[1][2..3])
                }
                Profile::Standard if c.len() == 3 => format!(
                    "{}{}{}{}{}{}",
                    &c[0][..1],
                    &c[1][..1],
                    &c[2][..1],
                    &c[0][2..3],
                    &c[1][2..3],
                    &c[2][2..3]
                ),
                Profile::Standard => format!(
                    "{}{}{}{}{}{}",
                    &c[0][..1],
                    &c[1][..1],
                    &c[2][..1],
                    &c[c.len() - 1][..1],
                    &c[0][2..3],
                    &c[1][2..3]
                ),
                Profile::Initials => c.iter().map(|s| &s[..1]).collect(),
                Profile::Css if c.len() == 2 => {
                    format!("{}{}", &c[0][..1], &c[1][2..c[1].len().min(4)])
                }
                Profile::CssSingle if c.len() == 1 => {
                    format!("{}{}", &c[0][..1], &c[0][2..c[0].len().min(5)])
                }
                _ => continue,
            };
            candidates.insert(code);
        }
        out.candidates = candidates.into_iter().collect();
        out
    }
}

pub fn minimum_prefix(text: &str, rule: Profile) -> usize {
    match rule {
        Profile::Initials => text.chars().count(),
        Profile::Css | Profile::CssSingle => 2,
        Profile::Standard => match text.chars().count() {
            1 => 1,
            2 => 3,
            3 => 3,
            _ => 4,
        },
        _ => 1,
    }
}

pub fn matches_encoding(entry: &Entry, encoding: &Encoding) -> bool {
    entry.code.len() >= minimum_prefix(&entry.text, encoding.profile)
        && encoding
            .candidates
            .iter()
            .any(|c| c.starts_with(&entry.code))
}

#[derive(Debug, Serialize)]
pub struct Suggestion {
    pub encoding: Encoding,
    pub existing: Vec<Entry>,
    pub choices: Vec<CodeChoice>,
    pub ordering_note: &'static str,
}
#[derive(Debug, Serialize)]
pub struct CodeChoice {
    pub full_code: String,
    pub shortest_free: Option<String>,
    pub slots: Vec<Slot>,
}
#[derive(Debug, Serialize)]
pub struct Slot {
    pub code: String,
    pub same_code: Vec<Entry>,
    pub descendant_entries: usize,
    pub ancestor_codes: Vec<String>,
}

impl Suggestion {
    /// Daily decision view; JSON retains all evidence and prefix impact fields.
    pub fn concise_text(&self) -> String {
        let mut lines = Vec::new();
        let mut existing: Vec<_> = self
            .existing
            .iter()
            .map(|entry| entry.code.as_str())
            .collect();
        existing.sort_by_key(|code| (code.len(), *code));
        existing.dedup();
        if !existing.is_empty() {
            lines.push(format!("已有 {}", existing.join("、")));
        }
        if !self.encoding.missing.is_empty() {
            lines.push(format!(
                "缺少单字编码：{}",
                self.encoding.missing.iter().collect::<String>()
            ));
        }
        lines.extend(self.encoding.issues.iter().cloned());
        if !self.encoding.ambiguous.is_empty() {
            lines.push(format!(
                "歧义：{}；用 --char-code 指定",
                self.encoding.ambiguous.iter().collect::<String>()
            ));
        }
        if self.encoding.truncated {
            lines.push("候选不完整".into());
        }
        let mut shown_paths = BTreeSet::new();
        for choice in &self.choices {
            let current = existing
                .iter()
                .filter(|code| choice.full_code.starts_with(**code))
                .map(|code| code.len())
                .min();
            let offer = choice
                .shortest_free
                .as_deref()
                .filter(|code| current.is_none_or(|length| code.len() < length));
            if let Some(code) = offer {
                lines.push(if self.choices.len() > 1 {
                    format!("{code}（全码 {}）", choice.full_code)
                } else {
                    code.to_owned()
                });
            } else if current.is_none() {
                lines.push(format!("{}：无空闲码", choice.full_code));
            }
            let limit = offer
                .map(str::len)
                .or(current.map(|length| length + 1))
                .unwrap_or(choice.full_code.len() + 1);
            for slot in choice.slots.iter().filter(|slot| slot.code.len() < limit) {
                let mut seen = BTreeSet::new();
                let words: Vec<_> = slot
                    .same_code
                    .iter()
                    .filter(|entry| {
                        entry.text != self.encoding.text && seen.insert(entry.text.as_str())
                    })
                    .map(|entry| entry.text.as_str())
                    .collect();
                if !words.is_empty() && shown_paths.insert(slot.code.as_str()) {
                    lines.push(format!("{} {}", slot.code, words.join("、")));
                }
            }
        }
        if lines.is_empty() {
            lines.push("无法生成编码".into());
        }
        lines.join("\n") + "\n"
    }
}

pub fn suggest(
    ws: &Workspace,
    encoder: &Encoder,
    text: &str,
    rule: Profile,
    all: bool,
) -> Suggestion {
    let encoding = encoder.encode(text, rule);
    let mut choices = Vec::new();
    for full_code in &encoding.candidates {
        let mut slots = Vec::new();
        let mut shortest_free = None;
        for len in minimum_prefix(text, rule)..=full_code.len() {
            let code = &full_code[..len];
            let same_code = ws
                .query(None, Some(code), None, all)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            if same_code.is_empty() && shortest_free.is_none() {
                shortest_free = Some(code.into());
            }
            let descendant_entries = ws
                .query(None, None, Some(code), all)
                .iter()
                .filter(|e| e.code != code)
                .count();
            let ancestor_codes = (1..len)
                .filter(|&l| !ws.query(None, Some(&code[..l]), None, all).is_empty())
                .map(|l| code[..l].into())
                .collect();
            slots.push(Slot {
                code: code.into(),
                same_code,
                descendant_entries,
                ancestor_codes,
            });
        }
        choices.push(CodeChoice {
            full_code: full_code.clone(),
            shortest_free,
            slots,
        });
    }
    Suggestion {
        encoding,
        existing: ws
            .query(Some(text), None, None, all)
            .into_iter()
            .cloned()
            .collect(),
        choices,
        ordering_note: "静态码位占用分析；不预测 Rime 候选次序或实际顶功效果。最短空闲码不等于无需确认键。",
    }
}
