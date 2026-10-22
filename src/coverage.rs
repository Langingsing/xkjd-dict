//! Check all phrase characters, including characters not used in phrase encoding.
use crate::{
    rules::{Profile, is_han, profile},
    workspace::Workspace,
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Debug, Serialize)]
pub struct MissingCharacter {
    pub character: char,
    pub positions_one_based: Vec<usize>,
}
#[derive(Debug, Serialize)]
pub struct CharacterGap {
    pub file: PathBuf,
    pub line: usize,
    pub text: String,
    pub active: bool,
    pub characters: Vec<MissingCharacter>,
}
#[derive(Debug, Serialize)]
pub struct PhraseCoverage {
    pub all_single_characters: usize,
    pub active_single_characters: usize,
    pub phrases_checked: usize,
    pub ignored_non_han_scalars: usize,
    pub absent_from_all: Vec<CharacterGap>,
    pub only_in_inactive: Vec<CharacterGap>,
    pub note: &'static str,
}
pub fn check(ws: &Workspace, all: bool) -> PhraseCoverage {
    let mut all_chars = BTreeSet::new();
    let mut active_chars = BTreeSet::new();
    for e in &ws.entries {
        if profile(&e.file) == Profile::Foreign {
            continue;
        }
        let mut chars = e.text.chars();
        if let Some(ch) = chars.next().filter(|ch| is_han(*ch))
            && chars.next().is_none()
        {
            all_chars.insert(ch);
            if ws.active_files.contains(&e.file) {
                active_chars.insert(ch);
            }
        }
    }
    let mut report = PhraseCoverage {
        all_single_characters: all_chars.len(),
        active_single_characters: active_chars.len(),
        phrases_checked: 0,
        ignored_non_han_scalars: 0,
        absent_from_all: vec![],
        only_in_inactive: vec![],
        note: "检查每个 Unicode 汉字及其位置，包括不参与取码的长词中间字；单字集合排除拼音/日语。非汉字标点、数字、字母另计且不当作缺字。集合存在性不代表字音/形码正确。",
    };
    for e in ws
        .entries
        .iter()
        .filter(|e| ws.selected(e, all) && profile(&e.file) != Profile::Foreign)
    {
        if e.text.chars().count() < 2 || !e.text.chars().any(is_han) {
            continue;
        }
        report.phrases_checked += 1;
        let mut absent: BTreeMap<char, Vec<usize>> = BTreeMap::new();
        let mut dormant: BTreeMap<char, Vec<usize>> = BTreeMap::new();
        for (position, ch) in e.text.chars().enumerate() {
            if !is_han(ch) {
                report.ignored_non_han_scalars += 1;
                continue;
            }
            if !all_chars.contains(&ch) {
                absent.entry(ch).or_default().push(position + 1);
            } else if !active_chars.contains(&ch) {
                dormant.entry(ch).or_default().push(position + 1);
            }
        }
        let gap = |chars: BTreeMap<char, Vec<usize>>| CharacterGap {
            file: e.file.clone(),
            line: e.line,
            text: e.text.clone(),
            active: ws.active_files.contains(&e.file),
            characters: chars
                .into_iter()
                .map(|(character, positions_one_based)| MissingCharacter {
                    character,
                    positions_one_based,
                })
                .collect(),
        };
        if !absent.is_empty() {
            report.absent_from_all.push(gap(absent));
        }
        if !dormant.is_empty() {
            report.only_in_inactive.push(gap(dormant));
        }
    }
    report
}

impl PhraseCoverage {
    pub fn markdown(&self) -> String {
        let mut out = format!(
            "\n## 词组逐字覆盖\n\n全部键道单字集合 {} 字，启用单字集合 {} 字；检查 {} 条词组。{} 条含全部单字库均未收录的汉字，{} 条含仅未启用单字库收录的汉字；两类可重叠。\n\n{}\n\n",
            self.all_single_characters,
            self.active_single_characters,
            self.phrases_checked,
            self.absent_from_all.len(),
            self.only_in_inactive.len(),
            self.note
        );
        for (title, gaps) in [
            ("全部单字库缺失", &self.absent_from_all),
            ("仅存在未启用单字库", &self.only_in_inactive),
        ] {
            out.push_str(&format!(
                "### {title}\n\n|文件:行|词组|启用|汉字及位置（1 起算）|\n|---|---|---|---|\n"
            ));
            for g in gaps {
                let word = g
                    .text
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
                    .replace('|', "&#124;");
                out.push_str(&format!(
                    "|{}:{}|{}|{}|{}|\n",
                    g.file.file_name().unwrap_or_default().to_string_lossy(),
                    g.line,
                    word,
                    g.active,
                    g.characters
                        .iter()
                        .map(|c| format!("{}:{:?}", c.character, c.positions_one_based))
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
            }
            out.push('\n');
        }
        out
    }
}
