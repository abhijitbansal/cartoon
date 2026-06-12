use super::{basename, Adapter, AdapterReport, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

pub struct Eslint;

impl Adapter for Eslint {
    fn name(&self) -> &'static str {
        "eslint"
    }
    fn matches(&self) -> &'static str {
        "eslint | npx eslint | bunx eslint | pnpx eslint"
    }
    fn detect(&self, argv: &[String]) -> bool {
        match argv {
            [first, ..] if basename(first) == "eslint" => true,
            [first, second, ..]
                if matches!(basename(first), "npx" | "bunx" | "pnpx")
                    && basename(second) == "eslint" =>
            {
                true
            }
            _ => false,
        }
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        if !has_format_flag(&argv) {
            argv.push("--format".into());
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

fn has_format_flag(argv: &[String]) -> bool {
    argv.iter()
        .any(|a| a == "-f" || a == "--format" || a.starts_with("--format="))
}

#[derive(Deserialize)]
struct EslintFile {
    #[serde(rename = "filePath")]
    file_path: String,
    #[serde(rename = "errorCount", default)]
    error_count: u64,
    #[serde(rename = "warningCount", default)]
    warning_count: u64,
    #[serde(default)]
    messages: Vec<EslintMessage>,
}

#[derive(Deserialize)]
struct EslintMessage {
    #[serde(rename = "ruleId")]
    rule_id: Option<String>,
    severity: u64,
    message: String,
    #[serde(default)]
    line: u64,
    #[serde(default)]
    column: u64,
}

const SEVERITY_ERROR: u64 = 2;

pub fn parse_stdout(stdout: &str) -> Result<Value> {
    let doc = crate::fallback::detect_json(stdout).context("no JSON document in eslint output")?;
    let files: Vec<EslintFile> =
        serde_json::from_value(doc).context("eslint JSON shape mismatch")?;

    let errors: u64 = files.iter().map(|f| f.error_count).sum();
    let warnings: u64 = files.iter().map(|f| f.warning_count).sum();

    let diagnostics: Vec<Value> = files
        .iter()
        .flat_map(|f| {
            f.messages.iter().map(|m| {
                json!({
                    "loc": format!("{}:{}:{}", f.file_path, m.line, m.column),
                    "severity": if m.severity == SEVERITY_ERROR { "error" } else { "warning" },
                    "rule": m.rule_id.clone().unwrap_or_default(),
                    "msg": m.message.lines().next().unwrap_or("").to_string(),
                })
            })
        })
        .collect();

    let mut value = json!({
        "runner": "eslint",
        "summary": { "errors": errors, "warnings": warnings, "files": files.len() },
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
        "filePath": "/p/src/a.js",
        "errorCount": 1,
        "warningCount": 1,
        "messages": [
          {
            "ruleId": "no-unused-vars",
            "severity": 2,
            "message": "'x' is defined but never used.",
            "line": 3,
            "column": 7
          },
          {
            "ruleId": "no-console",
            "severity": 1,
            "message": "Unexpected console statement.",
            "line": 8,
            "column": 1
          }
        ]
      },
      {
        "filePath": "/p/src/b.js",
        "errorCount": 1,
        "warningCount": 0,
        "messages": [
          {
            "ruleId": null,
            "severity": 2,
            "message": "Parsing error: Unexpected token }",
            "line": 12,
            "column": 2
          }
        ]
      }
    ]"#;

    #[test]
    fn detects_eslint_invocations() {
        assert!(Eslint.detect(&argv(&["eslint", "src/"])));
        assert!(Eslint.detect(&argv(&["./node_modules/.bin/eslint", "."])));
        assert!(Eslint.detect(&argv(&["npx", "eslint", "."])));
        assert!(Eslint.detect(&argv(&["bunx", "eslint"])));
        assert!(Eslint.detect(&argv(&["pnpx", "eslint", "src/"])));
        assert!(!Eslint.detect(&argv(&["npx", "prettier", "."])));
        assert!(!Eslint.detect(&argv(&["tsc"])));
    }

    #[test]
    fn prepare_appends_json_format() {
        let p = Eslint.prepare(argv(&["eslint", "src/"]));
        assert_eq!(p.argv, argv(&["eslint", "src/", "--format", "json"]));
        assert!(p.artifact.is_none());
    }

    #[test]
    fn prepare_respects_user_format() {
        let p = Eslint.prepare(argv(&["eslint", "--format", "stylish", "."]));
        assert_eq!(p.argv, argv(&["eslint", "--format", "stylish", "."]));
        let p = Eslint.prepare(argv(&["eslint", "-f", "json", "."]));
        assert_eq!(p.argv, argv(&["eslint", "-f", "json", "."]));
        let p = Eslint.prepare(argv(&["eslint", "--format=compact", "."]));
        assert_eq!(p.argv, argv(&["eslint", "--format=compact", "."]));
    }

    #[test]
    fn parses_two_files_three_messages() {
        let v = parse_stdout(FIXTURE).unwrap();
        assert_eq!(v["runner"], "eslint");
        assert_eq!(v["summary"]["errors"], 2);
        assert_eq!(v["summary"]["warnings"], 1);
        assert_eq!(v["summary"]["files"], 2);
        let diags = v["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 3);
        assert_eq!(diags[0]["loc"], "/p/src/a.js:3:7");
        assert_eq!(diags[0]["severity"], "error");
        assert_eq!(diags[0]["rule"], "no-unused-vars");
        assert_eq!(diags[1]["severity"], "warning");
        // null ruleId (parsing error) becomes empty rule
        assert_eq!(diags[2]["rule"], "");
        assert_eq!(diags[2]["msg"], "Parsing error: Unexpected token }");
    }

    #[test]
    fn clean_run_omits_diagnostics_key() {
        // eslint reports clean files with empty message lists
        let v = parse_stdout(
            r#"[{"filePath": "/p/src/a.js", "errorCount": 0, "warningCount": 0, "messages": []}]"#,
        )
        .unwrap();
        assert_eq!(v["summary"]["errors"], 0);
        assert_eq!(v["summary"]["files"], 1);
        assert!(v.get("diagnostics").is_none());
    }

    #[test]
    fn non_json_is_error() {
        assert!(parse_stdout("Oops! Something went wrong!").is_err());
    }
}
