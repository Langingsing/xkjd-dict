//! Batch static reverse lookup: reverse_batch ROOT SCHEMA < texts.json
use anyhow::{Context, Result};
use std::{collections::BTreeMap, io, path::Path};
use xkjd_dict::{reverse::ReverseIndex, workspace::Workspace};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(
        args.len() == 2,
        "usage: reverse_batch ROOT SCHEMA < texts.json"
    );
    let texts: Vec<String> = serde_json::from_reader(io::stdin())
        .context("expected a JSON array of text strings on stdin")?;
    let workspace = Workspace::load(Path::new(&args[0]), &args[1])?;
    if let Some(issue) = workspace.import_issues.iter().find(|issue| issue.active) {
        anyhow::bail!(
            "cannot compare an incomplete import graph: {}: {}",
            issue.file.display(),
            issue.message
        );
    }
    let index = ReverseIndex::new(&workspace, false, &[])?;
    let mut results = BTreeMap::new();
    for text in texts {
        if !results.contains_key(&text) {
            results.insert(text.clone(), index.lookup(&text, 1)?);
        }
    }
    serde_json::to_writer(io::stdout(), &results)?;
    Ok(())
}
