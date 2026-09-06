use assert_cmd::Command;

fn cartoon() -> Command {
    Command::cargo_bin("cartoon").unwrap()
}

fn have(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn fixture(rel: &str) -> String {
    format!("{}/tests/fixtures/e2e/{rel}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn e2e_pytest_failing_suite() {
    if !have("pytest") {
        eprintln!("SKIP: pytest not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let assert = cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["pytest", &fixture("pyproj")])
        .assert()
        .code(1); // pytest exit 1 = test failures, mirrored
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("runner: pytest"), "got:\n{out}");
    assert!(out.contains("failed: 1"), "got:\n{out}");
    assert!(out.contains("test_fail"), "got:\n{out}");
}

#[test]
fn e2e_unittest_failing_suite() {
    if !have("python3") {
        eprintln!("SKIP: python3 not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let assert = cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .current_dir(fixture("unittestproj_big"))
        .args(["python3", "-m", "unittest", "discover"])
        .assert()
        .code(1);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("runner: unittest"), "got:\n{out}");
    assert!(out.contains("failed: 6"), "got:\n{out}");
    assert!(out.contains("test_fail_alpha"), "got:\n{out}");
}

#[test]
fn e2e_unittest_tiny_suite_passes_through_when_report_costs_more() {
    // Two tests, one short traceback: the TOON report plus raw_log footer
    // would cost more tokens than unittest's own output, so the guard emits
    // the original streams byte-for-byte (exit code still mirrored).
    if !have("python3") {
        eprintln!("SKIP: python3 not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let assert = cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .current_dir(fixture("unittestproj"))
        .args(["python3", "-m", "unittest", "discover"])
        .assert()
        .code(1);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let err = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(!out.contains("runner: unittest"), "got:\n{out}");
    assert!(err.contains("FAILED (failures=1)"), "got:\n{err}");
}

#[test]
fn e2e_jest_failing_suite() {
    if !have("jest") {
        eprintln!("SKIP: jest not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let assert = cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .current_dir(fixture("jsproj"))
        .args(["jest"])
        .assert()
        .code(1);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("runner: jest"), "got:\n{out}");
    assert!(out.contains("failed: 1"), "got:\n{out}");
    assert!(out.contains("fails"), "got:\n{out}");
}

#[test]
fn e2e_adapters_lists_every_registered_adapter() {
    let assert = cartoon().args(["adapters"]).assert().success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for name in [
        "pytest",
        "unittest",
        "jest",
        "vitest",
        "swift-test",
        "xcodebuild-test",
        "ruff",
        "eslint",
        "tsc",
        "swift-build",
        "xcodebuild-build",
        "pre-commit",
        "cargo-test",
        "cargo-build",
        "go-test",
        "mypy",
        "phpunit",
        "rspec",
        "swiftlint",
    ] {
        assert!(
            out.lines().any(|l| l.starts_with(&format!("{name}: "))),
            "missing {name}:\n{out}"
        );
    }
}

#[test]
fn e2e_parse_failure_passes_through() {
    // A binary named `pytest` that emits garbage and no junit xml: the
    // adapter must fall back to the original output (tiny, so the generic
    // ladder cannot pay for itself either) with a stderr warning.
    let tmp = tempfile::tempdir().unwrap();
    let fake_dir = tmp.path().join("bin");
    std::fs::create_dir(&fake_dir).unwrap();
    let fake = fake_dir.join("pytest");
    std::fs::write(&fake, "#!/bin/sh\necho not a real pytest run\nexit 5\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let assert = cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args([fake.to_str().unwrap()])
        .assert()
        .code(5);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let err = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(out.contains("not a real pytest run"), "got:\n{out}");
    assert!(err.contains("failed to parse"), "got:\n{err}");
}

#[test]
fn tiny_pytest_run_passes_through_when_report_would_be_bigger() {
    // The ledger held 58 negative-saved adapter runs (pytest -q on a tiny
    // suite: 15 tokens in, 68 out). The adapter path must obey the same
    // net-savings guard as the ladder path.
    if !have("pytest") {
        eprintln!("SKIP: pytest not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("test_one.py"),
        "def test_ok():\n    assert True\n",
    )
    .unwrap();
    let state = tempfile::tempdir().unwrap();
    let out = cartoon()
        .env("XDG_STATE_HOME", state.path())
        .current_dir(dir.path())
        .args(["pytest", "-q", "-p", "no:cacheprovider"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1 passed"), "original emitted: {stdout}");
    assert!(
        !stdout.contains("runner: pytest"),
        "report would not pay for itself: {stdout}"
    );
}

#[test]
fn shell_string_pipe_to_tail_still_gets_the_adapter_report() {
    // Issue #12: `cartoon -c 'pytest -v | tail -5'` used to print the raw
    // tail. The pure filter is dropped (disclosed) and the adapter fires.
    if !have("pytest") {
        eprintln!("SKIP: pytest not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let cmd = format!("pytest -v {} | tail -5", fixture("pyproj"));
    let assert = cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["-c", &cmd])
        .assert()
        .code(1);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("runner: pytest"), "got:\n{out}");
    assert!(out.contains("pipe_filter_dropped: tail -5"), "got:\n{out}");
}
