//! `cartoon hook rewrite` and `hook install|uninstall` through the real
//! binary: code-loading flags and out-of-project paths are never
//! auto-approved, and install never destroys a settings file it can't read.
use assert_cmd::Command;
use predicates::str::contains;
use std::fs;

/// Every binary invocation in tests/ points XDG_STATE_HOME/XDG_CONFIG_HOME
/// at temp dirs (enforced by tests/isolation_lint.rs). HOME and
/// CLAUDE_CONFIG_DIR point at empty temp dirs too, so the hook never reads
/// the developer's real Claude Code permission rules.
fn isolated_state() -> &'static tempfile::TempDir {
    static STATE: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    STATE.get_or_init(|| tempfile::tempdir().expect("temp state dir"))
}

fn cartoon() -> Command {
    let mut cmd = Command::cargo_bin("cartoon").unwrap();
    cmd.env("XDG_STATE_HOME", isolated_state().path().join("state"))
        .env("XDG_CONFIG_HOME", isolated_state().path().join("config"))
        .env("HOME", isolated_state().path().join("home"))
        .env("CLAUDE_CONFIG_DIR", isolated_state().path().join("claude"))
        .env_remove("CLAUDE_PROJECT_DIR")
        .env_remove("CARTOON_NO_WRAP");
    cmd
}

fn rewrite(cwd: &std::path::Path, command: &str) -> String {
    let event = serde_json::json!({
        "tool_name": "Bash",
        "cwd": cwd,
        "tool_input": { "command": command },
    });
    rewrite_event(cartoon(), cwd, &event)
}

fn rewrite_event(mut cmd: Command, cwd: &std::path::Path, event: &serde_json::Value) -> String {
    let out = cmd
        .current_dir(cwd)
        .args(["hook", "rewrite"])
        .write_stdin(event.to_string())
        .assert()
        .success();
    String::from_utf8(out.get_output().stdout.clone()).unwrap()
}

fn write_settings(path: &std::path::Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// A temp HOME (with its own empty CLAUDE_CONFIG_DIR) and a temp project.
fn home_and_project() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();
    (tmp, home, proj)
}

fn cartoon_with_home(home: &std::path::Path) -> Command {
    let mut cmd = cartoon();
    cmd.env("HOME", home)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"));
    cmd
}

fn bash_event(cwd: &std::path::Path, command: &str) -> serde_json::Value {
    serde_json::json!({
        "tool_name": "Bash",
        "cwd": cwd,
        "tool_input": { "command": command },
    })
}

#[test]
fn deny_rule_in_project_settings_emits_nothing() {
    let (_tmp, home, proj) = home_and_project();
    write_settings(
        &proj.join(".claude/settings.json"),
        r#"{"permissions":{"deny":["Bash(make check:*)"]}}"#,
    );
    // `make check` alone would be rewritten; only the rule stops it.
    let event = bash_event(&proj, "make test && make check");
    assert_eq!(rewrite_event(cartoon_with_home(&home), &proj, &event), "");
}

#[test]
fn quoted_command_under_deny_rule_emits_nothing() {
    let (_tmp, home, proj) = home_and_project();
    write_settings(
        &proj.join(".claude/settings.json"),
        r#"{"permissions":{"deny":["Bash(make check:*)"]}}"#,
    );
    for cmd in [r#"make "check""#, "make 'check'", "make ch'eck'"] {
        let event = bash_event(&proj, cmd);
        assert_eq!(
            rewrite_event(cartoon_with_home(&home), &proj, &event),
            "",
            "{cmd}"
        );
    }
}

#[test]
fn ask_rule_in_home_settings_emits_nothing() {
    let (_tmp, home, proj) = home_and_project();
    write_settings(
        &home.join(".claude/settings.json"),
        r#"{"permissions":{"ask":["Bash(pytest:*)"]}}"#,
    );
    let event = bash_event(&proj, "pytest -q");
    assert_eq!(rewrite_event(cartoon_with_home(&home), &proj, &event), "");
}

#[test]
fn ask_rule_found_through_home_when_config_dir_unset() {
    let (_tmp, home, proj) = home_and_project();
    write_settings(
        &home.join(".claude/settings.json"),
        r#"{"permissions":{"ask":["Bash(pytest:*)"]}}"#,
    );
    let mut cmd = cartoon();
    cmd.env("HOME", &home).env_remove("CLAUDE_CONFIG_DIR");
    assert_eq!(
        rewrite_event(cmd, &proj, &bash_event(&proj, "pytest -q")),
        ""
    );
}

#[test]
fn rule_on_the_tool_guards_launcher_and_path_forms() {
    let (_tmp, home, proj) = home_and_project();
    write_settings(
        &proj.join(".claude/settings.json"),
        r#"{"permissions":{"ask":["Bash(pytest:*)","Bash(xcodebuild:*)","Bash(gradlew:*)"]}}"#,
    );
    for cmd in [
        "python -m pytest -q",
        "uv run pytest",
        "xcrun xcodebuild test -scheme A",
        "./gradlew test",
    ] {
        let event = bash_event(&proj, cmd);
        assert_eq!(
            rewrite_event(cartoon_with_home(&home), &proj, &event),
            "",
            "{cmd}"
        );
    }
}

#[test]
fn project_dir_rules_apply_when_shell_cwd_is_elsewhere() {
    let (tmp, home, proj) = home_and_project();
    let other = tmp.path().join("other");
    fs::create_dir_all(&other).unwrap();
    write_settings(
        &proj.join(".claude/settings.json"),
        r#"{"permissions":{"deny":["Bash(make check:*)"]}}"#,
    );
    let mut cmd = cartoon_with_home(&home);
    cmd.env("CLAUDE_PROJECT_DIR", &proj);
    let event = bash_event(&other, "make check");
    assert_eq!(rewrite_event(cmd, &other, &event), "");
}

#[test]
fn no_rules_still_rewrites() {
    let (_tmp, home, proj) = home_and_project();
    let event = bash_event(&proj, "pytest -q");
    let out = rewrite_event(cartoon_with_home(&home), &proj, &event);
    assert!(out.contains("updatedInput"), "{out}");
}

#[test]
fn plan_mode_emits_nothing() {
    let (_tmp, home, proj) = home_and_project();
    let mut event = bash_event(&proj, "pytest");
    event["permission_mode"] = serde_json::json!("plan");
    assert_eq!(rewrite_event(cartoon_with_home(&home), &proj, &event), "");
}

#[test]
fn malformed_settings_emit_nothing() {
    let (_tmp, home, proj) = home_and_project();
    write_settings(&proj.join(".claude/settings.local.json"), "{oops");
    let event = bash_event(&proj, "pytest -q");
    assert_eq!(rewrite_event(cartoon_with_home(&home), &proj, &event), "");
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
fn rewrite_never_approves_side_effects_or_watch_modes() {
    let tmp = tempfile::tempdir().unwrap();
    for cmd in [
        "mvn test deploy",
        "gradle build publish",
        "./gradlew :app:publish",
        "make deploy",
        "pytest && make deploy",
        "npm ci",
        "jest --watchAll",
        "cargo doc --open",
    ] {
        assert_eq!(rewrite(tmp.path(), cmd), "", "{cmd} must not be rewritten");
    }
    for cmd in [
        "npm ci --ignore-scripts",
        "gradle :lib:testDebugUnitTest",
        "make test",
    ] {
        let out = rewrite(tmp.path(), cmd);
        assert!(out.contains("updatedInput"), "{cmd}: {out}");
    }
}

#[test]
fn rewrite_never_approves_xcodebuild_exports_or_account_access() {
    let tmp = tempfile::tempdir().unwrap();
    for cmd in [
        "xcodebuild -exportArchive -archivePath build/App.xcarchive -exportOptionsPlist ExportOptions.plist -exportPath build/export test",
        "xcodebuild test -exportArchive -archivePath a -exportOptionsPlist E.plist",
        "xcodebuild archive -scheme App -archivePath test",
        "xcodebuild build install -scheme A",
        "xcodebuild build -allowProvisioningUpdates -scheme A",
        "xcodebuild build -IDESkipMacroFingerprintValidation=YES -scheme A",
        "xcodebuild test -scheme A -destination 'platform=iOS,id=00008110-000A1B2C3D4E'",
    ] {
        assert_eq!(rewrite(tmp.path(), cmd), "", "{cmd} must not be rewritten");
    }
    let out = rewrite(
        tmp.path(),
        "xcodebuild test -scheme A -destination 'platform=iOS Simulator,name=iPhone 15' -enableCodeCoverage YES",
    );
    assert!(out.contains("updatedInput"), "{out}");
}

#[test]
fn rewrite_still_approves_the_dev_loop() {
    let tmp = tempfile::tempdir().unwrap();
    let inside = tmp.path().join("tests/test_a.py");
    // An absolute path under the event's cwd is in-project.
    let cmds = [
        "pytest -q".to_string(),
        "pytest -k 'not slow'".to_string(),
        "cargo build --release && cargo test".to_string(),
        format!("pytest {}", inside.display()),
    ];
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
fn hook_install_copilot_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    for flag in ["--copilot", "--vscode", "--deny"] {
        cartoon()
            .current_dir(tmp.path())
            .args(["hook", "install", flag, "--project"])
            .assert()
            .failure()
            .stderr(contains("removed in 0.8.0"));
    }
    assert!(!tmp.path().join(".github").exists(), "nothing written");
    assert!(!tmp.path().join(".claude").exists(), "nothing written");
}

#[test]
fn hook_rewrite_rejects_flags() {
    cartoon()
        .args(["hook", "rewrite", "--deny-mode"])
        .write_stdin("{}")
        .assert()
        .failure()
        .stderr(contains("removed in 0.8.0"));
}

#[test]
fn copilot_event_shape_emits_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let event = serde_json::json!({
        "toolName": "bash",
        "toolArgs": "{\"command\":\"pytest\"}",
    });
    assert_eq!(rewrite_event(cartoon(), tmp.path(), &event), "");
    let vscode = serde_json::json!({
        "tool_name": "run_in_terminal",
        "cwd": tmp.path(),
        "tool_input": { "command": "pytest" },
    });
    assert_eq!(rewrite_event(cartoon(), tmp.path(), &vscode), "");
}

#[test]
fn uninstall_copilot_still_removes_a_legacy_hook_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(".github/hooks/cartoon.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        r#"{"hooks":{"preToolUse":[{"command":"cartoon hook rewrite"}]}}"#,
    )
    .unwrap();
    cartoon()
        .current_dir(tmp.path())
        .args(["hook", "uninstall", "--copilot", "--project"])
        .assert()
        .success();
    assert!(!path.exists());
}

#[test]
fn install_migrates_a_pre_0_8_entry_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let settings = tmp.path().join(".claude/settings.json");
    write_settings(
        &settings,
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash|run_in_terminal","hooks":[{"type":"command","command":"command -v cartoon >/dev/null 2>&1 && cartoon hook rewrite --deny-mode || exit 0"}]}]}}"#,
    );
    cartoon()
        .current_dir(tmp.path())
        .args(["hook", "install", "--project"])
        .assert()
        .success();
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    let arr = v["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["matcher"], "Bash");
    let cmd = arr[0]["hooks"][0]["command"].as_str().unwrap();
    assert!(!cmd.contains("--deny-mode"), "{cmd}");
}

const PRE_0_8_ENTRY: &str = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash|run_in_terminal","hooks":[{"type":"command","command":"command -v cartoon >/dev/null 2>&1 && cartoon hook rewrite --deny-mode || exit 0"}]}]}}"#;

fn run_in(home: &std::path::Path, proj: &std::path::Path, args: &[&str]) -> String {
    let out = cartoon()
        .env("HOME", home)
        .current_dir(proj)
        .args(args)
        .assert()
        .success();
    String::from_utf8(out.get_output().stdout.clone()).unwrap()
}

#[test]
fn doctor_does_not_count_a_legacy_copilot_file_as_an_installed_hook() {
    let (_tmp, home, proj) = home_and_project();
    write_settings(
        &home.join(".copilot/hooks/cartoon.json"),
        r#"{"hooks":{"preToolUse":[{"command":"cartoon hook rewrite"}]}}"#,
    );
    let out = run_in(&home, &proj, &["doctor"]);
    assert!(out.contains("# hook not installed"), "{out}");
    assert!(!out.contains("# hook installed"), "{out}");
    assert!(out.contains("cartoon hook uninstall --copilot"), "{out}");
    let status = run_in(&home, &proj, &["hook", "status"]);
    assert!(status.contains("legacy"), "{status}");
}

#[test]
fn status_and_doctor_flag_a_pre_0_8_entry_as_stale() {
    let (_tmp, home, proj) = home_and_project();
    write_settings(&home.join(".claude/settings.json"), PRE_0_8_ENTRY);
    let status = run_in(&home, &proj, &["hook", "status"]);
    assert!(status.contains("stale"), "{status}");
    assert!(!status.contains(": installed"), "{status}");
    let out = run_in(&home, &proj, &["doctor"]);
    assert!(out.contains("stale"), "{out}");
    assert!(out.contains("# hook stale"), "{out}");
    assert!(!out.contains("# hook installed"), "{out}");
    // Re-running install refreshes it, after which status is clean.
    run_in(&home, &proj, &["hook", "install"]);
    let status = run_in(&home, &proj, &["hook", "status"]);
    assert!(status.contains(": installed"), "{status}");
    assert!(!status.contains("stale"), "{status}");
}

/// The rewritten command the hook approved, or None when it emitted nothing
/// or something other than `allow`.
fn approved(out: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(out).ok()?;
    let hso = &v["hookSpecificOutput"];
    (hso["permissionDecision"] == "allow")
        .then(|| hso["updatedInput"]["command"].as_str().map(String::from))?
}

#[test]
fn rewrite_covers_the_agent_shapes_around_xcodebuild() {
    let (_tmp, home, proj) = home_and_project();
    fs::create_dir(proj.join("App")).unwrap();
    for (cmd, want) in [
        (
            "swift build 2>&1",
            "cartoon --merge-streams -c 'swift build'",
        ),
        (
            "xcodebuild test -scheme App 2>&1 | tail -40",
            "cartoon --merge-streams -c 'xcodebuild test -scheme App | tail -40'",
        ),
        (
            "cd App && xcodebuild test -scheme App",
            "cd App && cartoon --merge-streams -c 'xcodebuild test -scheme App'",
        ),
    ] {
        let out = rewrite_event(cartoon_with_home(&home), &proj, &bash_event(&proj, cmd));
        assert_eq!(approved(&out).as_deref(), Some(want), "{cmd}: {out}");
    }
}

#[test]
fn rewrite_refuses_python_modules_other_than_test_runners() {
    let (_tmp, home, proj) = home_and_project();
    let allowed = rewrite_event(
        cartoon_with_home(&home),
        &proj,
        &bash_event(&proj, "python -m pytest -q"),
    );
    assert!(allowed.contains("\"allow\""), "{allowed}");
    for cmd in [
        "python -m pip install x",
        "python -m http.server",
        "python -m mymod",
        "python -m cartoon -c x",
        "python3 -m cartoon -c x",
    ] {
        let out = rewrite_event(cartoon_with_home(&home), &proj, &bash_event(&proj, cmd));
        assert_eq!(out, "", "{cmd}");
    }
}

#[test]
fn rewrite_refuses_unsafe_pipes_and_cds() {
    let (_tmp, home, proj) = home_and_project();
    fs::create_dir(proj.join("App")).unwrap();
    for cmd in [
        "pytest | sh",
        "pytest | tail -f",
        "make | tail -5",
        "cd .. && pytest",
        "cd nope && pytest",
        "pytest > out.txt 2>&1",
    ] {
        let out = rewrite_event(cartoon_with_home(&home), &proj, &bash_event(&proj, cmd));
        assert_eq!(out, "", "{cmd}");
    }
}

#[cfg(unix)]
#[test]
fn rewrite_refuses_cd_into_a_symlink_leaving_the_project() {
    let (tmp, home, proj) = home_and_project();
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, proj.join("App")).unwrap();
    let event = bash_event(&proj, "cd App && pytest");
    assert_eq!(rewrite_event(cartoon_with_home(&home), &proj, &event), "");
}

#[test]
fn exact_deny_rule_still_stops_the_2to1_form() {
    let (_tmp, home, proj) = home_and_project();
    write_settings(
        &proj.join(".claude/settings.json"),
        r#"{"permissions":{"deny":["Bash(swift build)"]}}"#,
    );
    let event = bash_event(&proj, "swift build 2>&1");
    assert_eq!(rewrite_event(cartoon_with_home(&home), &proj, &event), "");
}

#[test]
fn exact_deny_rule_with_cd_still_stops_the_2to1_form() {
    let (_tmp, home, proj) = home_and_project();
    fs::create_dir(proj.join("App")).unwrap();
    let event = bash_event(&proj, "cd App && swift build 2>&1");
    // Positive control: with no rule the cd shape is rewritten.
    let open = rewrite_event(cartoon_with_home(&home), &proj, &event);
    assert!(open.contains("\"allow\""), "{open}");
    write_settings(
        &proj.join(".claude/settings.json"),
        r#"{"permissions":{"deny":["Bash(cd App && swift build)"]}}"#,
    );
    assert_eq!(rewrite_event(cartoon_with_home(&home), &proj, &event), "");
}
