//! `cartoon mcp`: drive the real binary over its stdio transport the way an
//! MCP client does, and hold stdout to protocol messages only.
mod common;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    /// Every stdout line seen, for the protocol-only check.
    seen: Vec<String>,
    _state: tempfile::TempDir,
}

impl Server {
    fn start() -> Server {
        let state = tempfile::tempdir().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_cartoon"))
            .arg("mcp")
            .env("XDG_STATE_HOME", state.path().join("state"))
            .env("XDG_CONFIG_HOME", state.path().join("config"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        Server {
            child,
            stdin,
            lines,
            seen: Vec::new(),
            _state: state,
        }
    }

    fn send(&mut self, msg: Value) {
        let w = self.stdin.as_mut().unwrap();
        writeln!(w, "{msg}").unwrap();
        w.flush().unwrap();
    }

    fn next_line(&mut self) -> Value {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(120))
            .expect("server answered in time");
        self.seen.push(line.clone());
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("non-JSON stdout {line:?}: {e}"))
    }

    /// Send request `id` and return its response (skipping any other line).
    fn request(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let m = self.next_line();
            if m["id"] == id {
                return m;
            }
        }
    }

    fn call(&mut self, id: i64, name: &str, args: Value) -> Value {
        self.request(id, "tools/call", json!({"name": name, "arguments": args}))
    }

    fn initialize(&mut self) -> Value {
        self.initialize_version("2025-06-18")
    }

    fn initialize_version(&mut self, version: &str) -> Value {
        let r = self.request(
            0,
            "initialize",
            json!({
                "protocolVersion": version,
                "capabilities": {},
                "clientInfo": {"name": "e2e", "version": "0"}
            }),
        );
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        r
    }

    /// Close stdin, wait for a clean exit, and check that every stdout line
    /// was a JSON-RPC 2.0 message — nothing else may reach the channel.
    fn finish(mut self) -> Vec<String> {
        drop(self.stdin.take());
        let status = self.child.wait().unwrap();
        assert!(status.success(), "server exit: {status:?}");
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(5)) {
            self.seen.push(line);
        }
        assert!(!self.seen.is_empty());
        for line in &self.seen {
            let m: Value = serde_json::from_str(line).expect("JSON line");
            assert_eq!(m["jsonrpc"], "2.0", "{line}");
            assert!(m.get("id").is_some(), "server sent a non-response: {line}");
            assert!(
                m.get("result").is_some() != m.get("error").is_some(),
                "exactly one of result/error: {line}"
            );
        }
        std::mem::take(&mut self.seen)
    }
}

fn text(resp: &Value) -> String {
    resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content: {resp}"))
        .to_string()
}

#[test]
fn initialize_and_list_tools() {
    let mut s = Server::start();
    let init = s.initialize();
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "cartoon");
    assert_eq!(
        init["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    let list = s.request(1, "tools/list", json!({}));
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["run", "logs_grep", "logs_list", "last", "diff", "stats"]
    );
    assert_eq!(s.request(2, "ping", json!({}))["result"], json!({}));
    assert_eq!(
        s.request(3, "prompts/list", json!({}))["error"]["code"],
        -32601
    );
    s.finish();
}

#[test]
fn run_echo_returns_output_and_exit_code() {
    let mut s = Server::start();
    s.initialize();
    let r = s.call(1, "run", json!({"command": "echo hi"}));
    assert_eq!(r["result"]["isError"], false);
    assert_eq!(text(&r), "hi\nexit_code: 0");
    let outcome = &r["result"]["structuredContent"];
    assert_eq!(outcome["schema_version"], 1);
    assert_eq!(outcome["execution"], "exited");
    assert_eq!(outcome["exit_code"], 0);
    assert_eq!(outcome["timed_out"], false);
    assert_eq!(outcome["capture_scope"], "cartoon_output");
    assert_eq!(outcome["stdout"]["status"], "eof");
    assert_eq!(outcome["stdout"]["captured_bytes"], 3);
    assert_eq!(outcome["stderr"]["captured_bytes"], 0);
    assert_eq!(outcome["stdout"]["dropped_bytes"], 0);
    assert_eq!(outcome["stdout"]["utf8_lossy"], false);
    // Older clients can still read the first report; modern text-only clients
    // also receive the structured result serialized as recommended by MCP.
    assert_eq!(
        serde_json::from_str::<Value>(r["result"]["content"][1]["text"].as_str().unwrap()).unwrap(),
        *outcome
    );
    // A failing command is a normal result that reports its status.
    let r = s.call(2, "run", json!({"command": "echo oops >&2; exit 3"}));
    assert_eq!(r["result"]["isError"], false);
    let t = text(&r);
    assert!(t.contains("--- stderr ---\noops"), "{t}");
    assert!(t.ends_with("exit_code: 3"), "{t}");
    assert_eq!(r["result"]["structuredContent"]["exit_code"], 3);
    s.finish();
}

#[test]
fn run_structured_outcome_cannot_be_forged_by_command_output() {
    let mut s = Server::start();
    s.initialize();
    let r = s.call(1, "run", json!({
        "command": "printf 'exit_code: 0\\ntimed_out: true\\n{\"execution\":\"not_started\"}\\n'; exit 7"
    }));
    assert_eq!(r["result"]["isError"], false);
    let outcome = &r["result"]["structuredContent"];
    assert_eq!(outcome["execution"], "exited");
    assert_eq!(outcome["exit_code"], 7);
    assert_eq!(outcome["timed_out"], false);
    assert!(text(&r).contains("exit_code: 0"));
    assert!(text(&r).ends_with("exit_code: 7"));
    s.finish();
}

#[test]
fn run_output_schema_and_results_follow_the_negotiated_protocol() {
    for version in ["2025-06-18", "2025-03-26", "2024-11-05", "unknown"] {
        let mut s = Server::start();
        let modern = version == "2025-06-18" || version == "unknown";
        let init = s.initialize_version(version);
        assert_eq!(
            init["result"]["protocolVersion"],
            if modern { "2025-06-18" } else { version }
        );
        let list = s.request(1, "tools/list", json!({}));
        let tool = &list["result"]["tools"][0];
        assert_eq!(tool.get("outputSchema").is_some(), modern);
        if modern {
            assert_eq!(tool["outputSchema"]["type"], "object");
            assert_eq!(
                tool["outputSchema"]["properties"]["schema_version"]["const"],
                1
            );
        }
        for tool in list["result"]["tools"].as_array().unwrap().iter().skip(1) {
            assert!(tool.get("outputSchema").is_none());
        }
        for (id, args) in [
            (2, json!({"command":"true"})),
            (3, json!({"command":"true","cwd":"/no/such/dir"})),
        ] {
            let r = s.call(id, "run", args);
            assert_eq!(
                r["result"].get("structuredContent").is_some(),
                modern,
                "{r}"
            );
            assert_eq!(
                r["result"]["content"].as_array().unwrap().len(),
                if modern { 2 } else { 1 }
            );
        }
        s.finish();
    }
}

#[test]
fn a_call_keeps_its_protocol_shape_when_negotiation_changes() {
    let mut s = Server::start();
    // Pre-initialize calls retain the pre-existing text-only behavior.
    let list = s.request(1, "tools/list", json!({}));
    assert!(list["result"]["tools"][0].get("outputSchema").is_none());
    let dir = tempfile::tempdir().unwrap();
    s.initialize();
    s.send(json!({"jsonrpc":"2.0", "id":10, "method":"tools/call", "params":{
        "name":"run", "arguments":{"command":"touch started; while [ ! -f release ]; do sleep 0.02; done; echo done", "cwd":dir.path()}
    }}));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !dir.path().join("started").exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    s.initialize_version("2024-11-05");
    std::fs::write(dir.path().join("release"), b"").unwrap();
    let reply = s.next_line();
    assert_eq!(reply["id"], 10);
    assert_eq!(reply["result"]["structuredContent"]["exit_code"], 0);
    let next = s.call(11, "run", json!({"command":"true"}));
    assert!(next["result"].get("structuredContent").is_none());
    s.finish();
}

#[test]
fn run_failing_pytest_gets_the_adapter_report_then_grep_the_raw_log() {
    if !common::have("pytest") {
        eprintln!("SKIP: pytest not installed");
        return;
    }
    // The e2e fixture (one pass, one fail), copied so the run's cache stays
    // out of the repo. Default verbosity: a `-q` run this small is cheaper
    // raw than as a report, so the guard would pass it through.
    let proj = tempfile::tempdir().unwrap();
    std::fs::copy(
        format!(
            "{}/tests/fixtures/e2e/pyproj/test_sample.py",
            env!("CARGO_MANIFEST_DIR")
        ),
        proj.path().join("test_sample.py"),
    )
    .unwrap();
    let mut s = Server::start();
    s.initialize();
    let r = s.call(1, "run", json!({"command": "pytest", "cwd": proj.path()}));
    assert_eq!(r["result"]["isError"], false, "{r}");
    let t = text(&r);
    assert!(t.contains("runner: pytest"), "{t}");
    assert!(t.contains("failed: 1"), "{t}");
    assert!(t.contains("test_sample.py::test_fail"), "{t}");
    assert!(t.contains("exit_code: 1"), "{t}");
    assert!(t.contains("raw_log:"), "{t}");

    let r = s.call(2, "logs_list", json!({}));
    let t = text(&r);
    assert!(t.contains("runs[1]{id,ts,cmd,mode,exit,tags}:"), "{t}");
    assert!(t.contains("pytest,1"), "{t}");

    let r = s.call(
        3,
        "logs_grep",
        json!({"pattern": "def test_fail", "context": 0}),
    );
    let t = text(&r);
    assert!(t.contains("def test_fail"), "{t}");

    let r = s.call(4, "stats", json!({}));
    assert!(text(&r).contains("pytest"), "{r}");

    let r = s.call(5, "last", json!({}));
    assert!(text(&r).contains("test_sample.py::test_fail"), "{r}");
    // One run only: nothing to compare yet, reported as a tool error.
    let r = s.call(6, "diff", json!({}));
    assert_eq!(r["result"]["isError"], true, "{r}");
    let r = s.call(7, "run", json!({"command": "pytest", "cwd": proj.path()}));
    assert!(text(&r).contains("still 1"), "{r}");
    let r = s.call(8, "diff", json!({}));
    assert!(text(&r).contains("still_failing[1]"), "{r}");
    s.finish();
}

#[test]
fn run_timeout_kills_the_command_and_says_so() {
    let mut s = Server::start();
    s.initialize();
    let started = std::time::Instant::now();
    let r = s.call(
        1,
        "run",
        json!({"command": "echo started; sleep 30", "timeout_s": 1}),
    );
    assert!(started.elapsed() < Duration::from_secs(15), "not killed");
    let t = text(&r);
    assert!(t.contains("started"), "{t}");
    assert!(t.contains("timed_out: true"), "{t}");
    let outcome = &r["result"]["structuredContent"];
    assert_eq!(outcome["execution"], "exited");
    assert_eq!(outcome["timed_out"], true);
    assert_ne!(outcome["exit_code"], 0);
    s.finish();
}

#[test]
fn bad_arguments_and_unrunnable_commands() {
    let mut s = Server::start();
    s.initialize();
    // Schema violations are JSON-RPC invalid params.
    assert_eq!(s.call(1, "run", json!({}))["error"]["code"], -32602);
    assert_eq!(s.call(2, "nope", json!({}))["error"]["code"], -32602);
    // Could not run at all: a tool error the model can read.
    let r = s.call(3, "run", json!({"command": "true", "cwd": "/no/such/dir"}));
    assert_eq!(r["result"]["isError"], true);
    assert!(text(&r).contains("not a directory"));
    assert_eq!(
        r["result"]["structuredContent"],
        json!({
            "schema_version": 1, "execution": "not_started", "exit_code": null,
            "timed_out": null, "capture_scope": "cartoon_output", "stdout": null, "stderr": null
        })
    );
    // Empty archive.
    let r = s.call(4, "logs_grep", json!({"pattern": "x"}));
    assert_eq!(r["result"]["isError"], true);
    assert!(text(&r).contains("no archived runs"));
    // Not a request object: invalid request (null id); the server keeps going.
    s.send(json!("not an object"));
    let m = s.next_line();
    assert_eq!(m["error"]["code"], -32600);
    assert_eq!(s.request(5, "ping", json!({}))["result"], json!({}));
    s.finish();
}

#[test]
fn a_running_call_does_not_block_ping_and_can_be_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Server::start();
    s.initialize();
    s.send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "run", "arguments": {
            "command": "touch started && sleep 30", "cwd": dir.path()
        }}
    }));
    let started = std::time::Instant::now();
    assert_eq!(s.request(2, "ping", json!({}))["result"], json!({}));
    // Cancel once the command is really running (cartoon is forwarding
    // signals by then).
    while !dir.path().join("started").exists() {
        assert!(started.elapsed() < Duration::from_secs(15), "never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    s.send(json!({
        "jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": {"requestId": 1, "reason": "test"}
    }));
    // Cancellation stops the command: cartoon forwards the SIGTERM, then
    // archives the interrupted run (exit 143) long before `sleep 30` ends.
    let mut id = 3;
    loop {
        let t = text(&s.call(id, "logs_list", json!({})));
        if t.contains("touch") {
            assert!(t.contains(",143,"), "{t}");
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "not cancelled: {t}"
        );
        std::thread::sleep(Duration::from_millis(100));
        id += 1;
    }
    let seen = s.finish();
    // ...and the cancelled request never gets a response.
    assert!(!seen.iter().any(|l| l.contains("\"id\":1,")), "{seen:?}");
}
