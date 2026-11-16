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
fn compress_aggressive_collapses_repeats() {
    // 200 repeats so the dedupe + raw_log footer clears the savings guard.
    cartoon()
        .args([
            "--compress",
            "aggressive",
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

#[test]
fn shell_grep_count_pipe_runs_natively() {
    cartoon()
        .args(["-c", r#"printf "a\nb\n" | grep -c a"#])
        .assert()
        .success()
        .stdout(contains("1"));
}

/// The left side is adapter-detected (a stand-in `pytest` on PATH), so only
/// the right-hand filter decides whether the pipe is split. `grep -c` must
/// keep its answer: the pipe runs whole under `sh -c`, nothing is dropped.
#[cfg(unix)]
#[test]
fn adapter_pipe_into_grep_count_runs_natively() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("pytest");
    std::fs::write(
        &fake,
        "#!/bin/sh\nprintf 'X one\\nY two\\nX three\\nX four\\n'\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();

    let out = cartoon()
        .env("XDG_STATE_HOME", tmp.path().join("state"))
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("PATH", path)
        .args(["-c", "pytest | grep -c X"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim(), "3", "exact grep -c answer: {stdout}");
    assert!(!stdout.contains("pipe_filter_dropped"), "{stdout}");
}

/// A stand-in tool on PATH for the dropped-filter tests below.
#[cfg(unix)]
fn stub_tool(dir: &std::path::Path, name: &str, script: &str) -> std::ffi::OsString {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join(name);
    std::fs::write(&fake, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap()
}

/// Final review I6: when no report replaces the dropped `| tail -N`, the
/// cut is applied in process, so the hook never makes output larger than
/// the agent asked for. The raw log stays archived.
///
/// Merged order is arrival order across two pipes, each read by its own
/// thread, so writes microseconds apart have no defined order. The stub
/// pauses before its stderr line so the line really arrives last; without
/// the pause ubuntu runners recorded it ahead of the stdout tail and the
/// cut dropped it.
#[cfg(unix)]
#[test]
fn dropped_tail_is_applied_when_the_adapter_output_is_unparsed() {
    let tmp = tempfile::tempdir().unwrap();
    let path = stub_tool(
        tmp.path(),
        "xcodebuild",
        "i=1; while [ $i -le 400 ]; do echo \"build line $i\"; i=$((i+1)); done; sleep 0.3; echo 'Testing failed: runner crashed' >&2; exit 65",
    );
    let out = cartoon()
        .env("XDG_STATE_HOME", tmp.path().join("state"))
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("PATH", path)
        .args([
            "--merge-streams",
            "-c",
            "xcodebuild test -scheme A | tail -40",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(65));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stdout.lines().count() <= 45, "cut to the tail:\n{stdout}");
    assert!(stdout.contains("runner crashed"), "{stdout}");
    assert!(!stdout.contains("build line 300\n"), "{stdout}");
    assert!(stderr.contains("tail -40"), "disclosed: {stderr}");
}

#[cfg(unix)]
#[test]
fn dropped_head_is_applied_when_the_adapter_output_is_unparsed() {
    let tmp = tempfile::tempdir().unwrap();
    let path = stub_tool(
        tmp.path(),
        "pytest",
        "i=1; while [ $i -le 400 ]; do echo \"collect line $i\"; i=$((i+1)); done; exit 1",
    );
    let out = cartoon()
        .env("XDG_STATE_HOME", tmp.path().join("state"))
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("PATH", path)
        .args(["-c", "pytest | head -n 3"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("collect line 1\n"), "{stdout}");
    assert!(!stdout.contains("collect line 4\n"), "{stdout}");
    assert!(stdout.lines().count() <= 8, "{stdout}");
}

/// The report would cost more than the raw output, so the raw output is
/// emitted: with the dropped `| tail -n 1` applied to it.
#[cfg(unix)]
#[test]
fn dropped_tail_is_applied_to_a_passthrough() {
    let tmp = tempfile::tempdir().unwrap();
    let script = r#"for a in "$@"; do case "$a" in --junit-xml=*) f="${a#--junit-xml=}";; esac; done
printf '<testsuite name="s" tests="1" failures="0" errors="0" skipped="0" time="0.1"><testcase classname="t" name="test_a" time="0.1"/></testsuite>' > "$f"
printf 'a\nb\nc\n'"#;
    let path = stub_tool(tmp.path(), "pytest", script);
    let out = cartoon()
        .env("XDG_STATE_HOME", tmp.path().join("state"))
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("PATH", path)
        .args(["-c", "pytest | tail -n 1"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "c\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("tail -n 1") && stderr.contains("raw_log"),
        "{stderr}"
    );
}

/// A cut that drops nothing leaves the passthrough silent and
/// byte-identical: no disclosure line making the output longer.
#[cfg(unix)]
#[test]
fn dropped_tail_that_cuts_nothing_stays_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let script = r#"for a in "$@"; do case "$a" in --junit-xml=*) f="${a#--junit-xml=}";; esac; done
printf '<testsuite name="s" tests="1" failures="0" errors="0" skipped="0" time="0.1"><testcase classname="t" name="test_a" time="0.1"/></testsuite>' > "$f"
printf 'a\nb\nc\n'"#;
    let path = stub_tool(tmp.path(), "pytest", script);
    let out = cartoon()
        .env("XDG_STATE_HOME", tmp.path().join("state"))
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("PATH", path)
        .args(["-c", "pytest | tail -40"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "a\nb\nc\n");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
}

/// Final review I15: `cartoon -c '<adapter cmd> 2>&1 | tail -N'` ran through
/// `sh` and returned tail's exit code 0 on a failing run.
#[cfg(unix)]
#[test]
fn merged_stderr_before_a_dropped_tail_keeps_the_tool_exit_code() {
    let tmp = tempfile::tempdir().unwrap();
    let path = stub_tool(
        tmp.path(),
        "pytest",
        // The pause makes the stderr line arrive last (see the I6 test above).
        "i=1; while [ $i -le 50 ]; do echo \"out $i\"; i=$((i+1)); done; sleep 0.3; echo 'boom on stderr' >&2; exit 1",
    );
    let out = cartoon()
        .env("XDG_STATE_HOME", tmp.path().join("state"))
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("PATH", path)
        .args(["-c", "pytest 2>&1 | tail -3"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("boom on stderr"), "merged: {stdout}");
    assert!(!stdout.contains("out 10\n"), "{stdout}");
}

/// Residuals round: the native-stdout passthrough ignored --merge-streams,
/// cutting only stdout and printing all of stderr. Measured: 302 lines for
/// `| tail -2`. The pipe would print the merged tail: 2 lines.
#[cfg(unix)]
#[test]
fn merged_native_passthrough_gets_the_merged_tail() {
    let tmp = tempfile::tempdir().unwrap();
    let script = r#"for a in "$@"; do case "$a" in --junit-xml=*) f="${a#--junit-xml=}";; esac; done
printf '<testsuite name="s" tests="1" failures="0" errors="0" skipped="0" time="0.1"><testcase classname="t" name="test_a" time="0.1"/></testsuite>' > "$f"
printf 'a\nb\nc\n'
i=1; while [ $i -le 300 ]; do echo "warn $i" >&2; i=$((i+1)); done"#;
    let path = stub_tool(tmp.path(), "pytest", script);
    let out = cartoon()
        .env("XDG_STATE_HOME", tmp.path().join("state"))
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("PATH", path)
        .args(["--merge-streams", "-c", "pytest | tail -2"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "warn 299\nwarn 300\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("warn "), "{stderr}");
    assert!(stderr.contains("tail -2"), "{stderr}");
}

/// Residuals round: with no raw log (archive cannot be written) the cut is
/// not applied, and stderr says so.
#[cfg(unix)]
#[test]
fn dropped_tail_without_a_raw_log_prints_everything_and_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let script = r#"for a in "$@"; do case "$a" in --junit-xml=*) f="${a#--junit-xml=}";; esac; done
printf '<testsuite name="s" tests="1" failures="0" errors="0" skipped="0" time="0.1"><testcase classname="t" name="test_a" time="0.1"/></testsuite>' > "$f"
printf 'a\nb\nc\n'"#;
    let path = stub_tool(tmp.path(), "pytest", script);
    // A regular file where the state directory should be: archiving fails.
    let state = tmp.path().join("state-is-a-file");
    std::fs::write(&state, "").unwrap();
    let out = cartoon()
        .env("XDG_STATE_HOME", &state)
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("PATH", path)
        .args(["-c", "pytest | tail -n 1"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "a\nb\nc\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("was not applied"), "{stderr}");
}
