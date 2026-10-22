//! Jiandao-only ordering inventory. Sections follow comments and blank lines;
//! changes of word length or code shape are diagnostics, not section boundaries.
use anyhow::{Context, Result};
use clap::Parser;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    collections::BTreeSet,
    fs,
    io::{BufWriter, Write},
    path::{Component, Path, PathBuf},
};
use xkjd_dict::{
    document::{Document, Entry},
    workspace::Workspace,
};

const MAX_INVERSION_SAMPLES: usize = 8;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    root: PathBuf,
    #[arg(long, default_value = "xkjd6")]
    schema: String,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Rule {
    Code,
    CodeDescending,
    WordLength,
    WordLengthDescending,
    WordLengthCode,
    WordLengthDescendingCode,
    CodeLengthCode,
}

const RULES: [Rule; 7] = [
    Rule::Code,
    Rule::CodeDescending,
    Rule::WordLength,
    Rule::WordLengthDescending,
    Rule::WordLengthCode,
    Rule::WordLengthDescendingCode,
    Rule::CodeLengthCode,
];

impl Rule {
    fn compare(self, a: &Entry, b: &Entry) -> Ordering {
        let lengths = || a.text.chars().count().cmp(&b.text.chars().count());
        match self {
            Self::Code => a.code.as_bytes().cmp(b.code.as_bytes()),
            Self::CodeDescending => a.code.as_bytes().cmp(b.code.as_bytes()).reverse(),
            Self::WordLength => lengths(),
            Self::WordLengthDescending => lengths().reverse(),
            Self::WordLengthCode => {
                lengths().then_with(|| a.code.as_bytes().cmp(b.code.as_bytes()))
            }
            Self::WordLengthDescendingCode => lengths()
                .reverse()
                .then_with(|| a.code.as_bytes().cmp(b.code.as_bytes())),
            Self::CodeLengthCode => a
                .code
                .chars()
                .count()
                .cmp(&b.code.chars().count())
                .then_with(|| a.code.as_bytes().cmp(b.code.as_bytes())),
        }
    }
}

#[derive(Serialize)]
struct Inversion {
    before_line: usize,
    after_line: usize,
    before_text: String,
    after_text: String,
    before_code: String,
    after_code: String,
}

#[derive(Serialize)]
struct Run {
    first_line: usize,
    last_line: usize,
    records: usize,
}

#[derive(Serialize)]
struct Measurement {
    rule: Rule,
    descents: usize,
    /// False for constant keys, including single-character word-length tests.
    informative: bool,
    /// Maximal monotone runs are diagnostics, not evidence of intended blocks.
    longest_monotone_run: Option<Run>,
    first_inversions: Vec<Inversion>,
}

fn measure(entries: &[&Entry], rule: Rule) -> Measurement {
    let mut descents = 0;
    let mut informative = false;
    let mut inversions = vec![];
    let mut best = (0, 0);
    let mut start = 0;
    for (index, pair) in entries.windows(2).enumerate() {
        let comparison = rule.compare(pair[0], pair[1]);
        informative |= comparison != Ordering::Equal;
        if comparison == Ordering::Greater {
            descents += 1;
            if index + 1 - start > best.1 - best.0 {
                best = (start, index + 1);
            }
            start = index + 1;
            if inversions.len() < MAX_INVERSION_SAMPLES {
                inversions.push(Inversion {
                    before_line: pair[0].line,
                    after_line: pair[1].line,
                    before_text: pair[0].text.clone(),
                    after_text: pair[1].text.clone(),
                    before_code: pair[0].code.clone(),
                    after_code: pair[1].code.clone(),
                });
            }
        }
    }
    if entries.len() - start > best.1 - best.0 {
        best = (start, entries.len());
    }
    let longest_monotone_run = (best.1 > best.0).then(|| Run {
        first_line: entries[best.0].line,
        last_line: entries[best.1 - 1].line,
        records: best.1 - best.0,
    });
    Measurement {
        rule,
        descents,
        informative,
        longest_monotone_run,
        first_inversions: inversions,
    }
}

#[derive(Serialize)]
struct Block {
    first_line: usize,
    last_line: usize,
    records: usize,
    ordered_by: Vec<Rule>,
    code_ascending: bool,
    code_descending: bool,
}

#[derive(Serialize)]
struct FileOrder {
    file: PathBuf,
    active: bool,
    sha256: String,
    records: usize,
    distinct_word_lengths: BTreeSet<usize>,
    malformed_lines: Vec<usize>,
    whole: Vec<Measurement>,
    sections: Vec<Section>,
    word_length_blocks: Vec<Block>,
    code_length_blocks: Vec<Block>,
    first_code_letter_blocks: Vec<Block>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    Start,
    Comment,
    BlankLine,
    CommentAndBlank,
}

#[derive(Serialize)]
struct Heading {
    line: usize,
    title: String,
}

#[derive(Serialize)]
struct Section {
    headings: Vec<Heading>,
    boundary: Boundary,
    range: Block,
    distinct_word_lengths: BTreeSet<usize>,
    measurements: Vec<Measurement>,
}

fn sections(doc: &Document) -> Vec<Section> {
    let lines: Vec<_> = doc.raw.lines().collect();
    let mut previous_end = doc.body_start;
    blocks(doc)
        .into_iter()
        .map(|entries| {
            let start = entries[0].line - 1;
            let gap = &lines[previous_end..start];
            let headings: Vec<_> = gap
                .iter()
                .enumerate()
                .filter_map(|(offset, line)| {
                    let title = line.trim().strip_prefix('#')?.trim();
                    (!title.is_empty() && !line.contains('\t')).then(|| Heading {
                        line: previous_end + offset + 1,
                        title: title.into(),
                    })
                })
                .collect();
            let blank = gap.iter().any(|line| line.trim().is_empty());
            let boundary = match (!headings.is_empty(), blank) {
                (true, true) => Boundary::CommentAndBlank,
                (true, false) => Boundary::Comment,
                (false, true) => Boundary::BlankLine,
                (false, false) => Boundary::Start,
            };
            previous_end = entries.last().unwrap().line;
            Section {
                headings,
                boundary,
                distinct_word_lengths: entries
                    .iter()
                    .map(|entry| entry.text.chars().count())
                    .collect(),
                measurements: RULES
                    .into_iter()
                    .map(|rule| measure(&entries, rule))
                    .collect(),
                range: block_reports(vec![entries]).pop().unwrap(),
            }
        })
        .collect()
}

fn blocks(doc: &Document) -> Vec<Vec<&Entry>> {
    let lines: Vec<_> = doc.raw.lines().collect();
    let mut blocks: Vec<Vec<&Entry>> = vec![];
    for entry in &doc.entries {
        let separated = blocks
            .last()
            .and_then(|block| block.last())
            .is_some_and(|previous| {
                let gap = &lines[previous.line..entry.line - 1];
                !gap.is_empty()
                    && gap
                        .iter()
                        .all(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
                    && gap
                        .iter()
                        .any(|line| line.trim().is_empty() || !line.contains('\t'))
            });
        if blocks.is_empty() || separated {
            blocks.push(vec![]);
        }
        blocks.last_mut().unwrap().push(entry);
    }
    blocks
}

fn partition(entries: &[Entry], boundary: impl Fn(&Entry, &Entry) -> bool) -> Vec<Vec<&Entry>> {
    let mut groups: Vec<Vec<&Entry>> = vec![];
    for entry in entries {
        if groups
            .last()
            .and_then(|group| group.last())
            .is_none_or(|previous| boundary(previous, entry))
        {
            groups.push(vec![]);
        }
        groups.last_mut().unwrap().push(entry);
    }
    groups
}

fn block_reports(groups: Vec<Vec<&Entry>>) -> Vec<Block> {
    groups
        .into_iter()
        .map(|entries| Block {
            first_line: entries[0].line,
            last_line: entries.last().unwrap().line,
            records: entries.len(),
            code_ascending: entries
                .windows(2)
                .all(|pair| Rule::Code.compare(pair[0], pair[1]) != Ordering::Greater),
            code_descending: entries
                .windows(2)
                .all(|pair| Rule::CodeDescending.compare(pair[0], pair[1]) != Ordering::Greater),
            ordered_by: RULES
                .into_iter()
                .filter(|rule| {
                    entries
                        .windows(2)
                        .all(|pair| rule.compare(pair[0], pair[1]) != Ordering::Greater)
                        && entries
                            .windows(2)
                            .any(|pair| rule.compare(pair[0], pair[1]) != Ordering::Equal)
                })
                .collect(),
        })
        .collect()
}

fn write_report(root: &Path, output: &Path, results: &[FileOrder]) -> Result<()> {
    // Reject parent traversal: resolving it lexically would change the meaning
    // of paths whose earlier components are symlinks.
    let absolute = if output.is_absolute() {
        output.to_owned()
    } else {
        std::env::current_dir()?.join(output)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                anyhow::bail!("报告路径不能包含 ..：{}", output.display());
            }
            Component::CurDir => {}
            _ => normalized.push(component.as_os_str()),
        }
    }
    let parent = normalized.parent().context("输出路径没有父目录")?;
    let mut ancestor = parent;
    while !ancestor.exists() {
        ancestor = ancestor.parent().context("输出路径没有已存在的父目录")?;
    }
    anyhow::ensure!(
        !fs::canonicalize(ancestor)?.starts_with(fs::canonicalize(root)?),
        "报告必须输出到词库目录之外：{}",
        output.display()
    );
    if let Ok(metadata) = fs::symlink_metadata(&normalized) {
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "报告目标不能是符号链接：{}",
            output.display()
        );
    }
    fs::create_dir_all(parent)?;
    // Atomic replacement also prevents writing through a hard link to a source.
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut buffered = BufWriter::new(&mut temporary);
        serde_json::to_writer_pretty(&mut buffered, results)?;
        buffered.flush()?;
    }
    temporary
        .persist(&normalized)
        .with_context(|| format!("写入报告 {}", output.display()))?;
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let ws = Workspace::load(&args.root, &args.schema)?;
    let mut results = vec![];
    for doc in ws.documents.iter().filter(|doc| {
        doc.path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().contains("xkjd6"))
    }) {
        let entries: Vec<_> = doc.entries.iter().collect();
        let sections = sections(doc);
        let digest = format!("{:x}", Sha256::digest(doc.raw.as_bytes()));
        // Verify the complete source bytes remain equal to the parsed snapshot.
        let current = fs::read(&doc.path)?;
        anyhow::ensure!(
            current == doc.raw.as_bytes(),
            "分析时文件发生变化：{}",
            doc.path.display()
        );
        results.push(FileOrder {
            file: doc.path.clone(),
            active: ws.active_files.contains(&doc.path),
            sha256: digest,
            records: entries.len(),
            distinct_word_lengths: entries
                .iter()
                .map(|entry| entry.text.chars().count())
                .collect(),
            malformed_lines: doc.malformed_lines.clone(),
            whole: RULES
                .into_iter()
                .map(|rule| measure(&entries, rule))
                .collect(),
            sections,
            word_length_blocks: block_reports(partition(&doc.entries, |a, b| {
                a.text.chars().count() != b.text.chars().count()
            })),
            code_length_blocks: block_reports(partition(&doc.entries, |a, b| {
                a.code.chars().count() != b.code.chars().count()
            })),
            first_code_letter_blocks: block_reports(partition(&doc.entries, |a, b| {
                a.code.chars().next() != b.code.chars().next()
            })),
        });
    }
    write_report(&ws.root, &args.output, &results)?;
    println!(
        "{} files; {} records; {}",
        results.len(),
        results.iter().map(|r| r.records).sum::<usize>(),
        args.output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn doc(body: &str) -> Document {
        Document::parse(
            "fixture.dict.yaml".into(),
            format!("---\nname: fixture\n...\n{body}"),
        )
    }
    #[test]
    fn explicit_blocks_and_trivial_lengths_are_distinguished_from_monotone_runs() {
        let doc = doc("甲\tb\n乙\tc\n\n# second\n丙\ta\n丁\td\n");
        let entries: Vec<_> = doc.entries.iter().collect();
        assert_eq!(measure(&entries, Rule::Code).descents, 1);
        assert!(!measure(&entries, Rule::WordLength).informative);
        assert_eq!(blocks(&doc).len(), 2);
        assert!(
            blocks(&doc)
                .iter()
                .all(|block| measure(block, Rule::Code).descents == 0)
        );
        let without_gap = self::doc("甲\tb\n乙\tc\n丙\ta\n丁\td\n");
        assert_eq!(blocks(&without_gap).len(), 1);
        let commented_record = self::doc("甲\tb\n#停用\ta\n乙\tc\n");
        assert_eq!(blocks(&commented_record).len(), 1);
        let blanks = self::doc("# 分类甲\n甲\tb\n\n乙\tc\n# 分类乙\n丙\ta\n");
        let sections = sections(&blanks);
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].headings[0].title, "分类甲");
        assert!(matches!(sections[1].boundary, Boundary::BlankLine));
        assert_eq!(sections[2].headings[0].title, "分类乙");
        assert!(matches!(sections[2].boundary, Boundary::Comment));
    }
    #[test]
    fn length_order_is_not_mistaken_for_length_then_code_order() {
        let doc = doc("甲\tz\n乙\ta\n甲乙\tb\n");
        let entries: Vec<_> = doc.entries.iter().collect();
        assert_eq!(measure(&entries, Rule::WordLength).descents, 0);
        assert_eq!(measure(&entries, Rule::WordLengthCode).descents, 1);
        let reverse = self::doc("甲\tz\n乙\ta\n");
        let reversed_entries: Vec<_> = reverse.entries.iter().collect();
        assert_eq!(measure(&reversed_entries, Rule::CodeDescending).descents, 0);
        assert_eq!(
            measure(&[], Rule::Code)
                .longest_monotone_run
                .map(|r| r.records),
            None
        );
    }

    #[test]
    fn report_cannot_overwrite_sources_or_write_through_links() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("rime");
        fs::create_dir(&root).unwrap();
        let source = root.join("source.dict.yaml");
        fs::write(&source, "source bytes").unwrap();
        assert!(write_report(&root, &source, &[]).is_err());
        assert!(write_report(&root, &root.join("missing/../report.json"), &[]).is_err());
        assert!(!root.join("missing").exists());
        let linked = temp.path().join("linked.json");
        fs::hard_link(&source, &linked).unwrap();
        write_report(&root, &linked, &[]).unwrap();
        assert_eq!(fs::read_to_string(&source).unwrap(), "source bytes");
        assert_eq!(fs::read_to_string(&linked).unwrap(), "[]");
        #[cfg(unix)]
        {
            let link = temp.path().join("symlink.json");
            std::os::unix::fs::symlink(&source, &link).unwrap();
            assert!(write_report(&root, &link, &[]).is_err());
            let directory_link = temp.path().join("rime-link");
            std::os::unix::fs::symlink(&root, &directory_link).unwrap();
            assert!(write_report(&root, &directory_link.join("report.json"), &[]).is_err());
        }
    }
}
