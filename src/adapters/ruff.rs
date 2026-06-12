use super::{basename, Adapter, AdapterReport, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

pub struct Ruff;

impl Adapter for Ruff {
    fn name(&self) -> &'static str {
        "ruff"
    }
    fn matches(&self) -> &'static str {
        "ruff check"
    }
    fn detect(&self, argv: &[String]) -> bool {
        match argv {
            [first, second, ..] => basename(first) == "ruff" && second == "check",
            _ => false,
        }
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        if !has_output_format_flag(&argv) {
            argv.push("--output-format".into());
            argv.push("json".into());
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
            // stdout was the JSON payload: consumed.
            passthrough_stdout: None,
            passthrough_stderr: if captured.stderr.is_empty() {
                None
            } else {
                Some(captured.stderr.clone())
            },
        })
    }
}

fn has_output_format_flag(argv: &[String]) -> bool {
    argv.iter()
        .any(|a| a == "--output-format" || a.starts_with("--output-format="))
}

#[derive(Deserialize)]
struct RuffFinding {
    code: Option<String>,
    message: String,
    filename: String,
    location: RuffLocation,
}

#[derive(Deserialize)]
struct RuffLocation {
    row: u64,
    column: u64,
}

pub fn parse_stdout(stdout: &str) -> Result<Value> {
    let doc = crate::fallback::detect_json(stdout).context("no JSON document in ruff output")?;
    let findings: Vec<RuffFinding> =
        serde_json::from_value(doc).context("ruff JSON shape mismatch")?;

    let diagnostics: Vec<Value> = findings
        .iter()
        .map(|f| {
            json!({
                "loc": format!("{}:{}:{}", f.filename, f.location.row, f.location.column),
                // ruff JSON carries no severity; everything it reports is an error.
                "severity": "error",
                "rule": f.code.clone().unwrap_or_default(),
                "msg": f.message.lines().next().unwrap_or("").to_string(),
            })
        })
        .collect();

    let mut value = json!({
        "runner": "ruff",
        "summary": { "errors": diagnostics.len(), "warnings": 0 },
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
        "code": "F821",
        "message": "Undefined name `x`",
        "filename": "src/a.py",
        "location": { "row": 10, "column": 5 },
        "end_location": { "row": 10, "column": 6 },
        "fix": null,
        "url": "https://docs.astral.sh/ruff/rules/undefined-name"
      },
      {
        "code": null,
        "message": "SyntaxError: Expected an expression",
        "filename": "src/b.py",
        "location": { "row": 3, "column": 1 },
        "end_location": { "row": 3, "column": 2 },
        "fix": null
      }
    ]"#;

    #[test]
    fn detects_ruff_check_only() {
        assert!(Ruff.detect(&argv(&["ruff", "check", "src/"])));
        assert!(Ruff.detect(&argv(&["/usr/local/bin/ruff", "check"])));
        assert!(!Ruff.detect(&argv(&["ruff", "format", "src/"])));
        assert!(!Ruff.detect(&argv(&["ruff"])));
        assert!(!Ruff.detect(&argv(&["uvx", "ruff", "check"])));
    }

    #[test]
    fn prepare_appends_json_output_format() {
        let p = Ruff.prepare(argv(&["ruff", "check", "src/"]));
        assert_eq!(
            p.argv,
            argv(&["ruff", "check", "src/", "--output-format", "json"])
        );
        assert!(p.artifact.is_none());
    }

    #[test]
    fn prepare_respects_user_output_format() {
        let p = Ruff.prepare(argv(&["ruff", "check", "--output-format", "github"]));
        assert_eq!(
            p.argv,
            argv(&["ruff", "check", "--output-format", "github"])
        );
        let p = Ruff.prepare(argv(&["ruff", "check", "--output-format=json"]));
        assert_eq!(p.argv, argv(&["ruff", "check", "--output-format=json"]));
    }

    #[test]
    fn parses_two_findings() {
        let v = parse_stdout(FIXTURE).unwrap();
        assert_eq!(v["runner"], "ruff");
        assert_eq!(v["summary"]["errors"], 2);
        assert_eq!(v["summary"]["warnings"], 0);
        let diags = v["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0]["loc"], "src/a.py:10:5");
        assert_eq!(diags[0]["severity"], "error");
        assert_eq!(diags[0]["rule"], "F821");
        assert_eq!(diags[0]["msg"], "Undefined name `x`");
        // null code (syntax error) becomes empty rule
        assert_eq!(diags[1]["rule"], "");
    }

    #[test]
    fn clean_run_omits_diagnostics_key() {
        let v = parse_stdout("[]").unwrap();
        assert_eq!(v["summary"]["errors"], 0);
        assert!(v.get("diagnostics").is_none());
    }

    #[test]
    fn non_json_is_error() {
        assert!(parse_stdout("ruff failed: bad config").is_err());
        assert!(parse_stdout("").is_err());
    }
}
