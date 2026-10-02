use super::report::{trim_trace, Failure, TestReport};
use super::{basename, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::sync::OnceLock;

pub struct Jest;

/// Id of the synthetic failure for a run the runner itself marks
/// unsuccessful (`success: false`) with no failed test or suite to show.
pub const RUN_FAILED_ID: &str = "(run)";

/// Console lines attached to one failing test, at most.
const MAX_CONSOLE_LINES: usize = 10;

impl Adapter for Jest {
    fn name(&self) -> &'static str {
        "jest"
    }
    fn matches(&self) -> &'static str {
        "jest | npx jest | bunx jest (not --watch/--watchAll)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        // Watch runs are long-lived and interactive; never capture them.
        if argv.iter().any(|a| is_watch_flag(a)) {
            return false;
        }
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
        let consoles = jest_console_entries(&captured.stderr);
        let mut report = parse_json_with(&captured.stdout, "jest", &consoles)?;
        // `success: false` with nothing failed (coverage threshold, open
        // handles under --detectOpenHandles, obsolete snapshots under --ci):
        // name jest's own reason and keep its human report.
        let unexplained = explain_run_failure(&mut report, &captured.stderr, "Jest: ");
        Ok(ParseOutcome {
            report: super::AdapterReport::Tests(report),
            // stdout was the JSON payload; stderr was jest's human report,
            // consumed unless the failure is otherwise unexplained. Console
            // output of failing tests is attached to their traces.
            passthrough_stdout: None,
            passthrough_stderr: (unexplained && !captured.stderr.is_empty())
                .then(|| captured.stderr.clone()),
        })
    }
}

fn is_watch_flag(a: &str) -> bool {
    let name = a.split('=').next().unwrap_or(a);
    matches!(name, "--watch" | "--watchAll")
}

/// Refine the synthetic `RUN_FAILED_ID` failure's message with the first
/// stderr line starting with `prefix` (jest: `Jest: "global" coverage
/// threshold … not met`). True when that synthetic failure is present.
pub fn explain_run_failure(report: &mut TestReport, stderr: &str, prefix: &str) -> bool {
    let Some(f) = report.failures.iter_mut().find(|f| f.id == RUN_FAILED_ID) else {
        return false;
    };
    if let Some(line) = stderr
        .lines()
        .map(|l| strip_ansi(l.trim()))
        .find(|l| l.starts_with(prefix))
    {
        f.msg = line;
    }
    true
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
    /// Suites that crashed before or outside any test (jest only).
    #[serde(rename = "numRuntimeErrorTestSuites", default)]
    runtime_error_suites: u64,
    #[serde(default)]
    success: Option<bool>,
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
    #[serde(default)]
    status: String,
    /// The suite-level failure text (missing module, syntax error, a
    /// throwing hook): the only place a suite that failed to run says why.
    #[serde(default)]
    message: String,
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

/// Console output captured from a test file's human report, to attach to
/// the failing test that printed it. `file` is as the runner printed it
/// (usually cwd-relative); the owning test is found by `test` name path
/// (vitest: `describe > name`) or by the source `line` of the call (jest).
pub struct ConsoleEntry {
    pub file: String,
    pub test: Option<String>,
    pub line: Option<u64>,
    pub lines: Vec<String>,
}

pub fn parse_json(stdout: &str) -> Result<TestReport> {
    parse_json_named(stdout, "jest")
}

pub fn parse_json_named(stdout: &str, runner: &'static str) -> Result<TestReport> {
    parse_json_with(stdout, runner, &[])
}

pub fn parse_json_with(
    stdout: &str,
    runner: &'static str,
    consoles: &[ConsoleEntry],
) -> Result<TestReport> {
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
    let mut suite_failures = 0u64;
    for file in &root.files {
        let mut any_failed = false;
        for a in &file.asserts {
            if a.status != "failed" {
                continue;
            }
            any_failed = true;
            let raw = a.failure_messages.join("\n");
            let clean = strip_ansi(&raw);
            let msg = clean.lines().next().unwrap_or("").to_string();
            let loc = match &a.location {
                Some(l) => format!("{}:{}", file.name, l.line),
                None => file.name.clone(),
            };
            let mut trace = trim_trace(&clean);
            // The first line is `msg`: don't print it twice.
            if trace.first() == Some(&msg) {
                trace.remove(0);
            }
            trace.extend(console_for(file, a, consoles));
            failures.push(Failure {
                id: a.full_name.clone(),
                loc,
                msg,
                trace,
            });
        }
        // A suite that failed with no failed test never ran its tests
        // (missing module, syntax error, a throwing hook): its `message` is
        // the only record of why, and it counts as a failure.
        if file.status == "failed" && !any_failed {
            suite_failures += 1;
            failures.push(suite_failure(file));
        }
    }
    let mut failed = root.failed + suite_failures;
    // jest counts suites that crashed at runtime separately; never report
    // fewer than it did, even when their testResults entry is missing.
    if root.runtime_error_suites > suite_failures {
        let missing = root.runtime_error_suites - suite_failures;
        failed += missing;
        failures.push(Failure {
            id: "(suites)".into(),
            loc: String::new(),
            msg: format!("{missing} test suite(s) failed to run (see raw_log)"),
            trace: Vec::new(),
        });
    }
    // The runner says the run failed but nothing above shows why: never
    // let that look like a clean pass.
    if root.success == Some(false) && failures.is_empty() {
        failed += 1;
        failures.push(Failure {
            id: RUN_FAILED_ID.into(),
            loc: String::new(),
            msg: format!("{runner} reported success: false with no failed test (see raw_log)"),
            trace: Vec::new(),
        });
    }

    Ok(TestReport {
        runner,
        total: root.total,
        passed: root.passed,
        failed,
        skipped: root.pending + root.todo,
        duration_s,
        failures,
    })
}

fn suite_failure(file: &JestFile) -> Failure {
    let clean = strip_ansi(&file.message);
    // jest leads with `● Test suite failed to run`; the first line after
    // it is the actual error.
    let msg = clean
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('\u{25cf}'))
        .unwrap_or("test suite failed to run")
        .to_string();
    let trace: Vec<String> = trim_trace(&clean)
        .into_iter()
        .filter(|l| !l.starts_with('\u{25cf}') && *l != msg)
        .collect();
    Failure {
        id: file.name.clone(),
        loc: String::new(),
        msg,
        trace,
    }
}

/// True when the runner-printed `printed` path names the JSON `name`
/// (absolute) file.
fn same_file(name: &str, printed: &str) -> bool {
    name == printed || name.ends_with(&format!("/{}", printed.trim_start_matches("./")))
}

/// Console lines printed by test `a` of `file`, bounded.
fn console_for(file: &JestFile, a: &JestAssert, consoles: &[ConsoleEntry]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in consoles.iter().filter(|c| same_file(&file.name, &c.file)) {
        let owner = if let Some(t) = &c.test {
            file.asserts
                .iter()
                .find(|x| x.full_name == *t || x.full_name == t.replace(" > ", " "))
        } else if let Some(line) = c.line {
            // The test whose declaration is the closest one above the
            // console call.
            file.asserts
                .iter()
                .filter(|x| x.location.as_ref().is_some_and(|l| l.line <= line))
                .max_by_key(|x| x.location.as_ref().map(|l| l.line))
        } else {
            None
        };
        if owner.is_some_and(|o| std::ptr::eq(o, a)) {
            out.extend(c.lines.iter().cloned());
        }
    }
    if out.len() > MAX_CONSOLE_LINES {
        let extra = out.len() - MAX_CONSOLE_LINES;
        out.truncate(MAX_CONSOLE_LINES);
        out.push(format!(
            "\u{2026} +{extra} console lines omitted (see raw_log)"
        ));
    }
    out
}

/// Console blocks from jest's human report on stderr:
///
/// ```text
/// FAIL src/ok.test.js
///   ● Console
///
///     console.log
///       debug value 42
///
///       at Object.log (src/ok.test.js:2:31)
/// ```
pub fn jest_console_entries(stderr: &str) -> Vec<ConsoleEntry> {
    static FILE: OnceLock<Regex> = OnceLock::new();
    static AT: OnceLock<Regex> = OnceLock::new();
    let file_re = FILE.get_or_init(|| Regex::new(r"^(?:PASS|FAIL) +(\S+)").unwrap());
    let at_re = AT.get_or_init(|| Regex::new(r"^at .*?\(?([^\s()]+):(\d+):\d+\)?$").unwrap());
    let mut entries: Vec<ConsoleEntry> = Vec::new();
    let mut file = String::new();
    let mut in_console = false;
    for raw in stderr.lines() {
        let line = strip_ansi(raw);
        if let Some(c) = file_re.captures(&line) {
            file = c[1].to_string();
            in_console = false;
            continue;
        }
        let t = line.trim();
        if t.starts_with('\u{25cf}') {
            in_console = t == "\u{25cf} Console";
            continue;
        }
        // A single-file run prints its console blocks bare, before the
        // file's FAIL/PASS line; the `at` line names the file then.
        let before_any_file = file.is_empty();
        if !(in_console || before_any_file) || t.is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent <= 4 && t.starts_with("console.") {
            entries.push(ConsoleEntry {
                file: file.clone(),
                test: None,
                line: None,
                lines: Vec::new(),
            });
            continue;
        }
        let Some(e) = entries.last_mut() else {
            continue;
        };
        if let Some(c) = at_re.captures(t) {
            if e.file.is_empty() {
                e.file = c[1].to_string();
            }
            if e.line.is_none() && (c[1] == e.file || same_file(&c[1], &e.file)) {
                e.line = c[2].parse().ok();
            }
        } else {
            e.lines.push(format!("console: {t}"));
        }
    }
    entries
}

pub fn strip_ansi(s: &str) -> String {
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

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/jest/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn captured(stdout: String, stderr: String, ok: bool) -> Captured {
        let status = std::process::Command::new(if ok { "true" } else { "false" })
            .status()
            .unwrap();
        Captured {
            stdout,
            stderr,
            status,
        }
    }

    fn report_of(out: ParseOutcome) -> TestReport {
        match out.report {
            super::super::AdapterReport::Tests(r) => r,
            _ => panic!("expected a test report"),
        }
    }

    #[test]
    fn suites_that_failed_to_run_are_failures() {
        // Real jest 30: one failed assertion, a missing module and a syntax
        // error (the last two never ran a test).
        let r = parse_json(&fixture("suite-errors.json")).unwrap();
        assert_eq!((r.total, r.passed, r.failed), (2, 1, 3));
        let missing = r
            .failures
            .iter()
            .find(|f| f.id.ends_with("src/missing.test.js"))
            .expect("missing-module suite reported");
        assert_eq!(
            missing.msg,
            "Cannot find module './does-not-exist' from 'src/missing.test.js'"
        );
        assert!(
            missing.trace.iter().any(|l| l.contains("> 1 | const x")),
            "{:?}",
            missing.trace
        );
        assert!(!missing
            .trace
            .iter()
            .any(|l| l.contains("Test suite failed")));
        let syntax = r
            .failures
            .iter()
            .find(|f| f.id.ends_with("src/syntax.test.js"))
            .expect("syntax-error suite reported");
        assert!(syntax.msg.starts_with("SyntaxError: "), "{}", syntax.msg);
    }

    #[test]
    fn failing_test_gets_its_console_output() {
        let out = Jest
            .parse(
                &captured(
                    fixture("suite-errors.json"),
                    fixture("suite-errors.stderr.txt"),
                    false,
                ),
                &Jest.prepare(vec!["jest".into()]),
            )
            .unwrap();
        let r = report_of(out);
        let f = r.failures.iter().find(|f| f.id == "fails").unwrap();
        assert!(
            f.trace.contains(&"console: debug value 42".to_string()),
            "{:?}",
            f.trace
        );
    }

    #[test]
    fn console_of_a_passing_test_is_not_attached() {
        let stderr = "FAIL src/a.test.js\n  \u{25cf} Console\n\n    console.log\n      from the passing test\n\n      at Object.log (src/a.test.js:1:30)\n\n  \u{25cf} b\n";
        let entries = jest_console_entries(stderr);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].line, Some(1));
        let json = r#"{"numTotalTests":2,"numPassedTests":1,"numFailedTests":1,"startTime":0,"testResults":[{"name":"/p/src/a.test.js","status":"failed","message":"","assertionResults":[{"fullName":"a","status":"passed","location":{"line":1}},{"fullName":"b","status":"failed","location":{"line":5},"failureMessages":["Error: boom"]}]}]}"#;
        let r = parse_json_with(json, "jest", &entries).unwrap();
        assert!(
            !r.failures[0].trace.iter().any(|l| l.contains("passing")),
            "{:?}",
            r.failures[0].trace
        );
    }

    #[test]
    fn single_file_console_block_names_its_file_from_the_at_line() {
        let stderr = "  console.log\n    debug value 42\n\n      at Object.log (src/ok.test.js:2:31)\n\nFAIL src/ok.test.js\n";
        let e = jest_console_entries(stderr);
        assert_eq!(e.len(), 1);
        assert_eq!((e[0].file.as_str(), e[0].line), ("src/ok.test.js", Some(2)));
        assert_eq!(e[0].lines, vec!["console: debug value 42"]);
    }

    #[test]
    fn success_false_with_nothing_failed_is_not_clean() {
        // Real jest 30 with an unmet coverageThreshold: every test passed.
        let out = Jest
            .parse(
                &captured(
                    fixture("coverage-threshold.json"),
                    fixture("coverage-threshold.stderr.txt"),
                    false,
                ),
                &Jest.prepare(vec!["jest".into()]),
            )
            .unwrap();
        assert!(out.passthrough_stderr.is_some());
        let r = report_of(out);
        assert_eq!(r.failed, 1);
        assert_eq!(r.failures[0].id, RUN_FAILED_ID);
        assert!(
            r.failures[0].msg.starts_with("Jest: Coverage for lines"),
            "{}",
            r.failures[0].msg
        );
    }

    #[test]
    fn runtime_error_suites_are_never_undercounted() {
        let json = r#"{"numTotalTests":0,"numPassedTests":0,"numFailedTests":0,"numRuntimeErrorTestSuites":2,"startTime":0,"testResults":[]}"#;
        let r = parse_json(json).unwrap();
        assert_eq!(r.failed, 2);
        assert!(r.failures[0].msg.starts_with("2 test suite(s) failed"));
    }

    #[test]
    fn watch_modes_are_not_detected() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(!Jest.detect(&a(&["jest", "--watch"])));
        assert!(!Jest.detect(&a(&["npx", "jest", "--watchAll"])));
        assert!(!Jest.detect(&a(&["jest", "--watchAll=true"])));
        assert!(Jest.detect(&a(&["jest", "--ci"])));
    }

    #[test]
    fn non_json_is_error() {
        assert!(parse_json("Tests: 1 failed").is_err());
    }
}
