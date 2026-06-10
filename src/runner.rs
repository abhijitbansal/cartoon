use anyhow::{Context, Result};
use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};

#[derive(Debug)]
pub struct Captured {
    pub stdout: String,
    pub stderr: String,
    pub status: ExitStatus,
}

/// Spawn argv[0] with argv[1..], capture both streams, wait for exit.
/// Non-UTF8 output is converted lossily (documented v1 limitation).
pub fn run(argv: &[String]) -> Result<Captured> {
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run {}", argv[0]))?;

    let mut err_pipe = child.stderr.take().expect("stderr piped");
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_pipe.read_to_end(&mut buf);
        buf
    });

    let mut out_buf = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout piped")
        .read_to_end(&mut out_buf)?;

    let status = child.wait()?;
    let err_buf = err_thread.join().expect("stderr reader panicked");

    Ok(Captured {
        stdout: String::from_utf8_lossy(&out_buf).into_owned(),
        stderr: String::from_utf8_lossy(&err_buf).into_owned(),
        status,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Captured {
        run(&["sh".to_string(), "-c".to_string(), script.to_string()]).unwrap()
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
}
