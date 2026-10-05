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
    buf: Arc<Mutex<(Vec<u8>, usize)>>, // (kept bytes, bytes dropped)
    done: Arc<AtomicBool>,
}

impl Capture {
    fn start(mut r: impl Read + Send + 'static) -> Self {
        let buf = Arc::new(Mutex::new((Vec::new(), 0usize)));
        let done = Arc::new(AtomicBool::new(false));
        let (b, d) = (buf.clone(), done.clone());
        std::thread::spawn(move || {
            let mut chunk = [0u8; 64 * 1024];
            while let Ok(n) = r.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let mut g = b.lock().unwrap();
                let room = CAPTURE_LIMIT.saturating_sub(g.0.len());
                let keep = n.min(room);
                g.0.extend_from_slice(&chunk[..keep]);
                g.1 += n - keep;
            }
            d.store(true, Ordering::SeqCst);
        });
        Capture { buf, done }
    }

    /// The text read so far. Waits briefly for EOF; a grandchild that
    /// escaped the kill could hold the pipe open forever, so never block.
    fn finish(self) -> String {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.done.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        let g = self.buf.lock().unwrap();
        let mut s = String::from_utf8_lossy(&g.0).into_owned();
        if g.1 > 0 {
            s.push_str(&format!(
                "\n[cartoon mcp: {} more bytes not captured; the raw log has everything]",
                g.1
            ));
        }
        s
    }
}

/// Run `a.command` through cartoon. `Err` only when it could not start.
pub fn execute(exe: &Path, a: &RunArgs, cancel: &AtomicBool) -> std::io::Result<RunResult> {
    let mut cmd = Command::new(exe);
    cmd.args(cartoon_args(a))
        // The server's stdin is the protocol channel: never hand it down.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = &a.cwd {
        if !dir.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("cwd {} is not a directory", dir.display()),
            ));
        }
        cmd.current_dir(dir);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own group: the timeout signals cartoon without touching the server.
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;
    let out = Capture::start(child.stdout.take().expect("piped"));
    let err = Capture::start(child.stderr.take().expect("piped"));
    let deadline = Instant::now() + a.timeout;
    let mut timed_out = false;
    let mut term_sent: Option<Instant> = None;
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        let now = Instant::now();
        match term_sent {
            None if now >= deadline || cancel.load(Ordering::SeqCst) => {
                timed_out = now >= deadline;
                terminate(&mut child, false);
                term_sent = Some(now);
            }
            Some(t) if now.duration_since(t) >= KILL_GRACE => {
                terminate(&mut child, true);
                break child.wait()?;
            }
            _ => {}
        }
        std::thread::sleep(POLL);
    };
    Ok(RunResult {
        stdout: out.finish(),
        stderr: err.finish(),
        exit_code: crate::runner::exit_code(&status),
        timed_out,
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
    }
}
