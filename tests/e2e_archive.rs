mod common;
use assert_cmd::Command;
use predicates::str::contains;

fn cartoon() -> Command {
    Command::cargo_bin("cartoon").unwrap()
}

/// JSON big enough that TOON encoding + the raw_log footer still beats the
/// original (the net-savings guard would otherwise fall back to passthrough).
const BIG_JSON_CMD: &str = r#"python3 -c 'import json; print(json.dumps([{"name": "instance-%d" % i, "state": "running", "zone": "us-east-1a"} for i in range(30)]))'"#;

#[test]
fn transformed_run_gets_footer_and_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let assert = cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["--tag", "e2e", "sh", "-c", BIG_JSON_CMD])
        .assert()
        .success()
        .stdout(contains("raw_log:"));
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let path = out
        .lines()
        .find(|l| l.starts_with("raw_log:"))
        .and_then(|l| l.split_once(' '))
        .map(|(_, p)| p.trim().trim_matches('"').to_string())
        .expect("footer path");
    let raw = std::fs::read_to_string(format!("{path}/stdout.log")).unwrap();
    assert!(
        raw.starts_with("[{\"name\": \"instance-0\""),
        "archived stdout is the ORIGINAL json, got: {raw}"
    );
    let meta = std::fs::read_to_string(format!("{path}/meta.json")).unwrap();
    assert!(meta.contains("\"e2e\""), "tag recorded");
}

#[test]
fn tiny_json_passes_through_when_transform_does_not_pay() {
    // TOON of a 8-byte object + raw_log footer > original → guard emits the
    // original byte-identically and records the run as passthrough.
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["sh", "-c", r#"echo '{"a": 1}'"#])
        .assert()
        .success()
        .stdout("{\"a\": 1}\n"); // exact: no footer, no TOON
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["logs"])
        .assert()
        .success()
        .stdout(contains("passthrough"));
}

#[test]
fn passthrough_is_byte_identical_but_archived() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["sh", "-c", "echo plain"])
        .assert()
        .success()
        .stdout("plain\n"); // exact: no footer appended
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["logs"])
        .assert()
        .success()
        .stdout(contains("passthrough"));
}

#[test]
fn raw_mode_is_byte_identical_but_archived() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["--raw", "sh", "-c", r#"echo '{"a": 1}'"#])
        .assert()
        .success()
        .stdout("{\"a\": 1}\n");
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["logs"])
        .assert()
        .success()
        .stdout(contains(",raw,"));
}

#[test]
fn logs_last_stdout_returns_raw_stream() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["sh", "-c", r#"echo '{"a": 1}'"#])
        .assert()
        .success();
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["logs", "--last", "--stdout"])
        .assert()
        .success()
        .stdout(contains(r#"{"a": 1}"#));
}

#[test]
fn logs_unknown_id_exits_2() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["logs", "20990101-000000-dead"])
        .assert()
        .code(2);
}

#[test]
fn e2e_pytest_footer_points_at_original_report() {
    if !common::have("pytest") {
        eprintln!("SKIP: pytest not installed");
        return;
    }
    let proj = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/e2e/pyproj");
    let tmp = tempfile::tempdir().unwrap();
    let assert = cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["pytest", proj])
        .assert()
        .code(1)
        .stdout(contains("raw_log:"));
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let path = out
        .lines()
        .find(|l| l.starts_with("raw_log:"))
        .and_then(|l| l.split_once(' '))
        .map(|(_, p)| p.trim().trim_matches('"').to_string())
        .unwrap();
    let raw = std::fs::read_to_string(format!("{path}/stdout.log")).unwrap();
    assert!(raw.contains("test_fail"), "original pytest report archived");
    assert!(
        raw.contains("short test summary") || raw.contains("FAILED"),
        "human report detail present: {raw}"
    );
}

#[test]
fn passthrough_without_trailing_newline_is_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["sh", "-c", "printf plain"])
        .assert()
        .success()
        .stdout("plain"); // exact bytes — no newline added
}

#[cfg(unix)]
#[test]
fn archived_runs_are_readable_only_by_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    // Both the transform path (reserve + write) and the passthrough path.
    for cmd in [BIG_JSON_CMD, "echo tiny"] {
        cartoon()
            .env("XDG_STATE_HOME", tmp.path())
            .args(["sh", "-c", cmd])
            .assert()
            .success();
    }
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let runs = tmp.path().join("cartoon/runs");
    assert_eq!(mode(&tmp.path().join("cartoon")), 0o700);
    assert_eq!(mode(&runs), 0o700);
    let dirs: Vec<_> = std::fs::read_dir(&runs).unwrap().flatten().collect();
    assert_eq!(dirs.len(), 2);
    for d in dirs {
        assert_eq!(mode(&d.path()), 0o700, "{:?}", d.path());
        for f in std::fs::read_dir(d.path()).unwrap().flatten() {
            assert_eq!(mode(&f.path()), 0o600, "{:?}", f.path());
        }
    }
    // The stats ledger records every command line too.
    assert_eq!(mode(&tmp.path().join("cartoon/stats.jsonl")), 0o600);
}

#[test]
fn back_to_back_runs_keep_every_raw_log() {
    // A concurrent or rapid-fire run must never prune a sibling's run that
    // was just written (keep_runs = 1 would otherwise delete it at once).
    let tmp = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(config.path().join("cartoon")).unwrap();
    std::fs::write(config.path().join("cartoon/config.toml"), "keep_runs = 1\n").unwrap();
    for _ in 0..3 {
        cartoon()
            .env("XDG_STATE_HOME", tmp.path())
            .env("XDG_CONFIG_HOME", config.path())
            .args(["sh", "-c", "echo hi"])
            .assert()
            .success();
    }
    let n = std::fs::read_dir(tmp.path().join("cartoon/runs"))
        .unwrap()
        .count();
    assert_eq!(n, 3);
}
