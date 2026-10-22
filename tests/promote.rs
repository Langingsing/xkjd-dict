use std::{fs, process::Command};
use tempfile::TempDir;
use xkjd_dict::{
    allocate::{self, AllocationDecision, AllocationPlan, PromotionRequest, Reason},
    document::Document,
    workspace::Workspace,
};

const FILE: &str = "xkjd6.cizu.dict.yaml";
const RAW: &str = "# 说明\r\n---\r\nname: xkjd6.cizu\r\ncolumns: [weight, code, text, comment]\r\n...\r\n# 分类\r\n8\tnhfg\t你好中果\t原占词\r\n7\tnhfga\t邻词\t不动\r\n9\tnhfgi\t你好中国\t提升\r\n6\tnhfgv\t后词\t不动\r\n\r\n# 尾注";

fn fixture() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("xkjd6.schema.yaml"),
        "translator:\n  dictionary: xkjd6.extended\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("xkjd6.extended.dict.yaml"),
        "---\nname: xkjd6.extended\nimport_tables: [xkjd6.cizu, user.xkjd6]\n...\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("user.xkjd6.dict.yaml"),
        "---\nname: user.xkjd6\n...\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("xkjd6dz.dict.yaml"),
        "---\nname: xkjd6dz\n...\n你\tnkiuai\n好\thzauai\n中\tfyiavi\n国\tgliavv\n果\tgliaaa\n",
    )
    .unwrap();
    fs::write(dir.path().join(FILE), RAW).unwrap();
    dir
}
fn request() -> PromotionRequest {
    PromotionRequest {
        text: "你好中国".into(),
        code: "nhfg".into(),
        ..Default::default()
    }
}
fn build(dir: &TempDir, req: &PromotionRequest) -> AllocationDecision {
    allocate::promote(&Workspace::load(dir.path(), "xkjd6").unwrap(), req).unwrap()
}
fn ready(decision: AllocationDecision) -> AllocationPlan {
    match decision {
        AllocationDecision::Ready(plan) => plan,
        AllocationDecision::Blocked(report) => panic!("{:?}", report.issue),
    }
}
fn blocked(decision: AllocationDecision, reason: Reason) {
    match decision {
        AllocationDecision::Blocked(report) => assert_eq!(report.issue.unwrap().reason, reason),
        AllocationDecision::Ready(plan) => panic!("unexpected ready: {:?}", plan.report),
    }
}

#[test]
fn promotion_reuses_released_code_and_preserves_other_bytes_and_fields() {
    let dir = fixture();
    let path = dir.path().join(FILE);
    let mut plan = ready(build(&dir, &request()));
    assert_eq!(plan.report.extend, "你好中果");
    assert_eq!(plan.report.assigned_code.as_deref(), Some("nhfgi"));
    assert_eq!(plan.report.promoted_from.as_deref(), Some("nhfgi"));
    assert_eq!(fs::read_to_string(&path).unwrap(), RAW);
    plan.apply().unwrap();
    let expected = RAW
        .replace("8\tnhfg\t你好中果\t原占词", "9\tnhfg\t你好中国\t提升")
        .replace("9\tnhfgi\t你好中国\t提升", "8\tnhfgi\t你好中果\t原占词");
    assert_eq!(fs::read_to_string(&path).unwrap(), expected);
    let doc = Document::read(&path).unwrap();
    assert!(
        doc.entries
            .windows(2)
            .all(|pair| pair[0].code <= pair[1].code)
    );
}

#[test]
fn occupancy_and_exhaustion_are_checked_after_releasing_only_selected_record() {
    let dir = fixture();
    let other = dir.path().join("user.xkjd6.dict.yaml");
    fs::write(&other, "---\nname: user.xkjd6\n...\n占用\tnhfgi\n").unwrap();
    assert_eq!(
        ready(build(&dir, &request()))
            .report
            .assigned_code
            .as_deref(),
        Some("nhfgia")
    );
    fs::write(
        &other,
        "---\nname: user.xkjd6\n...\n占用\tnhfgi\n占满\tnhfgia\n",
    )
    .unwrap();
    blocked(build(&dir, &request()), Reason::NoFreeCode);
    assert_eq!(fs::read_to_string(dir.path().join(FILE)).unwrap(), RAW);
}

#[test]
fn ambiguity_and_nonlegal_prefix_require_explicit_choices() {
    let dir = fixture();
    let reference = dir.path().join("xkjd6dz.dict.yaml");
    let mut data = fs::read_to_string(&reference).unwrap();
    data.push_str("国\tkliavv\n");
    fs::write(&reference, data).unwrap();
    blocked(build(&dir, &request()), Reason::AmbiguousEncoding);
    let mut req = request();
    req.full_code = Some("nhfgia".into());
    assert!(matches!(build(&dir, &req), AllocationDecision::Ready(_)));
    req.full_code = Some("wrong".into());
    blocked(build(&dir, &req), Reason::InvalidFullCode);
    let dir = fixture();
    let path = dir.path().join(FILE);
    fs::write(
        &path,
        RAW.replace("nhfgi\t你好中国", "zzzzz\t你好中国")
            .replace("nhfgv\t后词", "zzzzzz\t后词"),
    )
    .unwrap();
    blocked(build(&dir, &request()), Reason::InvalidPrefix);
}

#[test]
fn multiple_owners_records_cross_files_and_concurrent_changes_are_rejected() {
    let dir = fixture();
    let other = dir.path().join("user.xkjd6.dict.yaml");
    fs::write(&other, "---\nname: user.xkjd6\n...\n你好中国\tnhfg\n").unwrap();
    let mut alias_request = request();
    alias_request.from_code = Some("nhfgi".into());
    blocked(build(&dir, &alias_request), Reason::InvalidRequest);
    fs::write(&other, "---\nname: user.xkjd6\n...\n第三词\tnhfg\n").unwrap();
    blocked(build(&dir, &request()), Reason::RecordSelection);
    fs::write(&other, "---\nname: user.xkjd6\n...\n你好中国\tnhfgia\n").unwrap();
    blocked(build(&dir, &request()), Reason::RecordSelection);
    let mut req = request();
    req.from_code = Some("nhfgia".into());
    blocked(build(&dir, &req), Reason::AtomicFile);
    fs::write(&other, "---\nname: user.xkjd6\n...\n").unwrap();
    let mut plan = ready(build(&dir, &request()));
    fs::write(&other, "---\nname: user.xkjd6\n...\n后来占用\tnhfgi\n").unwrap();
    assert!(plan.apply().unwrap_err().to_string().contains("文件已变化"));
    assert_eq!(fs::read_to_string(dir.path().join(FILE)).unwrap(), RAW);
}

#[test]
fn group_and_promote_cli_are_compact_and_default_to_preview() {
    let dir = fixture();
    let binary = env!("CARGO_BIN_EXE_xkjd-dict");
    let output = Command::new(binary)
        .args([
            "--root",
            dir.path().to_str().unwrap(),
            "group",
            "nhfg",
            "--length",
            "4",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "nhfg  你好中果\n└─ i  你好中国\n"
    );
    let command = || {
        let mut cmd = Command::new(binary);
        cmd.args([
            "--root",
            dir.path().to_str().unwrap(),
            "promote",
            "你好中国",
            "nhfg",
        ]);
        cmd
    };
    let output = command().output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("你好中国 nhfgi → nhfg"));
    assert_eq!(fs::read_to_string(dir.path().join(FILE)).unwrap(), RAW);
    let output = command().arg("--apply").output().unwrap();
    assert!(output.status.success());
    let ws = Workspace::load(dir.path(), "xkjd6").unwrap();
    assert_eq!(
        ws.query(Some("你好中国"), Some("nhfg"), None, false).len(),
        1
    );
    assert_eq!(
        ws.query(Some("你好中果"), Some("nhfgi"), None, false).len(),
        1
    );
    assert!(ws.group("nhfg", Some(0), false).is_err());
}

#[test]
fn tree_keeps_intermediate_nodes_branches_and_same_code_words() {
    let dir = fixture();
    fs::write(
        dir.path().join(FILE),
        "---\nname: xkjd6.cizu\n...\n甲乙\tabia\n丙丁\tabio\n戊己\tabio\n庚辛\tabv\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("user.xkjd6.dict.yaml"),
        "---\nname: user.xkjd6\n...\n丙丁\tabio\n",
    )
    .unwrap();
    let ws = Workspace::load(dir.path(), "xkjd6").unwrap();
    assert_eq!(
        ws.group_tree("ab", None, false).unwrap(),
        "ab\n├─ i\n│  ├─ a  甲乙\n│  └─ o  丙丁、戊己\n└─ v  庚辛\n"
    );
    assert_eq!(ws.group("ab", None, false).unwrap().len(), 5);
    assert_eq!(ws.group_tree("ab", Some(3), false).unwrap(), "");
    assert_eq!(ws.group_tree("不存在", None, false).unwrap(), "");
}
