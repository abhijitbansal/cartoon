#![cfg(unix)]
use std::io::Write;
use std::process::{Command, Stdio};

fn cartoon_bin() -> &'static str {
    env!("CARGO_BIN_EXE_cartoon")
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
        .args(["ingest", "/no/such/file.log"])
        .output()
        .expect("run cartoon");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cannot read"), "{stderr}");
}
