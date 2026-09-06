//! `swiftlint` adapter — JSON diagnostics from `swiftlint lint --reporter json`.
use super::{basename, strip_xcrun, Adapter, AdapterReport, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

pub struct Swiftlint;

/// Subcommands / flags that opt this invocation OUT of the adapter: they
/// either rewrite files on disk (`--fix`, `--autocorrect`, `autocorrect`,
/// `--format` — the hook already refuses these; the adapter must not claim
/// them either) or don't emit the findings array we know how to parse
/// (`rules`, `reporters`, `docs`, `version`, `--version`, `--help`,
/// `generate-docs`, `baseline`).
const EXCLUDED_TOKENS: &[&str] = &[
    "--fix",
    "--autocorrect",
    "autocorrect",
    "--format",
    "rules",
    "reporters",
    "docs",
    "version",
    "--version",
    "--help",
    "generate-docs",
    "baseline",
];

impl Adapter for Swiftlint {
    fn name(&self) -> &'static str {
        "swiftlint"
    }
    fn matches(&self) -> &'static str {
        "swiftlint [lint] (--reporter json; not --fix / autocorrect)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        let argv = strip_xcrun(argv);
        let Some(first) = argv.first() else {
            return false;
        };
        if basename(first) != "swiftlint" {
            return false;
        }
        if argv.iter().any(|a| EXCLUDED_TOKENS.contains(&a.as_str())) {
            return false;
        }
        match argv.get(1) {
            None => true,
            // `analyze` needs `--compiler-log-path` to run at all and has no
            // fixture yet — leave it undetected until one exists.
            Some(second) => second.starts_with('-') || second == "lint",
        }
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        if !has_reporter_flag(&argv) {
            argv.push("--reporter".into());
            argv.push("json".into());
        }
        Prepared {
            argv,
            artifact: None,
        }
    }
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        // `--strict` turns warnings into a nonzero exit; count them as
        // errors too so the report explains the exit code.
        let strict = prepared.argv.iter().any(|a| a == "--strict");
        let value = parse_stdout(&captured.stdout, strict)?;
        // A nonzero exit with no violations (config error, missing files)
        // must not render as a clean run: keep the raw streams.
        let counted = value["summary"]["errors"].as_u64().unwrap_or(0)
            + value["summary"]["warnings"].as_u64().unwrap_or(0);
        let unexplained = !captured.status.success() && counted == 0;
        Ok(ParseOutcome {
            report: AdapterReport::Value(value),
            // stdout was the JSON payload: consumed.
            passthrough_stdout: (unexplained && !captured.stdout.is_empty())
                .then(|| captured.stdout.clone()),
            passthrough_stderr: if unexplained {
                (!captured.stderr.is_empty()).then(|| captured.stderr.clone())
            } else {
                filter_stderr(&captured.stderr)
            },
        })
    }
}

fn has_reporter_flag(argv: &[String]) -> bool {
    argv.iter()
        .any(|a| a == "--reporter" || a.starts_with("--reporter="))
}

/// swiftlint prints progress noise to stderr — `Linting Swift files in
/// current working directory` (or `... at paths <list>` when paths are
/// given) and `Done linting! Found N violations, ... in M files.` — that
/// carries no signal once we've parsed the JSON. Everything else (config
/// errors, parse failures) matters and passes through. Lines are split with
/// their terminators kept, so CRLF input re-joins as CRLF.
fn filter_stderr(stderr: &str) -> Option<String> {
    let remainder: String = stderr
        .split_inclusive('\n')
        .filter(|line| !is_progress_noise(line.trim_end_matches(['\n', '\r'])))
        .collect();
    if remainder.trim().is_empty() {
        None
    } else {
        Some(remainder)
    }
}

fn is_progress_noise(line: &str) -> bool {
    line == "Linting Swift files in current working directory"
        || line.starts_with("Linting Swift files at paths ")
        || line.starts_with("Done linting! Found ")
}

#[derive(Deserialize)]
struct SwiftlintFinding {
    character: Option<u64>,
    file: String,
    line: u64,
    reason: String,
    rule_id: String,
    severity: String,
}

/// `strict` mirrors `swiftlint --strict`, which fails the run on any
/// warning: warnings are counted and rendered as errors so the report
/// explains the nonzero exit code.
pub fn parse_stdout(stdout: &str, strict: bool) -> Result<Value> {
    let doc =
        crate::fallback::detect_json(stdout).context("no JSON document in swiftlint output")?;
    let findings: Vec<SwiftlintFinding> =
        serde_json::from_value(doc).context("swiftlint JSON shape mismatch")?;

    let mut errors: u64 = 0;
    let mut warnings: u64 = 0;
    let mut diagnostics: Vec<Value> = Vec::with_capacity(findings.len());
    for f in &findings {
        let severity = match f.severity.to_lowercase().as_str() {
            "error" => "error",
            "warning" if strict => "error",
            "warning" => "warning",
            other => anyhow::bail!("swiftlint: unrecognized severity {other:?}"),
        };
        if severity == "error" {
            errors += 1;
        } else {
            warnings += 1;
        }
        let loc = match f.character {
            Some(c) => format!("{}:{}:{}", f.file, f.line, c),
            None => format!("{}:{}", f.file, f.line),
        };
        diagnostics.push(json!({
            "loc": loc,
            "severity": severity,
            "rule": f.rule_id,
            "msg": f.reason.lines().next().unwrap_or(""),
        }));
    }

    let mut value = json!({
        "runner": "swiftlint",
        "summary": { "errors": errors, "warnings": warnings },
    });
    if !diagnostics.is_empty() {
        value["diagnostics"] = Value::Array(diagnostics);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    const FIXTURE: &str = r#"[
      {
        "character": 12,
        "file": "/Users/d/App/Sources/A.swift",
        "line": 18,
        "reason": "Line should be 120 characters or less; currently it has 131 characters",
        "rule_id": "line_length",
        "severity": "Warning",
        "type": "Line Length"
      },
      {
        "character": null,
        "file": "/Users/d/App/Sources/B.swift",
        "line": 5,
        "reason": "Force cast should be avoided",
        "rule_id": "force_cast",
        "severity": "Error",
        "type": "Force Cast"
      },
      {
        "character": 3,
        "file": "/Users/d/App/Sources/C.swift",
        "line": 40,
        "reason": "Trailing whitespace violates the trailing_whitespace rule",
        "rule_id": "trailing_whitespace",
        "severity": "Warning",
        "type": "Trailing Whitespace"
      }
    ]"#;

    const EMPTY_FIXTURE: &str = "[]";

    const XCODE_REPORTER_FIXTURE: &str =
        "/Users/d/A.swift:18:12: warning: Line Length Violation: Line should be 120 characters or less; currently it has 131 characters (line_length)";

    #[test]
    fn detects_bare_swiftlint() {
        assert!(Swiftlint.detect(&argv(&["swiftlint"])));
    }

    #[test]
    fn detects_lint_subcommand_with_flags() {
        assert!(Swiftlint.detect(&argv(&["swiftlint", "lint", "--strict"])));
    }

    #[test]
    fn detects_through_xcrun() {
        assert!(Swiftlint.detect(&argv(&["xcrun", "swiftlint"])));
    }

    #[test]
    fn detects_bare_flag_as_second_token() {
        assert!(Swiftlint.detect(&argv(&["swiftlint", "--config", ".swiftlint.yml"])));
    }

    #[test]
    fn rejects_fix() {
        assert!(!Swiftlint.detect(&argv(&["swiftlint", "--fix"])));
    }

    #[test]
    fn rejects_autocorrect() {
        assert!(!Swiftlint.detect(&argv(&["swiftlint", "autocorrect"])));
    }

    #[test]
    fn rejects_rules() {
        assert!(!Swiftlint.detect(&argv(&["swiftlint", "rules"])));
    }

    #[test]
    fn rejects_version() {
        assert!(!Swiftlint.detect(&argv(&["swiftlint", "version"])));
    }

    #[test]
    fn rejects_unrelated_binary() {
        assert!(!Swiftlint.detect(&argv(&["swift", "build"])));
    }

    #[test]
    fn prepare_appends_reporter_json() {
        let p = Swiftlint.prepare(argv(&["swiftlint", "lint"]));
        assert_eq!(p.argv, argv(&["swiftlint", "lint", "--reporter", "json"]));
        assert!(p.artifact.is_none());
    }

    #[test]
    fn prepare_respects_user_reporter() {
        let p = Swiftlint.prepare(argv(&["swiftlint", "lint", "--reporter", "xcode"]));
        assert_eq!(p.argv, argv(&["swiftlint", "lint", "--reporter", "xcode"]));

        let p = Swiftlint.prepare(argv(&["swiftlint", "lint", "--reporter=xcode"]));
        assert_eq!(p.argv, argv(&["swiftlint", "lint", "--reporter=xcode"]));
    }

    #[test]
    fn parses_mixed_findings() {
        let v = parse_stdout(FIXTURE, false).unwrap();
        assert_eq!(v["runner"], "swiftlint");
        assert_eq!(v["summary"]["errors"], 1);
        assert_eq!(v["summary"]["warnings"], 2);
        let diags = v["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 3);
        assert_eq!(diags[0]["loc"], "/Users/d/App/Sources/A.swift:18:12");
        assert_eq!(diags[0]["severity"], "warning");
        assert_eq!(diags[0]["rule"], "line_length");
        assert_eq!(
            diags[0]["msg"],
            "Line should be 120 characters or less; currently it has 131 characters"
        );
        // null character omits the trailing :character segment
        assert_eq!(diags[1]["loc"], "/Users/d/App/Sources/B.swift:5");
        assert_eq!(diags[1]["severity"], "error");
        assert_eq!(diags[1]["rule"], "force_cast");
    }

    #[test]
    fn clean_run_omits_diagnostics_key() {
        let v = parse_stdout(EMPTY_FIXTURE, false).unwrap();
        assert_eq!(v["summary"]["errors"], 0);
        assert_eq!(v["summary"]["warnings"], 0);
        assert!(v.get("diagnostics").is_none());
    }

    #[test]
    fn human_reporter_text_is_error() {
        assert!(parse_stdout(XCODE_REPORTER_FIXTURE, false).is_err());
        assert!(parse_stdout("", false).is_err());
    }

    #[test]
    fn unrecognized_severity_is_error() {
        let fixture = r#"[{"character":1,"file":"/a.swift","line":1,"reason":"weird","rule_id":"custom_rule","severity":"Info"}]"#;
        assert!(parse_stdout(fixture, false).is_err());
    }

    #[test]
    fn message_is_truncated_to_first_line() {
        let fixture = r#"[{"character":1,"file":"/a.swift","line":1,"reason":"First line.\nSecond line with detail.","rule_id":"r","severity":"Warning"}]"#;
        let v = parse_stdout(fixture, false).unwrap();
        assert_eq!(v["diagnostics"][0]["msg"], "First line.");
    }

    #[test]
    fn stderr_progress_noise_is_filtered() {
        let stderr = "Linting Swift files in current working directory\nDone linting! Found 3 violations, 1 serious in 12 files.\n";
        assert_eq!(filter_stderr(stderr), None);
    }

    #[test]
    fn stderr_config_error_passes_through() {
        let stderr = "Linting Swift files in current working directory\nerror: Invalid configuration: unknown rule 'bogus_rule'\nDone linting! Found 0 violations, 0 serious in 1 files.\n";
        assert_eq!(
            filter_stderr(stderr),
            Some("error: Invalid configuration: unknown rule 'bogus_rule'\n".to_string())
        );
    }

    #[test]
    fn stderr_filtering_preserves_crlf_terminator() {
        let stderr = "Linting Swift files in current working directory\r\nerror: bad config\r\nDone linting! Found 0 violations, 0 serious in 1 files.\r\n";
        assert_eq!(
            filter_stderr(stderr),
            Some("error: bad config\r\n".to_string())
        );
    }

    #[test]
    fn stderr_blank_after_filtering_is_none() {
        let stderr = "Linting Swift files at paths /a, /b\n\nDone linting! Found 0 violations, 0 serious in 2 files.\n";
        assert_eq!(filter_stderr(stderr), None);
    }

    fn status_fail() -> std::process::ExitStatus {
        std::process::Command::new("false").status().unwrap()
    }

    fn status_ok() -> std::process::ExitStatus {
        std::process::Command::new("true").status().unwrap()
    }

    #[test]
    fn config_failure_without_violations_passes_streams_through() {
        let cap = Captured {
            stdout: "[]\n".into(),
            stderr: "error: Could not read configuration: .swiftlint.yml\n".into(),
            status: status_fail(),
        };
        let out = Swiftlint
            .parse(&cap, &Swiftlint.prepare(vec!["swiftlint".into()]))
            .unwrap();
        assert!(out.passthrough_stdout.is_some());
        assert!(out.passthrough_stderr.is_some());
    }

    #[test]
    fn strict_promotes_warnings_to_errors_via_parse() {
        let cap = Captured {
            stdout: FIXTURE.into(),
            stderr: String::new(),
            status: status_fail(),
        };
        let prepared = Swiftlint.prepare(argv(&["swiftlint", "lint", "--strict"]));
        let out = Swiftlint.parse(&cap, &prepared).unwrap();
        let AdapterReport::Value(v) = out.report else {
            panic!("expected AdapterReport::Value");
        };
        // 1 original error + 2 warnings promoted by --strict
        assert_eq!(v["summary"]["errors"], 3);
        assert_eq!(v["summary"]["warnings"], 0);
        let diags = v["diagnostics"].as_array().unwrap();
        assert!(diags.iter().all(|d| d["severity"] == "error"));
    }

    #[test]
    fn unrecognized_severity_via_parse_is_error() {
        let fixture = r#"[{"character":1,"file":"/a.swift","line":1,"reason":"weird","rule_id":"custom_rule","severity":"Info"}]"#;
        let cap = Captured {
            stdout: fixture.into(),
            stderr: String::new(),
            status: status_ok(),
        };
        let prepared = Swiftlint.prepare(argv(&["swiftlint"]));
        assert!(Swiftlint.parse(&cap, &prepared).is_err());
    }
}
