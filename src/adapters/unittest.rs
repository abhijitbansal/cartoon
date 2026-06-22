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
        "python -m unittest | uv run python -m unittest"
    }
    fn detect(&self, argv: &[String]) -> bool {
        // `uv run python -m unittest` is transparent once we strip the wrapper.
        is_python_module(super::strip_uv_run(argv), "unittest")
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
    let (mut n_fail, mut n_err, mut n_skip) = (0u64, 0u64, 0u64);
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
                }
            }
        }
    }
    let failed = n_fail + n_err;
    let skipped = n_skip;
    let passed = total.saturating_sub(failed + skipped);

    let header = re(&HEADER, r"(?m)^(FAIL|ERROR): (\S+) \(([^)]+)\)");
    let file_line = re(&FILE_LINE, r#"File "([^"]+)", line (\d+)"#);
    let mut failures = Vec::new();
    for block in body.split(SEPARATOR) {
        let Some(h) = header.captures(block) else {
            continue;
        };
        let id = h[3].to_string();
        let loc = file_line
            .captures_iter(block)
            .filter(|c| !c[1].contains("site-packages") && !c[1].contains("/unittest/"))
            .last()
            .map(|c| format!("{}:{}", &c[1], &c[2]))
            .unwrap_or_default();
        let msg = block
            .lines()
            .rev()
            .find(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with('-')
            })
            .unwrap_or("")
            .trim()
            .to_string();
        let trace = trim_trace(block);
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
    fn parses_all_pass() {
        let r = parse_fixture("all-pass.txt");
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (4, 4, 0, 0));
    }

    #[test]
    fn unrecognized_text_is_error() {
        assert!(parse_text("random program output").is_err());
    }

    #[test]
    fn stray_ok_line_in_traceback_does_not_zero_counts() {
        let stderr = "F\n======================================================================\nFAIL: test_x (m.T.test_x)\n----------------------------------------------------------------------\nTraceback (most recent call last):\n  File \"/proj/t.py\", line 2, in test_x\nOK was not the expected value\nAssertionError: nope\n\n----------------------------------------------------------------------\nRan 1 test in 0.001s\n\nFAILED (failures=1)\n";
        let r = parse_text(stderr).unwrap();
        assert_eq!((r.total, r.failed, r.passed), (1, 1, 0));
    }
}
