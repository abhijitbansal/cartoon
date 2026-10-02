use super::jest::{self, ConsoleEntry};
use super::report::Failure;
use super::{basename, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use regex::Regex;
use std::path::PathBuf;
use std::sync::OnceLock;

pub struct Vitest;

impl Adapter for Vitest {
    fn name(&self) -> &'static str {
        "vitest"
    }
    fn matches(&self) -> &'static str {
        "vitest run | npx vitest run | bunx vitest run"
    }
    fn detect(&self, argv: &[String]) -> bool {
        // Bare `vitest` is watch mode; only `vitest run` is a one-shot batch.
        let rest = match argv {
            [first, rest @ ..] if basename(first) == "vitest" => rest,
            [first, second, rest @ ..]
                if matches!(basename(first), "npx" | "bunx" | "pnpx")
                    && basename(second) == "vitest" =>
            {
                rest
            }
            _ => return false,
        };
        rest.iter().any(|a| a == "run") && !rest.iter().any(|a| a == "--watch" || a == "-w")
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        if user_reporting(&argv) {
            // The user chose their own reporter/output file: leave argv
            // alone; parse() reads their JSON file, or stdout.
            return Prepared {
                argv,
                artifact: None,
            };
        }
        // vitest >= 4 writes `--reporter=json` output to a file under the
        // project (.vitest/json/output.json) instead of stdout, so always
        // name the file: a temp file, never one in the user's repo. The
        // default reporter keeps vitest's human report (and console output)
        // on stdout/stderr. Works on vitest 3 and 5.
        let artifact = tempfile::Builder::new()
            .prefix("cartoon-vitest-")
            .suffix(".json")
            .tempfile()
            .ok();
        if let Some(f) = &artifact {
            let insert_at = argv.iter().position(|a| a == "--").unwrap_or(argv.len());
            for flag in [
                format!("--outputFile.json={}", f.path().display()),
                "--reporter=json".into(),
                "--reporter=default".into(),
            ] {
                argv.insert(insert_at, flag);
            }
        } else {
            argv.push("--reporter=json".into());
        }
        Prepared {
            argv,
            artifact: artifact.map(super::Artifact::File),
        }
    }
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        let file = prepared
            .artifact_path()
            .or_else(|| user_json_output(&prepared.argv));
        let json = match &file {
            Some(p) => std::fs::read_to_string(p).context("vitest JSON report missing")?,
            None => captured.stdout.clone(),
        };
        // With the default reporter on stdout, console output of each test
        // is printed there as `stdout | file > test` blocks.
        let consoles = if file.is_some() {
            vitest_console_entries(&captured.stdout)
        } else {
            Vec::new()
        };
        let mut report = jest::parse_json_with(&json, "vitest", &consoles)?;
        // Errors thrown outside any test (a leaked timer, an unhandled
        // rejection) fail the run but are absent from the JSON report.
        let unhandled = unhandled_errors(&captured.stderr);
        if !unhandled.is_empty() {
            // They explain a `success: false`: drop the generic placeholder.
            let before = report.failures.len();
            report.failures.retain(|f| f.id != jest::RUN_FAILED_ID);
            let dropped = (before - report.failures.len()) as u64;
            report.failed = report.failed.saturating_sub(dropped);
            report.failed += unhandled.len() as u64;
            report.failures.extend(unhandled);
        }
        let unexplained = jest::explain_run_failure(&mut report, &captured.stderr, "ERROR: ");
        Ok(ParseOutcome {
            report: super::AdapterReport::Tests(report),
            // stdout was the JSON payload (or, with our injected file
            // reporter, vitest's human summary); stderr was its failure
            // detail. Both consumed unless the failure is otherwise
            // unexplained.
            passthrough_stdout: (unexplained && file.is_some() && !captured.stdout.is_empty())
                .then(|| captured.stdout.clone()),
            passthrough_stderr: (unexplained && !captured.stderr.is_empty())
                .then(|| captured.stderr.clone()),
        })
    }
}

/// A user-supplied `--reporter` or `--outputFile` (any form).
fn user_reporting(argv: &[String]) -> bool {
    argv.iter().any(|a| {
        let name = a.split('=').next().unwrap_or(a);
        name == "--reporter" || name == "--outputFile" || name.starts_with("--outputFile.")
    })
}

/// The JSON report path from a user's `--outputFile=<p>`,
/// `--outputFile.json=<p>` or the two-token forms.
fn user_json_output(argv: &[String]) -> Option<PathBuf> {
    for (i, a) in argv.iter().enumerate() {
        for flag in ["--outputFile.json", "--outputFile"] {
            if let Some(v) = a.strip_prefix(&format!("{flag}=")) {
                return Some(PathBuf::from(v));
            }
            if a == flag {
                return argv.get(i + 1).map(PathBuf::from);
            }
        }
    }
    None
}

/// `stdout | src/ok.test.js > suite > test` blocks from vitest's default
/// reporter, up to the next blank line.
fn vitest_console_entries(stdout: &str) -> Vec<ConsoleEntry> {
    static HEAD: OnceLock<Regex> = OnceLock::new();
    let head = HEAD.get_or_init(|| Regex::new(r"^(stdout|stderr) \| (\S+)(?: > (.+))?$").unwrap());
    let mut entries: Vec<ConsoleEntry> = Vec::new();
    let mut open = false;
    for raw in stdout.lines() {
        let line = jest::strip_ansi(raw);
        if let Some(c) = head.captures(line.trim_end()) {
            entries.push(ConsoleEntry {
                file: c[2].to_string(),
                test: c.get(3).map(|m| m.as_str().to_string()),
                line: None,
                lines: Vec::new(),
            });
            open = true;
            continue;
        }
        if line.trim().is_empty() {
            open = false;
            continue;
        }
        if open {
            if let Some(e) = entries.last_mut() {
                e.lines.push(format!("console: {}", line.trim()));
            }
        }
    }
    entries
}

/// One failure per error in vitest's `Unhandled Errors` section (stderr).
fn unhandled_errors(stderr: &str) -> Vec<Failure> {
    const RULE: char = '\u{23af}'; // ⎯
    let mut out: Vec<Failure> = Vec::new();
    let mut in_section = false;
    let mut current: Option<Failure> = None;
    for raw in stderr.lines() {
        let line = jest::strip_ansi(raw);
        let t = line.trim();
        let is_rule_line = t.starts_with(RULE);
        if is_rule_line {
            let title = t.trim_matches(|c: char| c == RULE || c.is_whitespace());
            if title.starts_with("Unhandled Error") {
                in_section = true;
                continue;
            }
            out.extend(current.take());
            if in_section && !title.is_empty() && !title.starts_with('[') {
                // `⎯⎯ Uncaught Exception ⎯⎯` / `⎯⎯ Unhandled Rejection ⎯⎯`
                current = Some(Failure {
                    id: "(unhandled error)".into(),
                    loc: String::new(),
                    msg: String::new(),
                    trace: vec![title.to_string()],
                });
            } else if title.is_empty() {
                in_section = false;
            }
            continue;
        }
        let Some(f) = current.as_mut() else {
            continue;
        };
        if t.is_empty() {
            continue;
        }
        if f.msg.is_empty() {
            f.msg = t.to_string();
        } else if (t.starts_with('\u{276f}')
            && !t.contains("node:internal")
            && !t.contains("node_modules"))
            || t.starts_with("This error originated in")
            || t.starts_with("The latest test that might")
        {
            f.trace.push(t.to_string());
        }
    }
    out.extend(current);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn captured(stdout: &str) -> Captured {
        use std::process::Command;
        let status = Command::new("true").status().unwrap();
        Captured {
            stdout: stdout.into(),
            stderr: String::new(),
            status,
        }
    }

    #[test]
    fn detects_vitest_run() {
        assert!(Vitest.detect(&argv(&["vitest", "run"])));
        assert!(Vitest.detect(&argv(&["npx", "vitest", "run", "src/"])));
        assert!(Vitest.detect(&argv(&["bunx", "vitest", "run"])));
        assert!(Vitest.detect(&argv(&["./node_modules/.bin/vitest", "run"])));
    }

    #[test]
    fn bare_vitest_is_watch_mode_not_detected() {
        assert!(!Vitest.detect(&argv(&["vitest"])));
        assert!(!Vitest.detect(&argv(&["npx", "vitest", "--coverage"])));
    }

    #[test]
    fn other_tools_not_detected() {
        assert!(!Vitest.detect(&argv(&["vite", "run"])));
        assert!(!Vitest.detect(&argv(&["jest", "run"])));
        assert!(!Vitest.detect(&argv(&[])));
    }

    #[test]
    fn prepare_writes_json_to_a_temp_file_and_keeps_the_default_reporter() {
        let p = Vitest.prepare(argv(&["vitest", "run", "src/"]));
        let path = p.artifact_path().expect("temp json artifact");
        assert_eq!(
            p.argv,
            vec![
                "vitest".to_string(),
                "run".into(),
                "src/".into(),
                "--reporter=default".into(),
                "--reporter=json".into(),
                format!("--outputFile.json={}", path.display()),
            ]
        );
        assert!(path.starts_with(std::env::temp_dir()));
    }

    #[test]
    fn prepare_respects_user_reporter_and_output_file() {
        for a in [
            argv(&["vitest", "run", "--reporter=json"]),
            argv(&["vitest", "run", "--reporter", "verbose"]),
            argv(&["vitest", "run", "--outputFile=out.json"]),
            argv(&["vitest", "run", "--outputFile.json", "r.json"]),
        ] {
            let p = Vitest.prepare(a.clone());
            assert_eq!(p.argv, a);
            assert!(p.artifact.is_none());
        }
        assert_eq!(
            user_json_output(&argv(&["vitest", "run", "--outputFile.json", "r.json"])),
            Some(PathBuf::from("r.json"))
        );
        assert_eq!(
            user_json_output(&argv(&["vitest", "--outputFile=o.json"])),
            Some(PathBuf::from("o.json"))
        );
    }

    #[test]
    fn watch_flag_is_not_detected() {
        assert!(!Vitest.detect(&argv(&["vitest", "run", "--watch"])));
    }

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/vitest/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    /// Parse a real vitest run as if our injected reporter had written
    /// `<v>-report.json`, with the run's human stdout/stderr.
    fn parse_real(v: &str) -> crate::adapters::report::TestReport {
        let prepared = Vitest.prepare(argv(&["vitest", "run"]));
        std::fs::write(
            prepared.artifact_path().unwrap(),
            fixture(&format!("{v}-report.json")),
        )
        .unwrap();
        let status = std::process::Command::new("false").status().unwrap();
        let cap = Captured {
            stdout: fixture(&format!("{v}-stdout.txt")),
            stderr: fixture(&format!("{v}-stderr.txt")),
            status,
        };
        match Vitest.parse(&cap, &prepared).unwrap().report {
            super::super::AdapterReport::Tests(r) => r,
            _ => panic!("expected test report"),
        }
    }

    #[test]
    fn real_vitest_3_and_5_runs_report_every_failure() {
        for v in ["v3", "v5"] {
            let r = parse_real(v);
            // 1 failed test + 2 suites that failed to load + 1 unhandled error.
            assert_eq!((r.total, r.passed, r.failed), (4, 3, 4), "{v}");
            let ids: Vec<&str> = r.failures.iter().map(|f| f.id.as_str()).collect();
            assert!(ids.contains(&"fails"), "{v}: {ids:?}");
            assert!(
                ids.iter().any(|i| i.ends_with("src/missing.test.js")),
                "{v}: {ids:?}"
            );
            assert!(
                ids.iter().any(|i| i.ends_with("src/syntax.test.js")),
                "{v}: {ids:?}"
            );
            let missing = r
                .failures
                .iter()
                .find(|f| f.id.ends_with("src/missing.test.js"))
                .unwrap();
            assert!(
                missing
                    .msg
                    .starts_with("Cannot find module './does-not-exist'"),
                "{v}: {}",
                missing.msg
            );
            let unhandled = r
                .failures
                .iter()
                .find(|f| f.id == "(unhandled error)")
                .expect("unhandled error reported");
            assert_eq!(unhandled.msg, "Error: late boom", "{v}");
            assert!(
                unhandled
                    .trace
                    .iter()
                    .any(|l| l.contains("src/unhandled.test.js:2:48")),
                "{v}: {:?}",
                unhandled.trace
            );
            let fails = r.failures.iter().find(|f| f.id == "fails").unwrap();
            assert!(
                fails.trace.contains(&"console: debug value 42".to_string()),
                "{v}: {:?}",
                fails.trace
            );
        }
    }

    #[test]
    fn missing_json_file_is_a_parse_error() {
        // vitest died before writing its report: fall back generically.
        let prepared = Vitest.prepare(argv(&["vitest", "run"]));
        assert!(Vitest.parse(&captured("boom"), &prepared).is_err());
    }

    #[test]
    fn parses_jest_shaped_fixture_with_vitest_runner() {
        let path = format!(
            "{}/tests/fixtures/jest/mixed.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let stdout = std::fs::read_to_string(path).unwrap();
        // A user-chosen `--reporter=json` still reports on stdout (vitest 3).
        let prepared = Vitest.prepare(argv(&["vitest", "run", "--reporter=json"]));
        let outcome = Vitest.parse(&captured(&stdout), &prepared).unwrap();
        match outcome.report {
            super::super::AdapterReport::Tests(r) => {
                assert_eq!(r.runner, "vitest");
                assert_eq!((r.total, r.passed, r.failed, r.skipped), (3, 1, 1, 1));
            }
            _ => panic!("expected test report"),
        }
        assert!(outcome.passthrough_stdout.is_none());
        assert!(outcome.passthrough_stderr.is_none());
    }
}
