use assert_cmd::Command;

fn cartoon() -> Command {
    Command::cargo_bin("cartoon").unwrap()
}

// `have()` panics instead of skipping under CARTOON_E2E_STRICT=1.
mod common;
use common::have;

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
        .env("XDG_CONFIG_HOME", tmp.path())
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
        .env("XDG_CONFIG_HOME", tmp.path())
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
        .env("XDG_CONFIG_HOME", tmp.path())
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
        .env("XDG_CONFIG_HOME", tmp.path())
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
fn e2e_pytest_exit_mid_run_is_not_reported_as_all_pass() {
    // `pytest.exit()` in the second test: pytest exits 2 with "1 passed";
    // the report must carry the abort reason, not a clean summary.
    if !have("pytest") {
        eprintln!("SKIP: pytest not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let assert = cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path())
        .current_dir(fixture("pyexit"))
        .args(["pytest", "-p", "no:cacheprovider"])
        .assert()
        .code(2);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("database not reachable, aborting run"),
        "got:\n{out}"
    );
    assert!(!out.contains("failed: 0"), "got:\n{out}");
}

#[test]
fn e2e_jest_suite_that_failed_to_run_is_reported() {
    if !have("jest") {
        eprintln!("SKIP: jest not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let assert = cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path())
        .current_dir(fixture("jsproj_suite_error"))
        .args(["jest"])
        .assert()
        .code(1);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("failed: 1"), "got:\n{out}");
    assert!(
        out.contains("Cannot find module './does-not-exist'"),
        "got:\n{out}"
    );
}

#[test]
fn e2e_vitest_reports_failures_without_writing_into_the_repo() {
    // vitest >= 4 writes `--reporter=json` output to .vitest/json/ in the
    // project; the adapter must read its own temp file instead.
    if !have("vitest") {
        eprintln!("SKIP: vitest not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir(&proj).unwrap();
    std::fs::copy(
        fixture("vitestproj/sample.test.js"),
        proj.join("sample.test.js"),
    )
    .unwrap();
    let assert = cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path())
        .current_dir(&proj)
        .args(["vitest", "run"])
        .assert()
        .code(1);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("runner: vitest"), "got:\n{out}");
    assert!(out.contains("failed: 1"), "got:\n{out}");
    assert!(out.contains("console: debug value 41"), "got:\n{out}");
    // (vitest's own node_modules/.vite cache may appear; a report may not.)
    assert!(
        !proj.join(".vitest").exists(),
        "report written into the repo"
    );
    let json_files: Vec<_> = std::fs::read_dir(&proj)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    assert!(json_files.is_empty(), "report written: {json_files:?}");
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
        .env("XDG_CONFIG_HOME", tmp.path())
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
        .env("XDG_CONFIG_HOME", state.path())
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
        .env("XDG_CONFIG_HOME", tmp.path())
        .args(["-c", &cmd])
        .assert()
        .code(1);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("runner: pytest"), "got:\n{out}");
    assert!(out.contains("pipe_filter_dropped: tail -5"), "got:\n{out}");
}
