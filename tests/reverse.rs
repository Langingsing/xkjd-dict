use std::{fs, process::Command};
use tempfile::TempDir;
use xkjd_dict::{
    reverse::{Certainty, ReclaimAssessment, ReverseIndex},
    workspace::Workspace,
};

fn fixture(body: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("xkjd6.schema.yaml"),
        "translator:\n  dictionary: user.xkjd6\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("user.xkjd6.dict.yaml"),
        format!("---\nname: user.xkjd6\n...\n{body}"),
    )
    .unwrap();
    dir
}

#[test]
fn reverse_distinguishes_shortest_existing_fewest_keys_and_ease() {
    let dir = fixture("会\thb\n悔\thb\n会\thbi\n看\tkf\n会看\thbkf\n会看\thbkfiu\n");
    let ws = Workspace::load(dir.path(), "xkjd6").unwrap();
    let index = ReverseIndex::new(&ws, false, &[]).unwrap();
    let result = index.lookup("会看", 5).unwrap();
    assert_eq!(result.shortest_existing, ["hbkf"]);
    assert_eq!(result.existing.len(), 2);
    assert_eq!(result.alternatives.fewest_keys[0].input, "hbkf ");
    let easier = &result.segmented.easier[0];
    assert_eq!(easier.input, "hbi kf ");
    assert_eq!(easier.burden.code_keys, 5);
    assert_eq!(easier.burden.confirmation_keys, 2);
    assert_eq!(easier.burden.boundaries, 1);
    assert_eq!(easier.burden.unresolved_selections, 0);
    let short = &result.segmented.fewest_keys[0];
    assert_eq!(short.input, "hb [核对候选:会] kf ");
    assert_eq!(short.burden.typed_keys, 6);
    assert_eq!(short.burden.selection_actions_lower_bound, 0);
    assert_eq!(short.burden.unknown_rank_segments, 1);
    assert_eq!(short.segments[0].certainty, Certainty::UnverifiedRank);
    let other_first = ReverseIndex::new(&ws, false, &["悔=hb".into()]).unwrap();
    let other = other_first.lookup("会看", 1).unwrap();
    assert_eq!(other.segmented.fewest_keys[0].input, "hb [选择:会] kf ");
    assert_eq!(
        other.segmented.fewest_keys[0]
            .burden
            .selection_actions_lower_bound,
        1
    );
    let confirmed = ReverseIndex::new(&ws, false, &["会=hb".into(), "看=kf".into()]).unwrap();
    let split = confirmed.lookup("会看", 1).unwrap();
    assert_eq!(split.segmented.easier[0].input, "hb kf ");
    assert_eq!(split.segmented.easier[0].burden.unresolved_selections, 0);
    let node = confirmed.reclaim("hbkf", 1, 1).unwrap();
    assert_eq!(
        node.candidates[0].assessment,
        ReclaimAssessment::ConfirmedCandidate
    );
    assert_eq!(node.candidates[0].extra_keys, Some(1));
    assert_eq!(
        confirmed.reclaim("hbkf", 1, 0).unwrap().candidates[0].assessment,
        ReclaimAssessment::ExtraKeys
    );
}

#[test]
fn segmentation_uses_global_search_not_longest_match_and_handles_unicode() {
    let dir = fixture("甲\ta\n甲乙\tabcd\n乙丙\tb\n丙\tcdef\n🙂\tx\n");
    let ws = Workspace::load(dir.path(), "xkjd6").unwrap();
    let index = ReverseIndex::new(&ws, false, &[]).unwrap();
    let result = index.lookup("甲乙丙", 1).unwrap();
    assert!(result.shortest_existing.is_empty());
    assert_eq!(result.alternatives.fewest_keys[0].input, "a b ");
    assert_eq!(
        index.lookup("甲🙂", 1).unwrap().alternatives.easier[0].input,
        "a x "
    );
    assert!(
        index
            .lookup("甲缺", 5)
            .unwrap()
            .alternatives
            .easier
            .is_empty()
    );
    assert!(index.lookup("甲缺", 5).unwrap().segmented.easier.is_empty());
}

#[test]
fn candidate_identity_deduplicates_sources_and_excludes_dormant_and_foreign_edges() {
    let dir = fixture("会\thb\n会\thb\n看\tkf\n");
    fs::write(
        dir.path().join("xkjd6.dormant.dict.yaml"),
        "---\nname: xkjd6.dormant\n...\n悔\thb\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("pinyin_simp.dict.yaml"),
        "---\nname: pinyin_simp\n...\n缺\tque\n",
    )
    .unwrap();
    let ws = Workspace::load(dir.path(), "xkjd6").unwrap();
    let index = ReverseIndex::new(&ws, false, &[]).unwrap();
    let result = index.lookup("会", 5).unwrap();
    assert_eq!(result.existing.len(), 1);
    assert_eq!(result.existing[0].certainty, Certainty::UniqueExact);
    assert_eq!(result.existing[0].candidates[0].sources.len(), 2);
    assert_eq!(result.alternatives.easier.len(), 1);
    let all = ReverseIndex::new(&ws, true, &[]).unwrap();
    assert_eq!(
        all.lookup("会", 1).unwrap().existing[0].certainty,
        Certainty::UnverifiedRank
    );
    assert!(all.lookup("缺", 1).unwrap().existing.is_empty());
}

#[test]
fn reclaim_cannot_use_its_whole_word_even_when_other_full_codes_exist() {
    let dir = fixture("会看\thbkf\n会看\thbkfiu\n");
    let ws = Workspace::load(dir.path(), "xkjd6").unwrap();
    let index = ReverseIndex::new(&ws, false, &[]).unwrap();
    let result = index.reclaim("hbkf", 3, 2).unwrap();
    assert_eq!(
        result.candidates[0].assessment,
        ReclaimAssessment::NoSegmentation
    );
    assert_eq!(result.candidates[0].extra_keys, None);
    assert!(result.candidates[0].segmented.easier.is_empty());
    assert!(index.reclaim("unused", 3, 2).unwrap().candidates.is_empty());
}

#[test]
fn evidence_and_limits_are_validated_without_guessing_rank() {
    let dir = fixture("会\thb\n悔\thb\n看\tkf\n会看\thbkf\n");
    let ws = Workspace::load(dir.path(), "xkjd6").unwrap();
    assert!(ReverseIndex::new(&ws, false, &["会=missing".into()]).is_err());
    assert!(ReverseIndex::new(&ws, false, &["会hb".into()]).is_err());
    assert!(ReverseIndex::new(&ws, false, &["会=hb".into(), "悔=hb".into()]).is_err());
    let index = ReverseIndex::new(&ws, false, &[]).unwrap();
    assert!(index.lookup("", 1).is_err());
    assert!(index.lookup(&"会".repeat(129), 1).is_err());
    assert!(index.lookup("会", 0).is_err());
    assert!(index.lookup("会", 21).is_err());
    assert!(index.reclaim("hb!", 1, 2).is_err());
    assert!(index.reclaim("unused", 0, 2).is_err());
    assert_eq!(
        index.reclaim("hbkf", 1, 2).unwrap().candidates[0].assessment,
        ReclaimAssessment::NeedsSelection
    );
    let all = ReverseIndex::new(&ws, true, &[]).unwrap();
    assert_eq!(
        all.reclaim("hbkf", 1, 2).unwrap().candidates[0].assessment,
        ReclaimAssessment::InactiveScope
    );
}

#[test]
fn reclaim_keeps_dictionary_uniqueness_distinct_from_observed_first_candidate() {
    let dir = fixture("会\thb\n看\tkf\n会看\thbkf\n");
    let ws = Workspace::load(dir.path(), "xkjd6").unwrap();
    let index = ReverseIndex::new(&ws, false, &[]).unwrap();
    assert_eq!(
        index.reclaim("hbkf", 1, 2).unwrap().candidates[0].assessment,
        ReclaimAssessment::StaticCandidate
    );
    let all = ReverseIndex::new(&ws, true, &[]).unwrap();
    assert_eq!(
        all.reclaim("hbkf", 1, 2).unwrap().candidates[0].assessment,
        ReclaimAssessment::InactiveScope
    );
    fs::write(
        dir.path().join("xkjd6.schema.yaml"),
        "translator:\n  dictionary: xkjd6.extended\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("xkjd6.extended.dict.yaml"),
        "---\nname: xkjd6.extended\nimport_tables: [user.xkjd6, missing]\n...\n",
    )
    .unwrap();
    let incomplete = Workspace::load(dir.path(), "xkjd6").unwrap();
    let index = ReverseIndex::new(&incomplete, false, &[]).unwrap();
    let result = index.reclaim("hbkf", 1, 2).unwrap();
    assert_eq!(
        result.candidates[0].assessment,
        ReclaimAssessment::IncompleteImports
    );
    assert_eq!(result.import_issues.len(), 1);
}

#[test]
fn confirmed_segmentation_is_not_hidden_by_a_shorter_static_plan_or_output_limit() {
    let dir = fixture("会\ta\n会\thb\n看\tb\n看\tkf\n会看\thbkf\n");
    let ws = Workspace::load(dir.path(), "xkjd6").unwrap();
    let index = ReverseIndex::new(&ws, false, &["会=hb".into(), "看=kf".into()]).unwrap();
    let result = index.lookup("会看", 1).unwrap();
    assert_eq!(result.segmented.easier[0].input, "a b ");
    assert_eq!(result.segmented.confirmed_first[0].input, "hb kf ");
    let result = index.reclaim("hbkf", 1, 2).unwrap();
    assert_eq!(
        result.candidates[0].assessment,
        ReclaimAssessment::ConfirmedCandidate
    );
    assert_eq!(
        result.candidates[0].recommended.as_ref().unwrap().input,
        "hb kf "
    );
    assert_eq!(result.candidates[0].extra_keys, Some(1));
    let strict = index.reclaim("hbkf", 1, 0).unwrap();
    assert_eq!(
        strict.candidates[0].assessment,
        ReclaimAssessment::StaticCandidate
    );
    assert_eq!(
        strict.candidates[0].recommended.as_ref().unwrap().input,
        "a b "
    );
}

#[test]
fn cli_json_is_read_only_and_uses_the_same_analysis_as_library() {
    let dir = fixture("会\thb\n悔\thb\n看\tkf\n回看\thbkf\n");
    let dict = dir.path().join("user.xkjd6.dict.yaml");
    let before = fs::read(&dict).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_xkjd-dict"))
        .arg("--root")
        .arg(dir.path())
        .args([
            "--json",
            "reverse",
            "会看",
            "--confirmed-first",
            "会=hb",
            "--limit",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["segmented"]["easier"][0]["input"], "hb kf ");
    assert_eq!(result["segmented"]["easier"][0]["burden"]["typed_keys"], 6);
    assert_eq!(fs::read(dict).unwrap(), before);
}
