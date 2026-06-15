use super::pytest::parse_junit_named;
use super::report::TestReport;
use super::{basename, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct SwiftTest;

/// Flags that run no tests, so SwiftPM never writes xunit xml.
/// (`swift test list` is gated positionally in detect.)
const NON_TEST_ARGS: &[&str] = &["--version", "--help", "-h", "--list-tests", "-l"];

impl Adapter for SwiftTest {
    fn name(&self) -> &'static str {
        "swift-test"
    }
    fn matches(&self) -> &'static str {
        "swift test (injects --parallel: SwiftPM only writes XCTest xunit in parallel mode)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        let is_swift_test = matches!(argv, [first, second, ..]
            if basename(first) == "swift" && second == "test");
        // `list` is only a subcommand in position 2 — a filter value like
        // `--filter list` must not disable the adapter.
        let is_list = argv.get(2).map(String::as_str) == Some("list");
        // A user-supplied --xunit-output means they want the file themselves;
        // injecting a second one would silently steal it.
        is_swift_test
            && !is_list
            && !argv
                .iter()
                .any(|a| NON_TEST_ARGS.contains(&a.as_str()) || a.starts_with("--xunit-output"))
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        let artifact = tempfile::Builder::new()
            .prefix("cartoon-xunit-")
            .suffix(".xml")
            .tempfile()
            .ok();
        if let Some(f) = &artifact {
            argv.push("--xunit-output".into());
            argv.push(f.path().display().to_string());
            // SwiftPM (verified on 6.3.2) only writes the XCTest xunit file
            // under --parallel; without it XCTest results silently vanish.
            // Respect an explicit user choice either way.
            if !argv
                .iter()
                .any(|a| a == "--parallel" || a == "--no-parallel")
            {
                argv.push("--parallel".into());
            }
        }
        Prepared {
            argv,
            artifact: artifact.map(super::Artifact::File),
        }
    }
    fn parse(&self, _captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        let path = prepared
            .artifact_path()
            .context("swift-test adapter has no xunit artifact")?;
        let report = parse_xunit_pair(&path)?;
        Ok(ParseOutcome {
            report: super::AdapterReport::Tests(report),
            // stdout was build progress + test log, stderr the XCTest run
            // narration — both are the human report the TOON output replaces.
            // (Build failures never reach here: no xml -> parse error -> the
            // app's passthrough fallback shows the raw streams.)
            passthrough_stdout: None,
            passthrough_stderr: None,
        })
    }
}

/// Swift 6 writes XCTest results to the given path and Swift Testing results
/// to a sibling `<stem>-swift-testing.xml`. Either file may be absent or hold
/// zero testcases; merge whatever parsed.
fn parse_xunit_pair(path: &Path) -> Result<TestReport> {
    let mut merged: Option<TestReport> = None;
    let mut last_err: Option<anyhow::Error> = None;
    let sibling = swift_testing_sibling(path);
    for p in [path.to_path_buf(), sibling.clone()] {
        let Ok(xml) = std::fs::read_to_string(&p) else {
            continue;
        };
        match parse_junit_named(&xml, "swift-test") {
            Ok(r) => {
                merged = Some(match merged {
                    None => r,
                    Some(acc) => merge(acc, r),
                })
            }
            // One file may legitimately be empty (single-framework project);
            // keep the error so a fully unusable pair reports the real cause.
            Err(e) => last_err = Some(e),
        }
    }
    // The main artifact is a NamedTempFile, but the sibling SwiftPM created
    // next to it is ours to clean up.
    let _ = std::fs::remove_file(&sibling);
    match (merged, last_err) {
        (Some(r), _) => Ok(r),
        (None, Some(e)) => Err(e.context("swift xunit output unusable")),
        (None, None) => anyhow::bail!("no test cases in swift xunit output"),
    }
}

fn swift_testing_sibling(path: &Path) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    path.with_file_name(format!("{stem}-swift-testing.xml"))
}

fn merge(a: TestReport, b: TestReport) -> TestReport {
    TestReport {
        runner: a.runner,
        total: a.total + b.total,
        passed: a.passed + b.passed,
        failed: a.failed + b.failed,
        skipped: a.skipped + b.skipped,
        duration_s: a.duration_s + b.duration_s,
        failures: a.failures.into_iter().chain(b.failures).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn fixture(name: &str) -> String {
        let path = format!(
            "{}/tests/fixtures/swift-test/{}",
            env!("CARGO_MANIFEST_DIR"),
            name
        );
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn detects_swift_test_invocations() {
        assert!(SwiftTest.detect(&argv(&["swift", "test"])));
        assert!(SwiftTest.detect(&argv(&["/usr/bin/swift", "test", "--filter", "Auth"])));
        assert!(!SwiftTest.detect(&argv(&["swift", "build"])));
        assert!(!SwiftTest.detect(&argv(&["swift", "run"])));
        assert!(!SwiftTest.detect(&argv(&["swiftc", "test"])));
    }

    #[test]
    fn skips_non_test_invocations() {
        assert!(!SwiftTest.detect(&argv(&["swift", "test", "list"])));
        assert!(!SwiftTest.detect(&argv(&["swift", "test", "--list-tests"])));
        assert!(!SwiftTest.detect(&argv(&["swift", "test", "-l"])));
        assert!(!SwiftTest.detect(&argv(&["swift", "test", "--help"])));
        assert!(!SwiftTest.detect(&argv(&["swift", "test", "--version"])));
    }

    #[test]
    fn list_gated_positionally_not_as_filter_value() {
        assert!(SwiftTest.detect(&argv(&["swift", "test", "--filter", "list"])));
    }

    #[test]
    fn malformed_xml_error_is_propagated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.xml");
        std::fs::write(&path, "<testsuites><unclosed").unwrap();
        let err = format!("{:#}", parse_xunit_pair(&path).unwrap_err());
        assert!(err.contains("swift xunit output unusable"), "got: {err}");
    }

    #[test]
    fn respects_user_xunit_output() {
        assert!(!SwiftTest.detect(&argv(&["swift", "test", "--xunit-output", "out.xml"])));
        assert!(!SwiftTest.detect(&argv(&["swift", "test", "--xunit-output=out.xml"])));
    }

    #[test]
    fn prepare_appends_xunit_and_parallel_flags() {
        let p = SwiftTest.prepare(argv(&["swift", "test", "--filter", "Auth"]));
        assert_eq!(
            &p.argv[..4],
            &argv(&["swift", "test", "--filter", "Auth"])[..]
        );
        assert_eq!(p.argv[4], "--xunit-output");
        assert!(p.argv[5].ends_with(".xml"));
        assert_eq!(p.argv[6], "--parallel");
        assert!(p.artifact.is_some());
    }

    #[test]
    fn prepare_respects_user_parallel_choice() {
        let p = SwiftTest.prepare(argv(&["swift", "test", "--parallel"]));
        assert_eq!(p.argv.iter().filter(|a| *a == "--parallel").count(), 1);
        let p = SwiftTest.prepare(argv(&["swift", "test", "--no-parallel"]));
        assert!(!p.argv.iter().any(|a| a == "--parallel"));
    }

    #[test]
    fn parses_xctest_mixed_results() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.xml");
        std::fs::write(&path, fixture("mixed.xml")).unwrap();
        let r = parse_xunit_pair(&path).unwrap();
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (3, 2, 1, 0));
        assert_eq!(r.runner, "swift-test");
        assert!((r.duration_s - 0.245).abs() < 1e-9);
        let f = &r.failures[0];
        assert_eq!(f.id, "CartoonKitTests.AuthTests.testTokenExpiry");
        assert!(f.msg.starts_with("XCTAssertLessThan failed"));
    }

    #[test]
    fn merges_xctest_and_swift_testing_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.xml");
        std::fs::write(&path, fixture("mixed.xml")).unwrap();
        std::fs::write(
            dir.path().join("out-swift-testing.xml"),
            fixture("swift-testing.xml"),
        )
        .unwrap();
        let r = parse_xunit_pair(&path).unwrap();
        assert_eq!((r.total, r.failed), (5, 2));
        assert!(r
            .failures
            .iter()
            .any(|f| f.id == "CartoonKitTests.greetingMatches()"));
        // sibling cleaned up after parse
        assert!(!dir.path().join("out-swift-testing.xml").exists());
    }

    #[test]
    fn swift_testing_file_alone_is_enough() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.xml");
        std::fs::write(
            dir.path().join("out-swift-testing.xml"),
            fixture("swift-testing.xml"),
        )
        .unwrap();
        let r = parse_xunit_pair(&path).unwrap();
        assert_eq!((r.total, r.passed, r.failed), (2, 1, 1));
    }

    #[test]
    fn parses_all_pass() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.xml");
        std::fs::write(&path, fixture("all-pass.xml")).unwrap();
        let r = parse_xunit_pair(&path).unwrap();
        assert_eq!((r.total, r.passed, r.failed), (2, 2, 0));
        assert!(r.failures.is_empty());
    }

    #[test]
    fn missing_files_are_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(parse_xunit_pair(&dir.path().join("out.xml")).is_err());
    }
}
