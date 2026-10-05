//! `golangci-lint run`. v2 (current) takes `--output.json.path=stdout`, v1
//! `--out-format json`; the two reject each other's flag, so the installed
//! major version is probed (`golangci-lint --version`) before injecting.
//! Both print the same `{"Issues": [...], "Report": {...}}` document,
//! rendered in the shared diagnostics shape.
use super::{basename, diagnostics, Adapter, AdapterReport, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

pub struct GolangciLint;

impl Adapter for GolangciLint {
    fn name(&self) -> &'static str {
        "golangci-lint"
    }
    fn matches(&self) -> &'static str {
        "golangci-lint run (JSON output; v1 and v2)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        let is_run = matches!(argv, [first, second, ..]
            if matches!(basename(first), "golangci-lint" | "golangci-lint.exe") && second == "run");
        // A user-chosen output format is theirs to read.
        is_run
            && !argv
                .iter()
                .any(|a| is_output_flag(a) || a == "-h" || a == "--help")
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        match probe_major(&argv[0]) {
            Some(1) => argv.extend(["--out-format".to_string(), "json".to_string()]),
            Some(_) => argv.push("--output.json.path=stdout".into()),
            // Unknown version: inject nothing (the run is the user's own;
            // parse fails and the generic ladder takes over).
            None => {}
        }
        Prepared {
            argv,
            artifact: None,
        }
    }
    fn parse(&self, captured: &Captured, _prepared: &Prepared) -> Result<ParseOutcome> {
        let value = parse_stdout(&captured.stdout)?;
        Ok(ParseOutcome {
            report: AdapterReport::Value(value),
            // stdout was the JSON document (plus v2's `N issues:` stats):
            // consumed. stderr carries config warnings and errors.
            passthrough_stdout: None,
            passthrough_stderr: (!captured.stderr.is_empty()).then(|| captured.stderr.clone()),
        })
    }
}

/// `--out-format` (v1), `--output.<format>.<key>` (v2), and v2's
/// `--output.*` family as a whole.
fn is_output_flag(a: &str) -> bool {
    let name = a.split('=').next().unwrap_or(a);
    name == "--out-format" || name.starts_with("--output.") || name == "--output"
}

/// Major version from `golangci-lint has version 2.5.0 built with …`
/// (v1 prints the same sentence; very old builds print `version v1.x`).
fn probe_major(bin: &str) -> Option<u64> {
    let out = std::process::Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    major_from(&String::from_utf8_lossy(&out.stdout))
}

fn major_from(version_line: &str) -> Option<u64> {
    let rest = version_line.split("version").nth(1)?.trim_start();
    let rest = rest.strip_prefix('v').unwrap_or(rest);
    rest.split('.').next()?.trim().parse().ok()
}

#[derive(Deserialize)]
struct Doc {
    #[serde(rename = "Issues", default)]
    issues: Option<Vec<Issue>>,
}

#[derive(Deserialize)]
struct Issue {
    #[serde(rename = "FromLinter", default)]
    linter: String,
    #[serde(rename = "Text")]
    text: String,
    #[serde(rename = "Severity", default)]
    severity: String,
    #[serde(rename = "Pos")]
    pos: Pos,
}

#[derive(Deserialize)]
struct Pos {
    #[serde(rename = "Filename")]
    file: String,
    #[serde(rename = "Line", default)]
    line: u64,
    #[serde(rename = "Column", default)]
    column: u64,
}

/// The first JSON value on stdout; v2 follows it with its `N issues:`
/// text stats, which are ignored.
pub fn parse_stdout(stdout: &str) -> Result<Value> {
    let start = stdout
        .find('{')
        .context("no JSON document in golangci-lint output")?;
    let mut stream = serde_json::Deserializer::from_str(&stdout[start..]).into_iter::<Doc>();
    let doc = stream
        .next()
        .context("no JSON document in golangci-lint output")?
        .context("golangci-lint JSON shape mismatch")?;
    let (mut errors, mut warnings) = (0u64, 0u64);
    let diags: Vec<Value> = doc
        .issues
        .unwrap_or_default()
        .into_iter()
        .map(|i| {
            // Severity is empty unless the config sets one; an issue fails
            // the run either way, so empty counts as an error.
            let sev = match i.severity.to_ascii_lowercase().as_str() {
                "warning" | "warn" => "warning",
                "info" | "hint" | "note" => "info",
                _ => "error",
            };
            match sev {
                "error" => errors += 1,
                _ => warnings += 1,
            }
            let loc = match (i.pos.line, i.pos.column) {
                (0, _) => i.pos.file.clone(),
                (l, 0) => format!("{}:{l}", i.pos.file),
                (l, c) => format!("{}:{l}:{c}", i.pos.file),
            };
            json!({
                "loc": loc,
                "severity": sev,
                "rule": i.linter,
                "msg": i.text.lines().next().unwrap_or("").to_string(),
            })
        })
        .collect();
    Ok(diagnostics::build_value(
        "golangci-lint",
        diags,
        errors,
        warnings,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detects_run_only() {
        assert!(GolangciLint.detect(&argv(&["golangci-lint", "run"])));
        assert!(GolangciLint.detect(&argv(&["golangci-lint", "run", "./..."])));
        assert!(!GolangciLint.detect(&argv(&["golangci-lint", "linters"])));
        assert!(!GolangciLint.detect(&argv(&["golangci-lint", "fmt"])));
        assert!(!GolangciLint.detect(&argv(&["golangci-lint"])));
    }

    #[test]
    fn user_output_format_declines() {
        for f in [
            "--out-format=json",
            "--out-format",
            "--output.json.path=out.json",
            "--output.text.path=stdout",
        ] {
            assert!(
                !GolangciLint.detect(&argv(&["golangci-lint", "run", f])),
                "{f}"
            );
        }
    }

    #[test]
    fn major_version_from_both_generations() {
        assert_eq!(
            major_from("golangci-lint has version 2.5.0 built with go1.25.1 from ff63786c on 2025-09-21T19:04:05Z"),
            Some(2)
        );
        assert_eq!(
            major_from("golangci-lint has version 1.64.8 built with go1.24.1 from 8b37f141 on 2025-03-17T20:41:53Z"),
            Some(1)
        );
        assert_eq!(major_from("golangci-lint has version v1.23.8"), Some(1));
        assert_eq!(major_from("command not found"), None);
    }

    #[test]
    fn unknown_binary_injects_nothing() {
        let a = argv(&["/nonexistent/golangci-lint", "run"]);
        assert_eq!(GolangciLint.prepare(a.clone()).argv, a);
    }

    #[test]
    fn parses_real_v2_output_with_trailing_stats() {
        let p = format!(
            "{}/tests/fixtures/golangci-lint/v2.stdout",
            env!("CARGO_MANIFEST_DIR")
        );
        let v = parse_stdout(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(v["runner"], "golangci-lint");
        assert_eq!(v["summary"]["errors"], 3);
        let d = v["diagnostics"].as_array().unwrap();
        assert_eq!(d[0]["loc"], "a.go:11:11");
        assert_eq!(d[0]["rule"], "errcheck");
        assert_eq!(
            d[0]["msg"],
            "Error return value of `os.Remove` is not checked"
        );
        assert_eq!(d[2]["rule"], "unused");
    }

    #[test]
    fn parses_v1_document_with_severities() {
        // v1 `--out-format json` shape (same keys; `Issues` may be null).
        let v1 = r#"{"Issues":[{"FromLinter":"gosimple","Text":"S1002: should omit comparison to bool constant","Severity":"warning","SourceLines":["if ok == true {"],"Replacement":null,"Pos":{"Filename":"pkg/a.go","Offset":120,"Line":9,"Column":5},"ExpectNoLint":false,"ExpectedNoLintLinter":""},{"FromLinter":"typecheck","Text":"undefined: foo","Severity":"","SourceLines":["\tfoo()"],"Replacement":null,"Pos":{"Filename":"pkg/b.go","Offset":0,"Line":3,"Column":2},"ExpectNoLint":false,"ExpectedNoLintLinter":""}],"Report":{"Linters":[{"Name":"gosimple","Enabled":true}]}}"#;
        let v = parse_stdout(v1).unwrap();
        assert_eq!(v["summary"]["errors"], 1);
        assert_eq!(v["summary"]["warnings"], 1);
        assert_eq!(v["diagnostics"][0]["severity"], "warning");
        let clean = parse_stdout(r#"{"Issues":null,"Report":{}}"#).unwrap();
        assert_eq!(clean["summary"]["errors"], 0);
        assert!(clean.get("diagnostics").is_none());
    }

    #[test]
    fn non_json_is_error() {
        assert!(parse_stdout("level=error msg=\"Running error: context loading failed\"").is_err());
        assert!(parse_stdout("").is_err());
        assert!(parse_stdout("{not json").is_err());
    }
}
