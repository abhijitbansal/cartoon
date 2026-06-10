use serde_json::{json, Map, Value};

#[derive(Debug)]
pub struct TestReport {
    pub runner: &'static str,
    pub total: u64,
    pub passed: u64,
    pub failed: u64,
    pub skipped: u64,
    pub duration_s: f64,
    pub failures: Vec<Failure>,
}

#[derive(Debug)]
pub struct Failure {
    pub id: String,
    pub loc: String,
    pub msg: String,
    pub trace: Vec<String>,
}

/// Asymmetric rendering: passes cost one summary block; failures keep
/// id/loc/msg rows plus trimmed traces. `fast_note` discloses injected
/// acceleration args (e.g. "-n auto") right after the runner line.
pub fn render(report: &TestReport, trace_lines: usize, fast_note: Option<&str>) -> String {
    let mut root = Map::new();
    root.insert("runner".into(), json!(report.runner));
    if let Some(f) = fast_note {
        root.insert("fast".into(), json!(f));
    }
    root.insert(
        "summary".into(),
        json!({
            "total": report.total,
            "passed": report.passed,
            "failed": report.failed,
            "skipped": report.skipped,
            "duration_s": report.duration_s,
        }),
    );
    if !report.failures.is_empty() {
        root.insert(
            "failures".into(),
            Value::Array(
                report
                    .failures
                    .iter()
                    .map(|f| json!({"id": f.id, "loc": f.loc, "msg": f.msg}))
                    .collect(),
            ),
        );
        let traces: Map<String, Value> = report
            .failures
            .iter()
            .filter_map(|f| {
                let capped: Vec<&String> = f.trace.iter().take(trace_lines).collect();
                if capped.is_empty() {
                    None
                } else {
                    Some((f.id.clone(), json!(capped)))
                }
            })
            .collect();
        if !traces.is_empty() {
            root.insert("traces".into(), Value::Object(traces));
        }
    }
    crate::toon::encode(&Value::Object(root))
}

const NOISE: &[&str] = &[
    "site-packages",
    "/_pytest/",
    "/unittest/case.py",
    "node_modules",
    "/jest-",
    "node:internal",
];

/// Keep user-code frames, drop framework internals, drop blank lines.
pub fn trim_trace(raw: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut skip_frame = false;
    for line in raw.lines() {
        let l = line.trim_end();
        let t = l.trim_start();
        let is_frame_header = t.starts_with("File \"") || t.starts_with("at ");
        if is_frame_header {
            skip_frame = NOISE.iter().any(|n| l.contains(n));
        }
        if !skip_frame && !t.is_empty() {
            lines.push(t.to_string());
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TestReport {
        TestReport {
            runner: "pytest",
            total: 48,
            passed: 45,
            failed: 2,
            skipped: 1,
            duration_s: 3.2,
            failures: vec![
                Failure {
                    id: "tests/test_auth.py::test_expiry".into(),
                    loc: "tests/test_auth.py:42".into(),
                    msg: "assert exp < now".into(),
                    trace: vec![
                        "tests/test_auth.py:42 in test_expiry".into(),
                        "assert token.exp < now()".into(),
                    ],
                },
                Failure {
                    id: "tests/test_user.py::test_create".into(),
                    loc: "tests/test_user.py:88".into(),
                    msg: "KeyError: 'email'".into(),
                    trace: vec![],
                },
            ],
        }
    }

    #[test]
    fn renders_summary_and_failures() {
        let out = render(&sample(), 20, None);
        assert!(out.contains("runner: pytest"), "got:\n{out}");
        assert!(out.contains("total: 48"));
        assert!(out.contains("failed: 2"));
        assert!(out.contains("failures[2]{id,loc,msg}:"));
        assert!(out.contains("tests/test_auth.py::test_expiry"));
    }

    #[test]
    fn empty_trace_gets_no_traces_entry() {
        let out = render(&sample(), 20, None);
        // traces section exists (first failure has a trace) but only one key
        let traces_idx = out.find("traces:").expect("traces section");
        let tail = &out[traces_idx..];
        assert!(tail.contains("test_expiry"));
        assert!(!tail.contains("test_create"));
    }

    #[test]
    fn all_pass_renders_no_failures_section() {
        let mut r = sample();
        r.failures.clear();
        r.failed = 0;
        r.passed = 47;
        let out = render(&r, 20, None);
        assert!(!out.contains("failures"));
        assert!(!out.contains("traces"));
    }

    #[test]
    fn trace_capped_at_limit() {
        let mut r = sample();
        r.failures[0].trace = (0..50).map(|i| format!("line {i}")).collect();
        let out = render(&r, 5, None);
        assert!(out.contains("line 4"));
        assert!(!out.contains("line 5"));
    }

    #[test]
    fn trim_trace_drops_framework_frames() {
        let raw = "Traceback (most recent call last):\n  File \"/usr/lib/python3/site-packages/_pytest/runner.py\", line 1, in run\n    framework()\n  File \"tests/test_auth.py\", line 42, in test_expiry\n    assert token.exp < now()\nAssertionError: assert exp < now";
        let t = trim_trace(raw);
        let joined = t.join("\n");
        assert!(joined.contains("tests/test_auth.py"), "got: {joined}");
        assert!(!joined.contains("site-packages"));
        assert!(joined.contains("AssertionError"));
    }

    #[test]
    fn zero_trace_lines_omits_traces_section() {
        let out = render(&sample(), 0, None);
        assert!(!out.contains("traces"));
    }

    #[test]
    fn trim_trace_drops_js_noise_frames() {
        let raw = "Error: expect(received).toBe(expected)\n    at Object.<anonymous> (/proj/src/auth.test.js:43:29)\n    at processTicksAndRejections (node:internal/process/task_queues/95:5)";
        let t = trim_trace(raw).join("\n");
        assert!(t.contains("auth.test.js"));
        assert!(!t.contains("node:internal"));
    }

    #[test]
    fn fast_note_renders_after_runner() {
        let out = render(&sample(), 20, Some("-n auto"));
        let runner_idx = out.find("runner: pytest").unwrap();
        // TOON quotes strings starting with '-' to avoid ambiguity, so the
        // value "-n auto" is rendered as: fast: "-n auto"
        let fast_idx = out.find("fast: \"-n auto\"").expect("fast line present");
        let summary_idx = out.find("summary:").unwrap();
        assert!(
            runner_idx < fast_idx && fast_idx < summary_idx,
            "got:\n{out}"
        );
    }

    #[test]
    fn no_fast_note_no_fast_line() {
        let out = render(&sample(), 20, None);
        assert!(!out.contains("fast:"), "got:\n{out}");
    }
}
