use super::{basename, Adapter, AdapterReport, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::Result;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

pub struct Tsc;

impl Adapter for Tsc {
    fn name(&self) -> &'static str {
        "tsc"
    }
    fn matches(&self) -> &'static str {
        "tsc | npx tsc | bunx tsc | pnpx tsc (not --watch)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        // --watch runs are long-lived; never capture them.
        if argv.iter().any(|a| a == "--watch" || a == "-w") {
            return false;
        }
        match argv {
            [first, ..] if basename(first) == "tsc" => true,
            [first, second, ..]
                if matches!(basename(first), "npx" | "bunx" | "pnpx")
                    && basename(second) == "tsc" =>
            {
                true
            }
            _ => false,
        }
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        if !has_pretty_flag(&argv) {
            argv.push("--pretty".into());
            argv.push("false".into());
        }
        Prepared {
            argv,
            artifact: None,
        }
    }
    fn parse(&self, captured: &Captured, _prepared: &Prepared) -> Result<ParseOutcome> {
        // Text parsing cannot fail: unmatched lines are simply ignored and a
        // clean tsc run prints nothing, so summary-only output is correct.
        let value = parse_stdout(&captured.stdout);
        Ok(ParseOutcome {
            report: AdapterReport::Value(value),
            // stdout was the diagnostics text: consumed.
            passthrough_stdout: None,
            passthrough_stderr: if captured.stderr.is_empty() {
                None
            } else {
                Some(captured.stderr.clone())
            },
        })
    }
}

fn has_pretty_flag(argv: &[String]) -> bool {
    argv.iter().any(|a| a.starts_with("--pretty"))
}

fn diagnostic_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(?P<file>.+?)\((?P<line>\d+),(?P<col>\d+)\): (?P<sev>error|warning) (?P<code>TS\d+): (?P<msg>.*)$")
            .unwrap()
    })
}

pub fn parse_stdout(stdout: &str) -> Value {
    let mut errors: u64 = 0;
    let mut warnings: u64 = 0;
    let mut diagnostics: Vec<Value> = Vec::new();

    for line in stdout.lines() {
        // "Found N errors..." summary lines don't match the location pattern,
        // so they are naturally excluded from the body.
        let Some(caps) = diagnostic_regex().captures(line) else {
            continue;
        };
        let severity = &caps["sev"];
        if severity == "error" {
            errors += 1;
        } else {
            warnings += 1;
        }
        diagnostics.push(json!({
            "loc": format!("{}:{}:{}", &caps["file"], &caps["line"], &caps["col"]),
            "severity": severity,
            "rule": &caps["code"],
            "msg": &caps["msg"],
        }));
    }

    let mut value = json!({
        "runner": "tsc",
        "summary": { "errors": errors, "warnings": warnings },
    });
    if !diagnostics.is_empty() {
        value["diagnostics"] = Value::Array(diagnostics);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    const FIXTURE: &str = "\
src/app.ts(10,5): error TS2322: Type 'string' is not assignable to type 'number'.
src/util.ts(3,1): error TS2304: Cannot find name 'foo'.

Found 2 errors.
";

    #[test]
    fn detects_tsc_invocations() {
        assert!(Tsc.detect(&argv(&["tsc"])));
        assert!(Tsc.detect(&argv(&["tsc", "--noEmit"])));
        assert!(Tsc.detect(&argv(&["./node_modules/.bin/tsc", "-p", "."])));
        assert!(Tsc.detect(&argv(&["npx", "tsc", "--noEmit"])));
        assert!(Tsc.detect(&argv(&["bunx", "tsc"])));
        assert!(Tsc.detect(&argv(&["pnpx", "tsc"])));
        assert!(!Tsc.detect(&argv(&["npx", "tsx", "script.ts"])));
        assert!(!Tsc.detect(&argv(&["eslint", "."])));
    }

    #[test]
    fn watch_mode_is_not_detected() {
        assert!(!Tsc.detect(&argv(&["tsc", "--watch"])));
        assert!(!Tsc.detect(&argv(&["tsc", "-w"])));
        assert!(!Tsc.detect(&argv(&["npx", "tsc", "--noEmit", "--watch"])));
    }

    #[test]
    fn prepare_appends_pretty_false() {
        let p = Tsc.prepare(argv(&["tsc", "--noEmit"]));
        assert_eq!(p.argv, argv(&["tsc", "--noEmit", "--pretty", "false"]));
        assert!(p.artifact.is_none());
    }

    #[test]
    fn prepare_respects_user_pretty() {
        let p = Tsc.prepare(argv(&["tsc", "--pretty", "false"]));
        assert_eq!(p.argv, argv(&["tsc", "--pretty", "false"]));
        let p = Tsc.prepare(argv(&["tsc", "--pretty"]));
        assert_eq!(p.argv, argv(&["tsc", "--pretty"]));
    }

    #[test]
    fn parses_two_errors_and_drops_summary_line() {
        let v = parse_stdout(FIXTURE);
        assert_eq!(v["runner"], "tsc");
        assert_eq!(v["summary"]["errors"], 2);
        assert_eq!(v["summary"]["warnings"], 0);
        let diags = v["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0]["loc"], "src/app.ts:10:5");
        assert_eq!(diags[0]["severity"], "error");
        assert_eq!(diags[0]["rule"], "TS2322");
        assert_eq!(
            diags[0]["msg"],
            "Type 'string' is not assignable to type 'number'."
        );
        assert_eq!(diags[1]["rule"], "TS2304");
        // "Found 2 errors." is excluded from diagnostics
        assert!(!diags
            .iter()
            .any(|d| d["msg"].as_str().unwrap().contains("Found 2 errors")));
    }

    #[test]
    fn clean_run_yields_summary_only() {
        let v = parse_stdout("");
        assert_eq!(v["summary"]["errors"], 0);
        assert_eq!(v["summary"]["warnings"], 0);
        assert!(v.get("diagnostics").is_none());
    }

    #[test]
    fn non_tsc_text_never_errors() {
        let v = parse_stdout("some random build banner\nnothing tsc-ish here\n");
        assert_eq!(v["summary"]["errors"], 0);
        assert!(v.get("diagnostics").is_none());
    }
}
