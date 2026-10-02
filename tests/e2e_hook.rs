//! `cartoon hook rewrite` and `hook install|uninstall` through the real
//! binary: code-loading flags and out-of-project paths are never
//! auto-approved, and install never destroys a settings file it can't read.
use assert_cmd::Command;
use predicates::str::contains;
use std::fs;

/// Every binary invocation in tests/ points XDG_STATE_HOME/XDG_CONFIG_HOME
/// at temp dirs (enforced by tests/isolation_lint.rs).
fn isolated_state() -> &'static tempfile::TempDir {
    static STATE: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    STATE.get_or_init(|| tempfile::tempdir().expect("temp state dir"))
}

fn cartoon() -> Command {
    let mut cmd = Command::cargo_bin("cartoon").unwrap();
    cmd.env("XDG_STATE_HOME", isolated_state().path().join("state"))
        .env("XDG_CONFIG_HOME", isolated_state().path().join("config"))
        .env_remove("CARTOON_NO_WRAP");
    cmd
}

fn rewrite(cwd: &std::path::Path, command: &str) -> String {
    let event = serde_json::json!({
        "tool_name": "Bash",
        "cwd": cwd,
        "tool_input": { "command": command },
    });
    let out = cartoon()
        .current_dir(cwd)
        .args(["hook", "rewrite"])
        .write_stdin(event.to_string())
        .assert()
        .success();
    String::from_utf8(out.get_output().stdout.clone()).unwrap()
}

#[test]
fn rewrite_never_approves_code_loading_flags() {
    let tmp = tempfile::tempdir().unwrap();
    for cmd in [
        "go test -exec /tmp/x ./...",
        "go test -toolexec /tmp/x ./...",
        "cargo test --config build.rustc-wrapper=/tmp/x",
        "make -f /tmp/x.mk",
        "make SHELL=/tmp/x",
        "jest --config /tmp/j.js",
        "mypy --config-file /tmp/x",
        "gradle test -I /tmp/init.gradle",
        "PYTEST_ADDOPTS=-pevil pytest",
        "pytest -p evil",
        "/tmp/bin/pytest -q",
        "pytest ../outside",
    ] {
        assert_eq!(rewrite(tmp.path(), cmd), "", "{cmd} must not be rewritten");
    }
}

#[test]
fn rewrite_still_approves_the_dev_loop() {
    let tmp = tempfile::tempdir().unwrap();
    let inside = tmp.path().join("tests/test_a.py");
    let mut cmds = vec![
        "pytest -q".to_string(),
        "pytest -k 'not slow'".to_string(),
        "cargo build --release && cargo test".to_string(),
    ];
    // An absolute path under the event's cwd is in-project. (Skipped if the
    // temp dir's own path trips the already-wrapped `cartoon` substring check.)
    if !inside.display().to_string().contains("cartoon") {
        cmds.push(format!("pytest {}", inside.display()));
    }
    for cmd in cmds {
        let out = rewrite(tmp.path(), &cmd);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["hookSpecificOutput"]["permissionDecision"], "allow",
            "{cmd}"
        );
    }
}

#[test]
fn install_refuses_a_settings_file_it_cannot_read_as_utf8() {
    let tmp = tempfile::tempdir().unwrap();
    let settings = tmp.path().join(".claude/settings.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let original = b"{\"theme\":\"\xff\"}".to_vec();
    fs::write(&settings, &original).unwrap();
    cartoon()
        .current_dir(tmp.path())
        .args(["hook", "install", "--project"])
        .assert()
        .failure()
        .stderr(contains("UTF-8"));
    assert_eq!(
        fs::read(&settings).unwrap(),
        original,
        "file must be untouched"
    );
}

#[test]
fn uninstall_reports_the_real_read_error() {
    let tmp = tempfile::tempdir().unwrap();
    // A directory where settings.json should be: not "not found".
    fs::create_dir_all(tmp.path().join(".claude/settings.json")).unwrap();
    cartoon()
        .current_dir(tmp.path())
        .args(["hook", "uninstall", "--project"])
        .assert()
        .failure()
        .stderr(contains("cannot read"));
    cartoon()
        .current_dir(tmp.path())
        .args(["hook", "install", "--project"])
        .assert()
        .failure()
        .stderr(contains("cannot read"));
}

#[cfg(unix)]
#[test]
fn install_and_uninstall_keep_settings_permissions_and_other_keys() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let settings = tmp.path().join(".claude/settings.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(&settings, r#"{"theme":"dark"}"#).unwrap();
    fs::set_permissions(&settings, fs::Permissions::from_mode(0o600)).unwrap();
    for sub in ["install", "uninstall"] {
        cartoon()
            .current_dir(tmp.path())
            .args(["hook", sub, "--project"])
            .assert()
            .success();
        let mode = fs::metadata(&settings).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "after {sub}");
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark", "after {sub}");
    }
    // No temp files left in the settings directory.
    assert_eq!(fs::read_dir(settings.parent().unwrap()).unwrap().count(), 1);
}

#[test]
fn copilot_install_refuses_an_unreadable_foreign_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(".github/hooks/cartoon.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = b"\xff\xfe not ours".to_vec();
    fs::write(&path, &original).unwrap();
    cartoon()
        .current_dir(tmp.path())
        .args(["hook", "install", "--copilot", "--project"])
        .assert()
        .failure();
    assert_eq!(fs::read(&path).unwrap(), original);
}
