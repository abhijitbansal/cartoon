//! `pre-commit` adapter — parses `pre-commit run` stdout (pre-commit prints
//! its report there, not stderr).
use super::report::{Failure, TestReport};
use super::{basename, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{bail, Result};
use regex::Regex;
use std::sync::OnceLock;

pub struct PreCommit;

impl Adapter for PreCommit {
    fn name(&self) -> &'static str {
        "pre-commit"
    }
    fn matches(&self) -> &'static str {
        "pre-commit [run …]"
    }
    fn detect(&self, argv: &[String]) -> bool {
        match argv.first() {
            Some(f) if basename(f) == "pre-commit" => {
                argv.len() == 1 || argv.get(1).map(String::as_str) == Some("run")
            }
            _ => false,
        }
    }
    fn prepare(&self, argv: Vec<String>) -> Prepared {
        let has_color = argv
            .iter()
            .any(|a| a == "--color" || a.starts_with("--color="));
        let mut argv = argv;
        if !has_color {
            argv.push("--color=never".to_string());
        }
        Prepared {
            argv,
            artifact: None,
        }
    }
    fn parse(&self, captured: &Captured, _prepared: &Prepared) -> Result<ParseOutcome> {
        let report = parse_text(&captured.stdout)?;
        // A nonzero exit with no failed hook (environment install failure,
        // `[ERROR] … InvalidConfigError`, `An unexpected error has occurred`
        // after some hooks already passed) must not read as a clean run:
        // keep the raw stdout so the real error reaches the agent.
        let unexplained = !captured.status.success() && report.failed == 0;
        let has_error_text = captured.stdout.contains("[ERROR]")
            || captured.stdout.contains("An unexpected error has occurred");
        Ok(ParseOutcome {
            report: super::AdapterReport::Tests(report),
            // stdout WAS the report — consumed, unless something went wrong
            // outside the hook status lines.
            passthrough_stdout: (unexplained || has_error_text).then(|| captured.stdout.clone()),
            passthrough_stderr: (!captured.stderr.is_empty()).then(|| captured.stderr.clone()),
        })
    }
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

/// `<hook name><dots (3+)><optional (detail)><Passed|Failed|Skipped>`, e.g.
/// `Ruff check...............................................Failed` or
/// `check xml............................................(no files to check)Skipped`.
fn status_line(line: &str) -> Option<(String, &'static str)> {
    static STATUS: OnceLock<Regex> = OnceLock::new();
    let caps = re(
        &STATUS,
        r"^(.+?)\.{3,}(?:\([^)]*\))?(Passed|Failed|Skipped)\s*$",
    )
    .captures(line.trim_end())?;
    let status = match &caps[2] {
        "Passed" => "Passed",
        "Failed" => "Failed",
        _ => "Skipped",
    };
    Some((caps[1].trim().to_string(), status))
}

/// Non-empty, trimmed lines — good enough for pre-commit's freeform hook
/// output (unlike `report::trim_trace`, which targets python/js tracebacks).
fn non_empty_trimmed(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

pub fn parse_text(stdout: &str) -> Result<TestReport> {
    let lines: Vec<&str> = stdout.lines().collect();
    let mut total = 0u64;
    let mut passed = 0u64;
    let mut failed = 0u64;
    let mut skipped = 0u64;
    let mut failures = Vec::new();
    let mut saw_status_line = false;

    let mut i = 0;
    while i < lines.len() {
        let Some((name, status)) = status_line(lines[i]) else {
            i += 1;
            continue;
        };
        saw_status_line = true;
        total += 1;
        i += 1;
        match status {
            "Passed" => passed += 1,
            "Skipped" => skipped += 1,
            _ => {
                failed += 1;
                let mut hook_id = String::new();
                let mut exit_code = String::new();
                let mut modified_files = false;
                let mut block: Vec<String> = Vec::new();
                while i < lines.len() && status_line(lines[i]).is_none() {
                    let t = lines[i].trim();
                    if let Some(id) = t.strip_prefix("- hook id:") {
                        hook_id = id.trim().to_string();
                    } else if let Some(code) = t.strip_prefix("- exit code:") {
                        exit_code = code.trim().to_string();
                    } else if t == "- files were modified by this hook" {
                        modified_files = true;
                    } else {
                        block.push(lines[i].to_string());
                    }
                    i += 1;
                }
                let output = non_empty_trimmed(&block);
                let (msg, trace) = match output.split_first() {
                    Some((first, rest)) => (first.clone(), rest.to_vec()),
                    None if modified_files => {
                        ("files were modified by this hook".to_string(), Vec::new())
                    }
                    None if !exit_code.is_empty() => (format!("exit code {exit_code}"), Vec::new()),
                    None => ("failed".to_string(), Vec::new()),
                };
                failures.push(Failure {
                    id: name,
                    loc: hook_id,
                    msg,
                    trace,
                });
            }
        }
    }

    if !saw_status_line {
        bail!("no pre-commit status line found — not pre-commit output");
    }

    Ok(TestReport {
        runner: "pre-commit",
        total,
        passed,
        failed,
        skipped,
        duration_s: 0.0,
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failing_status() -> std::process::ExitStatus {
        std::process::Command::new("false").status().unwrap()
    }

    #[test]
    fn nonzero_exit_with_no_failed_hook_passes_raw_stdout_through() {
        // Two hooks passed, then pre-commit itself died: the agent must see why.
        let stdout = "trim trailing whitespace.................................................Passed\nfix end of files.........................................................Passed\nAn unexpected error has occurred: CalledProcessError: command: ('/usr/bin/git', 'fetch')\nCheck the log at /home/u/.cache/pre-commit/pre-commit.log\n";
        let cap = Captured {
            stdout: stdout.to_string(),
            stderr: String::new(),
            status: failing_status(),
        };
        let out = PreCommit
            .parse(&cap, &PreCommit.prepare(vec!["pre-commit".into()]))
            .unwrap();
        assert!(
            out.passthrough_stdout
                .as_deref()
                .is_some_and(|s| s.contains("unexpected error")),
            "raw stdout kept on an unexplained failure"
        );
    }

    #[test]
    fn config_error_text_passes_through_even_with_failed_hooks() {
        let stdout = "[ERROR] Your pre-commit configuration is unstaged.\nRuff check...............................................................Failed\n- hook id: ruff\n- exit code: 1\n\nsrc/a.py:1:1: F401 unused import\n";
        let cap = Captured {
            stdout: stdout.to_string(),
            stderr: String::new(),
            status: failing_status(),
        };
        let out = PreCommit
            .parse(&cap, &PreCommit.prepare(vec!["pre-commit".into()]))
            .unwrap();
        assert!(out.passthrough_stdout.is_some());
    }

    #[test]
    fn modified_files_marker_becomes_the_message_when_the_hook_printed_nothing() {
        let stdout = "Ruff format..............................................................Failed\n- hook id: ruff-format\n- files were modified by this hook\n";
        let r = parse_text(stdout).unwrap();
        assert_eq!(r.failures[0].msg, "files were modified by this hook");
        let bare = "some hook.............................................................Failed\n- hook id: x\n";
        assert_eq!(parse_text(bare).unwrap().failures[0].msg, "failed");
    }

    const ALL_PASS: &str = "\
[INFO] Initializing environment for https://github.com/psf/black.
trim trailing whitespace.................................................Passed
fix end of files.........................................................Passed
check yaml................................................................Passed
check for added large files..............................................Passed
check for merge conflicts.................................................Passed
debug statements (python).................................................Passed
black.....................................................................Passed
check xml............................................(no files to check)Skipped
";

    const TWO_FAILURES: &str = "\
Ruff check...............................................................Failed
- hook id: ruff
- exit code: 1

src/a.py:10:5: F821 Undefined name `x`
Found 1 error.

Ruff format..............................................................Failed
- hook id: ruff-format
- files were modified by this hook

1 file reformatted, 3 files left unchanged

check yaml...............................................................Passed
";

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detects_bare_and_run_invocations() {
        assert!(PreCommit.detect(&argv(&["pre-commit"])));
        assert!(PreCommit.detect(&argv(&["pre-commit", "run"])));
        assert!(PreCommit.detect(&argv(&["pre-commit", "run", "--all-files"])));
    }

    #[test]
    fn does_not_detect_other_subcommands() {
        for sub in [
            "install",
            "uninstall",
            "autoupdate",
            "clean",
            "gc",
            "sample-config",
            "try-repo",
            "migrate-config",
            "init-templatedir",
            "--version",
            "--help",
        ] {
            assert!(
                !PreCommit.detect(&argv(&["pre-commit", sub])),
                "should not detect: {sub}"
            );
        }
    }

    #[test]
    fn prepare_appends_color_never() {
        let prepared = PreCommit.prepare(argv(&["pre-commit", "run"]));
        assert_eq!(prepared.argv, argv(&["pre-commit", "run", "--color=never"]));
    }

    #[test]
    fn prepare_respects_existing_color_flag() {
        let prepared = PreCommit.prepare(argv(&["pre-commit", "run", "--color", "always"]));
        assert_eq!(
            prepared.argv,
            argv(&["pre-commit", "run", "--color", "always"])
        );

        let prepared = PreCommit.prepare(argv(&["pre-commit", "run", "--color=auto"]));
        assert_eq!(prepared.argv, argv(&["pre-commit", "run", "--color=auto"]));
    }

    #[test]
    fn parses_all_pass_counts() {
        let r = parse_text(ALL_PASS).unwrap();
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (8, 7, 0, 1));
        assert!(r.failures.is_empty());
    }

    #[test]
    fn parses_failing_fixture() {
        let r = parse_text(TWO_FAILURES).unwrap();
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (3, 1, 2, 0));

        let ruff = &r.failures[0];
        assert_eq!(ruff.id, "Ruff check");
        assert_eq!(ruff.loc, "ruff");
        assert_eq!(ruff.msg, "src/a.py:10:5: F821 Undefined name `x`");
        assert_eq!(ruff.trace, vec!["Found 1 error.".to_string()]);
        assert!(!ruff.trace.iter().any(|l| l.starts_with("- hook id")));
        assert!(!ruff.trace.iter().any(|l| l.starts_with("- exit code")));

        let fmt = &r.failures[1];
        assert_eq!(fmt.id, "Ruff format");
        assert_eq!(fmt.loc, "ruff-format");
        assert_eq!(fmt.msg, "1 file reformatted, 3 files left unchanged");
        assert!(fmt.trace.is_empty());
    }

    #[test]
    fn garbage_stdout_is_error() {
        assert!(parse_text("random program output\nnothing dotted here\n").is_err());
    }
}
