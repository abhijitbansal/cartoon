use assert_cmd::Command;
use predicates::str::contains;

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
        .current_dir(fixture("unittestproj"))
        .args(["python3", "-m", "unittest", "discover"])
        .assert()
        .code(1);
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("runner: unittest"), "got:\n{out}");
    assert!(out.contains("failed: 1"), "got:\n{out}");
    assert!(out.contains("test_fail"), "got:\n{out}");
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
fn e2e_adapters_lists_three() {
    cartoon()
        .args(["adapters"])
        .assert()
        .success()
        .stdout(contains("pytest"))
        .stdout(contains("unittest"))
        .stdout(contains("jest"));
}

#[test]
fn e2e_parse_failure_passes_through() {
    // A binary named `pytest` that emits garbage and no junit xml: the
    // adapter must fall back to the original output with a stderr warning.
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
    assert!(err.contains("passing through"), "got:\n{err}");
}
