use std::{fs, process::Command};
use xkjd_dict::{
    document::Document,
    edit::{EditPlan, Operation},
};

#[test]
fn batch_additions_use_the_same_stable_insert_path() {
    let raw = "---\nname: x\n...\n甲\tbb\n乙\tcc\n# 尾注";
    let doc = Document::parse("x.dict.yaml".into(), raw.into());
    let plan = EditPlan::build(
        &doc,
        &[
            Operation::Add {
                text: "后".into(),
                code: "dd".into(),
            },
            Operation::Add {
                text: "前".into(),
                code: "aa".into(),
            },
            Operation::Add {
                text: "同码".into(),
                code: "bb".into(),
            },
        ],
    )
    .unwrap();
    assert_eq!(
        plan.updated,
        "---\nname: x\n...\n前\taa\n甲\tbb\n同码\tbb\n乙\tcc\n后\tdd\n# 尾注"
    );
}

#[test]
fn insert_preserves_records_comments_columns_and_missing_final_newline() {
    let raw = "# header\r\n---\r\nname: x\r\ncolumns: [code, text, weight]\r\n...\r\n# 分类\r\naa\t甲\t1\r\ncc\t丙\t3";
    let doc = Document::parse("x.dict.yaml".into(), raw.into());
    let plan = EditPlan::insert(&doc, "乙", "bb", None).unwrap();
    assert_eq!(plan.updated, raw.replace("cc\t丙", "bb\t乙\t\r\ncc\t丙"));
    let plan = EditPlan::insert(&doc, "同码", "aa", None).unwrap();
    assert_eq!(plan.updated, raw.replace("cc\t丙", "aa\t同码\t\r\ncc\t丙"));
    assert!(EditPlan::insert(&doc, "甲", "aa", None).is_err());
}

#[test]
fn insertion_handles_beginning_end_tail_comments_and_explicit_categories() {
    let raw = "---\nname: x\n...\n甲\tbb\n乙\tcc\n# 尾注";
    let doc = Document::parse("x.dict.yaml".into(), raw.into());
    assert_eq!(
        EditPlan::insert(&doc, "前", "aa", None).unwrap().updated,
        raw.replace("甲\tbb", "前\taa\n甲\tbb")
    );
    assert_eq!(
        EditPlan::insert(&doc, "后", "dd", None).unwrap().updated,
        raw.replace("# 尾注", "后\tdd\n# 尾注")
    );
    let raw = "---\nname: x\n...\n# A\n甲\tcc\n\n# B\n乙\taa\n";
    let doc = Document::parse("x.dict.yaml".into(), raw.into());
    assert!(EditPlan::insert(&doc, "丙", "bb", None).is_err());
    assert_eq!(
        EditPlan::insert(&doc, "丙", "bb", Some(5)).unwrap().updated,
        raw.replace("甲\tcc", "丙\tbb\n甲\tcc")
    );
    assert!(EditPlan::insert(&doc, "丙", "bb", Some(7)).is_err());
    assert!(
        EditPlan::insert(&doc, "丙", "bb", Some(9))
            .unwrap()
            .updated
            .ends_with("乙\taa\n丙\tbb\n")
    );
}

#[test]
fn cli_insert_preview_apply_and_concurrent_rejection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.dict.yaml");
    let original = "---\nname: x\n...\n甲\taa\n丙\tcc\n";
    fs::write(&path, original).unwrap();
    let run = |apply: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_xkjd-dict"));
        command.args([
            "--root",
            dir.path().to_str().unwrap(),
            "add",
            "--file",
            "x.dict.yaml",
            "乙",
            "bb",
        ]);
        if apply {
            command.arg("--apply");
        }
        command.output().unwrap()
    };
    let output = run(false);
    assert!(output.status.success(), "{:?}", output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("+乙\tbb"));
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    assert!(run(true).status.success());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        original.replace("丙\tcc", "乙\tbb\n丙\tcc")
    );
    let doc = Document::read(&path).unwrap();
    let plan = EditPlan::insert(&doc, "丁", "bc", None).unwrap();
    fs::write(&path, "external change").unwrap();
    assert!(plan.apply().is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "external change");
}

#[test]
fn suggest_is_compact_and_json_keeps_details() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("xkjd6.schema.yaml"),
        "translator:\n  dictionary: x\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("x.dict.yaml"),
        "---\nname: x\n...\n占用\tnkh\n远处\tnkhzia\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("xkjd6dz.dict.yaml"),
        "---\nname: xkjd6dz\n...\n你\tnkiuai\n好\thzauai\n",
    )
    .unwrap();
    let run = |json: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_xkjd-dict"));
        command.args(["--root", dir.path().to_str().unwrap(), "suggest", "你好"]);
        if json {
            command.arg("--json");
        }
        command.output().unwrap()
    };
    let output = run(false);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "nkhz\nnkh 占用\n"
    );
    let output = run(true);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["choices"][0]["full_code"], "nkhzia");
    assert!(json.get("ordering_note").is_some());
    let encoded = Command::new(env!("CARGO_BIN_EXE_xkjd-dict"))
        .args(["--root", dir.path().to_str().unwrap(), "encode", "你好"])
        .output()
        .unwrap();
    assert!(encoded.status.success());
    assert!(String::from_utf8_lossy(&encoded.stdout).contains("全码 nkhzia"));
    fs::write(
        dir.path().join("x.dict.yaml"),
        "---\nname: x\n...\n占用\tnkh\n你好\tnkhz\n",
    )
    .unwrap();
    let output = run(false);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "已有 nkhz\nnkh 占用\n"
    );
    fs::write(
        dir.path().join("x.dict.yaml"),
        "---\nname: x\n...\n甲\tnkh\n乙\tnkhz\n丙\tnkhzi\n丁\tnkhzia\n",
    )
    .unwrap();
    let output = run(false);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "nkhzia：无空闲码\nnkh 甲\nnkhz 乙\nnkhzi 丙\nnkhzia 丁\n"
    );
    fs::write(
        dir.path().join("xkjd6dz.dict.yaml"),
        "---\nname: xkjd6dz\n...\n你\tnkiuai\n好\thzauai\n好\thqauai\n",
    )
    .unwrap();
    let output = run(false);
    assert!(String::from_utf8_lossy(&output.stdout).contains("歧义：好"));
    fs::write(
        dir.path().join("xkjd6dz.dict.yaml"),
        "---\nname: xkjd6dz\n...\n你\tnkiuai\n",
    )
    .unwrap();
    let output = run(false);
    assert!(String::from_utf8_lossy(&output.stdout).contains("缺少单字编码：好"));
}
