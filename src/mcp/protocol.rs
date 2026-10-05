//! JSON-RPC 2.0 framing and the MCP lifecycle methods. Pure: `route` maps one
//! input line to what the server should do, so every protocol rule is
//! unit-testable without a process.
use serde_json::{json, Value};

/// Protocol revisions this server speaks, newest first. `initialize` echoes
/// the client's version when it is one of these, else offers the newest.
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

/// A JSON-RPC error object (code + message).
#[derive(Debug, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn invalid_params(message: impl Into<String>) -> Self {
        RpcError {
            code: INVALID_PARAMS,
            message: message.into(),
        }
    }
}

/// What the server does with one input line.
#[derive(Debug, PartialEq)]
pub enum Route {
    /// Write this message now.
    Reply(Value),
    /// Notification, or a response to us: nothing to write.
    Silent,
    /// `notifications/cancelled` for this request id.
    Cancel(Value),
    /// `tools/call`: run on a worker thread, then answer `id`.
    Call {
        id: Value,
        name: String,
        args: Value,
    },
}

pub fn negotiate_version(requested: Option<&str>) -> &'static str {
    SUPPORTED_VERSIONS
        .iter()
        .find(|v| Some(**v) == requested)
        .copied()
        .unwrap_or(SUPPORTED_VERSIONS[0])
}

/// Build the response to request `id` from a handler's outcome.
pub fn respond(id: &Value, outcome: Result<Value, RpcError>) -> Value {
    match outcome {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(e) => error(id, e.code, &e.message),
    }
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Shown to the model by clients that surface server instructions.
const INSTRUCTIONS: &str = "Use the `run` tool instead of a plain shell for test runs, \
builds, linters and JSON-emitting CLIs: it runs the command through cartoon, which \
returns a compact report (failures, counts, diagnostics) instead of the raw output. \
Never pipe its output through head/tail/grep — the full raw output is archived; search \
it with `logs_grep` instead of re-running.";

pub fn route(line: &str) -> Route {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Route::Reply(error(
                &Value::Null,
                PARSE_ERROR,
                &format!("parse error: {e}"),
            ))
        }
    };
    let Some(obj) = msg.as_object() else {
        // Batches were dropped from MCP in 2025-06-18; answer, don't guess.
        return Route::Reply(error(
            &Value::Null,
            INVALID_REQUEST,
            "expected a single JSON-RPC object",
        ));
    };
    let id = obj.get("id").cloned();
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        // A response to a server request (we send none) or garbage.
        return match id {
            Some(_) if obj.contains_key("result") || obj.contains_key("error") => Route::Silent,
            _ => Route::Reply(error(
                &id.unwrap_or(Value::Null),
                INVALID_REQUEST,
                "missing method",
            )),
        };
    };
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return match id {
            Some(id) => Route::Reply(error(&id, INVALID_REQUEST, "jsonrpc must be \"2.0\"")),
            None => Route::Silent,
        };
    }
    let params = obj.get("params").cloned().unwrap_or(json!({}));
    let Some(id) = id else {
        // Notifications never get a response.
        if method == "notifications/cancelled" {
            if let Some(rid) = params.get("requestId") {
                return Route::Cancel(rid.clone());
            }
        }
        return Route::Silent;
    };
    match method {
        "initialize" => {
            let requested = params.get("protocolVersion").and_then(Value::as_str);
            Route::Reply(respond(
                &id,
                Ok(json!({
                    "protocolVersion": negotiate_version(requested),
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": {
                        "name": "cartoon",
                        "title": "cartoon",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "instructions": INSTRUCTIONS,
                })),
            ))
        }
        "ping" => Route::Reply(respond(&id, Ok(json!({})))),
        "tools/list" => Route::Reply(respond(
            &id,
            Ok(json!({ "tools": super::tools::definitions() })),
        )),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Route::Reply(error(
                    &id,
                    INVALID_PARAMS,
                    "tools/call needs a string `name`",
                ));
            };
            if !super::tools::exists(name) {
                return Route::Reply(error(&id, INVALID_PARAMS, &format!("unknown tool: {name}")));
            }
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if !args.is_object() {
                return Route::Reply(error(&id, INVALID_PARAMS, "`arguments` must be an object"));
            }
            Route::Call {
                id,
                name: name.to_string(),
                args,
            }
        }
        _ => Route::Reply(error(
            &id,
            METHOD_NOT_FOUND,
            &format!("method not found: {method}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(line: &str) -> Value {
        match route(line) {
            Route::Reply(v) => v,
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    #[test]
    fn initialize_echoes_a_supported_version_else_offers_the_latest() {
        let r = reply(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
        );
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "cartoon");
        assert_eq!(
            r["result"]["serverInfo"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        assert!(r["result"]["capabilities"]["tools"].is_object());

        let r = reply(
            r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        );
        assert_eq!(r["result"]["protocolVersion"], SUPPORTED_VERSIONS[0]);
    }

    #[test]
    fn ping_answers_empty_result_and_keeps_string_ids() {
        let r = reply(r#"{"jsonrpc":"2.0","id":"abc","method":"ping"}"#);
        assert_eq!(r, json!({"jsonrpc":"2.0","id":"abc","result":{}}));
    }

    #[test]
    fn notifications_get_no_response() {
        for line in [
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/whatever","params":{}}"#,
            // a response to a server request
            r#"{"jsonrpc":"2.0","id":7,"result":{}}"#,
        ] {
            assert_eq!(route(line), Route::Silent, "{line}");
        }
    }

    #[test]
    fn cancelled_notification_names_the_request() {
        assert_eq!(
            route(
                r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":5}}"#
            ),
            Route::Cancel(json!(5))
        );
    }

    #[test]
    fn protocol_errors_carry_the_right_codes() {
        let r = reply(r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#);
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(r["id"], 3);
        assert_eq!(reply("{nope")["error"]["code"], PARSE_ERROR);
        assert_eq!(reply("{nope")["id"], Value::Null);
        assert_eq!(reply("[1,2]")["error"]["code"], INVALID_REQUEST);
        let r = reply(r#"{"jsonrpc":"1.0","id":4,"method":"ping"}"#);
        assert_eq!(r["error"]["code"], INVALID_REQUEST);
    }

    #[test]
    fn tools_call_validates_name_and_arguments() {
        let r = reply(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
        );
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
        let r = reply(r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{}}"#);
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
        let r = reply(
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"run","arguments":"echo"}}"#,
        );
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
        assert_eq!(
            route(r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"stats"}}"#),
            Route::Call {
                id: json!(8),
                name: "stats".into(),
                args: json!({})
            }
        );
    }

    #[test]
    fn tools_list_has_every_tool_with_a_schema() {
        let r = reply(r#"{"jsonrpc":"2.0","id":9,"method":"tools/list"}"#);
        let tools = r["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            ["run", "logs_grep", "logs_list", "last", "diff", "stats"]
        );
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object", "{t}");
            assert!(t["description"].as_str().unwrap().len() > 20, "{t}");
            assert!(t["annotations"]["readOnlyHint"].is_boolean(), "{t}");
        }
    }
}
