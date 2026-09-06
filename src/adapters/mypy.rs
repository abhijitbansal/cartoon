//! `mypy` adapter — parses mypy's `--output json` JSON-lines format into the
//! shared diagnostics shape.
use super::{
    basename, diagnostics, is_python_module, Adapter, AdapterReport, ParseOutcome, Prepared,
};
use crate::runner::Captured;
use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

pub struct Mypy;

/// Flags that make mypy informational (no run, no `--output json` payload to
/// parse). `--*-report` (`--html-report`, `--xml-report`, ...) is a family,
/// matched separately in `is_report_flag`.
const NON_RUN_FLAGS: &[&str] = &["--version", "-V", "--help", "-h", "--install-types"];

impl Adapter for Mypy {
    fn name(&self) -> &'static str {
        "mypy"
    }
    fn matches(&self) -> &'static str {
        "mypy | python -m mypy | uv run mypy | uvx mypy (--output json)"
    }
    fn detect(&self, full: &[String]) -> bool {
        let argv = super::strip_uv_run(full);
        let uv_wrapped = argv.len() != full.len();
        let is_mypy = argv.first().map(|a| basename(a) == "mypy").unwrap_or(false)
            || is_python_module(argv, "mypy")
            || (uv_wrapped && super::is_module_run(argv, "mypy"));
        let informational = argv
            .iter()
            .any(|a| NON_RUN_FLAGS.contains(&a.as_str()) || is_report_flag(a));
        is_mypy && !informational
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        if !has_output_flag(&argv) {
            // Nothing in mypy's CLI uses a `--` separator today, but insert
            // ahead of one defensively (mirrors cargo_build) rather than
            // appending blindly after trailing positional args.
            let insert_at = argv.iter().position(|a| a == "--").unwrap_or(argv.len());
            argv.insert(insert_at, "json".to_string());
            argv.insert(insert_at, "--output".to_string());
        }
        Prepared {
            argv,
            artifact: None,
        }
    }
    fn parse(&self, captured: &Captured, _prepared: &Prepared) -> Result<ParseOutcome> {
        // A crashed/rejected invocation (e.g. an older mypy that doesn't
        // understand `--output json`) writes its complaint to stderr and
        // leaves stdout empty. A 0/0 report here would look like a clean
        // pass, so bail and let the pipeline pass both raw streams through.
        if captured.stdout.trim().is_empty() && !captured.status.success() {
            anyhow::bail!("mypy exited with a failure status and produced no stdout");
        }
        let value = parse_stdout(&captured.stdout)?;
        // mypy exits 2 on fatal/config errors with no diagnostics: keep the
        // raw stdout so a clean-looking table never hides why it failed.
        let counted = value["summary"]["errors"].as_u64().unwrap_or(0)
            + value["summary"]["warnings"].as_u64().unwrap_or(0);
        let unexplained = !captured.status.success() && counted == 0;
        Ok(ParseOutcome {
            report: AdapterReport::Value(value),
            // stdout was mypy's JSON payload: consumed. stderr may hold
            // config errors the agent needs to see.
            passthrough_stdout: (unexplained && !captured.stdout.is_empty())
                .then(|| captured.stdout.clone()),
            passthrough_stderr: if captured.stderr.is_empty() {
                None
            } else {
                Some(captured.stderr.clone())
            },
        })
    }
}

fn has_output_flag(argv: &[String]) -> bool {
    argv.iter()
        .any(|a| a == "--output" || a.starts_with("--output="))
}

/// Matches `--html-report`, `--xml-report=DIR`, `--linecount-report`, etc.
fn is_report_flag(arg: &str) -> bool {
    let name = arg.split('=').next().unwrap_or(arg);
    name.starts_with("--") && name.ends_with("-report")
}

#[derive(Deserialize)]
struct MypyFinding {
    file: String,
    // File-level errors ("Duplicate module named…") carry -1/-1.
    line: i64,
    column: i64,
    message: String,
    code: Option<String>,
    severity: String,
}

pub fn parse_stdout(stdout: &str) -> Result<serde_json::Value> {
    let mut diagnostics_out = Vec::new();
    let mut errors: u64 = 0;
    let mut warnings: u64 = 0;
    let mut parsed_any = false;
    // Only mypy's exact zero-error phrasing is trusted as corroboration on
    // its own; a bare `Found N errors` (also printed by pre-JSON mypy in
    // human-text mode) must not suppress the shape-mismatch bail below.
    let mut clean_run_seen = false;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("Success: no issues found") {
            clean_run_seen = true;
            continue;
        }
        if trimmed.starts_with("Success:") || trimmed.starts_with("Found ") {
            continue;
        }
        if !trimmed.starts_with('{') {
            continue;
        }
        let finding = match serde_json::from_str::<MypyFinding>(trimmed) {
            Ok(f) => f,
            Err(e) => {
                // A JSON object that carries mypy's keys but no longer fits
                // our struct is shape drift: surface it (passthrough) instead
                // of silently reporting fewer findings.
                let looks_like_mypy = serde_json::from_str::<serde_json::Value>(trimmed)
                    .is_ok_and(|v| v.get("severity").is_some() || v.get("file").is_some());
                if looks_like_mypy {
                    anyhow::bail!("mypy JSON shape mismatch: {e}");
                }
                continue;
            }
        };
        parsed_any = true;
        match finding.severity.as_str() {
            "error" => errors += 1,
            "warning" => warnings += 1,
            // "note" (e.g. `reveal_type`) and any other severity are
            // surfaced but not counted in the error/warning summary.
            _ => {}
        }
        let loc = if finding.line < 0 || finding.column < 0 {
            finding.file.clone()
        } else {
            format!("{}:{}:{}", finding.file, finding.line, finding.column)
        };
        diagnostics_out.push(json!({
            "loc": loc,
            "severity": finding.severity,
            "rule": finding.code.clone().unwrap_or_default(),
            "msg": finding.message.lines().next().unwrap_or("").to_string(),
        }));
    }

    if !stdout.trim().is_empty() && !parsed_any && !clean_run_seen {
        anyhow::bail!("no mypy JSON diagnostics found (older mypy without --output json?)");
    }

    Ok(diagnostics::build_value(
        "mypy",
        diagnostics_out,
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

    const MIXED_FIXTURE: &str = r#"{"file":"src/a.py","line":10,"column":4,"message":"Incompatible return value type (got \"str\", expected \"int\")","hint":null,"code":"return-value","severity":"error"}
{"file":"src/a.py","line":15,"column":1,"message":"Missing type parameters","hint":null,"code":"type-arg","severity":"error"}
{"file":"src/a.py","line":20,"column":1,"message":"By the way, consider this","hint":null,"code":null,"severity":"note"}
Found 2 errors in 1 file (checked 3 source files)
"#;

    const SUCCESS_FIXTURE: &str = "Success: no issues found in 3 source files\n";

    const HUMAN_FIXTURE: &str =
        "src/a.py:10: error: Incompatible return value type  [return-value]\n";

    #[test]
    fn detects_mypy_invocations() {
        assert!(Mypy.detect(&argv(&["mypy", "."])));
        assert!(Mypy.detect(&argv(&["python", "-m", "mypy", "src"])));
        assert!(Mypy.detect(&argv(&["uv", "run", "mypy"])));
        assert!(Mypy.detect(&argv(&["uvx", "mypy"])));
        assert!(Mypy.detect(&argv(&["/venv/bin/mypy"])));
    }

    #[test]
    fn skips_non_mypy_and_informational_invocations() {
        assert!(!Mypy.detect(&argv(&["mypy", "--version"])));
        assert!(!Mypy.detect(&argv(&["pytest"])));
        assert!(!Mypy.detect(&argv(&["python", "-m", "pytest"])));
    }

    #[test]
    fn skips_install_types_and_report_flags() {
        assert!(!Mypy.detect(&argv(&["mypy", "--install-types"])));
        assert!(!Mypy.detect(&argv(&["mypy", "--html-report", "out/"])));
        assert!(!Mypy.detect(&argv(&["mypy", "--xml-report=out/"])));
    }

    #[test]
    fn prepare_appends_json_output() {
        let p = Mypy.prepare(argv(&["mypy", "src/"]));
        assert_eq!(p.argv, argv(&["mypy", "src/", "--output", "json"]));
        assert!(p.artifact.is_none());
    }

    #[test]
    fn prepare_inserts_before_separator() {
        let p = Mypy.prepare(argv(&["mypy", "src/", "--", "--extra"]));
        assert_eq!(
            p.argv,
            argv(&["mypy", "src/", "--output", "json", "--", "--extra"])
        );
    }

    #[test]
    fn prepare_respects_user_output_flag() {
        let p = Mypy.prepare(argv(&["mypy", "--output", "text"]));
        assert_eq!(p.argv, argv(&["mypy", "--output", "text"]));
        let p = Mypy.prepare(argv(&["mypy", "--output=text"]));
        assert_eq!(p.argv, argv(&["mypy", "--output=text"]));
    }

    #[test]
    fn parses_mixed_fixture() {
        let v = parse_stdout(MIXED_FIXTURE).unwrap();
        assert_eq!(v["runner"], "mypy");
        assert_eq!(v["summary"]["errors"], 2);
        assert_eq!(v["summary"]["warnings"], 0);
        let diags = v["diagnostics"].as_array().unwrap();
        // Errors + the trailing note, all surfaced; only errors/warnings
        // are counted in the summary.
        assert_eq!(diags.len(), 3);
        assert_eq!(diags[0]["loc"], "src/a.py:10:4");
        assert_eq!(diags[0]["severity"], "error");
        assert_eq!(diags[0]["rule"], "return-value");
        assert_eq!(
            diags[0]["msg"],
            "Incompatible return value type (got \"str\", expected \"int\")"
        );
        assert_eq!(diags[2]["severity"], "note");
    }

    #[test]
    fn success_fixture_has_zero_counts_and_no_diagnostics_key() {
        let v = parse_stdout(SUCCESS_FIXTURE).unwrap();
        assert_eq!(v["summary"]["errors"], 0);
        assert_eq!(v["summary"]["warnings"], 0);
        assert!(v.get("diagnostics").is_none());
    }

    #[test]
    fn human_format_without_json_flag_is_error() {
        assert!(parse_stdout(HUMAN_FIXTURE).is_err());
    }

    #[test]
    fn found_summary_alone_without_json_is_still_an_error() {
        // Old human-format mypy prints this exact trailing summary too, so
        // it must not be trusted as corroboration on its own.
        let text = "src/a.py:10: error: bad stuff  [return-value]\nFound 1 error in 1 file (checked 1 source file)\n";
        assert!(parse_stdout(text).is_err());
    }

    fn status_fail() -> std::process::ExitStatus {
        std::process::Command::new("false").status().unwrap()
    }

    fn status_ok() -> std::process::ExitStatus {
        std::process::Command::new("true").status().unwrap()
    }

    #[test]
    fn fatal_exit_without_diagnostics_passes_stdout_through() {
        let cap = Captured {
            stdout: "Success: no issues found in 1 source file\n".into(),
            stderr: "mypy.ini: [mypy]: Unrecognized option: strictt = True\n".into(),
            status: status_fail(),
        };
        let out = Mypy
            .parse(&cap, &Mypy.prepare(vec!["mypy".into()]))
            .unwrap();
        assert!(out.passthrough_stdout.is_some());
        assert!(out.passthrough_stderr.is_some());
    }

    #[test]
    fn finding_that_no_longer_fits_the_struct_is_an_error() {
        let drifted = "{\"file\":\"a.py\",\"line\":\"ten\",\"column\":1,\"message\":\"x\",\"hint\":null,\"code\":null,\"severity\":\"error\"}\n";
        assert!(parse_stdout(drifted).is_err());
    }

    #[test]
    fn empty_stdout_with_nonzero_exit_is_an_error() {
        let cap = Captured {
            stdout: String::new(),
            stderr: "usage: mypy [-h] ...\nmypy: error: unrecognized arguments: --output json\n"
                .into(),
            status: status_fail(),
        };
        let result = Mypy.parse(&cap, &Mypy.prepare(vec!["mypy".into()]));
        match result {
            Err(e) => assert!(e.to_string().contains("failure status")),
            Ok(_) => panic!("expected empty stdout + nonzero exit to error"),
        }
    }

    #[test]
    fn negative_line_and_column_render_as_file_only_loc() {
        let stdout = r#"{"file":"src/dup.py","line":-1,"column":-1,"message":"Duplicate module named 'dup'","hint":null,"code":"misc","severity":"error"}
Found 1 error in 1 file (checked 2 source files)
"#;
        let cap = Captured {
            stdout: stdout.into(),
            stderr: String::new(),
            status: status_ok(),
        };
        let out = Mypy
            .parse(&cap, &Mypy.prepare(vec!["mypy".into()]))
            .unwrap();
        let value = match out.report {
            AdapterReport::Value(v) => v,
            _ => panic!("expected a Value report"),
        };
        let diags = value["diagnostics"].as_array().unwrap();
        assert_eq!(diags[0]["loc"], "src/dup.py");
    }

    #[test]
    fn note_is_rendered_but_uncounted() {
        let stdout = r#"{"file":"src/a.py","line":5,"column":1,"message":"Revealed type is \"builtins.int\"","hint":null,"code":null,"severity":"note"}
Success: no issues found in 1 source file
"#;
        let cap = Captured {
            stdout: stdout.into(),
            stderr: String::new(),
            status: status_ok(),
        };
        let out = Mypy
            .parse(&cap, &Mypy.prepare(vec!["mypy".into()]))
            .unwrap();
        let value = match out.report {
            AdapterReport::Value(v) => v,
            _ => panic!("expected a Value report"),
        };
        assert_eq!(value["summary"]["errors"], 0);
        assert_eq!(value["summary"]["warnings"], 0);
        let diags = value["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0]["severity"], "note");
    }
}
