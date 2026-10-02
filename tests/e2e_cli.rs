#![cfg(unix)]
//! Argument-handling e2e: unknown options, `--`, per-subcommand `--help`,
//! `-c` pipe-filter elision boundaries and `--max-tokens` on ingest.
use std::io::Write;
use std::process::{Command, Output, Stdio};

fn isolated() -> &'static (tempfile::TempDir, tempfile::TempDir) {
    static DIRS: std::sync::OnceLock<(tempfile::TempDir, tempfile::TempDir)> =
        std::sync::OnceLock::new();
    DIRS.get_or_init(|| (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()))
}

fn cartoon() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cartoon"));
    let (state, config) = isolated();
    cmd.env("XDG_STATE_HOME", state.path())
        .env("XDG_CONFIG_HOME", config.path())
        .env_remove("CARTOON_MAX_TOKENS");
    cmd
}

fn run(args: &[&str]) -> Output {
    cartoon().args(args).output().expect("run cartoon")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn unknown_option_is_a_usage_error_not_command_not_found() {
    for flag in ["--rwa", "-x"] {
        let out = run(&[flag, "true"]);
        assert_eq!(out.status.code(), Some(2), "{flag}");
        let err = text(&out.stderr);
        assert!(err.contains("unexpected argument"), "{flag}: {err}");
        assert!(!err.contains("command not found"), "{flag}: {err}");
    }
}

#[test]
fn double_dash_runs_a_binary_whose_name_starts_with_a_dash() {
    let bin = tempfile::tempdir().unwrap();
    let script = bin.path().join("--weird-bin");
    std::fs::write(&script, "#!/bin/sh\necho weird ran \"$@\"\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = cartoon()
        .env("PATH", path)
        .args(["--", "--weird-bin", "-q"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("weird ran -q"));
}

#[test]
fn bare_dash_still_ingests_stdin() {
    let mut child = cartoon()
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"hello from stdin\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).contains("hello from stdin"));
}

#[test]
fn every_reserved_subcommand_prints_its_own_help_and_exits_0() {
    let cwd = tempfile::tempdir().unwrap();
    for (sub, needle) in [
        ("ingest", "ingest (<file> | -)"),
        ("learn", "cartoon learn"),
        ("doctor", "cartoon doctor"),
        ("init", "cartoon init"),
        ("stats", "cartoon stats"),
        ("logs", "cartoon logs"),
        ("adapters", "cartoon adapters"),
        ("hook", "cartoon hook"),
        ("instructions", "cartoon instructions"),
        ("shim", "cartoon shim"),
    ] {
        for flag in ["--help", "-h"] {
            let out = cartoon()
                .current_dir(cwd.path())
                .args([sub, flag])
                .output()
                .unwrap();
            let stdout = text(&out.stdout);
            assert_eq!(out.status.code(), Some(0), "{sub} {flag}: {stdout}");
            assert!(stdout.contains(needle), "{sub} {flag}: {stdout}");
            // Help never runs the subcommand.
            assert!(!stdout.contains("version:"), "{sub} ran doctor: {stdout}");
            assert!(!stdout.starts_with("init:"), "{sub} ran init: {stdout}");
            assert!(!stdout.contains("pytest:"), "{sub} listed adapters");
        }
    }
    // Nothing was written by install/hook/instructions help.
    assert_eq!(std::fs::read_dir(cwd.path()).unwrap().count(), 0);
}

#[test]
fn top_level_help_lists_every_reserved_subcommand() {
    let out = run(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    let help = text(&out.stdout);
    for sub in [
        "stats",
        "adapters",
        "doctor",
        "init",
        "logs",
        "learn",
        "hook",
        "shim",
        "instructions",
        "ingest",
    ] {
        assert!(help.contains(&format!("`{sub}`")), "{sub}:\n{help}");
    }
}

#[test]
fn pipe_filter_with_a_redirect_keeps_the_redirect() {
    let cwd = tempfile::tempdir().unwrap();
    let out = cartoon()
        .current_dir(cwd.path())
        .args(["-c", "pytest --version | tail -5 > f.txt"])
        .output()
        .unwrap();
    assert!(
        cwd.path().join("f.txt").exists(),
        "redirect dropped: {}",
        text(&out.stdout)
    );
}

#[test]
fn pipe_filter_followed_by_and_and_keeps_the_trailing_command() {
    let out = run(&["-c", "pytest --version | tail -2 && echo TRAILING_RAN"]);
    let stdout = text(&out.stdout);
    assert!(stdout.contains("TRAILING_RAN"), "{stdout}");
    assert!(!stdout.contains("pipe_filter_dropped"), "{stdout}");
}

#[test]
fn ingest_honors_the_max_tokens_flag() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.log");
    let body: String = (0..400)
        .map(|i| format!("step {i} compiled module_{i} in {}ms\n", i * 7 % 97))
        .collect();
    std::fs::write(&path, &body).unwrap();
    let p = path.to_str().unwrap();
    let full = run(&["ingest", p]);
    let capped = run(&["--max-tokens", "60", "ingest", p]);
    assert_eq!(capped.status.code(), Some(0));
    let (full, capped) = (text(&full.stdout), text(&capped.stdout));
    assert!(
        capped.len() * 3 < full.len(),
        "ceiling ignored: {} vs {} bytes",
        capped.len(),
        full.len()
    );
}
