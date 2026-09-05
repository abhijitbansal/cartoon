//! `rspec` adapter — appends `--format json --out <tmpfile>` for the machine
//! copy we parse; unless the user already picked their own `--format`, we
//! also inject `--format progress` so the run still prints a human report to
//! stdout (rspec supports stacking multiple formatters).
use super::report::{Failure, TestReport};
use super::{basename, Adapter, AdapterReport, Artifact, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use serde::Deserialize;

pub struct Rspec;

/// Tokens that make rspec informational (no example run, no JSON worth
/// injecting a formatter for). `--dry-run` is declined too: it emits JSON
/// that marks every example "passed" without actually running anything.
const INFO_FLAGS: &[&str] = &["--version", "-v", "--help", "-h", "--dry-run"];

impl Adapter for Rspec {
    fn name(&self) -> &'static str {
        "rspec"
    }
    fn matches(&self) -> &'static str {
        "rspec | bundle exec rspec (--format json --out <file>)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        is_rspec_invocation(argv)
            && !argv.iter().any(|a| INFO_FLAGS.contains(&a.as_str()))
            && !user_owns_output(argv)
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        let artifact = tempfile::Builder::new()
            .prefix("cartoon-rspec-")
            .suffix(".json")
            .tempfile()
            .ok();
        if let Some(f) = &artifact {
            // Keep the user's own formatter if they chose one; otherwise
            // give them the default progress bar back on stdout, since our
            // injected `--format json` alone would otherwise swallow it.
            if !has_user_format(&argv) {
                argv.push("--format".into());
                argv.push("progress".into());
            }
            argv.push("--format".into());
            argv.push("json".into());
            argv.push("--out".into());
            argv.push(f.path().display().to_string());
        }
        Prepared {
            argv,
            artifact: artifact.map(Artifact::File),
        }
    }
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        let path = prepared
            .artifact_path()
            .context("rspec adapter has no json artifact")?;
        let raw = std::fs::read_to_string(&path).context("rspec json artifact missing")?;
        if raw.trim().is_empty() {
            anyhow::bail!("rspec json artifact is empty");
        }
        let report = parse_json(&raw)?;
        // Nonzero exit with an empty failures list (load error, errors
        // outside of examples we couldn't attribute a message to,
        // `--fail-if-no-examples`, ...): the report doesn't explain the
        // exit code, so keep the human output instead of discarding it.
        let unexplained = !captured.status.success() && report.failures.is_empty();
        Ok(ParseOutcome {
            report: AdapterReport::Tests(report),
            passthrough_stdout: (unexplained && !captured.stdout.is_empty())
                .then(|| captured.stdout.clone()),
            passthrough_stderr: (!captured.stderr.is_empty()).then(|| captured.stderr.clone()),
        })
    }
}

fn is_rspec_invocation(argv: &[String]) -> bool {
    match argv {
        [first, ..] if basename(first) == "rspec" => true,
        [first, second, third, ..]
            if basename(first) == "bundle" && second == "exec" && basename(third) == "rspec" =>
        {
            true
        }
        _ => false,
    }
}

/// Extracts the user's `--format`/`-f` value (lowercased) if present, in any
/// of rspec's accepted forms: separate token, `--format=value`, or `-f`
/// glued directly to its value (`-fjson`, `-fj`).
fn user_format_value(argv: &[String]) -> Option<String> {
    let mut iter = argv.iter().peekable();
    while let Some(a) = iter.next() {
        if a == "--format" || a == "-f" {
            let v = iter.next()?;
            return Some(v.to_ascii_lowercase());
        }
        if let Some(rest) = a.strip_prefix("--format=") {
            return Some(rest.to_ascii_lowercase());
        }
        if let Some(rest) = a.strip_prefix("-f") {
            if !rest.is_empty() {
                return Some(rest.to_ascii_lowercase());
            }
        }
    }
    None
}

fn has_user_format(argv: &[String]) -> bool {
    user_format_value(argv).is_some()
}

/// The user already owns the machine-readable output (`--format`/`-f json`,
/// including its `j` alias and any glued/`=` spelling, or `--out`/`-o`) —
/// don't inject a second copy.
fn user_owns_output(argv: &[String]) -> bool {
    let owns_out = argv
        .iter()
        .any(|a| a == "--out" || a == "-o" || a.starts_with("--out="));
    owns_out || matches!(user_format_value(argv).as_deref(), Some("j") | Some("json"))
}

#[derive(Deserialize)]
struct RspecRoot {
    examples: Vec<RspecExample>,
    summary: RspecSummary,
    /// Text of errors raised outside any example (e.g. a `spec_helper.rb`
    /// load failure) — this is where `errors_outside_of_examples_count`
    /// actually gets explained.
    #[serde(default)]
    messages: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct RspecExample {
    full_description: String,
    status: String,
    file_path: String,
    line_number: i64,
    #[serde(default)]
    exception: Option<RspecException>,
}

#[derive(Deserialize)]
struct RspecException {
    class: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    backtrace: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct RspecSummary {
    duration: f64,
    example_count: u64,
    failure_count: u64,
    pending_count: u64,
    #[serde(default)]
    errors_outside_of_examples_count: u64,
}

/// Backtrace frames that are rspec/rubygems internals rather than user code.
fn is_framework_frame(line: &str) -> bool {
    line.contains("/gems/") || line.contains("/rspec-core/") || line.contains("bin/rspec")
}

/// First non-empty trimmed line, plus the trimmed non-empty lines after it.
fn split_first_line(raw: &str) -> (String, Vec<String>) {
    let mut lines = raw.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or("").to_string();
    (first, lines.map(str::to_string).collect())
}

pub fn parse_json(raw: &str) -> Result<TestReport> {
    let root: RspecRoot = serde_json::from_str(raw).context("rspec JSON shape mismatch")?;

    let total = root.summary.example_count;
    // Errors outside of examples count toward `failed` (rspec's own exit
    // code treats them as failures) but are not part of `example_count`,
    // so they must not be subtracted out of `passed`.
    let failed = root
        .summary
        .failure_count
        .saturating_add(root.summary.errors_outside_of_examples_count);
    let skipped = root.summary.pending_count;
    let passed = total
        .saturating_sub(root.summary.failure_count)
        .saturating_sub(skipped);

    let mut failures = Vec::new();
    for ex in root.examples.iter().filter(|e| e.status == "failed") {
        let file = ex.file_path.strip_prefix("./").unwrap_or(&ex.file_path);
        let loc = format!("{file}:{}", ex.line_number);
        let (msg, trace) = match &ex.exception {
            Some(exc) => {
                let (first_line, _) = split_first_line(exc.message.as_deref().unwrap_or(""));
                let msg = format!("{}: {first_line}", exc.class);
                let trace: Vec<String> = exc
                    .backtrace
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .filter(|l| !is_framework_frame(l))
                    .map(|l| l.trim().to_string())
                    .collect();
                (msg, trace)
            }
            None => (String::new(), Vec::new()),
        };
        failures.push(Failure {
            id: ex.full_description.clone(),
            loc,
            msg,
            trace,
        });
    }

    // Give each error outside of an example its own Failure entry so it
    // actually reaches the agent instead of hiding behind a bare count.
    if let Some(messages) = &root.messages {
        let multiple = messages.len() > 1;
        for (i, raw_msg) in messages.iter().enumerate() {
            let (msg, trace) = split_first_line(raw_msg);
            let id = if multiple {
                format!("errors outside of examples #{}", i + 1)
            } else {
                "errors outside of examples".to_string()
            };
            failures.push(Failure {
                id,
                loc: String::new(),
                msg,
                trace,
            });
        }
    }

    Ok(TestReport {
        runner: "rspec",
        total,
        passed,
        failed,
        skipped,
        duration_s: root.summary.duration,
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_PASS_JSON: &str = r#"{
        "version": "3.13.0",
        "seed": 1234,
        "examples": [
            {"id": "./spec/calc_spec.rb[1:1]", "description": "adds", "full_description": "Calc adds", "status": "passed", "file_path": "./spec/calc_spec.rb", "line_number": 5, "run_time": 0.001, "pending_message": null},
            {"id": "./spec/calc_spec.rb[1:2]", "description": "subtracts", "full_description": "Calc subtracts", "status": "passed", "file_path": "./spec/calc_spec.rb", "line_number": 9, "run_time": 0.001, "pending_message": null},
            {"id": "./spec/calc_spec.rb[1:3]", "description": "multiplies", "full_description": "Calc multiplies", "status": "passed", "file_path": "./spec/calc_spec.rb", "line_number": 13, "run_time": 0.001, "pending_message": null}
        ],
        "summary": {"duration": 0.0123, "example_count": 3, "failure_count": 0, "pending_count": 0, "errors_outside_of_examples_count": 0},
        "summary_line": "3 examples, 0 failures"
    }"#;

    const MIXED_JSON: &str = r#"{
        "version": "3.13.0",
        "seed": 1234,
        "examples": [
            {"id": "./spec/calc_spec.rb[1:1]", "description": "adds", "full_description": "Calc adds", "status": "failed", "file_path": "./spec/calc_spec.rb", "line_number": 5, "run_time": 0.002, "pending_message": null, "exception": {"class": "RSpec::Expectations::ExpectationNotMetError", "message": "\nexpected: 3\n     got: 4\n", "backtrace": ["./spec/calc_spec.rb:6:in `block (2 levels)'", "/gems/rspec-core-3.13.0/lib/rspec/core/example.rb:263:in `instance_exec'"]}},
            {"id": "./spec/calc_spec.rb[1:2]", "description": "divides", "full_description": "Calc divides", "status": "pending", "file_path": "./spec/calc_spec.rb", "line_number": 9, "run_time": 0.001, "pending_message": "not implemented yet"},
            {"id": "./spec/calc_spec.rb[1:3]", "description": "multiplies", "full_description": "Calc multiplies", "status": "passed", "file_path": "./spec/calc_spec.rb", "line_number": 13, "run_time": 0.001, "pending_message": null}
        ],
        "summary": {"duration": 0.0123, "example_count": 3, "failure_count": 1, "pending_count": 1, "errors_outside_of_examples_count": 0},
        "summary_line": "3 examples, 1 failure, 1 pending"
    }"#;

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn status_fail() -> std::process::ExitStatus {
        std::process::Command::new("false").status().unwrap()
    }

    #[test]
    fn detects_bare_and_wrapped_invocations() {
        assert!(Rspec.detect(&argv(&["rspec"])));
        assert!(Rspec.detect(&argv(&["bundle", "exec", "rspec", "spec/"])));
        assert!(Rspec.detect(&argv(&["bin/rspec", "--seed", "1"])));
    }

    #[test]
    fn skips_invocations_that_own_their_output() {
        assert!(!Rspec.detect(&argv(&["rspec", "--format", "json"])));
        assert!(!Rspec.detect(&argv(&["rspec", "--out", "r.json"])));
        assert!(!Rspec.detect(&argv(&["bundle", "exec", "rake"])));
        assert!(!Rspec.detect(&argv(&["rspec", "--version"])));
        assert!(!Rspec.detect(&argv(&["rspec", "--dry-run"])));
        // format aliases: glued short flag, "j" alias, "=" form, mixed case.
        assert!(!Rspec.detect(&argv(&["rspec", "-fj"])));
        assert!(!Rspec.detect(&argv(&["rspec", "-fjson"])));
        assert!(!Rspec.detect(&argv(&["rspec", "-f", "j"])));
        assert!(!Rspec.detect(&argv(&["rspec", "-fJSON"])));
        assert!(!Rspec.detect(&argv(&["rspec", "--format=json"])));
    }

    #[test]
    fn prepare_appends_progress_and_json_formatters() {
        let p = Rspec.prepare(argv(&["rspec", "spec/"]));
        let path = p.artifact_path().unwrap().display().to_string();
        assert_eq!(
            p.argv,
            vec!["rspec", "spec/", "--format", "progress", "--format", "json", "--out", &path]
        );
        assert!(p.artifact.is_some());
    }

    #[test]
    fn prepare_keeps_users_own_formatter_instead_of_progress() {
        let p = Rspec.prepare(argv(&["rspec", "--format", "documentation"]));
        let path = p.artifact_path().unwrap().display().to_string();
        assert_eq!(
            p.argv,
            vec![
                "rspec",
                "--format",
                "documentation",
                "--format",
                "json",
                "--out",
                &path
            ]
        );
    }

    #[test]
    fn parses_all_pass_counts_and_duration() {
        let r = parse_json(ALL_PASS_JSON).unwrap();
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (3, 3, 0, 0));
        assert!((r.duration_s - 0.0123).abs() < 1e-9);
        assert!(r.failures.is_empty());
    }

    #[test]
    fn parses_mixed_results() {
        let r = parse_json(MIXED_JSON).unwrap();
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (3, 1, 1, 1));
        let f = &r.failures[0];
        assert_eq!(f.id, "Calc adds");
        assert_eq!(f.loc, "spec/calc_spec.rb:5");
        assert!(
            f.msg
                .starts_with("RSpec::Expectations::ExpectationNotMetError: expected: 3"),
            "got: {}",
            f.msg
        );
        assert!(f.trace.iter().any(|l| l.contains("block (2 levels)")));
        assert!(!f.trace.iter().any(|l| l.contains("/gems/")));
    }

    #[test]
    fn null_exception_message_and_backtrace_parse_without_error() {
        let json = r#"{
            "examples": [
                {"full_description": "Calc adds", "status": "failed", "file_path": "./spec/calc_spec.rb", "line_number": 5, "exception": {"class": "RuntimeError", "message": null, "backtrace": null}}
            ],
            "summary": {"duration": 0.01, "example_count": 1, "failure_count": 1, "pending_count": 0, "errors_outside_of_examples_count": 0}
        }"#;
        let r = parse_json(json).unwrap();
        assert_eq!(r.failures.len(), 1);
        assert_eq!(r.failures[0].msg, "RuntimeError: ");
        assert!(r.failures[0].trace.is_empty());
    }

    #[test]
    fn pending_example_with_exception_is_not_reported_as_failure() {
        let json = r#"{
            "examples": [
                {"full_description": "Calc is pending but ran", "status": "pending", "file_path": "./spec/calc_spec.rb", "line_number": 9, "exception": {"class": "RSpec::Expectations::ExpectationNotMetError", "message": "expected: 1\n     got: 2", "backtrace": []}}
            ],
            "summary": {"duration": 0.01, "example_count": 1, "failure_count": 0, "pending_count": 1, "errors_outside_of_examples_count": 0}
        }"#;
        let r = parse_json(json).unwrap();
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (1, 0, 0, 1));
        assert!(r.failures.is_empty(), "pending exceptions are not failures");
    }

    #[test]
    fn outside_errors_produce_failure_entries_and_are_counted() {
        let json = r#"{
            "examples": [],
            "summary": {"duration": 0.001, "example_count": 0, "failure_count": 0, "pending_count": 0, "errors_outside_of_examples_count": 1},
            "messages": ["An error occurred while loading ./spec/calc_spec.rb.\nNameError: uninitialized constant Calc\n"]
        }"#;
        let r = parse_json(json).unwrap();
        assert_eq!(r.failed, 1);
        assert_eq!(r.failures.len(), 1);
        assert_eq!(r.failures[0].id, "errors outside of examples");
        assert_eq!(
            r.failures[0].msg,
            "An error occurred while loading ./spec/calc_spec.rb."
        );
        assert!(r.failures[0]
            .trace
            .iter()
            .any(|l| l.contains("NameError: uninitialized constant Calc")));
    }

    #[test]
    fn empty_artifact_is_parse_error() {
        let prepared = Rspec.prepare(argv(&["rspec"]));
        let captured = Captured::synthetic(String::new(), String::new());
        assert!(Rspec.parse(&captured, &prepared).is_err());
    }

    #[test]
    fn nonzero_exit_with_explained_failures_suppresses_stdout() {
        let prepared = Rspec.prepare(vec!["rspec".into()]);
        std::fs::write(prepared.artifact_path().unwrap(), MIXED_JSON).unwrap();
        let cap = Captured {
            stdout: "progress output the agent doesn't need".into(),
            stderr: String::new(),
            status: status_fail(),
        };
        let out = Rspec.parse(&cap, &prepared).unwrap();
        assert!(
            out.passthrough_stdout.is_none(),
            "failures list already explains the exit code"
        );
    }

    #[test]
    fn nonzero_exit_with_no_failures_passes_through_stdout() {
        let prepared = Rspec.prepare(vec!["rspec".into()]);
        std::fs::write(
            prepared.artifact_path().unwrap(),
            r#"{"examples":[],"summary":{"duration":0.001,"example_count":0,"failure_count":0,"pending_count":0,"errors_outside_of_examples_count":0}}"#,
        )
        .unwrap();
        let cap = Captured {
            stdout: "An error occurred while loading ./spec/calc_spec.rb.\nNameError: uninitialized constant Calc\n".into(),
            stderr: String::new(),
            status: status_fail(),
        };
        let out = Rspec.parse(&cap, &prepared).unwrap();
        assert!(
            out.passthrough_stdout.is_some(),
            "no failures explain the exit code, so stdout must survive"
        );
    }
}
