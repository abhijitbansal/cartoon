use super::report::{trim_trace, Failure, TestReport};
use super::{is_python_module, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use regex::Regex;
use std::sync::OnceLock;

const SEPARATOR: &str = "======================================================================";

pub struct Unittest;

impl Adapter for Unittest {
    fn name(&self) -> &'static str {
        "unittest"
    }
    fn matches(&self) -> &'static str {
        "python -m unittest | uv run [python] -m unittest"
    }
    fn detect(&self, full: &[String]) -> bool {
        // `uv run python -m unittest` is transparent once we strip the wrapper;
        // `uv run -m unittest` strips to a bare `-m unittest` module form.
        let argv = super::strip_uv_run(full);
        let uv_wrapped = argv.len() != full.len();
        is_python_module(argv, "unittest") || (uv_wrapped && super::is_module_run(argv, "unittest"))
    }
    fn prepare(&self, argv: Vec<String>) -> Prepared {
        Prepared {
            argv,
            artifact: None,
        }
    }
    fn parse(&self, captured: &Captured, _prepared: &Prepared) -> Result<ParseOutcome> {
        let report = parse_text(&captured.stderr)?;
        Ok(ParseOutcome {
            report: super::AdapterReport::Tests(report),
            // stdout holds user prints — the agent may need them.
            passthrough_stdout: (!captured.stdout.is_empty()).then(|| captured.stdout.clone()),
            // stderr WAS the report — consumed.
            passthrough_stderr: None,
        })
    }
}

/// `AssertionError: …`, `ValueError: …`, `module.CustomException: …`.
fn is_exception_line(t: &str) -> bool {
    static EXC: OnceLock<Regex> = OnceLock::new();
    re(
        &EXC,
        r"^[A-Za-z_][A-Za-z0-9_.]*(Error|Exception|Failure|Exit)\b",
    )
    .is_match(t)
}

/// The lines from `Traceback (most recent call last):` through the exception
/// line (inclusive); the whole block when no traceback is present.
fn traceback_span(block: &str, exception_idx: Option<usize>) -> &str {
    let Some(start) = block.find("Traceback (most recent call last):") else {
        return block;
    };
    let Some(exc_i) = exception_idx else {
        return &block[start..];
    };
    let mut end = block.len();
    let mut offset = 0;
    for (i, line) in block.split_inclusive('\n').enumerate() {
        offset += line.len();
        if i == exc_i {
            end = offset;
            break;
        }
    }
    if end < start {
        return &block[start..];
    }
    &block[start..end]
}

/// Lines after the exception line in a failure block: the `- `/`+ ` lines
/// of unittest's diff when present (the `?` hint lines and the "First list
/// contains…" prose are dropped), else the last few lines of explanation.
/// Bounded either way: the full detail stays in raw_log.
fn diff_tail(block: &str, exception_idx: usize) -> Vec<String> {
    const MAX: usize = 6;
    let lines: Vec<&str> = block
        .lines()
        .skip(exception_idx + 1)
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty() && !l.chars().all(|c| c == '-'))
        .collect();
    let diff: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| l.starts_with("- ") || l.starts_with("+ "))
        .collect();
    let picked = if diff.is_empty() { lines } else { diff };
    let skip = picked.len().saturating_sub(MAX);
    picked[skip..].iter().map(|l| l.to_string()).collect()
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

pub fn parse_text(stderr: &str) -> Result<TestReport> {
    static RAN: OnceLock<Regex> = OnceLock::new();
    static HEADER: OnceLock<Regex> = OnceLock::new();
    static FILE_LINE: OnceLock<Regex> = OnceLock::new();
    static TAIL: OnceLock<Regex> = OnceLock::new();

    let ran = re(&RAN, r"Ran (\d+) tests? in ([0-9.]+)s");
    let caps = ran
        .captures(stderr)
        .context("no 'Ran N tests' line — not unittest output")?;
    let total: u64 = caps[1].parse()?;
    let duration_s: f64 = caps[2].parse()?;

    // Cut the tail off so failure-block parsing never sees "Ran N tests...".
    let body = &stderr[..caps.get(0).map(|m| m.start()).unwrap_or(stderr.len())];

    // tail counts: "FAILED (failures=1, errors=2, skipped=1)" or "OK (skipped=1)"
    let tail = re(&TAIL, r"(?m)^(OK|FAILED)\s*(?:\(([^)]*)\))?");
    let (mut n_fail, mut n_err, mut n_skip, mut n_unexpected) = (0u64, 0u64, 0u64, 0u64);
    let tail_str = &stderr[caps.get(0).map(|m| m.end()).unwrap_or(0)..];
    if let Some(t) = tail.captures(tail_str) {
        if let Some(details) = t.get(2) {
            for part in details.as_str().split(',') {
                let part = part.trim();
                if let Some(v) = part.strip_prefix("failures=") {
                    n_fail = v.parse().unwrap_or(0);
                } else if let Some(v) = part.strip_prefix("errors=") {
                    n_err = v.parse().unwrap_or(0);
                } else if let Some(v) = part.strip_prefix("skipped=") {
                    n_skip = v.parse().unwrap_or(0);
                } else if let Some(v) = part.strip_prefix("unexpected successes=") {
                    // An @expectedFailure test that passed fails the run.
                    n_unexpected = v.parse().unwrap_or(0);
                }
            }
        }
    }
    let failed = n_fail + n_err + n_unexpected;
    let skipped = n_skip;
    let passed = total.saturating_sub(failed + skipped);

    let header = re(
        &HEADER,
        r"(?m)^(FAIL|ERROR|UNEXPECTED SUCCESS): (\S+) \(([^)]+)\)",
    );
    let file_line = re(&FILE_LINE, r#"File "([^"]+)", line (\d+)"#);
    let mut failures = Vec::new();
    for block in body.split(SEPARATOR) {
        let Some(h) = header.captures(block) else {
            continue;
        };
        let id = h[3].to_string();
        if &h[1] == "UNEXPECTED SUCCESS" {
            // Python 3.11+ lists these as blocks with no traceback.
            failures.push(Failure {
                id,
                loc: String::new(),
                msg: "unexpected success (test marked @expectedFailure passed)".into(),
                trace: Vec::new(),
            });
            continue;
        }
        let loc = file_line
            .captures_iter(block)
            .filter(|c| !c[1].contains("site-packages") && !c[1].contains("/unittest/"))
            .last()
            .map(|c| format!("{}:{}", &c[1], &c[2]))
            .unwrap_or_default();
        // The exception line ("AssertionError: Lists differ: …") is the
        // message. Fall back to the last non-separator line.
        let exception_idx = block.lines().position(|l| is_exception_line(l.trim()));
        let mut msg = exception_idx
            .and_then(|i| block.lines().nth(i))
            .or_else(|| {
                block.lines().rev().find(|l| {
                    let t = l.trim();
                    !t.is_empty() && !t.starts_with('-')
                })
            })
            .unwrap_or("")
            .trim()
            .to_string();
        // Trace: the frames from "Traceback" up to the exception line, which
        // is not repeated (it is `msg`). The FAIL header (already the id)
        // and the dashed separator are dropped.
        let trace_block = traceback_span(block, exception_idx);
        let mut trace = trim_trace(trace_block);
        if trace.last().is_some_and(|l| *l == msg) {
            trace.pop();
        }
        // Multi-line assertion detail (a list/dict/string diff) follows the
        // exception line; keep its compact tail, which also carries the
        // user's own message (`… : unexpected roles for alpha`).
        let detail = exception_idx
            .map(|i| diff_tail(block, i))
            .unwrap_or_default();
        if let Some((_, custom)) = detail.last().and_then(|l| l.rsplit_once(" : ")) {
            if !msg.contains(custom) {
                msg = format!("{msg} : {custom}");
            }
        }
        trace.extend(detail);
        failures.push(Failure {
            id,
            loc,
            msg,
            trace,
        });
    }

    Ok(TestReport {
        runner: "unittest",
        total,
        passed,
        failed,
        skipped,
        duration_s,
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_fixture(name: &str) -> crate::adapters::report::TestReport {
        let path = format!(
            "{}/tests/fixtures/unittest/{}",
            env!("CARGO_MANIFEST_DIR"),
            name
        );
        parse_text(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn parses_mixed_results() {
        let r = parse_fixture("mixed.txt");
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (4, 2, 1, 1));
        assert_eq!(r.duration_s, 0.012);
        let f = &r.failures[0];
        assert_eq!(f.id, "tests.test_auth.AuthTest.test_expiry");
        assert_eq!(f.loc, "/home/user/proj/tests/test_auth.py:42");
        assert_eq!(f.msg, "AssertionError: 1717000000 not less than 1716000000");
    }

    #[test]
    fn msg_is_the_exception_line_and_trace_stops_there() {
        let stderr = "F\n======================================================================\nFAIL: test_fail_alpha (test_many.ManyTests.test_fail_alpha)\n----------------------------------------------------------------------\nTraceback (most recent call last):\n  File \"/p/test_many.py\", line 22, in test_fail_alpha\n    self._explode(\"alpha\")\n    ~~~~~~~~~~~~~^^^^^^^^^\n  File \"/p/test_many.py\", line 19, in _explode\n    self.assertEqual(a, b)\nAssertionError: Lists differ: ['admin', 'editor', 'viewer'] != ['admin', 'editor']\n\nFirst list contains 1 additional elements.\nFirst extra element 2:\n'viewer'\n\n- ['admin', 'editor', 'viewer']\n?                   ----------\n+ ['admin', 'editor'] : unexpected roles for alpha\n\n----------------------------------------------------------------------\nRan 1 test in 0.001s\n\nFAILED (failures=1)\n";
        let r = parse_text(stderr).unwrap();
        let f = &r.failures[0];
        assert!(
            f.msg.starts_with("AssertionError: Lists differ"),
            "{}",
            f.msg
        );
        assert_eq!(f.loc, "/p/test_many.py:19");
        assert!(
            f.trace[0].starts_with("File \"/p/test_many.py\", line 22"),
            "{:?}",
            f.trace
        );
        // The user's own message survives, in msg and in the diff tail.
        assert!(
            f.msg.ends_with(" : unexpected roles for alpha"),
            "{}",
            f.msg
        );
        assert_eq!(
            f.trace.last().unwrap(),
            "+ ['admin', 'editor'] : unexpected roles for alpha"
        );
        assert!(
            f.trace
                .contains(&"- ['admin', 'editor', 'viewer']".to_string()),
            "{:?}",
            f.trace
        );
        // msg is not repeated as a trace line.
        assert!(
            !f.trace.iter().any(|l| l.starts_with("AssertionError")),
            "{:?}",
            f.trace
        );
        assert!(
            !f.trace.iter().any(|l| l.starts_with("? ")),
            "{:?}",
            f.trace
        );
        assert!(
            !f.trace.iter().any(|l| l.starts_with("FAIL:")),
            "{:?}",
            f.trace
        );
        assert!(
            !f.trace.iter().any(|l| l.starts_with("---")),
            "{:?}",
            f.trace
        );
        assert!(
            !f.trace.iter().any(|l| l.contains("~~~~^^^^")),
            "{:?}",
            f.trace
        );
        assert!(
            !f.trace.iter().any(|l| l.starts_with("First list")),
            "{:?}",
            f.trace
        );
    }

    #[test]
    fn unexpected_successes_count_as_failures() {
        // Captured from Python 3.11: one plain failure with a custom
        // message, one expected failure, one unexpected success.
        let r = parse_fixture("unexpected-success.txt");
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (4, 2, 2, 0));
        let ids: Vec<&str> = r.failures.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["test_us.T.test_fail", "test_us.T.test_unexpected"]
        );
        assert_eq!(r.failures[0].msg, "AssertionError: 1 != 2 : custom note");
        assert!(r.failures[1].msg.starts_with("unexpected success"));
    }

    #[test]
    fn unexpected_successes_counted_without_blocks() {
        // Python < 3.11 prints only the tail count.
        let stderr = "u\n----------------------------------------------------------------------\nRan 1 test in 0.000s\n\nFAILED (unexpected successes=1)\n";
        let r = parse_text(stderr).unwrap();
        assert_eq!((r.total, r.passed, r.failed), (1, 0, 1));
    }

    #[test]
    fn parses_all_pass() {
        let r = parse_fixture("all-pass.txt");
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (4, 4, 0, 0));
    }

    #[test]
    fn unrecognized_text_is_error() {
        assert!(parse_text("random program output").is_err());
    }

    #[test]
    fn detects_unittest_invocations() {
        let argv = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(Unittest.detect(&argv(&["python", "-m", "unittest"])));
        assert!(Unittest.detect(&argv(&["uv", "run", "python", "-m", "unittest"])));
        // uv's own module form.
        assert!(Unittest.detect(&argv(&["uv", "run", "-m", "unittest"])));
        assert!(Unittest.detect(&argv(&[
            "uv",
            "run",
            "--no-sync",
            "python",
            "-m",
            "unittest"
        ])));
        // Not unittest.
        assert!(!Unittest.detect(&argv(&["uv", "run", "-m", "pytest"])));
        assert!(!Unittest.detect(&argv(&["-m", "unittest"])));
    }

    #[test]
    fn stray_ok_line_in_traceback_does_not_zero_counts() {
        let stderr = "F\n======================================================================\nFAIL: test_x (m.T.test_x)\n----------------------------------------------------------------------\nTraceback (most recent call last):\n  File \"/proj/t.py\", line 2, in test_x\nOK was not the expected value\nAssertionError: nope\n\n----------------------------------------------------------------------\nRan 1 test in 0.001s\n\nFAILED (failures=1)\n";
        let r = parse_text(stderr).unwrap();
        assert_eq!((r.total, r.failed, r.passed), (1, 1, 0));
    }
}
