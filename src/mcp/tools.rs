//! The MCP tools: `run` (a wrapped command), `logs_grep` / `logs_list` (the
//! raw-log archive) and `stats` (the savings ledger). Argument errors are
//! JSON-RPC `invalid params`; a tool that ran but failed (no archived runs,
//! a bad regex, a command that could not start) is a result with
//! `isError: true`, so the model sees the reason and can correct itself.
use super::protocol::RpcError;
use super::run::{self, RunArgs};
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// `run` default and ceiling. Long enough for a real test suite; the
/// ceiling stops a typo (`timeout_s: 360000`) from pinning a worker forever.
const DEFAULT_TIMEOUT_S: f64 = 600.0;
const MAX_TIMEOUT_S: f64 = 3600.0;
/// `logs_grep` context ceiling: grep exists to read LESS than the log.
const MAX_CONTEXT: u64 = 20;
const DEFAULT_LIST_LIMIT: u64 = 20;

const RUN_DESCRIPTION: &str = "Run a shell command through cartoon and return a compact, \
token-efficient report instead of its raw output: test runners (pytest, jest, vitest, \
go test, cargo test, …), builds, linters and type checkers get failure-focused \
summaries; JSON output becomes TOON; anything else gets safe noise removal. Use it \
instead of a plain shell for any command whose output may be long. `command` is a \
shell string, exactly like `cartoon -c '<command>'`: pipes and operators work, and \
`<test cmd> | tail -n 20` style filters are unnecessary (dropped and disclosed). The \
complete raw output is archived — search it with logs_grep rather than re-running. \
The result ends with `exit_code: N` (the command's exit status; a failing test run is \
a normal result, not a tool error). SECURITY: this executes arbitrary commands with \
your user's privileges in `cwd`; it is gated only by the MCP client's tool approval.";

/// Tool definitions for `tools/list`, in a stable order.
pub fn definitions(structured: bool) -> Value {
    let mut tools = json!([
        {
            "name": "run",
            "title": "Run a command (compact output)",
            "description": RUN_DESCRIPTION,
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command string, e.g. \"pytest -q tests/\" or \"npm test\"."
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory (absolute, or relative to the server's). Default: the server's working directory."
                    },
                    "timeout_s": {
                        "type": "number",
                        "exclusiveMinimum": 0,
                        "maximum": MAX_TIMEOUT_S,
                        "description": "Kill the command after this many seconds (default 600)."
                    },
                    "compress": {
                        "type": "string",
                        "enum": ["safe", "aggressive"],
                        "description": "Compression for output no adapter recognizes: safe (default, non-lossy in practice) or aggressive (lossy; raw log is the escape hatch)."
                    },
                    "max_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Hard ceiling on returned tokens: head and tail kept, middle replaced by a disclosed marker."
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            },
            "annotations": {
                "title": "Run a command (compact output)",
                "readOnlyHint": false,
                "destructiveHint": true,
                "idempotentHint": false,
                "openWorldHint": false
            }
        },
        {
            "name": "logs_grep",
            "title": "Search a run's raw output",
            "description": "Search the complete archived raw stdout/stderr of a previous `run` with a regex, returning matching lines with line numbers and context (like `cartoon logs grep`). Use this instead of re-running a command to see what the compact report left out. Matches are capped at 50.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regular expression (Rust regex syntax)." },
                    "run_id": { "type": "string", "description": "Archived run id (from logs_list or a raw_log footer). Default: the most recent run." },
                    "context": { "type": "integer", "minimum": 0, "maximum": MAX_CONTEXT, "description": "Lines of context around each match (default 2)." }
                },
                "required": ["pattern"],
                "additionalProperties": false
            },
            "annotations": {
                "title": "Search a run's raw output",
                "readOnlyHint": true,
                "openWorldHint": false
            }
        },
        {
            "name": "logs_list",
            "title": "List recent runs",
            "description": "List recent archived runs, newest first: id, time, command, adapter (mode), exit code and tags (like `cartoon logs`). Use an id with logs_grep.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tag": { "type": "string", "description": "Only runs carrying this tag." },
                    "limit": { "type": "integer", "minimum": 1, "description": "Most runs to list (default 20)." }
                },
                "additionalProperties": false
            },
            "annotations": {
                "title": "List recent runs",
                "readOnlyHint": true,
                "openWorldHint": false
            }
        },
        {
            "name": "last",
            "title": "Re-read the last report",
            "description": "Re-show the newest archived run's report without re-running it (like `cartoon last`). Optional `cmd` picks the newest run whose command contains that substring.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cmd": { "type": "string", "description": "Only runs whose command contains this substring." }
                },
                "additionalProperties": false
            },
            "annotations": {
                "title": "Re-read the last report",
                "readOnlyHint": true,
                "openWorldHint": false
            }
        },
        {
            "name": "diff",
            "title": "What changed since the previous run",
            "description": "Compare the newest run with the previous run of the same command in the same directory (like `cartoon diff`): fixed, still failing and new failures. Pass both `id_a` and `id_b` to compare two specific runs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cmd": { "type": "string", "description": "Only runs whose command contains this substring." },
                    "id_a": { "type": "string", "description": "Older run id (with id_b)." },
                    "id_b": { "type": "string", "description": "Newer run id (with id_a)." }
                },
                "additionalProperties": false
            },
            "annotations": {
                "title": "What changed since the previous run",
                "readOnlyHint": true,
                "openWorldHint": false
            }
        },
        {
            "name": "stats",
            "title": "Token savings",
            "description": "Tokens saved by cartoon-wrapped runs, per adapter, from the local ledger (like `cartoon stats`).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "since": { "type": "string", "description": "Only runs within this window, e.g. \"7d\", \"24h\", \"30m\"." }
                },
                "additionalProperties": false
            },
            "annotations": {
                "title": "Token savings",
                "readOnlyHint": true,
                "openWorldHint": false
            }
        }
    ]);
    if structured {
        tools[0]["outputSchema"] = run_output_schema();
    }
    tools
}

fn run_output_schema() -> Value {
    let capture = json!({
        "type": ["object", "null"],
        "description": "Snapshot of cartoon's transformed stream, not original command output. EOF may still have dropped bytes or lossy UTF-8. Counts exclude rendering markers; at deadline/read_error dropped_bytes is only the observed lower bound.",
        "properties": {
            "status": {"type": "string", "enum": ["eof", "read_error", "deadline"]},
            "captured_bytes": {"type": "integer", "minimum": 0},
            "dropped_bytes": {"type": "integer", "minimum": 0},
            "utf8_lossy": {"type": "boolean"}
        },
        "required": ["status", "captured_bytes", "dropped_bytes", "utf8_lossy"],
        "additionalProperties": false
    });
    json!({
        "type": "object",
        "properties": {
            "schema_version": {"type": "integer", "const": 1},
            "execution": {
                "type": "string", "enum": ["exited", "not_started", "unknown"],
                "description": "exited: observed cartoon subprocess status; not_started: could not spawn it; unknown: observation failed after spawn. None proves task correctness or descendant cleanup."
            },
            "exit_code": {"type": ["integer", "null"], "description": "Observed cartoon subprocess exit code (128 + signal on Unix), null when unavailable."},
            "timed_out": {"type": ["boolean", "null"], "description": "Whether the server's execution timeout fired; null when unavailable. Separate from stream capture deadlines."},
            "capture_scope": {"type": "string", "const": "cartoon_output"},
            "stdout": capture,
            "stderr": capture
        },
        "required": ["schema_version", "execution", "exit_code", "timed_out", "capture_scope", "stdout", "stderr"],
        "additionalProperties": false
    })
}

fn run_result(
    result: Result<run::RunResult, run::RunError>,
    a: &RunArgs,
    structured: bool,
) -> Value {
    let (mut response, outcome) = match result {
        Ok(r) => (
            text_result(r.render(a), false),
            json!({
                "schema_version": 1,
                "execution": "exited",
                "exit_code": r.exit_code,
                "timed_out": r.timed_out,
                "capture_scope": "cartoon_output",
                "stdout": r.stdout_capture,
                "stderr": r.stderr_capture
            }),
        ),
        Err(e) => (
            text_result(format!("could not run command: {e}"), true),
            json!({
                "schema_version": 1,
                "execution": match e { run::RunError::NotStarted(_) => "not_started", run::RunError::Observation(_) => "unknown" },
                "exit_code": null,
                "timed_out": null,
                "capture_scope": "cartoon_output",
                "stdout": null,
                "stderr": null
            }),
        ),
    };
    if structured {
        // Preserve the first human-readable block. MCP recommends a JSON text
        // fallback as well for clients that do not consume structuredContent.
        response["content"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"text", "text":outcome.to_string()}));
        response["structuredContent"] = outcome;
    }
    response
}

pub fn exists(name: &str) -> bool {
    matches!(
        name,
        "run" | "logs_grep" | "logs_list" | "last" | "diff" | "stats"
    )
}

/// A successful (`isError: false`) or failed tool result with one text block.
fn text_result(text: String, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

/// Execute tool `name`. Called on a worker thread; `cancel` is raised by
/// `notifications/cancelled` or by the client closing stdin.
pub fn call(
    name: &str,
    args: &Value,
    exe: &Path,
    cancel: &AtomicBool,
    structured: bool,
) -> Result<Value, RpcError> {
    let args = args
        .as_object()
        .ok_or_else(|| RpcError::invalid_params("`arguments` must be an object"))?;
    match name {
        "run" => {
            let a = parse_run(args)?;
            Ok(run_result(run::execute(exe, &a, cancel), &a, structured))
        }
        "logs_grep" => {
            let pattern = req_str(args, "pattern")?;
            let run_id = opt_str(args, "run_id")?;
            let context = opt_u64(args, "context")?.unwrap_or(2);
            if context > MAX_CONTEXT {
                return Err(RpcError::invalid_params(format!(
                    "`context` must be at most {MAX_CONTEXT}"
                )));
            }
            Ok(match logs_grep(pattern, run_id, context as usize) {
                Ok(text) => text_result(text.trim_end().to_string(), false),
                Err(e) => text_result(e.to_string(), true),
            })
        }
        "logs_list" => {
            let tag = opt_str(args, "tag")?;
            let limit = opt_u64(args, "limit")?.unwrap_or(DEFAULT_LIST_LIMIT).max(1) as usize;
            Ok(text_result(logs_list(tag, limit), false))
        }
        "last" => {
            let cmd = opt_str(args, "cmd")?;
            let Some(root) = crate::paths::runs_dir() else {
                return Ok(text_result("no state directory".into(), true));
            };
            Ok(match crate::last::render_last(&root, cmd) {
                Some(text) => text_result(text.trim_end().to_string(), false),
                None => text_result("no archived run matches".into(), true),
            })
        }
        "diff" => {
            let cmd = opt_str(args, "cmd")?;
            let ids = match (opt_str(args, "id_a")?, opt_str(args, "id_b")?) {
                (Some(a), Some(b)) => Some((a.to_string(), b.to_string())),
                (None, None) => None,
                _ => return Err(RpcError::invalid_params("`id_a` and `id_b` go together")),
            };
            let Some(root) = crate::paths::runs_dir() else {
                return Ok(text_result("no state directory".into(), true));
            };
            Ok(match crate::last::render_diff(&root, ids, cmd) {
                Ok(text) => text_result(text.trim_end().to_string(), false),
                Err(msg) => text_result(msg, true),
            })
        }
        "stats" => {
            let since = opt_str(args, "since")?;
            Ok(match crate::stats::report(since) {
                Ok(text) => text_result(text, false),
                Err(e) => text_result(e.to_string(), true),
            })
        }
        other => Err(RpcError::invalid_params(format!("unknown tool: {other}"))),
    }
}

fn parse_run(args: &Map<String, Value>) -> Result<RunArgs, RpcError> {
    reject_unknown(
        args,
        &["command", "cwd", "timeout_s", "compress", "max_tokens"],
    )?;
    let command = req_str(args, "command")?;
    if command.trim().is_empty() {
        return Err(RpcError::invalid_params("`command` is empty"));
    }
    let timeout_s = match args.get("timeout_s") {
        None | Some(Value::Null) => DEFAULT_TIMEOUT_S,
        Some(v) => match v.as_f64() {
            Some(t) if t > 0.0 && t <= MAX_TIMEOUT_S => t,
            _ => {
                return Err(RpcError::invalid_params(format!(
                    "`timeout_s` must be a number in (0, {MAX_TIMEOUT_S}]"
                )))
            }
        },
    };
    let compress = opt_str(args, "compress")?;
    if let Some(c) = compress {
        if c != "safe" && c != "aggressive" {
            return Err(RpcError::invalid_params(
                "`compress` must be \"safe\" or \"aggressive\"",
            ));
        }
    }
    let max_tokens = opt_u64(args, "max_tokens")?;
    if max_tokens == Some(0) {
        return Err(RpcError::invalid_params("`max_tokens` must be at least 1"));
    }
    Ok(RunArgs {
        command: command.to_string(),
        cwd: opt_str(args, "cwd")?.map(Into::into),
        timeout: Duration::from_secs_f64(timeout_s),
        compress: compress.map(String::from),
        max_tokens: max_tokens.map(|n| n as usize),
    })
}

fn reject_unknown(args: &Map<String, Value>, known: &[&str]) -> Result<(), RpcError> {
    match args.keys().find(|k| !known.contains(&k.as_str())) {
        Some(k) => Err(RpcError::invalid_params(format!("unknown argument `{k}`"))),
        None => Ok(()),
    }
}

fn req_str<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, RpcError> {
    opt_str(args, key)?
        .ok_or_else(|| RpcError::invalid_params(format!("missing required string `{key}`")))
}

fn opt_str<'a>(args: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, RpcError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(RpcError::invalid_params(format!(
            "`{key}` must be a string"
        ))),
    }
}

fn opt_u64(args: &Map<String, Value>, key: &str) -> Result<Option<u64>, RpcError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| {
            RpcError::invalid_params(format!("`{key}` must be a non-negative integer"))
        }),
    }
}

fn logs_grep(pattern: &str, run_id: Option<&str>, context: usize) -> anyhow::Result<String> {
    let id = match run_id {
        Some(id) => id.to_string(),
        None => crate::archive::last_id().ok_or_else(|| anyhow::anyhow!("no archived runs yet"))?,
    };
    let (_, stdout, stderr) = crate::archive::load(&id)?;
    let re = regex::Regex::new(pattern).map_err(|e| anyhow::anyhow!("invalid pattern: {e}"))?;
    Ok(crate::logs_cmd::render_grep(
        &id, &stdout, &stderr, &re, context,
    ))
}

fn logs_list(tag: Option<&str>, limit: usize) -> String {
    let metas = crate::archive::list(tag);
    let shown = metas.len().min(limit);
    let mut out = crate::logs_cmd::render_list(&metas[..shown]);
    if shown < metas.len() {
        out.push_str(&format!(
            "\n({shown} of {} runs shown; raise `limit` for more)",
            metas.len()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    // Validate the schema keywords this tool emits, without adding a runtime
    // validator dependency. This is not a general JSON Schema implementation.
    fn assert_schema(value: &Value, schema: &Value) {
        let types = match &schema["type"] {
            Value::String(t) => vec![t.as_str()],
            Value::Array(ts) => ts.iter().map(|t| t.as_str().unwrap()).collect(),
            other => panic!("unexpected schema type: {other}"),
        };
        assert!(
            types.iter().any(|t| match *t {
                "null" => value.is_null(),
                "object" => value.is_object(),
                "boolean" => value.is_boolean(),
                "string" => value.is_string(),
                "integer" => value.is_i64() || value.is_u64(),
                _ => false,
            }),
            "{value} does not match {schema}"
        );
        if let Some(expected) = schema.get("const") {
            assert_eq!(value, expected);
        }
        if let Some(variants) = schema.get("enum") {
            assert!(variants.as_array().unwrap().contains(value));
        }
        if let Some(minimum) = schema.get("minimum") {
            assert!(value.as_u64().unwrap() >= minimum.as_u64().unwrap());
        }
        if let Some(fields) = value.as_object() {
            for key in schema["required"].as_array().unwrap() {
                assert!(fields.contains_key(key.as_str().unwrap()));
            }
            assert_eq!(schema["additionalProperties"], false);
            for (key, field) in fields {
                assert_schema(field, &schema["properties"][key]);
            }
        }
    }

    #[test]
    fn observation_errors_are_unknown_not_a_claim_that_no_process_started() {
        let a = parse_run(&obj(json!({"command":"true"}))).unwrap();
        let r = run_result(
            Err(run::RunError::Observation(std::io::ErrorKind::Other.into())),
            &a,
            true,
        );
        assert_eq!(r["isError"], true);
        assert_eq!(
            r["structuredContent"],
            json!({
                "schema_version":1, "execution":"unknown", "exit_code":null,
                "timed_out":null, "capture_scope":"cartoon_output", "stdout":null, "stderr":null
            })
        );
        assert_schema(&r["structuredContent"], &run_output_schema());
    }

    #[test]
    fn startup_errors_conform_to_the_schema_including_null_observations() {
        let r = call(
            "run",
            &json!({"command":"true"}),
            Path::new("/no/such/executable"),
            &AtomicBool::new(false),
            true,
        )
        .unwrap();
        assert_eq!(r["structuredContent"]["execution"], "not_started");
        assert_schema(&r["structuredContent"], &run_output_schema());
    }

    #[test]
    fn run_args_parse_with_defaults() {
        let a = parse_run(&obj(json!({"command": "pytest -q"}))).unwrap();
        assert_eq!(a.command, "pytest -q");
        assert_eq!(a.timeout, Duration::from_secs(600));
        assert_eq!(a.cwd, None);
        assert_eq!(a.compress, None);
        assert_eq!(a.max_tokens, None);
        let a = parse_run(&obj(json!({
            "command": "npm test", "cwd": "/tmp", "timeout_s": 1.5,
            "compress": "aggressive", "max_tokens": 800
        })))
        .unwrap();
        assert_eq!(a.timeout, Duration::from_millis(1500));
        assert_eq!(a.cwd.as_deref(), Some(Path::new("/tmp")));
        assert_eq!(a.compress.as_deref(), Some("aggressive"));
        assert_eq!(a.max_tokens, Some(800));
    }

    #[test]
    fn bad_run_args_are_invalid_params() {
        for bad in [
            json!({}),
            json!({"command": ""}),
            json!({"command": 3}),
            json!({"command": "ls", "timeout_s": 0}),
            json!({"command": "ls", "timeout_s": "10"}),
            json!({"command": "ls", "timeout_s": 99999}),
            json!({"command": "ls", "compress": "max"}),
            json!({"command": "ls", "max_tokens": -1}),
            json!({"command": "ls", "max_tokens": 0}),
            json!({"command": "ls", "shell": "bash"}),
        ] {
            let e = parse_run(&obj(bad.clone())).unwrap_err();
            assert_eq!(e.code, super::super::protocol::INVALID_PARAMS, "{bad}");
        }
    }

    #[test]
    fn schemas_list_exactly_the_parsed_run_arguments() {
        let defs = definitions(false);
        let props = defs[0]["inputSchema"]["properties"].as_object().unwrap();
        let mut keys: Vec<&str> = props.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["command", "compress", "cwd", "max_tokens", "timeout_s"]
        );
        // `run` changes the world; the archive/ledger tools only read.
        assert_eq!(defs[0]["annotations"]["readOnlyHint"], false);
        for t in defs.as_array().unwrap()[1..].iter() {
            assert_eq!(t["annotations"]["readOnlyHint"], true, "{t}");
        }
        assert!(RUN_DESCRIPTION.contains("SECURITY"));
    }

    #[cfg(unix)]
    fn synthetic_run(script: &str) -> Value {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("synthetic-cartoon");
        std::fs::write(&exe, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = call(
            "run",
            &json!({"command":"synthetic"}),
            &exe,
            &AtomicBool::new(false),
            true,
        )
        .unwrap();
        assert_schema(&result["structuredContent"], &run_output_schema());
        result
    }

    #[test]
    #[cfg(unix)]
    fn structured_capture_discloses_dropped_bytes_without_changing_exit() {
        let r = synthetic_run("head -c 4194305 /dev/zero; printf warn >&2; exit 7");
        let o = &r["structuredContent"];
        assert_eq!(o["exit_code"], 7);
        assert_eq!(o["stdout"]["status"], "eof");
        assert_eq!(o["stdout"]["captured_bytes"], 4194304);
        assert_eq!(o["stdout"]["dropped_bytes"], 1);
        assert_eq!(o["stderr"]["captured_bytes"], 4);
        assert_eq!(o["stderr"]["dropped_bytes"], 0);
        let r = synthetic_run("head -c 4194305 /dev/zero >&2; exit 0");
        assert_eq!(r["structuredContent"]["stderr"]["dropped_bytes"], 1);
        assert_eq!(r["structuredContent"]["stderr"]["captured_bytes"], 4194304);
        assert_eq!(r["structuredContent"]["stdout"]["captured_bytes"], 0);
    }

    #[test]
    #[cfg(unix)]
    fn structured_capture_discloses_lossy_utf8() {
        let r = synthetic_run("printf '\\377'; printf '\\376' >&2");
        for stream in ["stdout", "stderr"] {
            let capture = &r["structuredContent"][stream];
            assert_eq!(capture["status"], "eof");
            assert_eq!(capture["captured_bytes"], 1);
            assert_eq!(capture["utf8_lossy"], true);
        }
    }

    #[test]
    #[cfg(unix)]
    fn structured_capture_does_not_claim_eof_for_an_inherited_open_pipe() {
        let r = synthetic_run("sleep 3 &\nprintf partial\nexit 0");
        let o = &r["structuredContent"];
        assert_eq!(o["execution"], "exited");
        assert_eq!(o["exit_code"], 0);
        assert_eq!(o["timed_out"], false);
        assert_eq!(o["stdout"]["status"], "deadline");
        assert_eq!(o["stdout"]["captured_bytes"], 7);
    }
}
