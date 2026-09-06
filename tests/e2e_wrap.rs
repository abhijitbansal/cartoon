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

#[test]
fn junit_flag_renders_a_test_report_for_any_command() {
    let state = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let xml = dir.path().join("r.xml");
    // The command itself writes the file (so its mtime is newer than the run).
    // Real gradle output is hundreds of lines; the report must beat THAT
    // (the net-savings guard passes tiny outputs through untouched).
    let script = format!(
        "i=0; while [ $i -lt 60 ]; do echo \"> Task :module$i:compileJava UP-TO-DATE\"; i=$((i+1)); done; printf '%s' '<testsuite tests=\"2\" time=\"0.2\"><testcase classname=\"c\" name=\"ok\"/><testcase classname=\"c\" name=\"bad\"><failure message=\"boom\">trace</failure></testcase></testsuite>' > {}; exit 1",
        xml.display()
    );
    cartoon()
        .env("XDG_STATE_HOME", state.path())
        .args(["--junit", xml.to_str().unwrap(), "sh", "-c", &script])
        .assert()
        .code(1)
        .stdout(contains("runner: sh"))
        .stdout(contains("failed: 1"))
        .stdout(contains("boom"));
}

#[test]
fn junit_flag_ignores_a_stale_file_and_warns() {
    let state = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let xml = dir.path().join("old.xml");
    std::fs::write(
        &xml,
        "<testsuite tests=\"1\"><testcase name=\"t\"/></testsuite>",
    )
    .unwrap();
    // Backdate the file so it predates the run.
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
    std::fs::File::open(&xml)
        .unwrap()
        .set_modified(old)
        .unwrap();
    cartoon()
        .env("XDG_STATE_HOME", state.path())
        .args([
            "--junit",
            xml.to_str().unwrap(),
            "sh",
            "-c",
            "echo compile error; exit 2",
        ])
        .assert()
        .code(2)
        .stdout(contains("compile error"))
        .stderr(contains("stale"));
}

#[test]
fn max_tokens_caps_output_and_discloses() {
    let state = tempfile::tempdir().unwrap();
    let out = cartoon()
        .env("XDG_STATE_HOME", state.path())
        .args([
            "--max-tokens",
            "120",
            "sh",
            "-c",
            "i=0; while [ $i -lt 400 ]; do echo \"distinct line number $i of the log\"; i=$((i+1)); done",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("distinct line number 0 of"),
        "head kept: {stdout}"
    );
    assert!(
        stdout.contains("distinct line number 399 of"),
        "tail kept: {stdout}"
    );
    assert!(
        stdout.contains("omitted") && stdout.contains("cartoon logs grep"),
        "{stdout}"
    );
    assert!(
        stdout.lines().count() < 60,
        "capped: {} lines",
        stdout.lines().count()
    );
}

#[test]
fn max_tokens_env_var_applies_too() {
    let state = tempfile::tempdir().unwrap();
    let out = cartoon()
        .env("XDG_STATE_HOME", state.path())
        .env("CARTOON_MAX_TOKENS", "80")
        .args([
            "sh",
            "-c",
            "i=0; while [ $i -lt 300 ]; do echo \"row $i\"; i=$((i+1)); done",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("omitted"), "{stdout}");
}

#[test]
fn doctor_reports_every_section_and_exits_zero() {
    let state = tempfile::tempdir().unwrap();
    cartoon()
        .env("XDG_STATE_HOME", state.path())
        .env("XDG_CONFIG_HOME", state.path())
        .args(["doctor"])
        .assert()
        .success()
        .stdout(contains("allowlist_without_adapter"))
        .stdout(contains("ledger:"))
        .stdout(contains("hook"));
}
