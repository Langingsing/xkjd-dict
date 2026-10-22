use std::{fs, process::Command};
use tempfile::TempDir;
use xkjd_dict::{
    allocate::{self, AllocationDecision, AllocationPlan, AllocationRequest, Reason, Status},
    document::Document,
    workspace::Workspace,
};

const FILE: &str = "xkjd6.cizu.dict.yaml";
const KEEP: &str = "你好中国";
const EXTEND: &str = "你好中果";

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
    fs::write(dir.path().join(FILE), "# 保留头部\r\n---\r\nname: xkjd6.cizu\r\ncolumns: [weight, code, text, comment]\r\n...\r\n# 分类\r\n9\tnhfg\t你好中国\t保留\r\n8\tnhfg\t你好中果\t移动\r\n7\tnhfga\t邻词\t不变\r\n6\tnhfgv\t后词\t不变\r\n\r\n# 尾注").unwrap();
    dir
}

fn request() -> AllocationRequest {
    AllocationRequest {
        keep: KEEP.into(),
        extend: EXTEND.into(),
        ..Default::default()
    }
}

fn build(dir: &TempDir, request: &AllocationRequest) -> AllocationDecision {
    allocate::plan(&Workspace::load(dir.path(), "xkjd6").unwrap(), request).unwrap()
}

fn ready(decision: AllocationDecision) -> AllocationPlan {
    match decision {
        AllocationDecision::Ready(plan) => plan,
        AllocationDecision::Blocked(report) => panic!("unexpected block: {:?}", report.issue),
    }
}

fn blocked(decision: AllocationDecision, reason: Reason) {
    match decision {
        AllocationDecision::Blocked(report) => assert_eq!(report.issue.unwrap().reason, reason),
        AllocationDecision::Ready(plan) => panic!("unexpected plan: {:?}", plan.report),
    }
}

fn occupancy(dir: &TempDir, body: &str) {
    fs::write(
        dir.path().join("user.xkjd6.dict.yaml"),
        format!("---\nname: user.xkjd6\n...\n{body}"),
    )
    .unwrap();
}

#[test]
fn preview_apply_is_lossless_and_stably_ordered_with_extra_columns() {
    let dir = fixture();
    let path = dir.path().join(FILE);
    let original = fs::read_to_string(&path).unwrap();
    let mut plan = ready(build(&dir, &request()));
    assert_eq!(plan.report.status, Status::Ready);
    assert_eq!(plan.report.assigned_code.as_deref(), Some("nhfgi"));
    assert_eq!(plan.report.full_code.as_deref(), Some("nhfgia"));
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    assert!(plan.report.diff.contains("+8\tnhfgi\t你好中果\t移动"));
    plan.apply().unwrap();
    let expected = original.replace(
        "8\tnhfg\t你好中果\t移动\r\n7\tnhfga\t邻词\t不变",
        "7\tnhfga\t邻词\t不变\r\n8\tnhfgi\t你好中果\t移动",
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), expected);
    assert_eq!(plan.report.status, Status::Applied);
    let doc = Document::read(&path).unwrap();
    assert!(
        doc.entries
            .windows(2)
            .all(|pair| pair[0].code <= pair[1].code)
    );
}

#[test]
fn exact_occupancy_extends_to_full_code_and_reports_exhaustion() {
    let dir = fixture();
    occupancy(&dir, "占用\tnhfgi\n");
    assert_eq!(
        ready(build(&dir, &request()))
            .report
            .assigned_code
            .as_deref(),
        Some("nhfgia")
    );
    occupancy(&dir, "占用\tnhfgi\n占满\tnhfgia\n");
    blocked(build(&dir, &request()), Reason::NoFreeCode);
    occupancy(&dir, "第三方\tnhfg\n");
    blocked(build(&dir, &request()), Reason::OtherCollision);
}

#[test]
fn ambiguous_readings_and_shapes_require_explicit_selection() {
    let dir = fixture();
    let reference = dir.path().join("xkjd6dz.dict.yaml");
    let original = fs::read_to_string(&reference).unwrap();
    fs::write(&reference, format!("{original}你\tnkuvai\n")).unwrap();
    blocked(build(&dir, &request()), Reason::AmbiguousEncoding);
    let mut selected = request();
    selected.full_code = Some("nhfgua".into());
    assert_eq!(
        ready(build(&dir, &selected))
            .report
            .assigned_code
            .as_deref(),
        Some("nhfgu")
    );
    selected.full_code = Some("invalid".into());
    blocked(build(&dir, &selected), Reason::InvalidFullCode);
    selected.full_code = None;
    selected.character_codes = vec!["你=nkiuai".into()];
    assert_eq!(
        ready(build(&dir, &selected))
            .report
            .assigned_code
            .as_deref(),
        Some("nhfgi")
    );
    // A different reading whose unused shape does not change the phrase code
    // must still be disclosed instead of silently choosing that reading.
    fs::write(&reference, format!("{original}果\tkqaaaa\n")).unwrap();
    blocked(build(&dir, &request()), Reason::AmbiguousEncoding);
}

#[test]
fn dormant_occupancy_is_selected_explicitly_and_missing_data_is_not_guessed() {
    let dir = fixture();
    fs::write(
        dir.path().join("private.xkjd6.dict.yaml"),
        "---\nname: private.xkjd6\n...\n闲置\tnhfgi\n",
    )
    .unwrap();
    assert_eq!(
        ready(build(&dir, &request()))
            .report
            .assigned_code
            .as_deref(),
        Some("nhfgi")
    );
    let mut all = request();
    all.all = true;
    assert_eq!(
        ready(build(&dir, &all)).report.assigned_code.as_deref(),
        Some("nhfgia")
    );
    let reference = dir.path().join("xkjd6dz.dict.yaml");
    let raw = fs::read_to_string(&reference)
        .unwrap()
        .replace("果\tgliaaa\n", "");
    fs::write(reference, raw).unwrap();
    blocked(build(&dir, &request()), Reason::MissingReference);
    let mut explicit = request();
    explicit.character_codes.push("果=gliaaa".into());
    ready(build(&dir, &explicit));
}

#[test]
fn categories_and_unordered_blocks_are_not_silently_reorganized() {
    let dir = fixture();
    fs::write(
        dir.path().join(FILE),
        "---\nname: xkjd6.cizu\n...\n你好中国\tnhfg\n你好中果\tnhfg\n\n# 下一分类\n邻词\tnhfga\n",
    )
    .unwrap();
    blocked(build(&dir, &request()), Reason::Ordering);
    fs::write(
        dir.path().join(FILE),
        "---\nname: xkjd6.cizu\n...\n后词\tnhfgv\n你好中国\tnhfg\n你好中果\tnhfg\n",
    )
    .unwrap();
    blocked(build(&dir, &request()), Reason::Ordering);
}

#[test]
fn tail_without_newline_survives_relocation() {
    let dir = fixture();
    fs::write(
        dir.path().join(FILE),
        "---\nname: xkjd6.cizu\n...\n你好中国\tnhfg\n你好中果\tnhfg\n邻词\tnhfga",
    )
    .unwrap();
    ready(build(&dir, &request())).apply().unwrap();
    let after = fs::read_to_string(dir.path().join(FILE)).unwrap();
    assert!(after.ends_with("邻词\tnhfga\n你好中果\tnhfgi"));
}

#[test]
fn applying_rejects_changes_to_any_planning_input_or_dictionary_manifest() {
    for changed in [
        FILE,
        "user.xkjd6.dict.yaml",
        "xkjd6dz.dict.yaml",
        "xkjd6.schema.yaml",
        "new.dict.yaml",
    ] {
        let dir = fixture();
        let mut plan = ready(build(&dir, &request()));
        let original = fs::read(dir.path().join(FILE)).unwrap();
        fs::write(dir.path().join(changed), "external change").unwrap();
        assert!(
            plan.apply().unwrap_err().to_string().contains("变化"),
            "{changed}"
        );
        if changed != FILE {
            assert_eq!(fs::read(dir.path().join(FILE)).unwrap(), original);
        }
        assert_eq!(
            fs::read_to_string(dir.path().join(changed)).unwrap(),
            "external change"
        );
    }
}

#[test]
fn record_and_shared_code_selection_are_explicit() {
    let dir = fixture();
    occupancy(&dir, "你好中国\tnhfgv\n你好中果\tnhfgv\n");
    blocked(build(&dir, &request()), Reason::SharedCodeSelection);
    let mut explicit = request();
    explicit.code = Some("nhfg".into());
    explicit.file = Some(FILE.into());
    explicit.line = Some(8);
    ready(build(&dir, &explicit));
    explicit.line = Some(7);
    blocked(build(&dir, &explicit), Reason::RecordSelection);
}

#[test]
fn cli_single_command_previews_then_applies_and_emits_machine_readable_blocks() {
    let dir = fixture();
    let binary = env!("CARGO_BIN_EXE_xkjd-dict");
    let original = fs::read(dir.path().join(FILE)).unwrap();
    let preview = Command::new(binary)
        .args([
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
            "allocate",
            KEEP,
            EXTEND,
        ])
        .output()
        .unwrap();
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(report["status"], "ready");
    assert_eq!(report["assigned_code"], "nhfgi");
    assert_eq!(fs::read(dir.path().join(FILE)).unwrap(), original);
    let apply = Command::new(binary)
        .args([
            "--root",
            dir.path().to_str().unwrap(),
            "allocate",
            KEEP,
            EXTEND,
            "--apply",
        ])
        .output()
        .unwrap();
    assert!(apply.status.success());
    assert_ne!(fs::read(dir.path().join(FILE)).unwrap(), original);
    let blocked = Command::new(binary)
        .args([
            "--root",
            dir.path().to_str().unwrap(),
            "--json",
            "allocate",
            KEEP,
            EXTEND,
            "--apply",
        ])
        .output()
        .unwrap();
    assert!(!blocked.status.success());
    let report: serde_json::Value = serde_json::from_slice(&blocked.stdout).unwrap();
    assert_eq!(report["status"], "blocked");
    assert_eq!(report["issue"]["reason"], "shared_code_selection");
}

#[test]
fn missing_graph_invalid_prefix_and_duplicate_records_are_reported() {
    let dir = fixture();
    occupancy(&dir, &format!("{EXTEND}\tnhfg\n"));
    blocked(build(&dir, &request()), Reason::RecordSelection);
    occupancy(&dir, "");
    let raw = fs::read_to_string(dir.path().join(FILE)).unwrap();
    fs::write(dir.path().join(FILE), raw.replace("\tnhfg\t", "\tbad\t")).unwrap();
    blocked(build(&dir, &request()), Reason::InvalidPrefix);
    fs::remove_file(dir.path().join("user.xkjd6.dict.yaml")).unwrap();
    blocked(build(&dir, &request()), Reason::IncompleteWorkspace);
}

#[test]
fn invalid_occupancy_columns_are_not_treated_as_free_slots() {
    let dir = fixture();
    fs::write(
        dir.path().join("user.xkjd6.dict.yaml"),
        "---\nname: user.xkjd6\ncolumns: [text, weight]\n...\n占用\tnhfgi\n",
    )
    .unwrap();
    blocked(build(&dir, &request()), Reason::IncompleteWorkspace);
}

#[test]
fn truncated_combinations_and_malformed_reference_are_not_used() {
    let dir = fixture();
    let mut reference = "---\nname: xkjd6dz\n...\n".to_owned();
    for ch in "你好中果".chars() {
        for sound in "cdfghjklm".chars() {
            reference.push_str(&format!("{ch}\tb{sound}i\n"));
        }
    }
    fs::write(dir.path().join("xkjd6dz.dict.yaml"), &reference).unwrap();
    blocked(build(&dir, &request()), Reason::TruncatedEncoding);
    fs::write(
        dir.path().join("xkjd6dz.dict.yaml"),
        format!("{reference}malformed\n"),
    )
    .unwrap();
    blocked(build(&dir, &request()), Reason::IncompleteWorkspace);
}

#[test]
fn equal_code_order_and_embedded_carriage_return_are_preserved() {
    let dir = fixture();
    let path = dir.path().join(FILE);
    let original = fs::read_to_string(&path).unwrap().replace(
        "7\tnhfga\t邻词\t不变\r\n",
        "7\tnhfga\t邻词\t不变\r\n5\tnhfga\t第二邻\t额外\r\r\n",
    );
    fs::write(&path, &original).unwrap();
    ready(build(&dir, &request())).apply().unwrap();
    let after = fs::read_to_string(path).unwrap();
    assert!(after.contains(
        "7\tnhfga\t邻词\t不变\r\n5\tnhfga\t第二邻\t额外\r\r\n8\tnhfgi\t你好中果\t移动\r\n"
    ));
}

#[test]
fn workspace_changed_before_planning_is_also_rejected() {
    let dir = fixture();
    let ws = Workspace::load(dir.path(), "xkjd6").unwrap();
    occupancy(&dir, "新占用\tnhfgi\n");
    assert!(
        allocate::plan(&ws, &request())
            .unwrap_err()
            .to_string()
            .contains("变化")
    );
}

#[cfg(unix)]
#[test]
fn allocation_refuses_a_symlink_edit_target() {
    use std::os::unix::fs::symlink;
    let dir = fixture();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join(FILE);
    let original = fs::read(dir.path().join(FILE)).unwrap();
    fs::write(&target, &original).unwrap();
    fs::remove_file(dir.path().join(FILE)).unwrap();
    symlink(&target, dir.path().join(FILE)).unwrap();
    let ws = Workspace::load(dir.path(), "xkjd6").unwrap();
    assert!(
        allocate::plan(&ws, &request())
            .unwrap_err()
            .to_string()
            .contains("符号链接")
    );
    assert_eq!(fs::read(target).unwrap(), original);
}
