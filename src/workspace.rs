use crate::document::{Document, Entry};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Default, Debug)]
struct Node {
    children: BTreeMap<char, Node>,
    entries: Vec<usize>,
}

/// Owned nodes and stable entry IDs; no parent references or unsafe lifetimes.
#[derive(Default, Debug)]
pub struct Trie {
    root: Node,
}

impl Trie {
    fn render(&self, prefix: &str, entries: &[Entry]) -> String {
        let mut node = &self.root;
        for ch in prefix.chars() {
            let Some(child) = node.children.get(&ch) else {
                return String::new();
            };
            node = child;
        }
        fn label(node: &Node, entries: &[Entry]) -> String {
            let mut words = Vec::new();
            for &id in &node.entries {
                let word = entries[id].text.as_str();
                if !words.contains(&word) {
                    words.push(word);
                }
            }
            if words.is_empty() {
                String::new()
            } else {
                format!("  {}", words.join("、"))
            }
        }
        fn children(node: &Node, entries: &[Entry], indent: &str, out: &mut String) {
            for (index, (ch, child)) in node.children.iter().enumerate() {
                let last = index + 1 == node.children.len();
                out.push_str(&format!(
                    "{indent}{} {ch}{}\n",
                    if last { "└─" } else { "├─" },
                    label(child, entries)
                ));
                children(
                    child,
                    entries,
                    &format!("{indent}{}", if last { "   " } else { "│  " }),
                    out,
                );
            }
        }
        let mut out = format!("{prefix}{}\n", label(node, entries));
        children(node, entries, "", &mut out);
        out
    }

    pub fn insert(&mut self, code: &str, entry: usize) {
        let mut node = &mut self.root;
        for ch in code.chars() {
            node = node.children.entry(ch).or_default();
        }
        node.entries.push(entry);
    }
    pub fn prefix(&self, code: &str) -> Vec<usize> {
        let mut node = &self.root;
        for ch in code.chars() {
            match node.children.get(&ch) {
                Some(n) => node = n,
                None => return vec![],
            }
        }
        fn visit(node: &Node, out: &mut Vec<usize>) {
            out.extend(&node.entries);
            for child in node.children.values() {
                visit(child, out);
            }
        }
        let mut out = Vec::new();
        visit(node, &mut out);
        out
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportIssue {
    pub file: PathBuf,
    pub message: String,
    pub active: bool,
}

#[derive(Deserialize)]
struct Schema {
    translator: Translator,
}
#[derive(Deserialize)]
struct Translator {
    dictionary: String,
}

#[derive(Debug)]
pub struct Workspace {
    pub root: PathBuf,
    pub dictionary: String,
    pub documents: Vec<Document>,
    pub entries: Vec<Entry>,
    pub active_files: BTreeSet<PathBuf>,
    pub import_issues: Vec<ImportIssue>,
    pub trie: Trie,
    pub by_text: BTreeMap<String, Vec<usize>>,
    pub by_code: BTreeMap<String, Vec<usize>>,
    pub(crate) schema_path: PathBuf,
    pub(crate) schema_text: String,
}

impl Workspace {
    pub fn load(root: &Path, schema: &str) -> Result<Self> {
        if schema.contains(['/', '\\']) {
            bail!("schema 必须是名称而不是路径");
        }
        let root =
            fs::canonicalize(root).with_context(|| format!("读取词库目录 {}", root.display()))?;
        let schema_path = root.join(format!("{schema}.schema.yaml"));
        let schema_text = fs::read_to_string(&schema_path)
            .with_context(|| format!("读取 {}", schema_path.display()))?;
        let schema_yaml: Schema =
            serde_yaml_ng::from_str(&schema_text).context("解析 schema YAML")?;
        let dictionary = schema_yaml.translator.dictionary;
        let mut paths = fs::read_dir(&root)?
            .map(|r| r.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.retain(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".dict.yaml"))
                && p.is_file()
        });
        paths.sort();
        let documents = paths
            .iter()
            .map(|p| Document::read(p))
            .collect::<Result<Vec<_>>>()?;
        let mut ws = Self {
            root,
            dictionary,
            documents,
            entries: vec![],
            active_files: BTreeSet::new(),
            import_issues: vec![],
            trie: Trie::default(),
            by_text: BTreeMap::new(),
            by_code: BTreeMap::new(),
            schema_path,
            schema_text,
        };
        let main = ws.root.join(format!("{}.dict.yaml", ws.dictionary));
        ws.walk_import(&main, &mut BTreeSet::new(), true);
        // Validate even dormant import graphs without treating them as active.
        let all_paths: Vec<_> = ws.documents.iter().map(|d| d.path.clone()).collect();
        for path in all_paths {
            ws.walk_import(&path, &mut BTreeSet::new(), false);
        }
        ws.import_issues.sort_by(|a, b| {
            (&a.file, &a.message, !a.active).cmp(&(&b.file, &b.message, !b.active))
        });
        ws.import_issues
            .dedup_by(|a, b| a.file == b.file && a.message == b.message);
        for doc in &ws.documents {
            for entry in &doc.entries {
                let id = ws.entries.len();
                ws.trie.insert(&entry.code, id);
                ws.by_text.entry(entry.text.clone()).or_default().push(id);
                ws.by_code.entry(entry.code.clone()).or_default().push(id);
                ws.entries.push(entry.clone());
            }
        }
        Ok(ws)
    }

    fn walk_import(&mut self, path: &Path, stack: &mut BTreeSet<PathBuf>, active: bool) {
        if stack.contains(path) {
            self.import_issues.push(ImportIssue {
                file: path.to_owned(),
                message: "import_tables 循环引用".into(),
                active,
            });
            return;
        }
        let Some(doc) = self.documents.iter().find(|d| d.path == path) else {
            self.import_issues.push(ImportIssue {
                file: path.to_owned(),
                message: "导入文件不存在或不在词库目录内".into(),
                active,
            });
            return;
        };
        let imports = doc
            .header
            .as_ref()
            .map(|h| h.import_tables.clone())
            .unwrap_or_default();
        if active {
            self.active_files.insert(path.to_owned());
        }
        stack.insert(path.to_owned());
        for name in imports {
            if name.contains(['/', '\\']) || name == ".." {
                self.import_issues.push(ImportIssue {
                    file: path.to_owned(),
                    message: format!("不支持跨目录导入 {name}"),
                    active,
                });
            } else {
                self.walk_import(&self.root.join(format!("{name}.dict.yaml")), stack, active);
            }
        }
        stack.remove(path);
    }

    pub fn selected(&self, entry: &Entry, all: bool) -> bool {
        all || self.active_files.contains(&entry.file)
    }
    /// Prefix family ordered by code length/code, independent of candidate rank.
    pub fn group(&self, prefix: &str, length: Option<usize>, all: bool) -> Result<Vec<Entry>> {
        if prefix.is_empty() || length == Some(0) {
            bail!("前缀不能为空，词长必须大于零");
        }
        let mut entries: Vec<_> = self
            .query(None, None, Some(prefix), all)
            .into_iter()
            .filter(|entry| length.is_none_or(|length| entry.text.chars().count() == length))
            .cloned()
            .collect();
        entries.sort_by(|a, b| {
            (a.code.len(), &a.code, &a.file, a.line).cmp(&(b.code.len(), &b.code, &b.file, b.line))
        });
        Ok(entries)
    }

    /// Render only selected entries; empty intermediate prefixes remain visible.
    pub fn group_tree(&self, prefix: &str, length: Option<usize>, all: bool) -> Result<String> {
        let entries = self.group(prefix, length, all)?;
        let mut trie = Trie::default();
        for (id, entry) in entries.iter().enumerate() {
            trie.insert(&entry.code, id);
        }
        Ok(trie.render(prefix, &entries))
    }
    pub fn query(
        &self,
        text: Option<&str>,
        code: Option<&str>,
        prefix: Option<&str>,
        all: bool,
    ) -> Vec<&Entry> {
        let mut ids = if let Some(t) = text {
            self.by_text.get(t).cloned().unwrap_or_default()
        } else if let Some(c) = code {
            self.by_code.get(c).cloned().unwrap_or_default()
        } else {
            self.trie.prefix(prefix.unwrap_or(""))
        };
        ids.sort_unstable();
        ids.into_iter()
            .map(|id| &self.entries[id])
            .filter(|e| {
                self.selected(e, all)
                    && code.is_none_or(|c| e.code == c)
                    && prefix.is_none_or(|p| e.code.starts_with(p))
            })
            .collect()
    }
}
