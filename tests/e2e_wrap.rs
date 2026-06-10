use assert_cmd::Command;
use predicates::str::contains;

fn cartoon() -> Command {
    Command::cargo_bin("cartoon").unwrap()
}

#[test]
fn json_output_becomes_toon() {
    cartoon()
        .args(["sh", "-c", r#"echo '{"a": 1, "tags": ["x", "y"]}'"#])
        .assert()
        .success()
        .stdout(contains("a: 1"))
        .stdout(contains("tags[2]: x,y"));
}

#[test]
fn plain_output_passes_through_with_exit_code() {
    cartoon()
        .args(["sh", "-c", "echo plain text; exit 4"])
        .assert()
        .code(4)
        .stdout(contains("plain text"));
}

#[test]
fn raw_flag_bypasses_transform() {
    cartoon()
        .args(["--raw", "sh", "-c", r#"echo '{"a": 1}'"#])
        .assert()
        .success()
        .stdout(contains(r#"{"a": 1}"#));
}

#[test]
fn missing_command_exits_127() {
    cartoon()
        .args(["definitely-not-a-real-binary-xyz"])
        .assert()
        .code(127);
}

#[test]
fn heuristic_flag_compresses_repeats() {
    cartoon()
        .args([
            "--heuristic",
            "sh",
            "-c",
            "for i in 1 2 3 4 5; do echo tick; done",
        ])
        .assert()
        .success()
        .stdout(contains("(x5)"));
}

#[test]
fn stats_records_and_reports() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().to_str().unwrap();
    cartoon()
        .env("XDG_STATE_HOME", state)
        .args(["sh", "-c", r#"echo '{"a": 1}'"#])
        .assert()
        .success();
    cartoon()
        .env("XDG_STATE_HOME", state)
        .args(["stats"])
        .assert()
        .success()
        .stdout(contains("calls: 1"));
}

#[test]
fn bad_since_exits_2_not_panic() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["stats", "--since", "7é"])
        .assert()
        .code(2);
}

#[test]
fn raw_mode_writes_no_stats() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["--raw", "sh", "-c", "echo hi"])
        .assert()
        .success();
    assert!(!tmp.path().join("cartoon/stats.jsonl").exists());
}
