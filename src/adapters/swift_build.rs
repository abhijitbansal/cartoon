use super::{basename, diagnostics, Adapter, AdapterReport, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::Result;
use serde_json::Value;

pub struct SwiftBuild;

const NON_BUILD_ARGS: &[&str] = &["--version", "--help", "-h"];

impl Adapter for SwiftBuild {
    fn name(&self) -> &'static str {
        "swift-build"
    }
    fn matches(&self) -> &'static str {
        "swift build"
    }
    fn detect(&self, argv: &[String]) -> bool {
        let argv = super::strip_xcrun(argv);
        matches!(argv, [first, second, ..]
            if basename(first) == "swift" && second == "build")
            && !argv.iter().any(|a| NON_BUILD_ARGS.contains(&a.as_str()))
    }
    fn prepare(&self, argv: Vec<String>) -> Prepared {
        // Diagnostics are already machine-parseable text; nothing to inject.
        Prepared {
            argv,
            artifact: None,
        }
    }
    fn parse(&self, captured: &Captured, _prepared: &Prepared) -> Result<ParseOutcome> {
        // Diagnostics land on stdout on Swift 6.3, stderr on older
        // toolchains — scan both (separately: joining the streams could weld
        // a split line into a phantom diagnostic).
        let (mut diags, mut errors, mut warnings) = diagnostics::collect(&captured.stdout);
        let (d2, e2, w2) = diagnostics::collect(&captured.stderr);
        diags.extend(d2);
        errors += e2;
        warnings += w2;
        let matched = errors + warnings;
        let value = diagnostics::build_value("swift-build", diags, errors, warnings);
        // A failed build with zero matched diagnostics (linker error, manifest
        // error, ...) must not be swallowed — the agent needs the raw streams.
        let unexplained_failure = !captured.status.success() && matched == 0;
        Ok(ParseOutcome {
            report: AdapterReport::Value(value),
            passthrough_stdout: (unexplained_failure && !captured.stdout.is_empty())
                .then(|| captured.stdout.clone()),
            passthrough_stderr: (unexplained_failure && !captured.stderr.is_empty())
                .then(|| captured.stderr.clone()),
        })
    }
}

/// Returns the TOON value and how many diagnostic lines matched. Kept as a
/// thin wrapper so the existing swift-build tests read naturally.
pub fn parse_text(text: &str) -> (Value, u64) {
    let (diagnostics, errors, warnings) = diagnostics::collect(text);
    (
        diagnostics::build_value("swift-build", diagnostics, errors, warnings),
        errors + warnings,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    const FIXTURE: &str = "\
/Users/dev/proj/Sources/App/Auth.swift:12:5: error: cannot find 'foo' in scope
    foo()
    ^
/Users/dev/proj/Sources/App/Main.swift:3:10: warning: result of call to 'run()' is unused
    run()
    ~~~~~
";

    #[test]
    fn detects_swift_build_invocations() {
        assert!(SwiftBuild.detect(&argv(&["swift", "build"])));
        assert!(SwiftBuild.detect(&argv(&["/usr/bin/swift", "build", "-c", "release"])));
        assert!(SwiftBuild.detect(&argv(&["xcrun", "swift", "build"])));
        assert!(!SwiftBuild.detect(&argv(&["swift", "test"])));
        assert!(!SwiftBuild.detect(&argv(&["swift", "run"])));
        assert!(!SwiftBuild.detect(&argv(&["swift", "build", "--help"])));
        assert!(!SwiftBuild.detect(&argv(&["cargo", "build"])));
    }

    #[test]
    fn prepare_leaves_argv_untouched() {
        let p = SwiftBuild.prepare(argv(&["swift", "build", "-c", "release"]));
        assert_eq!(p.argv, argv(&["swift", "build", "-c", "release"]));
        assert!(p.artifact.is_none());
    }

    #[test]
    fn parses_error_and_warning_drops_caret_lines() {
        let (v, matched) = parse_text(FIXTURE);
        assert_eq!(matched, 2);
        assert_eq!(v["runner"], "swift-build");
        assert_eq!(v["summary"]["errors"], 1);
        assert_eq!(v["summary"]["warnings"], 1);
        let diags = v["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 2);
        assert_eq!(
            diags[0]["loc"],
            "/Users/dev/proj/Sources/App/Auth.swift:12:5"
        );
        assert_eq!(diags[0]["severity"], "error");
        assert_eq!(diags[0]["msg"], "cannot find 'foo' in scope");
        assert_eq!(diags[1]["severity"], "warning");
    }

    #[test]
    fn clean_build_yields_summary_only() {
        let (v, matched) = parse_text("");
        assert_eq!(matched, 0);
        assert_eq!(v["summary"]["errors"], 0);
        assert!(v.get("diagnostics").is_none());
    }

    #[test]
    fn note_lines_are_not_counted() {
        let (v, matched) =
            parse_text("/p/a.swift:1:1: note: add 'static' to make this declaration static\n");
        assert_eq!(matched, 0);
        assert_eq!(v["summary"]["errors"], 0);
    }

    // Swift 6.3 style: diagnostics on stdout with numbered gutter context.
    const SIX_THREE_STDOUT: &str = "\
[3/4] Emitting module SwiftDemo
/Users/dev/proj/Sources/App/Broken.swift:2:12: error: cannot find 'undefinedVar' in scope
1 | public func broken() -> Int {
2 |     return undefinedVar
  |            `- error: cannot find 'undefinedVar' in scope
3 | }
";

    #[test]
    fn parses_swift_six_three_gutter_format_without_double_count() {
        let (v, matched) = parse_text(SIX_THREE_STDOUT);
        assert_eq!(matched, 1);
        assert_eq!(v["summary"]["errors"], 1);
        let diags = v["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0]["msg"], "cannot find 'undefinedVar' in scope");
    }

    #[test]
    fn bare_number_file_token_is_rejected() {
        let (v, matched) = parse_text("1:2:3: error: not a real path\n");
        assert_eq!(matched, 0);
        assert!(v.get("diagnostics").is_none());
    }

    #[test]
    fn split_line_across_streams_makes_no_phantom_diagnostic() {
        use std::os::unix::process::ExitStatusExt;
        // stdout ends mid-path with no trailing newline; stderr completes a
        // diagnostic-shaped line. Joined naively they would match.
        let captured = Captured {
            stdout: "/Users/dev/proj/Sources/App/Auth.swift:12".into(),
            stderr: ":5: error: phantom\n".into(),
            status: std::process::ExitStatus::from_raw(0),
        };
        let out = SwiftBuild
            .parse(&captured, &SwiftBuild.prepare(argv(&["swift", "build"])))
            .unwrap();
        let AdapterReport::Value(v) = out.report else {
            panic!("expected value report")
        };
        assert_eq!(v["summary"]["errors"], 0);
    }

    #[test]
    fn unexplained_failure_passes_both_streams_through() {
        use std::os::unix::process::ExitStatusExt;
        let captured = Captured {
            stdout: "Building for debugging...\n".into(),
            stderr: "error: link command failed with exit code 1\n".into(),
            status: std::process::ExitStatus::from_raw(256), // exit code 1
        };
        let out = SwiftBuild
            .parse(&captured, &SwiftBuild.prepare(argv(&["swift", "build"])))
            .unwrap();
        assert!(out.passthrough_stderr.is_some());
        assert!(out.passthrough_stdout.is_some());
    }

    #[test]
    fn explained_failure_consumes_streams() {
        use std::os::unix::process::ExitStatusExt;
        for (stdout, stderr) in [
            (String::new(), FIXTURE.to_string()),
            (SIX_THREE_STDOUT.to_string(), String::new()),
        ] {
            let captured = Captured {
                stdout,
                stderr,
                status: std::process::ExitStatus::from_raw(256),
            };
            let out = SwiftBuild
                .parse(&captured, &SwiftBuild.prepare(argv(&["swift", "build"])))
                .unwrap();
            assert!(out.passthrough_stderr.is_none());
            assert!(out.passthrough_stdout.is_none());
        }
    }
}
