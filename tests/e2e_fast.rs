use assert_cmd::Command;
use predicates::prelude::*;

/// Fake pytest that exits 4 on `-n` (xdist missing) and succeeds without it,
/// writing minimal junit xml to the --junit-xml path cartoon injected.
///
/// The stderr line `pytest: error: unrecognized arguments: -n auto` is
/// load-bearing: it is the exact signature src/app.rs's fallback matches
/// (exit 4 + "unrecognized arguments" + the joined fast args). If real
/// pytest ever changes this wording, update both the fallback and this
/// fixture.
#[cfg(unix)]
const FAKE_PYTEST: &str = r#"#!/bin/sh
junit=""
saw_n=0
for a in "$@"; do
  case "$a" in
    --junit-xml=*) junit="${a#--junit-xml=}" ;;
    -n) saw_n=1 ;;
  esac
done
if [ "$saw_n" = "1" ]; then
  echo "usage: pytest [options] [file_or_dir] [...]" >&2
  echo "pytest: error: unrecognized arguments: -n auto" >&2
  exit 4
fi
cat > "$junit" <<'XML'
<?xml version="1.0" encoding="utf-8"?>
<testsuites><testsuite name="pytest" tests="2" failures="0" skipped="0" time="0.01">
<testcase classname="t" name="test_a" file="tests/t.py" line="1" time="0.005"/>
<testcase classname="t" name="test_b" file="tests/t.py" line="5" time="0.005"/>
</testsuite></testsuites>
XML
echo "2 passed"
exit 0
"#;

#[cfg(unix)]
fn setup_fake_pytest(dir: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("pytest");
    std::fs::write(&bin, FAKE_PYTEST).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

#[cfg(unix)]
#[test]
fn fast_falls_back_serially_when_xdist_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let fake = setup_fake_pytest(tmp.path());
    Command::cargo_bin("cartoon")
        .unwrap()
        .env("XDG_STATE_HOME", state.path())
        .args(["--fast", fake.to_str().unwrap()])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("runner: pytest"))
        .stdout(predicate::str::contains("total: 2"))
        .stdout(predicate::str::contains("fast:").not())
        .stderr(predicate::str::contains("--fast unavailable"));
}

#[cfg(unix)]
#[test]
fn without_fast_flag_fake_pytest_never_sees_dash_n() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let fake = setup_fake_pytest(tmp.path());
    Command::cargo_bin("cartoon")
        .unwrap()
        .env("XDG_STATE_HOME", state.path())
        .args([fake.to_str().unwrap()])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("total: 2"))
        .stdout(predicate::str::contains("fast:").not())
        .stderr(predicate::str::contains("--fast unavailable").not());
}

fn xdist_available() -> bool {
    std::process::Command::new("python3")
        .args(["-c", "import xdist"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Real-tool test: requires pytest + pytest-xdist on PATH (CI installs both).
#[test]
fn real_pytest_fast_discloses_and_counts_match() {
    if !xdist_available() {
        if std::env::var("CI").is_ok() {
            panic!("pytest-xdist must be installed in CI (ci.yml pip install step)");
        }
        eprintln!("skipping: pytest-xdist not importable");
        return;
    }
    let proj = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    std::fs::write(
        proj.path().join("test_demo.py"),
        "def test_ok():\n    assert True\n\ndef test_bad():\n    assert 1 == 2\n",
    )
    .unwrap();
    Command::cargo_bin("cartoon")
        .unwrap()
        .env("XDG_STATE_HOME", state.path())
        .current_dir(proj.path())
        .args(["--fast", "pytest"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("runner: pytest"))
        .stdout(predicate::str::contains("fast: \"-n auto\""))
        .stdout(predicate::str::contains("total: 2"))
        .stdout(predicate::str::contains("failed: 1"));
}
