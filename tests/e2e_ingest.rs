#![cfg(unix)]
use std::io::Write;
use std::process::{Command, Stdio};

fn cartoon_bin() -> &'static str {
    env!("CARGO_BIN_EXE_cartoon")
}

/// Every run archives into XDG_STATE_HOME; point it at a per-test temp dir so
/// `cargo test` never writes into (or prunes) the developer's real archive.
fn isolated_state() -> &'static tempfile::TempDir {
    static STATE: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    STATE.get_or_init(|| tempfile::tempdir().expect("temp state dir"))
}

fn noisy_log() -> String {
    let mut s = String::from("\x1b[32mbuild started\x1b[0m\n");
    for _ in 0..60 {
        s.push_str("copying asset bundle to staging area\n");
    }
    s.push_str("build finished\n");
    s
}

#[test]
fn ingest_file_compresses_like_a_wrapped_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("build.log");
    std::fs::write(&path, noisy_log()).unwrap();
    let out = Command::new(cartoon_bin())
        .env("XDG_STATE_HOME", isolated_state().path())
        .args(["ingest", path.to_str().unwrap()])
        .output()
        .expect("run cartoon");
    assert_eq!(out.status.code(), Some(0), "ingest exits 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("(x60)"), "repeats collapsed: {stdout}");
    assert!(!stdout.contains("\x1b["), "ANSI stripped");
    assert!(stdout.contains("raw_log"), "archived with footer");
}

#[test]
fn stdin_dash_ingests_piped_log() {
    let mut child = Command::new(cartoon_bin())
        .env("XDG_STATE_HOME", isolated_state().path())
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cartoon");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(noisy_log().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("(x60)"), "stdin compressed: {stdout}");
}

#[test]
fn ingest_missing_file_exits_2() {
    let out = Command::new(cartoon_bin())
        .env("XDG_STATE_HOME", isolated_state().path())
        .args(["ingest", "/no/such/file.log"])
        .output()
        .expect("run cartoon");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cannot read"), "{stderr}");
}

#[test]
fn ingest_sniffs_xcodebuild_shaped_wrapper_script_logs() {
    // A ./build.sh log: no argv0 to match, but the content is xcodebuild's.
    let mut log = String::from("Build settings from command line:\n    SDKROOT = iphoneos\n");
    for i in 0..80 {
        log.push_str(&format!(
            "CompileSwift normal arm64 /Users/d/App/Sources/File{i}.swift\n"
        ));
    }
    log.push_str("/Users/d/App/Sources/Auth.swift:18:9: error: cannot find 'tokn' in scope\n        tokn = refresh()\n        ^~~~\n** BUILD FAILED **\n");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("build.log");
    std::fs::write(&path, log).unwrap();
    let out = Command::new(cartoon_bin())
        .env("XDG_STATE_HOME", isolated_state().path())
        .args(["ingest", path.to_str().unwrap()])
        .output()
        .expect("run cartoon");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("runner: xcodebuild-build"),
        "sniffed: {stdout}"
    );
    assert!(stdout.contains("Auth.swift:18:9"), "{stdout}");
    assert!(stdout.contains("errors: 1"), "{stdout}");
}
