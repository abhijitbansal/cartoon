//! `cartoon mcp` — a Model Context Protocol server over stdio, for agents
//! that have no PreToolUse hook to rewrite their shell calls (Cursor, Codex,
//! Windsurf, Claude Desktop). Instead of hoping the agent prefixes
//! `cartoon`, the agent gets a `run` tool that IS cartoon.
//!
//! Transport: newline-delimited JSON-RPC 2.0 on stdin/stdout (the MCP stdio
//! transport). stdout carries protocol messages and nothing else — every
//! diagnostic goes to stderr, and the commands `run` executes are spawned
//! with their own pipes, so their output can never leak onto the channel.
//!
//! Requests are read on the main thread; each `tools/call` runs on its own
//! thread so a long test run doesn't block `ping` or a second call, and
//! `notifications/cancelled` can stop it. Responses are written whole, one
//! line each, under a lock.
mod protocol;
mod run;
mod tools;

use anyhow::{bail, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use protocol::{negotiate_version, SUPPORTED_VERSIONS};

const USAGE: &str = "usage: cartoon mcp   (MCP server on stdin/stdout; see docs/agents.md)";

/// Entry point for `cartoon mcp`. Returns the process exit code.
pub fn run(args: &[String]) -> Result<i32> {
    if !args.is_empty() {
        bail!(USAGE);
    }
    let exe = std::env::current_exe()?;
    let stdin = std::io::stdin();
    serve(stdin.lock(), Box::new(std::io::stdout()), exe)?;
    Ok(0)
}

type Out = Arc<Mutex<Box<dyn Write + Send>>>;

/// Write one JSON-RPC message as a single line. A failed write means the
/// client is gone; the read loop ends on EOF right after.
fn send(out: &Out, msg: &Value) {
    let mut w = out.lock().unwrap_or_else(|p| p.into_inner());
    let line = msg.to_string(); // serde_json escapes newlines: one line
    if writeln!(w, "{line}").and_then(|_| w.flush()).is_err() {
        eprintln!("cartoon mcp: client closed stdout");
    }
}

/// In-flight `tools/call` requests, by JSON-RPC id (serialized), so a
/// `notifications/cancelled` can flag the right one.
type InFlight = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

/// Serve until `input` reaches EOF. `exe` is the cartoon binary `run`
/// re-invokes, so every wrapped command goes through the real pipeline.
pub fn serve<R: BufRead>(input: R, out: Box<dyn Write + Send>, exe: PathBuf) -> Result<()> {
    let out: Out = Arc::new(Mutex::new(out));
    let inflight: InFlight = Arc::default();
    let mut workers = Vec::new();
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match protocol::route(&line) {
            protocol::Route::Reply(msg) => send(&out, &msg),
            protocol::Route::Silent => {}
            protocol::Route::Cancel(id) => {
                if let Some(flag) = inflight.lock().unwrap().get(&id.to_string()) {
                    flag.store(true, Ordering::SeqCst);
                }
            }
            protocol::Route::Call { id, name, args } => {
                let cancel = Arc::new(AtomicBool::new(false));
                let key = id.to_string();
                inflight.lock().unwrap().insert(key.clone(), cancel.clone());
                let (out, inflight, exe) = (out.clone(), inflight.clone(), exe.clone());
                workers.push(std::thread::spawn(move || {
                    let reply = tools::call(&name, &args, &exe, &cancel);
                    inflight.lock().unwrap().remove(&key);
                    // A cancelled request gets no response (MCP cancellation).
                    if !cancel.load(Ordering::SeqCst) {
                        send(&out, &protocol::respond(&id, reply));
                    }
                }));
                workers.retain(|w| !w.is_finished());
            }
        }
    }
    // Client closed stdin: stop whatever is still running, then exit.
    for flag in inflight.lock().unwrap().values() {
        flag.store(true, Ordering::SeqCst);
    }
    for w in workers {
        let _ = w.join();
    }
    Ok(())
}
