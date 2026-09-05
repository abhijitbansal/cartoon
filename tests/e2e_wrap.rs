use assert_cmd::Command;
use predicates::str::contains;

fn cartoon() -> Command {
    Command::cargo_bin("cartoon").unwrap()
}

#[test]
fn json_output_becomes_toon() {
    // Output must be large enough that TOON + the raw_log footer beats the
    // original, or the net-savings guard rightly falls back to passthrough.
    cartoon()
        .args([
            "python3",
            "-c",
            r#"import json; print(json.dumps([{"name": "instance-%d" % i, "state": "running", "zone": "us-east-1a"} for i in range(30)]))"#,
        ])
        .assert()
        .success()
        .stdout(contains("{name,state,zone}"))
        .stdout(contains("instance-0,running,us-east-1a"));
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
    // 200 repeats so the dedupe + raw_log footer clears the savings guard.
    cartoon()
        .args([
            "--heuristic",
            "sh",
            "-c",
            "i=0; while [ $i -lt 200 ]; do echo tick tock tick; i=$((i+1)); done",
        ])
        .assert()
        .success()
        .stdout(contains("(x200)"));
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

#[test]
fn project_cartoon_toml_pin_changes_the_tier_at_run_time() {
    // A project pin for argv0 `sh` set to aggressive must filter INFO lines;
    // the default safe tier keeps them. Proves main.rs reads the merged config.
    let state = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(
        repo.path().join(".cartoon.toml"),
        "[command.sh]\nlevel = \"aggressive\"\n",
    )
    .unwrap();
    let mut script = String::from("i=0; while [ $i -lt 120 ]; do echo \"2026-06-11 INFO item $i\"; i=$((i+1)); done; echo '2026-06-11 ERROR boom'");
    script.push(';');
    let out = cartoon()
        .current_dir(repo.path())
        .env("XDG_STATE_HOME", state.path())
        .env("XDG_CONFIG_HOME", state.path())
        .args(["sh", "-c", &script])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("INFO item 3"),
        "aggressive pin from .cartoon.toml applied: {stdout}"
    );
    assert!(stdout.contains("ERROR boom"), "{stdout}");
}
