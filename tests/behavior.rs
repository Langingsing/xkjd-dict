use std::fs;
use tempfile::TempDir;
use xkjd_dict::{
    audit::{self, Severity},
    document::Document,
    edit::{EditPlan, Operation},
    rules::{Encoder, Profile},
    workspace::Workspace,
};

fn fixture() -> TempDir {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("xkjd6.schema.yaml"),
        "translator:\n  dictionary: xkjd6.extended\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("xkjd6.extended.dict.yaml"),
        "---\nname: xkjd6.extended\nimport_tables:\n  - user.xkjd6\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("user.xkjd6.dict.yaml"),
        "---\nname: user.xkjd6\n...\n你好\tnkhzi\n",
    )
    .unwrap();
    fs::write(temp.path().join("xkjd6dz.dict.yaml"),"---\nname: xkjd6dz\n...\n你\tnkiuai\n好\thzauai\n重\twyuvia\n重\tfyuvia\n庆\tqgov\n国\tgliavv\n中\tfyiavi\n长\tjpuv\n长\tqpuv\n").unwrap();
    temp
}

#[test]
fn lossless_edits_keep_comments_fields_and_line_endings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.dict.yaml");
    let raw = "# 说明\r\n---\r\nname: x\r\nsort: original\r\n...\r\n# 分组\r\n甲\tjs\t10\r\n\r\n乙\tyk\t20";
    fs::write(&path, raw).unwrap();
    let doc = Document::read(&path).unwrap();
    assert_eq!(EditPlan::build(&doc, &[]).unwrap().updated, raw);
    let plan = EditPlan::build(
        &doc,
        &[Operation::Update {
            line: 7,
            text: None,
            code: Some("jsi".into()),
        }],
    )
    .unwrap();
    assert_eq!(plan.updated, raw.replace("甲\tjs\t10", "甲\tjsi\t10"));
    plan.apply().unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), plan.updated);
}

#[test]
fn deleting_middle_and_last_preserves_tail() {
    let raw = "---\nname: x\n...\n甲\tjs\n乙\tyk\n丙\tbg\n# 尾注\n";
    let doc = Document::parse("x.dict.yaml".into(), raw.into());
    let plan = EditPlan::build(
        &doc,
        &[Operation::Remove { line: 4 }, Operation::Remove { line: 6 }],
    )
    .unwrap();
    assert_eq!(plan.updated, "---\nname: x\n...\n乙\tyk\n# 尾注\n");
    assert!(
        EditPlan::build(
            &doc,
            &[Operation::Remove { line: 4 }, Operation::Remove { line: 4 }]
        )
        .is_err()
    );
}

#[test]
fn concurrent_change_is_rejected_without_overwriting() {
    let temp = fixture();
    let file = temp.path().join("user.xkjd6.dict.yaml");
    let doc = Document::read(&file).unwrap();
    let plan = EditPlan::build(
        &doc,
        &[Operation::Add {
            text: "你好啊".into(),
            code: "nhx".into(),
        }],
    )
    .unwrap();
    fs::write(&file, "external edit").unwrap();
    assert!(plan.apply().unwrap_err().to_string().contains("文件已变化"));
    assert_eq!(fs::read_to_string(file).unwrap(), "external edit");
}

#[test]
fn columns_and_missing_final_newline_are_preserved() {
    let doc = Document::parse(
        "x.dict.yaml".into(),
        "---\nname: x\ncolumns: [code, text, weight]\n...\njs\t甲\t3".into(),
    );
    assert_eq!(doc.entries[0].text, "甲");
    let plan = EditPlan::build(
        &doc,
        &[Operation::Add {
            text: "乙".into(),
            code: "yk".into(),
        }],
    )
    .unwrap();
    assert!(plan.updated.ends_with("js\t甲\t3\nyk\t乙\t\n"));
    let updated = Document::parse(doc.path.clone(), plan.updated);
    assert_eq!(updated.entries.len(), 2);
}

#[test]
fn trie_query_imports_and_dormant_files() {
    let temp = fixture();
    let ws = Workspace::load(temp.path(), "xkjd6").unwrap();
    assert_eq!(ws.query(Some("你"), None, None, false).len(), 0);
    assert_eq!(ws.query(Some("你"), None, None, true).len(), 1);
    assert_eq!(ws.query(None, None, Some("nk"), false).len(), 1);
    assert_eq!(ws.query(Some("你好"), Some("wrong"), None, true).len(), 0);
    assert!(ws.import_issues.is_empty());
}

#[test]
fn encoding_lengths_polyphony_and_overrides() {
    let temp = fixture();
    let ws = Workspace::load(temp.path(), "xkjd6").unwrap();
    let enc = Encoder::from_workspace(&ws);
    assert_eq!(
        enc.encode("你好", Profile::Standard).candidates,
        vec!["nkhzia"]
    );
    assert_eq!(
        enc.encode("你好国", Profile::Standard).candidates,
        vec!["nhgiai"]
    );
    assert_eq!(
        enc.encode("你好国你好", Profile::Standard).candidates,
        vec!["nhghia"]
    );
    assert_eq!(
        enc.encode("重庆", Profile::Standard).candidates,
        vec!["fyqguo", "wyqguo"]
    );
    assert_eq!(
        enc.encode("长", Profile::Standard).candidates,
        vec!["jpuv", "qpuv"]
    );
    assert_eq!(enc.encode("你好", Profile::Css).candidates, vec!["nau"]);
    assert_eq!(
        enc.encode("你", Profile::CssSingle).candidates,
        vec!["niua"]
    );
    assert_eq!(enc.encode("你好", Profile::Initials).candidates, vec!["nh"]);
    assert_eq!(
        enc.encode("未知", Profile::Standard).missing,
        vec!['未', '知']
    );
    let enc = enc.with_overrides(&["重=wyuvia".into()]).unwrap();
    assert_eq!(enc.encode("重庆", Profile::Standard).candidates.len(), 1);
}

#[test]
fn audit_distinguishes_structure_reading_shape_and_foreign() {
    let temp = fixture();
    fs::write(temp.path().join("user.xkjd6.dict.yaml"),"---\nname: wrong\n...\n你好\tnkhzi\n你好\tnkhzv\n你好\txxxx\n你好\tnk!\n未知\twq\n你好\tnkhzi\n").unwrap();
    fs::write(
        temp.path().join("pinyin_simp.dict.yaml"),
        "---\nname: pinyin_simp\n...\n你\tni\t100\n",
    )
    .unwrap();
    let ws = Workspace::load(temp.path(), "xkjd6").unwrap();
    let report = audit::check(&ws, &Encoder::from_workspace(&ws), true);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.rule == "shape_or_prefix_mismatch" && d.severity == Severity::Error)
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.rule == "reading_mismatch" && d.severity == Severity::Review)
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.rule == "code_alphabet" && d.severity == Severity::Error)
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.rule == "missing_reference" && d.severity == Severity::Review)
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.rule == "dictionary_name")
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.rule == "duplicate_pair")
    );
    assert_eq!(report.coverage[&Profile::Foreign].not_applicable, 1);
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.file.ends_with("pinyin_simp.dict.yaml"))
    );
}

#[test]
fn imports_report_missing_and_cycles_even_when_dormant() {
    let temp = fixture();
    fs::write(
        temp.path().join("other.dict.yaml"),
        "---\nname: other\nimport_tables: [other, absent]\n",
    )
    .unwrap();
    let ws = Workspace::load(temp.path(), "xkjd6").unwrap();
    assert!(ws.import_issues.iter().any(|i| i.message.contains("循环")));
    assert!(
        ws.import_issues
            .iter()
            .any(|i| i.message.contains("不存在"))
    );
    assert!(
        !ws.active_files
            .contains(&temp.path().join("other.dict.yaml"))
    );
    let active = audit::check(&ws, &Encoder::from_workspace(&ws), false);
    assert!(!active.diagnostics.iter().any(|d| d.rule == "import"));
}

#[test]
fn special_codes_are_not_judged_as_standard_phrase_codes() {
    let temp = fixture();
    for (name, body) in [
        ("abbr.xkjd6", "木\tv\n"),
        ("syxb.xkjd6dz", "视\tekei\n"),
        ("xkjd6.wxw", "你好\tna\n"),
        ("xkjd6.wxwdanzi", "你\tniu\n"),
        ("shengsheng.xkjd6", "你好\tnh\n"),
        ("symbols.xkjd6", "？\t;w\n"),
    ] {
        fs::write(
            temp.path().join(format!("{name}.dict.yaml")),
            format!("---\nname: {name}\n...\n{body}"),
        )
        .unwrap();
    }
    let ws = Workspace::load(temp.path(), "xkjd6").unwrap();
    let report = audit::check(&ws, &Encoder::from_workspace(&ws), true);
    assert_eq!(report.count(Severity::Error), 0);
    assert_eq!(report.coverage[&Profile::Css].matched, 1);
    assert_eq!(report.coverage[&Profile::Syxb].review, 1);
}

#[test]
fn renamed_dicts_keep_standard_rules_and_special_prefixes_keep_their_profiles() {
    use std::path::Path;
    for name in ["compose", "user", "subject", "media", "life", "name"] {
        assert_eq!(
            xkjd_dict::rules::profile(Path::new(&format!("{name}.xkjd6.dict.yaml"))),
            Profile::Standard
        );
    }
    for (name, expected) in [
        ("abbr", Profile::Shape),
        ("math", Profile::Custom),
        ("private", Profile::Custom),
        ("symbols", Profile::Symbol),
        ("shengsheng", Profile::Initials),
    ] {
        assert_eq!(
            xkjd_dict::rules::profile(Path::new(&format!("{name}.xkjd6.dict.yaml"))),
            expected
        );
    }
}

#[test]
fn suggestions_include_collisions_and_descendant_impact() {
    let temp = fixture();
    let ws = Workspace::load(temp.path(), "xkjd6").unwrap();
    let result = xkjd_dict::rules::suggest(
        &ws,
        &Encoder::from_workspace(&ws),
        "你好",
        Profile::Standard,
        false,
    );
    let choice = &result.choices[0];
    assert_eq!(choice.shortest_free.as_deref(), Some("nkh"));
    assert!(
        choice
            .slots
            .iter()
            .any(|s| s.code == "nkhzi" && s.same_code.len() == 1)
    );
    assert!(
        choice
            .slots
            .iter()
            .any(|s| s.code == "nkh" && s.descendant_entries == 1)
    );
}

#[test]
fn guided_dictionary_allows_long_disambiguating_suffixes() {
    let temp = fixture();
    fs::write(
        temp.path().join("xkjd6.iboot.dict.yaml"),
        "---\nname: xkjd6.iboot\n...\n不亦说乎\tibyyhvoouo\n",
    )
    .unwrap();
    let ws = Workspace::load(temp.path(), "xkjd6").unwrap();
    let report = audit::check(&ws, &Encoder::from_workspace(&ws), true);
    assert_eq!(report.count(Severity::Error), 0);
    assert_eq!(report.coverage[&Profile::Guided].review, 1);
}

#[test]
fn malformed_delimiter_recovers_rows_for_audit_but_cannot_be_edited() {
    let doc = Document::parse("x.dict.yaml".into(), "---\nname: x\n甲\tjs\n".into());
    assert_eq!(doc.entries.len(), 1);
    assert!(!doc.terminated_header);
    assert!(EditPlan::build(&doc, &[]).is_err());
}

#[test]
fn unsupported_encoding_is_explicit_and_overrides_are_validated() {
    let temp = fixture();
    let ws = Workspace::load(temp.path(), "xkjd6").unwrap();
    let enc = Encoder::from_workspace(&ws);
    assert!(!enc.encode("你", Profile::Css).issues.is_empty());
    assert!(!enc.encode("B站", Profile::Standard).issues.is_empty());
    assert!(enc.with_overrides(&["你=bad".into()]).is_err());
}

#[test]
fn phrase_membership_checks_middle_characters_without_using_foreign_dicts() {
    let temp = fixture();
    fs::write(
        temp.path().join("user.xkjd6.dict.yaml"),
        "---\nname: user.xkjd6\n...\n你\tn\n好\thz\n你好国缺缺你好\tnhgni\n你🙂好\tnkhz\n",
    )
    .unwrap();
    // A large pinyin dictionary must not mask a missing Jiandao single entry.
    fs::write(
        temp.path().join("pinyin_simp.dict.yaml"),
        "---\nname: pinyin_simp\n...\n缺\tque\n",
    )
    .unwrap();
    let ws = Workspace::load(temp.path(), "xkjd6").unwrap();
    let report = xkjd_dict::coverage::check(&ws, true);
    assert_eq!(report.absent_from_all.len(), 1);
    assert_eq!(report.absent_from_all[0].characters[0].character, '缺');
    assert_eq!(
        report.absent_from_all[0].characters[0].positions_one_based,
        vec![4, 5]
    );
    assert_eq!(report.only_in_inactive.len(), 1);
    assert_eq!(report.only_in_inactive[0].characters[0].character, '国');
    assert_eq!(report.ignored_non_han_scalars, 1);
    let audit = audit::check(&ws, &Encoder::from_workspace(&ws), false);
    assert_eq!(audit.character_coverage.absent_from_all.len(), 1);
}

#[test]
fn edits_reject_text_that_the_parser_would_treat_as_a_comment() {
    let doc = Document::parse("x.dict.yaml".into(), "---\nname: x\n...\n甲\tjs\n".into());
    for text in ["  #隐藏词条", "\u{3000}#隐藏词条", "   "] {
        assert!(
            EditPlan::build(
                &doc,
                &[Operation::Add {
                    text: text.into(),
                    code: "abc".into()
                }]
            )
            .is_err()
        );
        assert!(
            EditPlan::build(
                &doc,
                &[Operation::Update {
                    line: 4,
                    text: Some(text.into()),
                    code: None
                }]
            )
            .is_err()
        );
    }
}

#[test]
fn cli_preview_apply_and_batch_have_real_effect_only_when_requested() {
    use std::process::Command;
    let temp = fixture();
    let path = temp.path().join("user.xkjd6.dict.yaml");
    let initial = fs::read(&path).unwrap();
    let binary = env!("CARGO_BIN_EXE_xkjd-dict");
    let preview = Command::new(binary)
        .args([
            "--root",
            temp.path().to_str().unwrap(),
            "add",
            "--file",
            "user.xkjd6.dict.yaml",
            "测试",
            "cek",
        ])
        .output()
        .unwrap();
    assert!(preview.status.success());
    assert_eq!(fs::read(&path).unwrap(), initial);
    let apply = Command::new(binary)
        .args([
            "--root",
            temp.path().to_str().unwrap(),
            "add",
            "--file",
            "user.xkjd6.dict.yaml",
            "测试",
            "cek",
            "--apply",
        ])
        .output()
        .unwrap();
    assert!(apply.status.success());
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .ends_with("测试\tcek\n你好\tnkhzi\n")
    );
    let batch = temp.path().join("batch.json");
    fs::write(
        &batch,
        r#"[{"op":"update","line":5,"code":"nkhzia"},{"op":"remove","line":4}]"#,
    )
    .unwrap();
    let applied = Command::new(binary)
        .args([
            "--root",
            temp.path().to_str().unwrap(),
            "batch",
            "--file",
            "user.xkjd6.dict.yaml",
            "--operations",
            batch.to_str().unwrap(),
            "--apply",
        ])
        .output()
        .unwrap();
    assert!(applied.status.success());
    assert!(
        fs::read_to_string(path)
            .unwrap()
            .ends_with("你好\tnkhzia\n")
    );
}
