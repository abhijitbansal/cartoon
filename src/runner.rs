use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// The child's output as text, which is what adapters, the ladder and token
/// counting work on. Lossy only when the child wrote non-UTF-8: the original
/// bytes then live alongside in `RunOutput` and every passthrough path emits
/// those instead.
#[derive(Debug)]
pub struct Captured {
    pub stdout: String,
    pub stderr: String,
    pub status: ExitStatus,
}

impl Captured {
    /// A Captured that no process produced (e.g. `cartoon ingest` reading an
    /// existing log). Status is a synthetic success; callers pass the real
    /// exit code separately everywhere it matters.
    pub fn synthetic(stdout: String, stderr: String) -> Self {
        Captured {
            stdout,
            stderr,
            status: success_status(),
        }
    }
}

fn success_status() -> ExitStatus {
    #[cfg(unix)]
    let status = <ExitStatus as std::os::unix::process::ExitStatusExt>::from_raw(0);
    #[cfg(windows)]
    let status = <ExitStatus as std::os::windows::process::ExitStatusExt>::from_raw(0);
    status
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Everything one run produced: the text view adapters parse, the original
/// bytes when they differ from that text, the order chunks arrived in, and
/// whether a signal interrupted the run.
#[derive(Debug)]
pub struct RunOutput {
    pub captured: Captured,
    /// Original stdout when it was not valid UTF-8 (`None`: the text IS the
    /// bytes — the common case, so no second copy is held).
    stdout_raw: Option<Vec<u8>>,
    stderr_raw: Option<Vec<u8>>,
    /// Arrival order as (stream, byte length) runs; adjacent chunks of the
    /// same stream are merged, so this stays tiny unless the streams
    /// genuinely interleave.
    order: Vec<(Stream, usize)>,
    /// The signal (SIGINT/SIGTERM/SIGHUP) cartoon received and forwarded
    /// while the child ran.
    pub interrupted: Option<i32>,
}

impl RunOutput {
    /// Wrap bytes no process produced (`cartoon ingest`). Non-UTF-8 input is
    /// accepted: the text view is lossy, passthrough emits the bytes.
    pub fn from_bytes(stdout: Vec<u8>) -> Self {
        let len = stdout.len();
        let (text, raw) = split_utf8(stdout);
        RunOutput {
            captured: Captured::synthetic(text, String::new()),
            stdout_raw: raw,
            stderr_raw: None,
            order: vec![(Stream::Stdout, len)],
            interrupted: None,
        }
    }

    pub fn stdout_bytes(&self) -> &[u8] {
        self.stdout_raw
            .as_deref()
            .unwrap_or(self.captured.stdout.as_bytes())
    }

    pub fn stderr_bytes(&self) -> &[u8] {
        self.stderr_raw
            .as_deref()
            .unwrap_or(self.captured.stderr.as_bytes())
    }

    /// True when either stream was not valid UTF-8 (the text view is lossy).
    pub fn has_raw_bytes(&self) -> bool {
        self.stdout_raw.is_some() || self.stderr_raw.is_some()
    }

    /// Write both streams' original bytes to `out`/`err` in the order the
    /// child produced them. Returns the first write error (e.g. BrokenPipe);
    /// the caller decides what that means.
    pub fn replay(&self, out: &mut dyn Write, err: &mut dyn Write) -> std::io::Result<()> {
        for (s, bytes) in self.segments() {
            let w: &mut dyn Write = match s {
                Stream::Stdout => out,
                Stream::Stderr => err,
            };
            w.write_all(bytes)?;
            w.flush()?;
        }
        out.flush()?;
        err.flush()
    }

    /// Both streams as one text in arrival order — what a terminal (or
    /// `2>&1`) shows. Lossy only where the bytes are not UTF-8.
    pub fn merged_text(&self) -> String {
        if self.captured.stderr.is_empty() {
            return self.captured.stdout.clone();
        }
        let mut bytes = Vec::with_capacity(self.stdout_bytes().len() + self.stderr_bytes().len());
        for (_, b) in self.segments() {
            bytes.extend_from_slice(b);
        }
        split_utf8(bytes).0
    }

    /// The original bytes as (stream, slice) runs in arrival order, then
    /// anything the order log does not cover (synthetic captures).
    fn segments(&self) -> Vec<(Stream, &[u8])> {
        let (o, e) = (self.stdout_bytes(), self.stderr_bytes());
        let (mut oi, mut ei) = (0usize, 0usize);
        let mut out = Vec::with_capacity(self.order.len() + 2);
        for &(s, n) in &self.order {
            let (buf, i) = match s {
                Stream::Stdout => (o, &mut oi),
                Stream::Stderr => (e, &mut ei),
            };
            let end = (*i + n).min(buf.len());
            out.push((s, &buf[*i..end]));
            *i = end;
        }
        out.push((Stream::Stdout, &o[oi..]));
        out.push((Stream::Stderr, &e[ei..]));
        out.retain(|(_, b)| !b.is_empty());
        out
    }
}

fn split_utf8(bytes: Vec<u8>) -> (String, Option<Vec<u8>>) {
    match String::from_utf8(bytes) {
        Ok(s) => (s, None), // zero-copy: the Vec becomes the String
        Err(e) => {
            let bytes = e.into_bytes();
            (String::from_utf8_lossy(&bytes).into_owned(), Some(bytes))
        }
    }
}

/// How `run_with` treats the child's output while it runs.
#[derive(Debug, Default, Clone)]
pub struct RunOpts {
    /// Tee every chunk to cartoon's own stdout/stderr the moment it arrives
    /// (`--raw`): live, byte-identical, in arrival order.
    pub stream: bool,
    /// Print `cartoon: still running (...)` to stderr at this interval.
    pub heartbeat: Option<Duration>,
}

impl RunOpts {
    /// Non-streaming run with the heartbeat from `CARTOON_HEARTBEAT`
    /// (seconds; `0` turns it off; default 60).
    pub fn buffered() -> Self {
        RunOpts {
            stream: false,
            heartbeat: heartbeat_from_env(),
        }
    }
}

pub fn heartbeat_from_env() -> Option<Duration> {
    parse_heartbeat(std::env::var("CARTOON_HEARTBEAT").ok().as_deref())
}

fn parse_heartbeat(v: Option<&str>) -> Option<Duration> {
    let secs = v.and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(60);
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// Spawn argv[0] with argv[1..], capture both streams, wait for exit.
pub fn run(argv: &[String]) -> Result<Captured> {
    run_with(argv, &RunOpts::default()).map(|o| o.captured)
}

const CHUNK: usize = 64 * 1024;

/// Spawn and capture with ordering, optional live tee, heartbeat and signal
/// forwarding (unix: the child runs in its own process group and
/// SIGINT/SIGTERM/SIGHUP sent to cartoon are forwarded to that group; cartoon
/// keeps reading until the child exits so partial output is never lost).
pub fn run_with(argv: &[String], opts: &RunOpts) -> Result<RunOutput> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    let own_group = signals::prepare(&mut cmd);
    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to run {}", argv[0]))?;
    #[cfg(unix)]
    let _forwarding = signals::Forwarding::start(child.id() as i32, own_group);

    let (tx, rx) = mpsc::channel::<(Stream, Vec<u8>)>();
    // Set when cartoon's own stdout is gone (`--raw … | head`): the stdout
    // reader then drops its pipe so the child gets EPIPE/SIGPIPE, exactly as
    // it would without cartoon in the middle.
    let stop_stdout = Arc::new(AtomicBool::new(false));
    let readers = [
        spawn_reader(
            Box::new(child.stdout.take().expect("stdout piped")),
            Stream::Stdout,
            tx.clone(),
            Some(stop_stdout.clone()),
        ),
        spawn_reader(
            Box::new(child.stderr.take().expect("stderr piped")),
            Stream::Stderr,
            tx,
            None,
        ),
    ];

    let started = Instant::now();
    let mut last_beat = started;
    let mut out_buf = Vec::new();
    let mut err_buf = Vec::new();
    let mut order: Vec<(Stream, usize)> = Vec::new();
    let mut out_ok = true;
    let mut err_ok = true;
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok((s, chunk)) => {
                if opts.stream {
                    match s {
                        Stream::Stdout if out_ok => {
                            let mut o = std::io::stdout().lock();
                            if o.write_all(&chunk).and_then(|_| o.flush()).is_err() {
                                out_ok = false;
                                stop_stdout.store(true, Ordering::Relaxed);
                            }
                        }
                        Stream::Stderr if err_ok => {
                            let mut e = std::io::stderr().lock();
                            err_ok = e.write_all(&chunk).and_then(|_| e.flush()).is_ok();
                        }
                        _ => {}
                    }
                }
                match order.last_mut() {
                    Some((last, n)) if *last == s => *n += chunk.len(),
                    _ => order.push((s, chunk.len())),
                }
                match s {
                    Stream::Stdout => out_buf.extend_from_slice(&chunk),
                    Stream::Stderr => err_buf.extend_from_slice(&chunk),
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if let Some(every) = opts.heartbeat {
            if !opts.stream && last_beat.elapsed() >= every {
                last_beat = Instant::now();
                let mb = (out_buf.len() + err_buf.len()) as f64 / (1024.0 * 1024.0);
                // Best effort: a closed stderr must not stop the capture.
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "cartoon: still running ({}s, {mb:.1} MB captured)",
                    started.elapsed().as_secs()
                );
            }
        }
    }
    for r in readers {
        let _ = r.join();
    }
    let status = child.wait()?;
    #[cfg(unix)]
    let interrupted = _forwarding.finish();
    #[cfg(not(unix))]
    let interrupted = None;

    let (stdout, stdout_raw) = split_utf8(out_buf);
    let (stderr, stderr_raw) = split_utf8(err_buf);
    Ok(RunOutput {
        captured: Captured {
            stdout,
            stderr,
            status,
        },
        stdout_raw,
        stderr_raw,
        order,
        interrupted,
    })
}

fn spawn_reader(
    mut pipe: Box<dyn Read + Send>,
    stream: Stream,
    tx: mpsc::Sender<(Stream, Vec<u8>)>,
    stop: Option<Arc<AtomicBool>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = vec![0u8; CHUNK];
        loop {
            if stop.as_ref().is_some_and(|s| s.load(Ordering::Relaxed)) {
                return; // drops the pipe: the child sees EPIPE
            }
            match pipe.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => {
                    if tx.send((stream, buf[..n].to_vec())).is_err() {
                        return;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                // A read error means the child's pipe closed unexpectedly.
                Err(_) => return,
            }
        }
    })
}

/// Child exit code; signal death maps to conventional 128+N (unix).
pub fn exit_code(status: &ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    1
}

/// `SIGTERM` style name for a signal number (for the `interrupted` flag).
pub fn signal_name(sig: i32) -> String {
    #[cfg(unix)]
    {
        let name = match sig {
            libc::SIGINT => Some("SIGINT"),
            libc::SIGTERM => Some("SIGTERM"),
            libc::SIGHUP => Some("SIGHUP"),
            _ => None,
        };
        if let Some(n) = name {
            return n.to_string();
        }
    }
    format!("signal {sig}")
}

#[cfg(unix)]
mod signals {
    //! Forward SIGINT/SIGTERM/SIGHUP to the child instead of dying with it:
    //! cartoon must outlive the child to emit (and archive) what it printed.
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    const SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

    /// Child pid (0: no child running). Read by the signal handler.
    static CHILD: AtomicI32 = AtomicI32::new(0);
    /// The child leads its own process group (forward to `-pid`).
    static GROUP: AtomicBool = AtomicBool::new(false);
    /// Last signal received while a child ran (0: none).
    static RECEIVED: AtomicI32 = AtomicI32::new(0);

    extern "C" fn on_signal(sig: libc::c_int, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
        RECEIVED.store(sig, Ordering::SeqCst);
        let pid = CHILD.load(Ordering::SeqCst);
        if pid <= 0 {
            return;
        }
        if GROUP.load(Ordering::SeqCst) {
            // SAFETY: kill(2) is async-signal-safe.
            unsafe { libc::kill(-pid, sig) };
            return;
        }
        // Same process group as cartoon (interactive terminal): a
        // terminal-generated signal (si_code > 0, SI_KERNEL) already reached
        // the child; forward only what was sent to cartoon alone (kill(1)).
        // SAFETY: the kernel passes a valid siginfo with SA_SIGINFO.
        let user_sent = info.is_null() || unsafe { (*info).si_code } <= 0;
        if user_sent {
            // SAFETY: kill(2) is async-signal-safe.
            unsafe { libc::kill(pid, sig) };
        }
    }

    /// Put the child in its own process group so a forwarded signal reaches
    /// its whole tree (`sh -c '…; sleep 5'` must not orphan `sleep`) — but
    /// only when stdin is not a terminal: a background process group that
    /// reads the terminal is stopped by SIGTTIN, so interactive runs keep
    /// the terminal's group (Ctrl-C reaches the child directly then).
    pub fn prepare(cmd: &mut Command) -> bool {
        // SAFETY: isatty has no preconditions.
        let tty = unsafe { libc::isatty(0) } == 1;
        if !tty {
            cmd.process_group(0);
        }
        !tty
    }

    /// Handlers installed for the lifetime of one child; the previous
    /// dispositions come back on `finish` (or drop).
    pub struct Forwarding {
        old: Vec<(libc::c_int, libc::sigaction)>,
        done: bool,
    }

    impl Forwarding {
        pub fn start(pid: i32, group: bool) -> Self {
            GROUP.store(group, Ordering::SeqCst);
            RECEIVED.store(0, Ordering::SeqCst);
            CHILD.store(pid, Ordering::SeqCst);
            let mut old = Vec::new();
            for sig in SIGNALS {
                // SAFETY: a zeroed sigaction is a valid starting point; the
                // handler only touches atomics and calls kill(2).
                unsafe {
                    let mut sa: libc::sigaction = std::mem::zeroed();
                    sa.sa_sigaction = on_signal as *const () as usize;
                    sa.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
                    libc::sigemptyset(&mut sa.sa_mask);
                    let mut prev: libc::sigaction = std::mem::zeroed();
                    if libc::sigaction(sig, &sa, &mut prev) == 0 {
                        old.push((sig, prev));
                    }
                }
            }
            Forwarding { old, done: false }
        }

        /// Restore the previous handlers; returns the signal received while
        /// the child ran, if any.
        pub fn finish(mut self) -> Option<i32> {
            self.restore();
            match RECEIVED.swap(0, Ordering::SeqCst) {
                0 => None,
                s => Some(s),
            }
        }

        fn restore(&mut self) {
            if self.done {
                return;
            }
            self.done = true;
            CHILD.store(0, Ordering::SeqCst);
            for (sig, prev) in &self.old {
                // SAFETY: restoring a disposition sigaction itself returned.
                unsafe { libc::sigaction(*sig, prev, std::ptr::null_mut()) };
            }
        }
    }

    impl Drop for Forwarding {
        fn drop(&mut self) {
            self.restore();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Captured {
        run(&["sh".to_string(), "-c".to_string(), script.to_string()]).unwrap()
    }

    fn sh_out(script: &str) -> RunOutput {
        run_with(
            &["sh".to_string(), "-c".to_string(), script.to_string()],
            &RunOpts::default(),
        )
        .unwrap()
    }

    #[test]
    fn captures_stdout_and_stderr_separately() {
        let c = sh("echo out; echo err >&2");
        assert_eq!(c.stdout, "out\n");
        assert_eq!(c.stderr, "err\n");
    }

    #[test]
    fn mirrors_exit_code() {
        let c = sh("exit 3");
        assert_eq!(exit_code(&c.status), 3);
    }

    #[test]
    fn missing_command_is_not_found_error() {
        let err = run(&["definitely-not-a-real-binary-xyz".to_string()]).unwrap_err();
        let io = err.downcast_ref::<std::io::Error>().unwrap();
        assert_eq!(io.kind(), std::io::ErrorKind::NotFound);
    }

    #[cfg(unix)]
    #[test]
    fn signal_death_maps_to_128_plus_n() {
        let c = sh("kill -TERM $$");
        assert_eq!(exit_code(&c.status), 143);
    }

    #[test]
    fn non_utf8_bytes_are_kept_exactly() {
        let o = sh_out(r"printf 'a\377b'; printf 'c\376' >&2");
        assert_eq!(o.stdout_bytes(), b"a\xffb");
        assert_eq!(o.stderr_bytes(), b"c\xfe");
        assert!(o.has_raw_bytes());
        // The text view is lossy, for transforms only.
        assert_eq!(o.captured.stdout, "a\u{fffd}b");
    }

    #[test]
    fn utf8_output_holds_no_second_copy() {
        let o = sh_out("echo hi");
        assert!(!o.has_raw_bytes());
        assert_eq!(o.stdout_bytes(), b"hi\n");
    }

    #[test]
    fn replay_preserves_arrival_order() {
        // Sleeps force separate chunks so the order is deterministic.
        let o = sh_out("echo out1; sleep 0.1; echo ERR1 >&2; sleep 0.1; echo out2");
        let mut merged = Vec::new();
        {
            struct Tagged<'a>(&'a std::cell::RefCell<Vec<u8>>, &'static str);
            impl Write for Tagged<'_> {
                fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                    if !b.is_empty() {
                        let mut v = self.0.borrow_mut();
                        v.extend_from_slice(self.1.as_bytes());
                        v.extend_from_slice(b);
                    }
                    Ok(b.len())
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    Ok(())
                }
            }
            let cell = std::cell::RefCell::new(Vec::new());
            o.replay(&mut Tagged(&cell, "O:"), &mut Tagged(&cell, "E:"))
                .unwrap();
            merged.extend(cell.into_inner());
        }
        assert_eq!(
            String::from_utf8(merged).unwrap(),
            "O:out1\nE:ERR1\nO:out2\n"
        );
    }

    #[test]
    fn merged_text_follows_arrival_order() {
        let o = sh_out("echo out1; sleep 0.1; echo ERR1 >&2; sleep 0.1; echo out2");
        assert_eq!(o.merged_text(), "out1\nERR1\nout2\n");
        // Synthetic captures (no order log) are stdout then stderr.
        let s = RunOutput::from_bytes(b"only\n".to_vec());
        assert_eq!(s.merged_text(), "only\n");
        let lossy = sh_out(r"printf 'a\377\n'; sleep 0.1; printf 'b\n' >&2");
        assert_eq!(lossy.merged_text(), "a\u{fffd}\nb\n");
    }

    #[test]
    fn ingest_bytes_accept_non_utf8() {
        let o = RunOutput::from_bytes(b"caf\xe9\n".to_vec());
        assert_eq!(o.stdout_bytes(), b"caf\xe9\n");
        assert_eq!(o.captured.stdout, "caf\u{fffd}\n");
    }

    #[test]
    fn heartbeat_interval_parsing() {
        assert_eq!(parse_heartbeat(None), Some(Duration::from_secs(60)));
        assert_eq!(parse_heartbeat(Some("0")), None);
        assert_eq!(parse_heartbeat(Some("5")), Some(Duration::from_secs(5)));
        assert_eq!(parse_heartbeat(Some("junk")), Some(Duration::from_secs(60)));
    }
}
