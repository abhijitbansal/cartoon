use super::report::{trim_trace, Failure, TestReport};
use super::{basename, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use serde::Deserialize;

pub struct Jest;

impl Adapter for Jest {
    fn name(&self) -> &'static str {
        "jest"
    }
    fn matches(&self) -> &'static str {
        "jest | npx jest | bunx jest"
    }
    fn detect(&self, argv: &[String]) -> bool {
        match argv {
            [first, ..] if basename(first) == "jest" => true,
            [first, second, ..]
                if matches!(basename(first), "npx" | "bunx")
                    && super::basename(second) == "jest" =>
            {
                true
            }
            _ => false,
        }
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        argv.push("--json".into());
        argv.push("--testLocationInResults".into());
        Prepared {
            argv,
            artifact: None,
        }
    }
    fn parse(&self, captured: &Captured, _prepared: &Prepared) -> Result<ParseOutcome> {
        let report = parse_json(&captured.stdout)?;
        Ok(ParseOutcome {
            report: super::AdapterReport::Tests(report),
            // stdout was the JSON payload; stderr was jest's human report.
            // Both consumed. v1 limitation: console.log output inside tests
            // is not forwarded (it lives inside the jest report).
            passthrough_stdout: None,
            passthrough_stderr: None,
        })
    }
}

#[derive(Deserialize)]
struct JestRoot {
    #[serde(rename = "numTotalTests")]
    total: u64,
    #[serde(rename = "numPassedTests")]
    passed: u64,
    #[serde(rename = "numFailedTests")]
    failed: u64,
    #[serde(rename = "numPendingTests", default)]
    pending: u64,
    #[serde(rename = "numTodoTests", default)]
    todo: u64,
    #[serde(rename = "startTime")]
    start_time: f64,
    #[serde(rename = "testResults")]
    files: Vec<JestFile>,
}

#[derive(Deserialize)]
struct JestFile {
    name: String,
    #[serde(rename = "endTime", default)]
    end_time: f64,
    #[serde(rename = "assertionResults")]
    asserts: Vec<JestAssert>,
}

#[derive(Deserialize)]
struct JestAssert {
    #[serde(rename = "fullName")]
    full_name: String,
    status: String,
    #[serde(rename = "failureMessages", default)]
    failure_messages: Vec<String>,
    #[serde(default)]
    location: Option<JestLoc>,
}

#[derive(Deserialize)]
struct JestLoc {
    line: u64,
}

pub fn parse_json(stdout: &str) -> Result<TestReport> {
    parse_json_named(stdout, "jest")
}

pub fn parse_json_named(stdout: &str, runner: &'static str) -> Result<TestReport> {
    let json_value =
        crate::fallback::detect_json(stdout).context("no JSON document in jest output")?;
    let root: JestRoot = serde_json::from_value(json_value).context("jest JSON shape mismatch")?;

    let end_max = root
        .files
        .iter()
        .map(|f| f.end_time)
        .fold(0.0_f64, f64::max);
    let duration_s = ((end_max - root.start_time) / 1000.0).max(0.0);

    let mut failures = Vec::new();
    for file in &root.files {
        for a in &file.asserts {
            if a.status != "failed" {
                continue;
            }
            let raw = a.failure_messages.join("\n");
            let clean = strip_ansi(&raw);
            let msg = clean.lines().next().unwrap_or("").to_string();
            let loc = match &a.location {
                Some(l) => format!("{}:{}", file.name, l.line),
                None => file.name.clone(),
            };
            failures.push(Failure {
                id: a.full_name.clone(),
                loc,
                msg,
                trace: trim_trace(&clean),
            });
        }
    }

    Ok(TestReport {
        runner,
        total: root.total,
        passed: root.passed,
        failed: root.failed,
        skipped: root.pending + root.todo,
        duration_s,
        failures,
    })
}

fn strip_ansi(s: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static ANSI: OnceLock<Regex> = OnceLock::new();
    ANSI.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap())
        .replace_all(s, "")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_fixture() -> crate::adapters::report::TestReport {
        let path = format!(
            "{}/tests/fixtures/jest/mixed.json",
            env!("CARGO_MANIFEST_DIR")
        );
        parse_json(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn prepare_appends_json_flags() {
        let p = Jest.prepare(vec!["jest".into(), "src/".into()]);
        assert_eq!(
            p.argv,
            vec!["jest", "src/", "--json", "--testLocationInResults"]
        );
    }

    #[test]
    fn parses_mixed_results() {
        let r = parse_fixture();
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (3, 1, 1, 1));
        assert!((r.duration_s - 1.3).abs() < 0.01, "got {}", r.duration_s);
        let f = &r.failures[0];
        assert_eq!(f.id, "auth refreshes expired token");
        assert_eq!(f.loc, "/home/user/proj/src/auth.test.js:42");
        assert_eq!(f.msg, "Error: expect(received).toBe(expected)");
        assert!(f.trace.iter().any(|l| l.contains("Expected: true")));
        // node internals dropped by trim_trace
        assert!(!f.trace.iter().any(|l| l.contains("task_queues")));
    }

    #[test]
    fn non_json_is_error() {
        assert!(parse_json("Tests: 1 failed").is_err());
    }
}
