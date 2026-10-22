use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Header {
    pub name: Option<String>,
    #[serde(default)]
    pub import_tables: Vec<String>,
    #[serde(default)]
    pub columns: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub file: PathBuf,
    pub line: usize,
    pub text: String,
    pub code: String,
    pub fields: Vec<String>,
}

#[derive(Debug)]
pub struct Document {
    pub path: PathBuf,
    pub raw: String,
    pub header: Option<Header>,
    pub header_error: Option<String>,
    pub entries: Vec<Entry>,
    pub malformed_lines: Vec<usize>,
    pub body_start: usize,
    pub terminated_header: bool,
}

impl Document {
    pub fn validate_columns(&self) -> Result<()> {
        if let Some(columns) = self
            .header
            .as_ref()
            .and_then(|header| header.columns.as_ref())
            && (!columns.iter().any(|name| name == "text")
                || !columns.iter().any(|name| name == "code")
                || columns
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != columns.len())
        {
            bail!(
                "{}：columns 声明缺少 text/code 或列名重复",
                self.path.display()
            );
        }
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("读取 UTF-8 词库 {}", path.display()))?;
        Ok(Self::parse(path.to_owned(), raw))
    }

    pub fn parse(path: PathBuf, raw: String) -> Self {
        let lines: Vec<_> = raw.lines().collect();
        let start = lines
            .iter()
            .position(|line| line.trim_start_matches('\u{feff}').trim() == "---");
        let end = start.and_then(|s| {
            lines
                .iter()
                .enumerate()
                .skip(s + 1)
                .find(|(_, line)| line.trim() == "...")
                .map(|(i, _)| i)
        });
        // Recover tab-separated records after a missing delimiter for auditing,
        // but never permit editing that malformed document.
        let recovered_body = lines
            .iter()
            .position(|line| !line.trim_start().starts_with('#') && line.contains('\t'));
        let header_end = end.unwrap_or_else(|| recovered_body.unwrap_or(lines.len()));
        let body_start = end.map_or_else(|| recovered_body.unwrap_or(lines.len()), |n| n + 1);
        let parsed = start
            .filter(|&s| header_end > s)
            .map(|s| serde_yaml_ng::from_str::<Header>(&lines[s + 1..header_end].join("\n")));
        let (header, header_error) = match parsed {
            Some(Ok(h)) => (Some(h), None),
            Some(Err(e)) => (None, Some(e.to_string())),
            None => (None, Some("缺少有效位置的 YAML 起始标记 ---".into())),
        };
        let mut entries = Vec::new();
        let mut malformed_lines = Vec::new();
        let columns = header.as_ref().and_then(|h| h.columns.as_ref());
        let text_col = columns
            .and_then(|c| c.iter().position(|v| v == "text"))
            .unwrap_or(0);
        let code_col = columns
            .and_then(|c| c.iter().position(|v| v == "code"))
            .unwrap_or(1);
        for (i, line) in lines.iter().enumerate().skip(body_start) {
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            let fields: Vec<_> = line.split('\t').map(str::to_owned).collect();
            if fields.len() <= text_col.max(code_col) {
                malformed_lines.push(i + 1);
                continue;
            }
            entries.push(Entry {
                file: path.clone(),
                line: i + 1,
                text: fields[text_col].clone(),
                code: fields[code_col].clone(),
                fields,
            });
        }
        Self {
            path,
            raw,
            header,
            header_error,
            entries,
            malformed_lines,
            body_start,
            terminated_header: end.is_some(),
        }
    }

    pub fn columns(&self) -> (usize, usize) {
        let columns = self.header.as_ref().and_then(|h| h.columns.as_ref());
        (
            columns
                .and_then(|c| c.iter().position(|v| v == "text"))
                .unwrap_or(0),
            columns
                .and_then(|c| c.iter().position(|v| v == "code"))
                .unwrap_or(1),
        )
    }
}
