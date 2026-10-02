use super::xcodebuild::{action, Action};
use super::{diagnostics, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::Result;

pub struct XcodebuildBuild;

impl Adapter for XcodebuildBuild {
    fn name(&self) -> &'static str {
        "xcodebuild-build"
    }
    fn matches(&self) -> &'static str {
        "xcodebuild build / build-for-testing / archive / -exportArchive (no test action)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        matches!(action(argv), Some(Action::Build) | Some(Action::Archive))
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
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        // xcodebuild emits the same clang format as swift. A failed build with
        // no located error (linker, signing, missing scheme, ...) must not be
        // swallowed — even when a warning matched — so its tool-level errors
        // are counted and the raw streams pass through.
        let runner = if action(&prepared.argv) == Some(Action::Archive) {
            "xcodebuild-archive"
        } else {
            "xcodebuild-build"
        };
        Ok(diagnostics::parse_build_streams(runner, captured))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::AdapterReport;

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
        assert!(XcodebuildBuild.detect(&argv(&["xcodebuild", "archive", "-scheme", "A"])));
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
        let AdapterReport::Value(v) = out.report else {
            panic!("expected value report")
        };
        assert_eq!(v["summary"]["errors"], 1);
    }

    #[test]
    fn warning_does_not_mask_a_signing_failure() {
        use std::os::unix::process::ExitStatusExt;
        let captured = Captured {
            stdout: "\
/Users/dev/App/Sources/App/View.swift:7:1: warning: 'NavigationView' is deprecated
error: Signing for \"App\" requires a development team. Select a development team in the Signing & Capabilities editor. (in target 'App' from project 'App')
** BUILD FAILED **
"
            .into(),
            stderr: String::new(),
            status: std::process::ExitStatus::from_raw(16640), // exit 65
        };
        let out = XcodebuildBuild
            .parse(
                &captured,
                &XcodebuildBuild.prepare(argv(&["xcodebuild", "build"])),
            )
            .unwrap();
        let AdapterReport::Value(v) = &out.report else {
            panic!("expected value report")
        };
        assert_eq!(v["summary"]["errors"], 1, "{v}");
        assert_eq!(v["summary"]["warnings"], 1);
        assert!(out
            .passthrough_stdout
            .as_deref()
            .is_some_and(|s| s.contains("Signing for")));
    }
}
