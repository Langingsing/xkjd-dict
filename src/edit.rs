use crate::document::Document;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use similar::TextDiff;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub fn dictionary_path(root: &Path, file: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(root)?;
    let candidate = if file.is_absolute() {
        file.to_owned()
    } else {
        root.join(file)
    };
    if fs::symlink_metadata(&candidate)?.file_type().is_symlink() {
        bail!("编辑目标不能是符号链接");
    }
    let file = fs::canonicalize(&candidate)?;
    if file.parent() != Some(&root)
        || !file
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".dict.yaml"))
    {
        bail!("编辑目标必须是词库目录内的直接 .dict.yaml 文件");
    }
    Ok(file)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Add {
        text: String,
        code: String,
    },
    Update {
        line: usize,
        text: Option<String>,
        code: Option<String>,
    },
    Remove {
        line: usize,
    },
}

#[derive(Debug)]
pub struct EditPlan {
    pub path: PathBuf,
    original: String,
    pub updated: String,
}

impl EditPlan {
    /// Insert after equal codes in a sorted, unclassified body; category placement
    /// must be selected explicitly. Existing record contents are never reordered.
    pub fn insert(
        doc: &Document,
        text: &str,
        code: &str,
        before_line: Option<usize>,
    ) -> Result<Self> {
        validate_document(doc)?;
        validate(text, code)?;
        if doc
            .entries
            .iter()
            .any(|entry| entry.text == text && entry.code == code)
        {
            bail!("添加失败：词条与编码已存在");
        }
        let mut updated = append_record(doc, text, code);
        let raw_lines: Vec<_> = doc.raw.split_inclusive('\n').collect();
        let offset = if let Some(line) = before_line {
            if line == raw_lines.len() + 1 {
                doc.raw.len()
            } else {
                let entry = doc
                    .entries
                    .iter()
                    .find(|entry| entry.line == line)
                    .context("插入位置必须是原始词条行号，或总行数 + 1（末尾）")?;
                raw_lines[..entry.line - 1]
                    .iter()
                    .map(|line| line.len())
                    .sum()
            }
        } else {
            if !doc.malformed_lines.is_empty()
                || !doc
                    .entries
                    .windows(2)
                    .all(|pair| pair[0].code <= pair[1].code)
            {
                bail!("词库存在格式异常或编码无序，请用 --before-line 明确插入位置");
            }
            if let (Some(first), Some(last)) = (doc.entries.first(), doc.entries.last())
                && last.line - first.line + 1 != doc.entries.len()
            {
                bail!("词库包含分类块，请用 --before-line 明确插入位置");
            }
            if let Some(entry) = doc.entries.iter().find(|entry| entry.code.as_str() > code) {
                raw_lines[..entry.line - 1]
                    .iter()
                    .map(|line| line.len())
                    .sum()
            } else if let Some(entry) = doc.entries.last() {
                raw_lines[..entry.line].iter().map(|line| line.len()).sum()
            } else {
                doc.raw.len()
            }
        };
        if offset < doc.raw.len() {
            let mut addition = &updated[doc.raw.len()..];
            if !doc.raw.ends_with('\n') {
                addition = addition
                    .strip_prefix("\r\n")
                    .or_else(|| addition.strip_prefix('\n'))
                    .unwrap_or(addition);
            }
            updated = format!("{}{}{}", &doc.raw[..offset], addition, &doc.raw[offset..]);
        }
        Ok(Self {
            path: doc.path.clone(),
            original: doc.raw.clone(),
            updated,
        })
    }

    /// Move only record contents within the selected comment/blank-delimited block.
    /// Physical line endings, comments and stable equal-code order are retained.
    pub fn update_code_ordered(doc: &Document, line: usize, code: &str) -> Result<Self> {
        Self::update_codes_ordered(doc, &[(line, code)])
    }

    /// Change original record lines together, sorting only their existing blocks.
    pub fn update_codes_ordered(doc: &Document, changes: &[(usize, &str)]) -> Result<Self> {
        let operations: Vec<_> = changes
            .iter()
            .map(|(line, code)| Operation::Update {
                line: *line,
                text: None,
                code: Some((*code).into()),
            })
            .collect();
        let mut plan = Self::build(doc, &operations)?;
        let original: Vec<_> = doc.raw.split_inclusive('\n').collect();
        let boundary = |text: &str| text.trim().is_empty() || text.trim_start().starts_with('#');
        let mut blocks = std::collections::BTreeSet::new();
        for &(line, _) in changes {
            let start = (doc.body_start..line - 1)
                .rev()
                .find(|&i| boundary(original[i]))
                .map_or(doc.body_start, |i| i + 1);
            let end = (line..original.len())
                .find(|&i| boundary(original[i]))
                .unwrap_or(original.len());
            blocks.insert((start, end));
        }
        let mut lines: Vec<String> = plan
            .updated
            .split_inclusive('\n')
            .map(str::to_owned)
            .collect();
        let (_, code_column) = doc.columns();
        for (start, end) in blocks {
            let entries: Vec<_> = doc
                .entries
                .iter()
                .filter(|entry| entry.line > start && entry.line <= end)
                .collect();
            if entries.len() != end - start
                || !entries.windows(2).all(|pair| pair[0].code <= pair[1].code)
            {
                bail!("目标分类块存在格式异常或编码无序，拒绝自动重排其他词条");
            }
            let mut records: Vec<_> = lines[start..end]
                .iter()
                .map(|row| {
                    let content = row
                        .strip_suffix("\r\n")
                        .or_else(|| row.strip_suffix('\n'))
                        .unwrap_or(row);
                    (
                        content
                            .split('\t')
                            .nth(code_column)
                            .unwrap_or_default()
                            .to_owned(),
                        content.to_owned(),
                    )
                })
                .collect();
            records.sort_by(|a, b| a.0.cmp(&b.0));
            for (index, (_, content)) in (start..end).zip(records) {
                let suffix = if original[index].ends_with("\r\n") {
                    "\r\n"
                } else if original[index].ends_with('\n') {
                    "\n"
                } else {
                    ""
                };
                lines[index] = format!("{content}{suffix}");
            }
        }
        plan.updated = lines.concat();
        let after = Document::parse(doc.path.clone(), plan.updated.clone());
        if doc
            .entries
            .windows(2)
            .all(|pair| pair[0].code <= pair[1].code)
            && !after
                .entries
                .windows(2)
                .all(|pair| pair[0].code <= pair[1].code)
        {
            bail!("新编码需要跨越分类块，拒绝改变分类归属");
        }
        Ok(plan)
    }

    /// All line selectors refer to the original document, including in batch edits.
    pub fn build(doc: &Document, operations: &[Operation]) -> Result<Self> {
        validate_document(doc)?;
        let mut lines: Vec<String> = doc.raw.split_inclusive('\n').map(str::to_owned).collect();
        let (text_col, code_col) = doc.columns();
        let mut touched = std::collections::BTreeSet::new();
        let mut additions = Vec::new();
        for operation in operations {
            match operation {
                Operation::Add { text, code } => {
                    validate(text, code)?;
                    if doc
                        .entries
                        .iter()
                        .any(|e| e.text == *text && e.code == *code)
                        || additions.iter().any(|(t, c)| t == text && c == code)
                    {
                        bail!("添加失败：词条与编码已存在");
                    }
                    additions.push((text.clone(), code.clone()));
                }
                Operation::Update { line, text, code } => {
                    if text.is_none() && code.is_none() {
                        bail!("更新必须提供 text 或 code");
                    }
                    let entry = doc
                        .entries
                        .iter()
                        .find(|e| e.line == *line)
                        .context("所选行不是可编辑词条")?;
                    if !touched.insert(*line) {
                        bail!("同一批次不能重复修改行 {line}");
                    }
                    let new_text = text.as_deref().unwrap_or(&entry.text);
                    let new_code = code.as_deref().unwrap_or(&entry.code);
                    validate(new_text, new_code)?;
                    let mut fields = entry.fields.clone();
                    fields[text_col] = new_text.into();
                    fields[code_col] = new_code.into();
                    let old = &lines[line - 1];
                    let suffix = if old.ends_with("\r\n") {
                        "\r\n"
                    } else if old.ends_with('\n') {
                        "\n"
                    } else {
                        ""
                    };
                    lines[line - 1] = format!("{}{suffix}", fields.join("\t"));
                }
                Operation::Remove { line } => {
                    if !doc.entries.iter().any(|e| e.line == *line) {
                        bail!("删除失败：行 {line} 不是词条");
                    }
                    if !touched.insert(*line) {
                        bail!("同一批次不能重复修改行 {line}");
                    }
                    lines[line - 1].clear();
                }
            }
        }
        let mut updated = lines.concat();
        for (text, code) in additions {
            let current = Document::parse(doc.path.clone(), updated);
            updated = Self::insert(&current, &text, &code, None)?.updated;
        }
        Ok(Self {
            path: doc.path.clone(),
            original: doc.raw.clone(),
            updated,
        })
    }
    pub fn diff(&self) -> String {
        TextDiff::from_lines(&self.original, &self.updated)
            .unified_diff()
            .header(
                &self.path.display().to_string(),
                &self.path.display().to_string(),
            )
            .to_string()
    }
    pub fn apply(&self) -> Result<()> {
        if fs::symlink_metadata(&self.path)?.file_type().is_symlink() {
            bail!("拒绝替换符号链接 {}", self.path.display());
        }
        let current =
            fs::read(&self.path).with_context(|| format!("复核 {}", self.path.display()))?;
        if Sha256::digest(&current) != Sha256::digest(self.original.as_bytes()) {
            bail!(
                "文件已变化，拒绝写入 {}，请重新生成编辑计划",
                self.path.display()
            );
        }
        if self.updated == self.original {
            return Ok(());
        }
        let parent = self.path.parent().context("编辑路径缺少父目录")?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.as_file()
            .set_permissions(fs::metadata(&self.path)?.permissions())?;
        temp.write_all(self.updated.as_bytes())?;
        temp.as_file().sync_all()?;
        // Optimistic conflict check immediately before atomic replacement.
        if fs::read(&self.path)? != current {
            bail!("文件已变化，拒绝写入 {}", self.path.display());
        }
        temp.persist(&self.path)
            .map_err(|e| e.error)
            .with_context(|| format!("原子替换 {}", self.path.display()))?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
}

fn validate_document(doc: &Document) -> Result<()> {
    if doc.header_error.is_some() || !doc.terminated_header {
        bail!("编辑 {}：词库头部无效或缺少 ...", doc.path.display());
    }
    doc.validate_columns()
}

fn append_record(doc: &Document, text: &str, code: &str) -> String {
    let (text_col, code_col) = doc.columns();
    let newline = if doc.raw.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let width = doc
        .header
        .as_ref()
        .and_then(|header| header.columns.as_ref())
        .map_or(2, Vec::len)
        .max(text_col.max(code_col) + 1);
    let mut fields = vec![String::new(); width];
    fields[text_col] = text.into();
    fields[code_col] = code.into();
    let mut updated = doc.raw.clone();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push_str(newline);
    }
    updated.push_str(&fields.join("\t"));
    updated.push_str(newline);
    updated
}

fn validate(text: &str, code: &str) -> Result<()> {
    if text.trim().is_empty()
        || code.is_empty()
        || text.contains(['\t', '\n', '\r'])
        || code.chars().any(char::is_whitespace)
        || text.trim_start().starts_with('#')
    {
        bail!("词条/编码不能为空，不得含换行或字段分隔符；词条不能以 # 开头");
    }
    Ok(())
}
