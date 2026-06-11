use super::{basename, jest, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::Result;

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
        rest.iter().any(|a| a == "run")
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        argv.push("--reporter=json".into());
        Prepared {
            argv,
            artifact: None,
        }
    }
    fn parse(&self, captured: &Captured, _prepared: &Prepared) -> Result<ParseOutcome> {
        let report = jest::parse_json_named(&captured.stdout, "vitest")?;
        Ok(ParseOutcome {
            report: super::AdapterReport::Tests(report),
            // stdout was the JSON payload; stderr was vitest's human progress.
            passthrough_stdout: None,
            passthrough_stderr: None,
        })
    }
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
    fn prepare_appends_json_reporter() {
        let p = Vitest.prepare(vec!["vitest".into(), "run".into(), "src/".into()]);
        assert_eq!(p.argv, vec!["vitest", "run", "src/", "--reporter=json"]);
        assert!(p.artifact.is_none());
    }

    #[test]
    fn parses_jest_shaped_fixture_with_vitest_runner() {
        let path = format!(
            "{}/tests/fixtures/jest/mixed.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let stdout = std::fs::read_to_string(path).unwrap();
        let prepared = Vitest.prepare(vec!["vitest".into(), "run".into()]);
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
