//! End-to-end checks of the run pipeline itself (runner + app): what reaches
//! the agent when the run fails oddly, is interrupted, emits bytes that are
//! not UTF-8, writes to a closed pipe, or is cut by `--max-tokens`.
use assert_cmd::Command;
use std::io::Read;
use std::path::Path;

fn cartoon(state: &Path) -> Command {
    let mut c = Command::cargo_bin("cartoon").unwrap();
    c.env("XDG_STATE_HOME", state)
        .env("XDG_CONFIG_HOME", state.join("config"))
        .env_remove("CARTOON_MAX_TOKENS")
        .env_remove("CARTOON_HEARTBEAT");
    c
}

fn bin() -> std::process::Command {
    std::process::Command::new(env!("CARGO_BIN_EXE_cartoon"))
}

fn have(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn out_of(a: &assert_cmd::assert::Assert) -> (String, String) {
    let o = a.get_output();
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

fn ledger(state: &Path) -> String {
    std::fs::read_to_string(state.join("cartoon/stats.jsonl")).unwrap_or_default()
}

fn only_run_dir(state: &Path) -> std::path::PathBuf {
    let runs: Vec<_> = std::fs::read_dir(state.join("cartoon/runs"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(runs.len(), 1, "{runs:?}");
    runs[0].clone()
}

// §1.1 ---------------------------------------------------------------------

#[test]
fn json_tail_never_swallows_the_failure_before_it() {
    let state = tempfile::tempdir().unwrap();
    let script = r#"i=1; while [ $i -le 50 ]; do echo "test_case_$i ... ok"; i=$((i+1)); done; echo "test_case_51 ... FAILED"; echo '{"coverage": 81.5}'"#;
    let a = cartoon(state.path())
        .args(["sh", "-c", script])
        .assert()
        .success();
    let (out, _) = out_of(&a);
    assert!(out.contains("test_case_51 ... FAILED"), "{out}");
    assert!(out.contains("coverage"), "{out}");
}

#[test]
fn ndjson_is_encoded_as_one_array() {
    let state = tempfile::tempdir().unwrap();
    let script = r#"i=0; while [ $i -lt 40 ]; do echo "{\"id\": $i, \"state\": \"running\", \"zone\": \"us-east-1a\"}"; i=$((i+1)); done"#;
    let a = cartoon(state.path())
        .args(["sh", "-c", script])
        .assert()
        .success();
    let (out, _) = out_of(&a);
    assert!(out.contains("[40]{id,state,zone}"), "{out}");
    assert!(out.contains("0,running,us-east-1a"), "{out}");
    assert!(out.contains("39,running,us-east-1a"), "{out}");
}

#[test]
fn json_numbers_reach_toon_exactly() {
    // serde_json's arbitrary_precision: no f64 rounding on the way through.
    let state = tempfile::tempdir().unwrap();
    // NDJSON large enough for the TOON table to beat the raw_log footer.
    let script = r#"i=10; while [ $i -lt 50 ]; do echo "{\"id\": 1234567890123456789012345678$i, \"ratio\": 1.50, \"tiny\": 1e-400, \"zero\": -0}"; i=$((i+1)); done"#;
    let a = cartoon(state.path())
        .args(["sh", "-c", script])
        .assert()
        .success();
    let (out, _) = out_of(&a);
    assert!(out.contains("[40]{id,ratio,tiny,zero}:"), "{out}");
    assert!(
        out.contains("\n  123456789012345678901234567810,1.5,1e-400,0\n"),
        "{out}"
    );
    assert!(out.contains("123456789012345678901234567849,1.5"), "{out}");
}

// §1.2 ---------------------------------------------------------------------

fn pytest_project(dir: &Path, body: &str) {
    let mut src = String::new();
    for i in 0..30 {
        src.push_str(&format!("def test_ok_{i}():\n    assert True\n\n"));
    }
    src.push_str(body);
    std::fs::write(dir.join("test_mod.py"), src).unwrap();
}

#[test]
fn adapter_report_with_no_failures_keeps_the_reason_for_a_nonzero_exit() {
    if !have("pytest") {
        eprintln!("SKIP: pytest not installed");
        return;
    }
    let state = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    pytest_project(
        proj.path(),
        "def test_zz_stop():\n    import pytest\n    pytest.exit('database fixture unavailable', returncode=3)\n",
    );
    let a = cartoon(state.path())
        .current_dir(proj.path())
        .args(["pytest", "-v", "-p", "no:cacheprovider", "test_mod.py"])
        .assert()
        .code(3);
    let (out, err) = out_of(&a);
    let all = format!("{out}{err}");
    assert!(all.contains("database fixture unavailable"), "{all}");
    // Whether or not the guard kept the report, the exit code is never hidden.
    assert!(
        out.contains("exit_code: 3") || !out.contains("runner: pytest"),
        "{out}"
    );
}

#[test]
fn adapter_report_shows_a_nonzero_exit_code() {
    if !have("pytest") {
        eprintln!("SKIP: pytest not installed");
        return;
    }
    let state = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    pytest_project(proj.path(), "def test_bad():\n    assert 1 == 2\n");
    let a = cartoon(state.path())
        .current_dir(proj.path())
        .args(["pytest", "-v", "-p", "no:cacheprovider", "test_mod.py"])
        .assert()
        .code(1);
    let (out, _) = out_of(&a);
    assert!(out.contains("runner: pytest"), "{out}");
    assert!(out.contains("failed: 1"), "{out}");
    assert!(out.contains("exit_code: 1"), "{out}");
}

// §1.14 --------------------------------------------------------------------

#[test]
fn max_tokens_keeps_a_mid_log_error() {
    let state = tempfile::tempdir().unwrap();
    let script = r#"i=0; while [ $i -lt 300 ]; do if [ $i -eq 150 ]; then echo "ERROR: migration 0042 failed"; else echo "step $i compiled"; fi; i=$((i+1)); done"#;
    let a = cartoon(state.path())
        .args(["--max-tokens", "300", "sh", "-c", script])
        .assert()
        .success();
    let (out, _) = out_of(&a);
    assert!(out.contains("migration 0042 failed"), "{out}");
    assert!(out.contains("omitted"), "{out}");
}

// §4.3 ---------------------------------------------------------------------

#[test]
fn max_tokens_budgets_stderr_too() {
    let state = tempfile::tempdir().unwrap();
    let a = cartoon(state.path())
        .args(["--max-tokens", "50", "sh", "-c", "seq 1 20000 >&2"])
        .assert()
        .success();
    let o = a.get_output();
    let total = o.stdout.len() + o.stderr.len();
    assert!(total < 600, "{total} bytes emitted under --max-tokens 50");
}

#[test]
fn stderr_goes_through_the_ladder() {
    let state = tempfile::tempdir().unwrap();
    let a = cartoon(state.path())
        .args([
            "sh",
            "-c",
            "i=0; while [ $i -lt 300 ]; do echo 'warning: deprecated flag --foo' >&2; i=$((i+1)); done; echo done",
        ])
        .assert()
        .success();
    let (out, err) = out_of(&a);
    assert!(err.matches("deprecated flag").count() < 5, "{err}");
    assert!(err.contains("x300") || err.contains("300"), "{err}");
    assert!(format!("{out}{err}").contains("raw_log:"));
}

// §4.5 ---------------------------------------------------------------------

#[test]
fn raw_is_byte_identical_for_non_utf8() {
    let state = tempfile::tempdir().unwrap();
    let a = cartoon(state.path())
        .args(["--raw", "sh", "-c", r"printf 'a\377b'; printf 'c\376' >&2"])
        .assert()
        .success();
    assert_eq!(a.get_output().stdout, b"a\xffb");
    assert_eq!(a.get_output().stderr, b"c\xfe");
    let dir = only_run_dir(state.path());
    assert_eq!(std::fs::read(dir.join("stdout.log")).unwrap(), b"a\xffb");
}

#[test]
fn passthrough_is_byte_identical_for_non_utf8() {
    let state = tempfile::tempdir().unwrap();
    let a = cartoon(state.path())
        .args(["sh", "-c", r"printf 'caf\351 au lait\n\001\002\377'"])
        .assert()
        .success();
    assert_eq!(a.get_output().stdout, b"caf\xe9 au lait\n\x01\x02\xff");
}

#[test]
fn ingest_accepts_non_utf8_stdin() {
    let state = tempfile::tempdir().unwrap();
    let a = cartoon(state.path())
        .args(["ingest", "-"])
        .write_stdin(b"caf\xe9\n".to_vec())
        .assert()
        .success();
    assert_eq!(a.get_output().stdout, b"caf\xe9\n");
}

// §4.4 ---------------------------------------------------------------------

#[test]
fn passthrough_and_raw_preserve_stream_order() {
    let state = tempfile::tempdir().unwrap();
    let inner = "echo out1; sleep 0.2; echo ERR1 >&2; sleep 0.2; echo out2";
    for raw in [false, true] {
        let flag = if raw { "--raw " } else { "" };
        let script = format!(
            "'{}' {flag}sh -c '{inner}' 2>&1",
            env!("CARGO_BIN_EXE_cartoon")
        );
        let o = std::process::Command::new("sh")
            .args(["-c", &script])
            .env("XDG_STATE_HOME", state.path())
            .env("XDG_CONFIG_HOME", state.path())
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&o.stdout),
            "out1\nERR1\nout2\n",
            "raw={raw}"
        );
    }
}

#[test]
fn raw_streams_output_live() {
    let state = tempfile::tempdir().unwrap();
    let mut child = bin()
        .args(["--raw", "sh", "-c", "echo first; sleep 3; echo second"])
        .env("XDG_STATE_HOME", state.path())
        .env("XDG_CONFIG_HOME", state.path())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    let mut stdout = child.stdout.take().unwrap();
    let mut buf = [0u8; 6];
    stdout.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"first\n");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "first line arrived only after {:?}",
        started.elapsed()
    );
    let mut rest = String::new();
    stdout.read_to_string(&mut rest).unwrap();
    assert_eq!(rest, "second\n");
    assert!(child.wait().unwrap().success());
}

#[test]
fn heartbeat_on_long_runs_and_can_be_turned_off() {
    let state = tempfile::tempdir().unwrap();
    let a = cartoon(state.path())
        .env("CARTOON_HEARTBEAT", "1")
        .args(["sh", "-c", "sleep 2.2; echo done"])
        .assert()
        .success();
    let (out, err) = out_of(&a);
    assert_eq!(out, "done\n");
    assert!(err.contains("cartoon: still running ("), "{err}");
    assert!(err.contains("MB captured)"), "{err}");

    let a = cartoon(state.path())
        .env("CARTOON_HEARTBEAT", "0")
        .args(["sh", "-c", "sleep 1.2; echo done"])
        .assert()
        .success();
    assert!(!out_of(&a).1.contains("still running"));
}

// §4.2 ---------------------------------------------------------------------

#[test]
fn closed_stdout_keeps_the_child_exit_code_and_records_stats() {
    let state = tempfile::tempdir().unwrap();
    let mut child = bin()
        .args(["sh", "-c", "seq 1 200000; exit 5"])
        .env("XDG_STATE_HOME", state.path())
        .env("XDG_CONFIG_HOME", state.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut buf = [0u8; 2];
    stdout.read_exact(&mut buf).unwrap();
    drop(stdout); // `| head -1`
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(5), "stderr: {err}");
    assert!(!err.contains("panicked"), "{err}");
    assert!(ledger(state.path()).contains("\"exit\":5"));
}

#[cfg(target_os = "linux")]
#[test]
fn dev_full_does_not_panic() {
    let state = tempfile::tempdir().unwrap();
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let o = bin()
        .args(["sh", "-c", "echo hello; exit 3"])
        .env("XDG_STATE_HOME", state.path())
        .env("XDG_CONFIG_HOME", state.path())
        .stdout(full)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(3));
    assert!(!String::from_utf8_lossy(&o.stderr).contains("panicked"));
}

// §4.1 ---------------------------------------------------------------------

#[cfg(target_os = "linux")]
#[test]
fn sigterm_is_forwarded_and_partial_output_is_kept() {
    let state = tempfile::tempdir().unwrap();
    let pidfile = state.path().join("sleep.pid");
    let script = format!(
        "echo partial; sleep 30 & echo $! > '{}'; wait",
        pidfile.display()
    );
    let child = bin()
        .args(["sh", "-c", &script])
        .env("XDG_STATE_HOME", state.path())
        .env("XDG_CONFIG_HOME", state.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Wait until the grandchild exists, then TERM cartoon only.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !pidfile.exists() || std::fs::read_to_string(&pidfile).unwrap().trim().is_empty() {
        assert!(std::time::Instant::now() < deadline, "child never started");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    let kill = std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(kill.success());
    let o = child.wait_with_output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(143), "stderr: {err}");
    assert!(out.contains("partial"), "stdout: {out:?} stderr: {err}");
    assert!(err.contains("interrupted by SIGTERM"), "{err}");
    // The grandchild (same process group) got the signal too.
    let pid = std::fs::read_to_string(&pidfile).unwrap();
    // Gone, or a zombie waiting for a reaper (a container's PID 1 may not reap).
    let alive = || {
        std::fs::read_to_string(format!("/proc/{}/stat", pid.trim()))
            .map(|st| !st.contains(") Z "))
            .unwrap_or(false)
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while alive() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let alive = alive();
    assert!(!alive, "sleep {pid} was orphaned");
    // Archived and recorded like any other run.
    let dir = only_run_dir(state.path());
    assert!(std::fs::read_to_string(dir.join("stdout.log"))
        .unwrap()
        .contains("partial"));
    assert!(ledger(state.path()).contains("\"exit\":143"));
}

// §4.10 --------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn non_executable_file_exits_126_with_the_cause() {
    let state = tempfile::tempdir().unwrap();
    let script = state.path().join("noexec.sh");
    std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
    let a = cartoon(state.path())
        .arg(script.to_str().unwrap())
        .assert()
        .code(126);
    let (_, err) = out_of(&a);
    assert!(err.contains("Permission denied"), "{err}");

    let a = cartoon(state.path())
        .arg(state.path().to_str().unwrap())
        .assert()
        .code(126);
    assert!(out_of(&a).1.contains("cannot execute"));
}

#[test]
fn bad_max_tokens_env_warns() {
    let state = tempfile::tempdir().unwrap();
    let a = cartoon(state.path())
        .env("CARTOON_MAX_TOKENS", "abc")
        .args(["sh", "-c", "echo hi"])
        .assert()
        .success();
    let (out, err) = out_of(&a);
    assert_eq!(out, "hi\n");
    assert!(err.contains("CARTOON_MAX_TOKENS"), "{err}");
}

// §5 -----------------------------------------------------------------------

#[test]
fn failed_archive_means_no_raw_log_pointer_and_no_lossy_output() {
    let tmp = tempfile::tempdir().unwrap();
    // A state "dir" that is a regular file: the archive cannot be written.
    let state = tmp.path().join("not-a-dir");
    std::fs::write(&state, "").unwrap();
    let json = r#"import json; print(json.dumps([{"name": "instance-%d" % i, "state": "running"} for i in range(30)]))"#;
    let a = Command::cargo_bin("cartoon")
        .unwrap()
        .env("XDG_STATE_HOME", &state)
        .env("XDG_CONFIG_HOME", tmp.path())
        .args(["python3", "-c", json])
        .assert()
        .success();
    let (out, _) = out_of(&a);
    assert!(!out.contains("raw_log"), "{out}");
    assert!(out.starts_with("[{\"name\": \"instance-0\""), "{out}");
}
