//! `cargo build` / `cargo check` / `cargo clippy` adapter.
//!
//! Injects `--message-format=json` unless the user already passed their own
//! `--message-format`; a user override is honored only when it's a JSON
//! flavor ("json", "json-diagnostic-short", ...) — anything else (human,
//! short) sends diagnostics to stderr as unparseable text, which is always
//! an `Err`, empty stdout or not. rustc then emits one JSON object per line
//! on stdout; cargo keeps its own progress banners — "Compiling foo",
//! "Finished dev [...]" — on stderr.
//!
//! Only `reason: "compiler-message"` lines carry diagnostics.
//! `compiler-artifact` / `build-script-executed` / `build-finished` lines
//! are noise for counting but still prove the stream really is cargo JSON —
//! a clean, warning-free build emits none of the former, so gating "is this
//! cargo JSON at all" on `compiler-message` alone would wrongly bail on
//! every successful build. Summary messages ("aborting due to 1 previous
//! error", "N warnings emitted") are also `compiler-message` but restate
//! counts already tallied per-diagnostic; they're recognized by text shape,
//! not by span/code absence, because a real span-less error (`linking with
//! \`cc\` failed`) must still be counted. Workspaces / `--all-targets`
//! recompile and re-emit the identical diagnostic once per compilation
//! unit; dedup on (loc, rule, msg) before counting.
use super::{basename, diagnostics, Adapter, AdapterReport, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{bail, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::OnceLock;

pub struct CargoBuild;

impl Adapter for CargoBuild {
    fn name(&self) -> &'static str {
        "cargo-build"
    }
    fn matches(&self) -> &'static str {
        "cargo build | check | clippy (--message-format=json)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        match argv.first() {
            Some(first) if basename(first) == "cargo" => {
                matches!(subcommand(argv), Some("build" | "check" | "clippy"))
            }
            _ => false,
        }
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        if message_format_value(&argv).is_none() {
            // clippy's trailing `-- -D warnings` (rustc-level lint flags) must
            // stay last and untouched, so insert before the `--` separator
            // rather than appending blindly.
            let insert_at = argv.iter().position(|a| a == "--").unwrap_or(argv.len());
            argv.insert(insert_at, "--message-format=json".to_string());
        }
        Prepared {
            argv,
            artifact: None,
        }
    }
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        if let Some(fmt) = message_format_value(&prepared.argv) {
            if !fmt.starts_with("json") {
                bail!(
                    "cargo ran with --message-format={fmt}; cartoon only parses JSON message formats"
                );
            }
        }

        let runner = runner_label(&prepared.argv);

        if captured.stdout.trim().is_empty() {
            // With a JSON format confirmed above, empty stdout is either a
            // genuinely clean build (exit 0) or a failure that never got far
            // enough to emit any compiler JSON (missing manifest, ...) — the
            // latter's explanation lives only in stderr.
            let value = diagnostics::build_value(runner, Vec::new(), 0, 0);
            let unexplained_failure = !captured.status.success();
            return Ok(ParseOutcome {
                report: AdapterReport::Value(value),
                passthrough_stdout: None, // nothing to pass through: stdout was empty
                passthrough_stderr: (unexplained_failure && !captured.stderr.is_empty())
                    .then(|| captured.stderr.clone()),
            });
        }

        let (diags, errors, warnings, found_cargo_json) = parse_lines(&captured.stdout)?;
        if !found_cargo_json {
            bail!(
                "no recognized cargo JSON (compiler-message/-artifact, build-finished, ...) \
                 found in stdout — unexpected output?"
            );
        }

        let matched = errors + warnings;
        let value = diagnostics::build_value(runner, diags, errors, warnings);
        // A failed run with zero matched diagnostics (linker error, missing
        // manifest, ...) means the JSON stream never explained the failure —
        // pass the raw streams through rather than reporting a clean 0/0.
        let unexplained_failure = !captured.status.success() && matched == 0;

        Ok(ParseOutcome {
            report: AdapterReport::Value(value),
            passthrough_stdout: (unexplained_failure && !captured.stdout.is_empty())
                .then(|| captured.stdout.clone()),
            passthrough_stderr: if unexplained_failure {
                (!captured.stderr.is_empty()).then(|| captured.stderr.clone())
            } else {
                // Normal path: stderr is cargo's "Compiling .../Finished ..."
                // progress noise unless it actually carries diagnostic text.
                stderr_has_diagnostic_text(&captured.stderr).then(|| captured.stderr.clone())
            },
        })
    }
}

/// The value of a `--message-format` flag already present in argv, in either
/// `--message-format value` or `--message-format=value` form. `None` when
/// the flag isn't present at all.
fn message_format_value(argv: &[String]) -> Option<&str> {
    for (i, a) in argv.iter().enumerate() {
        if let Some(v) = a.strip_prefix("--message-format=") {
            return Some(v);
        }
        if a == "--message-format" {
            return argv.get(i + 1).map(String::as_str);
        }
    }
    None
}

/// argv[1] is normally the subcommand, but `cargo +nightly build` inserts a
/// toolchain selector first — skip past it so detection and the runner label
/// both still see "build" regardless of where `prepare` injected its flag.
fn subcommand(argv: &[String]) -> Option<&str> {
    match argv.get(1).map(String::as_str) {
        Some(tok) if tok.starts_with('+') => argv.get(2).map(String::as_str),
        other => other,
    }
}

fn runner_label(argv: &[String]) -> &'static str {
    match subcommand(argv) {
        Some("check") => "cargo-check",
        Some("clippy") => "cargo-clippy",
        _ => "cargo-build",
    }
}

fn stderr_has_diagnostic_text(stderr: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?m)^(warning|error)(\[|:)").unwrap())
        .is_match(stderr)
}

fn is_recognized_reason(reason: &str) -> bool {
    matches!(
        reason,
        "compiler-message" | "compiler-artifact" | "build-script-executed" | "build-finished"
    )
}

/// cargo's own restatements of counts already tallied per-diagnostic
/// ("aborting due to 1 previous error", "N warnings emitted") — recognized
/// by message text, not by absent span/code, since a real span-less error
/// (a linker failure, "cannot find crate") must still be counted.
fn is_summary_message(text: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^\d+ warnings? emitted").unwrap());
    text.starts_with("aborting due to") || re.is_match(text)
}

#[derive(Deserialize)]
struct CargoMessage {
    message: String,
    level: String,
    code: Option<CargoCode>,
    #[serde(default)]
    spans: Vec<CargoSpan>,
}

#[derive(Deserialize)]
struct CargoCode {
    code: String,
}

#[derive(Deserialize)]
struct CargoSpan {
    file_name: String,
    #[serde(default)]
    is_primary: bool,
    line_start: u64,
    column_start: u64,
}

/// Returns (diagnostics, errors, warnings, found any recognized cargo JSON
/// line). The last flag answers "was this a real `--message-format=json`
/// stream at all" — any of `compiler-message` / `compiler-artifact` /
/// `build-script-executed` / `build-finished` counts, since a clean build
/// emits only the latter three.
fn parse_lines(stdout: &str) -> Result<(Vec<Value>, u64, u64, bool)> {
    let mut diagnostics = Vec::new();
    let mut errors: u64 = 0;
    let mut warnings: u64 = 0;
    let mut found_cargo_json = false;
    let mut seen: HashSet<(String, String, String)> = HashSet::new();

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue; // not JSON at all (a banner/progress line slipped onto stdout)
        };
        let Some(reason) = value.get("reason").and_then(Value::as_str) else {
            continue;
        };
        if is_recognized_reason(reason) {
            found_cargo_json = true;
        }
        if reason != "compiler-message" {
            continue;
        }
        let Some(msg_value) = value.get("message") else {
            continue;
        };
        let msg: CargoMessage = match serde_json::from_value(msg_value.clone()) {
            Ok(m) => m,
            // Shape drift in a compiler-message object is worse than silent
            // under-counting — surface it.
            Err(e) => bail!("cargo JSON shape mismatch: {e}"),
        };
        if msg.level != "error" && msg.level != "warning" {
            continue; // "note" / "help" carry no counted severity.
        }
        let text = msg.message.lines().next().unwrap_or("").to_string();
        if is_summary_message(&text) {
            continue;
        }
        let span = msg
            .spans
            .iter()
            .find(|s| s.is_primary)
            .or_else(|| msg.spans.first());
        let loc = span
            .map(|s| format!("{}:{}:{}", s.file_name, s.line_start, s.column_start))
            .unwrap_or_default();
        let rule = msg.code.map(|c| c.code).unwrap_or_default();

        // Workspaces / --all-targets recompile and re-report the same
        // diagnostic once per compilation unit; keep the first occurrence.
        if !seen.insert((loc.clone(), rule.clone(), text.clone())) {
            continue;
        }
        if msg.level == "error" {
            errors += 1;
        } else {
            warnings += 1;
        }
        diagnostics.push(json!({
            "loc": loc,
            "severity": msg.level,
            "rule": rule,
            "msg": text,
        }));
    }
    Ok((diagnostics, errors, warnings, found_cargo_json))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    // One compiler-artifact line (noise), one E0308 error whose primary span
    // is the second of two spans, one clippy warning, one unused_variables
    // warning, two summary lines (empty spans, null code — must not double
    // count), and a trailing build-finished line.
    const FIXTURE: &str = r#"
{"reason":"compiler-artifact","package_id":"foo 0.1.0 (path+file:///repo)","target":{"name":"foo"},"profile":{},"features":[],"filenames":[],"executable":null,"fresh":false}
{"reason":"compiler-message","package_id":"foo 0.1.0 (path+file:///repo)","target":{"name":"foo"},"message":{"rendered":"error[E0308]: mismatched types\n","children":[],"code":{"code":"E0308","explanation":null},"level":"error","message":"mismatched types","spans":[{"file_name":"src/lib.rs","line_start":5,"column_start":1,"is_primary":false},{"file_name":"src/main.rs","line_start":10,"column_start":5,"is_primary":true}]}}
{"reason":"compiler-message","package_id":"foo 0.1.0 (path+file:///repo)","target":{"name":"foo"},"message":{"rendered":"warning: unneeded `return` statement\n","children":[],"code":{"code":"clippy::needless_return","explanation":null},"level":"warning","message":"unneeded `return` statement","spans":[{"file_name":"src/lib.rs","line_start":42,"column_start":9,"is_primary":true}]}}
{"reason":"compiler-message","package_id":"foo 0.1.0 (path+file:///repo)","target":{"name":"foo"},"message":{"rendered":"warning: unused variable: `x`\n","children":[],"code":{"code":"unused_variables","explanation":null},"level":"warning","message":"unused variable: `x`","spans":[{"file_name":"src/main.rs","line_start":20,"column_start":9,"is_primary":true}]}}
{"reason":"compiler-message","package_id":"foo 0.1.0 (path+file:///repo)","target":{"name":"foo"},"message":{"rendered":"error: aborting due to 1 previous error\n","children":[],"code":null,"level":"error","message":"aborting due to 1 previous error","spans":[]}}
{"reason":"compiler-message","package_id":"foo 0.1.0 (path+file:///repo)","target":{"name":"foo"},"message":{"rendered":"warning: 2 warnings emitted\n","children":[],"code":null,"level":"warning","message":"2 warnings emitted","spans":[]}}
{"reason":"build-finished","success":false}
"#;

    // A clean, warning-free build: only compiler-artifact and build-finished
    // lines, no compiler-message at all.
    const CLEAN_BUILD_FIXTURE: &str = r#"
{"reason":"compiler-artifact","package_id":"foo 0.1.0 (path+file:///repo)","target":{"name":"foo"},"profile":{},"features":[],"filenames":[],"executable":null,"fresh":false}
{"reason":"build-finished","success":true}
"#;

    // The same warning re-reported for two compilation units (lib + test
    // target under --all-targets).
    const DUPLICATE_DIAGNOSTIC_FIXTURE: &str = r#"
{"reason":"compiler-message","package_id":"foo 0.1.0 (path+file:///repo)","target":{"name":"foo"},"message":{"rendered":"warning: unused variable: `x`\n","children":[],"code":{"code":"unused_variables","explanation":null},"level":"warning","message":"unused variable: `x`","spans":[{"file_name":"src/main.rs","line_start":20,"column_start":9,"is_primary":true}]}}
{"reason":"compiler-message","package_id":"foo 0.1.0 (path+file:///repo)","target":{"name":"foo"},"message":{"rendered":"warning: unused variable: `x`\n","children":[],"code":{"code":"unused_variables","explanation":null},"level":"warning","message":"unused variable: `x`","spans":[{"file_name":"src/main.rs","line_start":20,"column_start":9,"is_primary":true}]}}
{"reason":"build-finished","success":true}
"#;

    // A span-less error that is NOT a summary restatement — must still count.
    const LINK_ERROR_FIXTURE: &str = r#"
{"reason":"compiler-message","package_id":"foo 0.1.0 (path+file:///repo)","target":{"name":"foo"},"message":{"rendered":"error: linking with `cc` failed: exit status: 1\n","children":[],"code":null,"level":"error","message":"linking with `cc` failed: exit status: 1","spans":[]}}
{"reason":"build-finished","success":false}
"#;

    #[test]
    fn detects_build_check_clippy_only() {
        assert!(CargoBuild.detect(&argv(&["cargo", "build"])));
        assert!(CargoBuild.detect(&argv(&["cargo", "check", "--all-targets"])));
        assert!(CargoBuild.detect(&argv(&["cargo", "clippy"])));
        assert!(CargoBuild.detect(&argv(&["/usr/local/bin/cargo", "build"])));
        assert!(CargoBuild.detect(&argv(&["cargo", "+nightly", "build"])));
        assert!(!CargoBuild.detect(&argv(&["cargo", "test"])));
        assert!(!CargoBuild.detect(&argv(&["cargo", "run"])));
        assert!(!CargoBuild.detect(&argv(&["cargo", "publish"])));
        assert!(!CargoBuild.detect(&argv(&["cargo"])));
        assert!(!CargoBuild.detect(&argv(&["cargo", "+nightly"])));
        assert!(!CargoBuild.detect(&argv(&["rustc", "build"])));
    }

    #[test]
    fn prepare_appends_when_no_double_dash() {
        let p = CargoBuild.prepare(argv(&["cargo", "build"]));
        assert_eq!(p.argv, argv(&["cargo", "build", "--message-format=json"]));
        assert!(p.artifact.is_none());
    }

    #[test]
    fn prepare_inserts_before_double_dash_for_clippy() {
        let p = CargoBuild.prepare(argv(&[
            "cargo",
            "clippy",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ]));
        assert_eq!(
            p.argv,
            argv(&[
                "cargo",
                "clippy",
                "--all-targets",
                "--message-format=json",
                "--",
                "-D",
                "warnings",
            ])
        );
    }

    #[test]
    fn prepare_respects_user_message_format() {
        let p = CargoBuild.prepare(argv(&["cargo", "build", "--message-format", "human"]));
        assert_eq!(
            p.argv,
            argv(&["cargo", "build", "--message-format", "human"])
        );
        let p = CargoBuild.prepare(argv(&["cargo", "build", "--message-format=human"]));
        assert_eq!(p.argv, argv(&["cargo", "build", "--message-format=human"]));
    }

    fn captured(stdout: &str, stderr: &str, success: bool) -> Captured {
        #[cfg(unix)]
        let status = {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(if success { 0 } else { 256 })
        };
        #[cfg(not(unix))]
        let status = std::process::Command::new(if success { "true" } else { "false" })
            .status()
            .unwrap();
        Captured {
            stdout: stdout.into(),
            stderr: stderr.into(),
            status,
        }
    }

    #[test]
    fn parses_mixed_fixture() {
        let prepared = CargoBuild.prepare(argv(&["cargo", "build"]));
        let out = CargoBuild
            .parse(&captured(FIXTURE, "", true), &prepared)
            .unwrap();
        let AdapterReport::Value(v) = out.report else {
            panic!("expected value report")
        };
        assert_eq!(v["runner"], "cargo-build");
        assert_eq!(v["summary"]["errors"], 1);
        assert_eq!(v["summary"]["warnings"], 2);
        let diags = v["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 3);
        assert_eq!(diags[0]["loc"], "src/main.rs:10:5");
        assert_eq!(diags[0]["rule"], "E0308");
        assert!(diags.iter().any(|d| d["rule"] == "clippy::needless_return"));
        assert!(out.passthrough_stdout.is_none());
    }

    #[test]
    fn runner_label_reflects_subcommand() {
        let prepared = CargoBuild.prepare(argv(&["cargo", "clippy"]));
        let out = CargoBuild
            .parse(&captured(FIXTURE, "", true), &prepared)
            .unwrap();
        let AdapterReport::Value(v) = out.report else {
            panic!("expected value report")
        };
        assert_eq!(v["runner"], "cargo-clippy");
    }

    #[test]
    fn runner_label_skips_leading_toolchain_token() {
        let prepared = CargoBuild.prepare(argv(&["cargo", "+nightly", "clippy"]));
        let out = CargoBuild
            .parse(&captured(FIXTURE, "", true), &prepared)
            .unwrap();
        let AdapterReport::Value(v) = out.report else {
            panic!("expected value report")
        };
        assert_eq!(v["runner"], "cargo-clippy");
    }

    #[test]
    fn clean_build_with_only_artifact_and_finished_lines_is_zero_counts() {
        let prepared = CargoBuild.prepare(argv(&["cargo", "build"]));
        let out = CargoBuild
            .parse(&captured(CLEAN_BUILD_FIXTURE, "", true), &prepared)
            .unwrap();
        let AdapterReport::Value(v) = out.report else {
            panic!("expected value report")
        };
        assert_eq!(v["summary"]["errors"], 0);
        assert_eq!(v["summary"]["warnings"], 0);
        assert!(v.get("diagnostics").is_none());
        assert!(out.passthrough_stdout.is_none());
        assert!(out.passthrough_stderr.is_none());
    }

    #[test]
    fn duplicate_diagnostic_across_compilation_units_counted_once() {
        let prepared = CargoBuild.prepare(argv(&["cargo", "build", "--all-targets"]));
        let out = CargoBuild
            .parse(&captured(DUPLICATE_DIAGNOSTIC_FIXTURE, "", true), &prepared)
            .unwrap();
        let AdapterReport::Value(v) = out.report else {
            panic!("expected value report")
        };
        assert_eq!(v["summary"]["warnings"], 1);
        assert_eq!(v["diagnostics"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn spanless_link_error_is_counted_not_treated_as_summary() {
        let prepared = CargoBuild.prepare(argv(&["cargo", "build"]));
        let out = CargoBuild
            .parse(&captured(LINK_ERROR_FIXTURE, "", false), &prepared)
            .unwrap();
        let AdapterReport::Value(v) = out.report else {
            panic!("expected value report")
        };
        assert_eq!(v["summary"]["errors"], 1);
        let diags = v["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0]["msg"], "linking with `cc` failed: exit status: 1");
        // A matched diagnostic explains the failure, so this is NOT an
        // unexplained failure despite the nonzero exit — no raw passthrough.
        assert!(out.passthrough_stdout.is_none());
    }

    #[test]
    fn human_format_output_is_error() {
        let prepared = CargoBuild.prepare(argv(&["cargo", "build", "--message-format", "human"]));
        let human = "warning: unused variable: `x`\n --> src/main.rs:3:5\n";
        assert!(CargoBuild
            .parse(&captured(human, "", true), &prepared)
            .is_err());
    }

    #[test]
    fn user_non_json_message_format_errors_even_with_empty_stdout() {
        // Human format sends everything to stderr; stdout is empty. The old
        // "stdout empty => Ok 0/0" shortcut must not swallow this.
        let prepared = CargoBuild.prepare(argv(&["cargo", "build", "--message-format", "human"]));
        let stderr = "error: could not compile `foo` due to previous error\n";
        assert!(CargoBuild
            .parse(&captured("", stderr, false), &prepared)
            .is_err());
    }

    #[test]
    fn unexplained_failure_passes_raw_streams_through() {
        // Missing manifest / linker failure: cargo aborts before emitting any
        // JSON — the real explanation lives only in stderr. Also covers the
        // empty-stdout + nonzero-exit case.
        let prepared = CargoBuild.prepare(argv(&["cargo", "build"]));
        let stderr = "error: could not find `Cargo.toml` in `/tmp` or any parent directory\n";
        let out = CargoBuild
            .parse(&captured("", stderr, false), &prepared)
            .unwrap();
        let AdapterReport::Value(v) = out.report else {
            panic!("expected value report")
        };
        assert_eq!(v["summary"]["errors"], 0);
        assert_eq!(v["summary"]["warnings"], 0);
        assert!(out.passthrough_stdout.is_none());
        assert_eq!(out.passthrough_stderr.as_deref(), Some(stderr));
    }

    #[test]
    fn progress_noise_stderr_is_dropped_when_explained() {
        let prepared = CargoBuild.prepare(argv(&["cargo", "build"]));
        let stderr = "   Compiling foo v0.1.0 (/repo)\n    Finished dev [unoptimized] target(s)\n";
        let out = CargoBuild
            .parse(&captured(FIXTURE, stderr, true), &prepared)
            .unwrap();
        assert!(out.passthrough_stderr.is_none());
    }

    fn status_ok() -> std::process::ExitStatus {
        std::process::Command::new("true").status().unwrap()
    }

    #[test]
    fn compiler_message_that_no_longer_fits_the_struct_is_an_error() {
        let stdout = "{\"reason\":\"compiler-message\",\"message\":\"not an object\"}\n";
        let cap = Captured {
            stdout: stdout.into(),
            stderr: String::new(),
            status: status_ok(),
        };
        let prepared = CargoBuild.prepare(vec!["cargo".into(), "build".into()]);
        assert!(CargoBuild.parse(&cap, &prepared).is_err());
    }
}
