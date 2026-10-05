//! The plugin's SessionStart hook (hooks/session-start.sh) under a
//! controlled PATH: silent when the cartoon binary is present and current,
//! a one-time install note when it is missing, a one-time upgrade hint when
//! it is older than the plugin, and never a failing exit.
#![cfg(unix)]
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn plugin_version() -> String {
    let plugin = fs::read_to_string(root().join(".claude-plugin/plugin.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&plugin).unwrap();
    json["version"].as_str().unwrap().to_string()
}

/// A temp dir holding `bin/` (the only PATH entry: `mkdir`, the one external
/// tool the script uses, plus an optional `cartoon`) and isolated XDG dirs.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let mkdir = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|d| d.join("mkdir"))
            .find(|p| p.is_file())
            .expect("mkdir on PATH");
        symlink(mkdir, bin.join("mkdir")).unwrap();
        Sandbox { dir }
    }

    fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    /// Put the real cartoon binary on the sandbox PATH.
    fn with_real_cartoon(self) -> Self {
        symlink(
            env!("CARGO_BIN_EXE_cartoon"),
            self.dir.path().join("bin/cartoon"),
        )
        .unwrap();
        self
    }

    /// Put a fake cartoon that reports `version` on the sandbox PATH.
    fn with_fake_cartoon(self, version: &str) -> Self {
        let path = self.dir.path().join("bin/cartoon");
        fs::write(&path, format!("#!/bin/sh\necho 'cartoon {version}'\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        self
    }

    fn run_with_state(&self, state: &Path) -> (i32, String) {
        let out = Command::new("/bin/sh")
            .arg(root().join("hooks/session-start.sh"))
            .env_clear()
            .env("PATH", self.dir.path().join("bin"))
            .env("HOME", self.dir.path())
            .env("CLAUDE_PLUGIN_ROOT", root())
            .env("XDG_STATE_HOME", state)
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8(out.stdout).unwrap(),
        )
    }

    fn run(&self) -> (i32, String) {
        self.run_with_state(&self.state())
    }
}

/// The hook's stdout must be the documented SessionStart JSON shape.
fn context_of(stdout: &str) -> String {
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("hook stdout is not JSON ({e}): {stdout}"));
    let out = &json["hookSpecificOutput"];
    assert_eq!(out["hookEventName"], "SessionStart");
    assert_eq!(json["systemMessage"], out["additionalContext"]);
    out["additionalContext"].as_str().unwrap().to_string()
}

#[test]
fn silent_when_the_current_binary_is_on_path() {
    let sb = Sandbox::new().with_real_cartoon();
    assert_eq!(sb.run(), (0, String::new()));
    assert!(!sb.state().exists(), "no marker when nothing was shown");
}

#[test]
fn silent_when_the_binary_is_newer_than_the_plugin() {
    let sb = Sandbox::new().with_fake_cartoon("999.0.0");
    assert_eq!(sb.run(), (0, String::new()));
}

#[test]
fn missing_binary_gets_install_instructions_once() {
    let sb = Sandbox::new();
    let (code, out) = sb.run();
    assert_eq!(code, 0);
    let ctx = context_of(&out);
    for install in [
        "uv tool install cartoon",
        "npm i -g cartoon-wrap",
        "cargo install cartoon",
        "brew install cartoon",
    ] {
        assert!(ctx.contains(install), "missing `{install}` in: {ctx}");
    }
    assert!(sb
        .state()
        .join(format!("cartoon/plugin-hint-missing-{}", plugin_version()))
        .exists());
    assert_eq!(sb.run(), (0, String::new()), "second session stays quiet");
}

#[test]
fn older_binary_gets_one_upgrade_hint() {
    let sb = Sandbox::new().with_fake_cartoon("0.0.9");
    let (code, out) = sb.run();
    assert_eq!(code, 0);
    let ctx = context_of(&out);
    assert!(ctx.contains("binary is 0.0.9"), "{ctx}");
    assert!(
        ctx.contains(&format!("plugin {}", plugin_version())),
        "{ctx}"
    );
    assert!(ctx.contains("upgrade"), "{ctx}");
    assert_eq!(sb.run(), (0, String::new()), "second session stays quiet");
}

#[test]
fn version_compare_is_numeric_not_lexical() {
    // 0.10.0 > 0.9.0 even though "1" < "9" as text.
    let (maj, min) = {
        let v = plugin_version();
        let mut p = v.split('.').map(|n| n.parse::<u32>().unwrap());
        (p.next().unwrap(), p.next().unwrap())
    };
    let newer = format!("{maj}.{}.0", min + 10);
    let sb = Sandbox::new().with_fake_cartoon(&newer);
    assert_eq!(sb.run(), (0, String::new()), "{newer} is not older");
}

#[test]
fn unwritable_state_dir_never_fails_the_session() {
    let sb = Sandbox::new();
    // A regular file where the state dir should be: the marker can't be
    // written, so the note is skipped rather than repeated every session.
    let blocker = sb.dir.path().join("blocker");
    fs::write(&blocker, "").unwrap();
    assert_eq!(sb.run_with_state(&blocker), (0, String::new()));
}

#[test]
fn hooks_json_registers_the_session_start_script() {
    let hooks: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root().join("hooks/hooks.json")).unwrap())
            .unwrap();
    let entry = &hooks["hooks"]["SessionStart"][0];
    assert_eq!(entry["matcher"], "startup|resume");
    let cmd = entry["hooks"][0]["command"].as_str().unwrap();
    assert!(
        cmd.contains("${CLAUDE_PLUGIN_ROOT}/hooks/session-start.sh"),
        "{cmd}"
    );
    // The PreToolUse rewrite hook is still there.
    assert_eq!(
        hooks["hooks"]["PreToolUse"][0]["matcher"],
        "Bash|run_in_terminal"
    );
}
