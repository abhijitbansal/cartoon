use super::xcodebuild::{action, Action};
use super::{diagnostics, Adapter, AdapterReport, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::Result;

pub struct XcodebuildBuild;

impl Adapter for XcodebuildBuild {
    fn name(&self) -> &'static str {
        "xcodebuild-build"
    }
    fn matches(&self) -> &'static str {
        "xcodebuild build / build-for-testing (no test action)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        action(argv) == Some(Action::Build)
    }
    fn prepare(&self, argv: Vec<String>) -> Prepared {
        // Diagnostics are already machine-parseable text; nothing to inject.
        // Deliberately no `-quiet`: keep the raw streams complete for the
        // passthrough fallback (the summary replaces the noise in the report).
        Prepared {
            argv,
            artifact: None,
        }
    }
    fn parse(&self, captured: &Captured, _prepared: &Prepared) -> Result<ParseOutcome> {
        // Scan both streams separately (joining could weld a split line into a
        // phantom diagnostic). xcodebuild emits the same clang format as swift.
        let (mut diags, mut errors, mut warnings) = diagnostics::collect(&captured.stdout);
        let (d2, e2, w2) = diagnostics::collect(&captured.stderr);
        diags.extend(d2);
        errors += e2;
        warnings += w2;
        let matched = errors + warnings;
        let value = diagnostics::build_value("xcodebuild-build", diags, errors, warnings);
        // Failed build with zero matched diagnostics (linker, signing, missing
        // scheme, ...) must not be swallowed — pass the raw streams through.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    // Real xcodebuild build diagnostics interleave with progress banners.
    const FIXTURE: &str = "\
Build settings from command line:
    SDKROOT = macosx26.0
note: Building targets in dependency order
CompileSwift normal arm64 /Users/dev/App/Sources/App/Auth.swift
/Users/dev/App/Sources/App/Auth.swift:18:9: error: cannot find 'tokn' in scope
        tokn = refresh()
        ^~~~
/Users/dev/App/Sources/App/View.swift:7:1: warning: 'NavigationView' is deprecated
struct V: View {
^
** BUILD FAILED **
";

    #[test]
    fn detects_build_action_only() {
        assert!(XcodebuildBuild.detect(&argv(&["xcodebuild", "build"])));
        assert!(XcodebuildBuild.detect(&argv(&["xcodebuild", "-scheme", "A", "build"])));
        assert!(!XcodebuildBuild.detect(&argv(&["xcodebuild", "test"])));
        assert!(!XcodebuildBuild.detect(&argv(&["xcodebuild", "clean", "test"])));
        assert!(!XcodebuildBuild.detect(&argv(&["swift", "build"])));
    }

    #[test]
    fn prepare_leaves_argv_untouched() {
        let p = XcodebuildBuild.prepare(argv(&["xcodebuild", "build", "-scheme", "A"]));
        assert_eq!(p.argv, argv(&["xcodebuild", "build", "-scheme", "A"]));
        assert!(p.artifact.is_none());
    }

    #[test]
    fn parses_diagnostics_dropping_banners_and_carets() {
        use std::os::unix::process::ExitStatusExt;
        let captured = Captured {
            stdout: FIXTURE.into(),
            stderr: String::new(),
            status: std::process::ExitStatus::from_raw(256),
        };
        let out = XcodebuildBuild
            .parse(
                &captured,
                &XcodebuildBuild.prepare(argv(&["xcodebuild", "build"])),
            )
            .unwrap();
        let AdapterReport::Value(v) = out.report else {
            panic!("expected value report")
        };
        assert_eq!(v["summary"]["errors"], 1);
        assert_eq!(v["summary"]["warnings"], 1);
        let diags = v["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 2);
        assert_eq!(
            diags[0]["loc"],
            "/Users/dev/App/Sources/App/Auth.swift:18:9"
        );
        assert_eq!(diags[0]["msg"], "cannot find 'tokn' in scope");
        // Diagnostics matched → not an unexplained failure → no passthrough.
        assert!(out.passthrough_stdout.is_none());
    }

    #[test]
    fn unexplained_failure_passes_streams_through() {
        use std::os::unix::process::ExitStatusExt;
        let captured = Captured {
            stdout: "** BUILD FAILED **\n".into(),
            stderr: "xcodebuild: error: Scheme Ghost not found\n".into(),
            status: std::process::ExitStatus::from_raw(256),
        };
        let out = XcodebuildBuild
            .parse(
                &captured,
                &XcodebuildBuild.prepare(argv(&["xcodebuild", "build"])),
            )
            .unwrap();
        assert!(out.passthrough_stdout.is_some());
        assert!(out.passthrough_stderr.is_some());
    }
}
