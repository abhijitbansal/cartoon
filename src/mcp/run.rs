//! The `run` tool: re-invoke this very binary as `cartoon -c '<command>'`
//! so adapters, the net-savings guard, the archive, the stats ledger and
//! signal forwarding all behave exactly as on the command line — the server
//! only adds a timeout, cancellation and output capture.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct RunArgs {
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub timeout: Duration,
    pub compress: Option<String>,
    pub max_tokens: Option<usize>,
}

#[derive(Debug)]
pub struct RunResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
    pub stdout_capture: CaptureMetadata,
    pub stderr_capture: CaptureMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStatus {
    Eof,
    ReadError,
    Deadline,
}

/// Observation of cartoon's transformed output, not the command's raw log.
/// EOF alone does not imply lossless capture: inspect dropped bytes and UTF-8.
#[derive(Debug, serde::Serialize)]
pub struct CaptureMetadata {
    pub status: CaptureStatus,
    pub captured_bytes: usize,
    pub dropped_bytes: usize,
    pub utf8_lossy: bool,
}

#[derive(Debug)]
pub enum RunError {
    NotStarted(std::io::Error),
    Observation(std::io::Error),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStarted(e) | Self::Observation(e) => e.fmt(f),
        }
    }
}

/// Per-stream capture ceiling. cartoon's own output is already compact; this
/// only bounds a passthrough of something enormous (the archive keeps all).
const CAPTURE_LIMIT: usize = 4 << 20;
/// After SIGTERM (which cartoon forwards to the command's process group),
/// how long the command gets to exit before cartoon's group is SIGKILLed.
const KILL_GRACE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);

/// The argv after the binary. `--shell=<cmd>` (not `-c <cmd>`) so a command
/// starting with `-` is still taken as the value.
pub fn cartoon_args(a: &RunArgs) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(c) = &a.compress {
        v.push(format!("--compress={c}"));
    }
    if let Some(n) = a.max_tokens {
        v.push(format!("--max-tokens={n}"));
    }
    v.push(format!("--shell={}", a.command));
    v
}

/// One stream, read on its own thread into a shared, capped buffer.
struct Capture {
    state: Arc<Mutex<CaptureState>>,
}

#[derive(Default)]
struct CaptureState {
    bytes: Vec<u8>,
    dropped: usize,
    terminal: Option<CaptureStatus>,
}

impl Capture {
    fn start(mut r: impl Read + Send + 'static) -> Self {
        let state = Arc::new(Mutex::new(CaptureState::default()));
        let shared = state.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 64 * 1024];
            loop {
                let n = match r.read(&mut chunk) {
                    Ok(0) => {
                        shared.lock().unwrap().terminal = Some(CaptureStatus::Eof);
                        break;
                    }
                    Ok(n) => n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        shared.lock().unwrap().terminal = Some(CaptureStatus::ReadError);
                        break;
                    }
                };
                let mut g = shared.lock().unwrap();
                let room = CAPTURE_LIMIT.saturating_sub(g.bytes.len());
                let keep = n.min(room);
                g.bytes.extend_from_slice(&chunk[..keep]);
                g.dropped = g.dropped.saturating_add(n - keep);
            }
        });
        Capture { state }
    }

    /// The text read so far. Waits briefly for EOF; a grandchild that
    /// escaped the kill could hold the pipe open forever, so never block.
    fn finish(self) -> (String, CaptureMetadata) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.state.lock().unwrap().terminal.is_none() && Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        // Text and metadata must describe the same snapshot even if the reader
        // is still draining a pipe held open by a descendant.
        let g = self.state.lock().unwrap();
        let meta = CaptureMetadata {
            status: g.terminal.unwrap_or(CaptureStatus::Deadline),
            captured_bytes: g.bytes.len(),
            dropped_bytes: g.dropped,
            utf8_lossy: std::str::from_utf8(&g.bytes).is_err(),
        };
        let mut s = String::from_utf8_lossy(&g.bytes).into_owned();
        if g.dropped > 0 {
            s.push_str(&format!(
                "\n[cartoon mcp: {} more bytes not captured; the raw log has everything]",
                g.dropped
            ));
        }
        (s, meta)
    }
}

/// Run `a.command` through cartoon. Observation errors can occur after spawn;
/// they must not be presented as proof that no command executed.
pub fn execute(exe: &Path, a: &RunArgs, cancel: &AtomicBool) -> Result<RunResult, RunError> {
    let mut cmd = Command::new(exe);
    cmd.args(cartoon_args(a))
        // The server's stdin is the protocol channel: never hand it down.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = &a.cwd {
        if !dir.is_dir() {
            return Err(RunError::NotStarted(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("cwd {} is not a directory", dir.display()),
            )));
        }
        cmd.current_dir(dir);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own group: the timeout signals cartoon without touching the server.
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(RunError::NotStarted)?;
    let out = Capture::start(child.stdout.take().expect("piped"));
    let err = Capture::start(child.stderr.take().expect("piped"));
    let deadline = Instant::now() + a.timeout;
    let mut timed_out = false;
    let mut term_sent: Option<Instant> = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Ok(st),
            Ok(None) => {}
            Err(e) => break Err(e),
        };
        let now = Instant::now();
        match term_sent {
            None if now >= deadline || cancel.load(Ordering::SeqCst) => {
                timed_out = now >= deadline;
                terminate(&mut child, false);
                term_sent = Some(now);
            }
            Some(t) if now.duration_since(t) >= KILL_GRACE => {
                terminate(&mut child, true);
                break child.wait();
            }
            _ => {}
        }
        std::thread::sleep(POLL);
    };
    let status = status.map_err(|e| {
        // Best effort only; an observation failure is never a cleanup receipt.
        terminate(&mut child, true);
        let _ = child.wait();
        RunError::Observation(e)
    })?;
    let (stdout, stdout_capture) = out.finish();
    let (stderr, stderr_capture) = err.finish();
    Ok(RunResult {
        stdout,
        stderr,
        exit_code: crate::runner::exit_code(&status),
        timed_out,
        stdout_capture,
        stderr_capture,
    })
}

/// SIGTERM cartoon's process group (cartoon forwards it to the command's own
/// group, then still emits and archives what was printed), or SIGKILL it.
fn terminate(child: &mut Child, hard: bool) {
    #[cfg(unix)]
    {
        let sig = if hard { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: kill(2) on a process group we created; no memory effects.
        unsafe { libc::kill(-(child.id() as i32), sig) };
    }
    #[cfg(not(unix))]
    {
        let _ = hard;
        let _ = child.kill();
    }
}

impl RunResult {
    /// The tool's text: what cartoon printed, stderr labelled, then the
    /// exit status (and the timeout, if it fired).
    pub fn render(&self, a: &RunArgs) -> String {
        let mut s = self.stdout.trim_end_matches('\n').to_string();
        let err = self.stderr.trim_end_matches('\n');
        if !err.is_empty() {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str("--- stderr ---\n");
            s.push_str(err);
        }
        if !s.is_empty() {
            s.push('\n');
        }
        if self.timed_out {
            s.push_str(&format!(
                "timed_out: true (killed after {}s; raise timeout_s for longer runs)\n",
                a.timeout.as_secs_f64()
            ));
        }
        let exit = format!("exit_code: {}", self.exit_code);
        // An adapter report that already states it (pytest) needn't repeat.
        if !self.timed_out && self.stdout.lines().any(|l| l == exit) {
            return s.trim_end_matches('\n').to_string();
        }
        s.push_str(&exit);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(cmd: &str) -> RunArgs {
        RunArgs {
            command: cmd.into(),
            cwd: None,
            timeout: Duration::from_secs(10),
            compress: None,
            max_tokens: None,
        }
    }

    fn empty_result() -> RunResult {
        let meta = || CaptureMetadata {
            status: CaptureStatus::Eof,
            captured_bytes: 0,
            dropped_bytes: 0,
            utf8_lossy: false,
        };
        RunResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
            stdout_capture: meta(),
            stderr_capture: meta(),
        }
    }

    #[test]
    fn argv_passes_flags_and_keeps_a_hyphenated_command_as_the_value() {
        let mut a = args("-weird --flag");
        a.compress = Some("aggressive".into());
        a.max_tokens = Some(500);
        assert_eq!(
            cartoon_args(&a),
            [
                "--compress=aggressive",
                "--max-tokens=500",
                "--shell=-weird --flag"
            ]
        );
    }

    #[test]
    fn render_labels_stderr_and_ends_with_the_exit_code() {
        let r = RunResult {
            stdout: "report\n".into(),
            stderr: "warn\n".into(),
            exit_code: 1,
            timed_out: false,
            ..empty_result()
        };
        assert_eq!(
            r.render(&args("x")),
            "report\n--- stderr ---\nwarn\nexit_code: 1"
        );
        let r = RunResult {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
            ..empty_result()
        };
        assert_eq!(r.render(&args("x")), "exit_code: 0");
    }

    #[test]
    fn render_does_not_repeat_an_exit_code_the_report_already_states() {
        let r = RunResult {
            stdout: "failed: 1\nexit_code: 1\nraw_log: /x\n".into(),
            stderr: String::new(),
            exit_code: 1,
            timed_out: false,
            ..empty_result()
        };
        assert_eq!(r.render(&args("x")), "failed: 1\nexit_code: 1\nraw_log: /x");
    }

    #[test]
    fn render_discloses_a_timeout() {
        let r = RunResult {
            stdout: "partial\n".into(),
            stderr: String::new(),
            exit_code: 143,
            timed_out: true,
            ..empty_result()
        };
        let text = r.render(&args("x"));
        assert!(text.contains("timed_out: true (killed after 10s"), "{text}");
        assert!(text.ends_with("exit_code: 143"));
    }

    #[test]
    fn a_missing_cwd_is_an_error_not_a_run() {
        let mut a = args("true");
        a.cwd = Some("/definitely/not/here".into());
        let e = execute(Path::new("/bin/sh"), &a, &AtomicBool::new(false)).unwrap_err();
        assert!(e.to_string().contains("not a directory"), "{e}");
        assert!(matches!(e, RunError::NotStarted(_)));
    }

    #[test]
    fn a_missing_executable_is_not_started() {
        let e = execute(
            Path::new("/no/such/executable"),
            &args("true"),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(matches!(e, RunError::NotStarted(_)));
    }

    #[test]
    fn capture_distinguishes_read_error_from_eof_and_retries_interrupted_reads() {
        struct InterruptedThenError(usize);
        impl Read for InterruptedThenError {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                self.0 += 1;
                match self.0 {
                    1 => Err(std::io::ErrorKind::Interrupted.into()),
                    2 => {
                        bytes[0] = b'x';
                        Ok(1)
                    }
                    _ => Err(std::io::ErrorKind::Other.into()),
                }
            }
        }
        let (text, meta) = Capture::start(InterruptedThenError(0)).finish();
        assert_eq!(text, "x");
        assert_eq!(meta.status, CaptureStatus::ReadError);
        assert_eq!(meta.captured_bytes, 1);
        assert_eq!(meta.dropped_bytes, 0);
        assert!(!meta.utf8_lossy);
    }

    #[test]
    fn capture_exact_limit_is_not_truncated_but_a_split_utf8_character_is_lossy() {
        let bytes = vec![b'x'; CAPTURE_LIMIT];
        let (_, meta) = Capture::start(std::io::Cursor::new(bytes)).finish();
        assert_eq!(meta.status, CaptureStatus::Eof);
        assert_eq!(meta.captured_bytes, CAPTURE_LIMIT);
        assert_eq!(meta.dropped_bytes, 0);
        assert!(!meta.utf8_lossy);

        let mut bytes = vec![b'x'; CAPTURE_LIMIT - 1];
        bytes.extend_from_slice(&[0xc3, 0xa9]);
        let (_, meta) = Capture::start(std::io::Cursor::new(bytes)).finish();
        assert_eq!(meta.dropped_bytes, 1);
        assert!(meta.utf8_lossy);
    }
}
