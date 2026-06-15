use super::report::{Failure, TestReport};
use super::xcodebuild::{action, Action};
use super::{Adapter, Artifact, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct XcodebuildTest;

/// Absolute system `xcrun` shim — NEVER a bare `$PATH` lookup. cartoon spawns
/// this secondary tool on the user's behalf, so a poisoned PATH must not be
/// able to hijack it. `/usr/bin/xcrun` is macOS-provided and itself respects
/// the active toolchain via `xcode-select`.
const XCRUN: &str = "/usr/bin/xcrun";

/// Cap on xcresulttool stdout. The summary is counts + failures (never the
/// build log), so a realistic payload is tiny; this only bounds a pathological
/// one. Set high enough that real summaries never truncate (truncation →
/// parse error → passthrough, which would dump the whole raw log).
const MAX_SUMMARY_BYTES: u64 = 64 * 1024 * 1024;

impl Adapter for XcodebuildTest {
    fn name(&self) -> &'static str {
        "xcodebuild-test"
    }
    fn matches(&self) -> &'static str {
        "xcodebuild test / test-without-building (parses the .xcresult summary)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        action(argv) == Some(Action::Test)
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        // A user-supplied -resultBundlePath means they own the bundle; use it,
        // don't inject a second one, and never delete it (artifact stays None).
        if user_bundle_path(&argv).is_some() {
            return Prepared {
                argv,
                artifact: None,
            };
        }
        // xcodebuild refuses a pre-existing -resultBundlePath, so point it at a
        // not-yet-existing child of a fresh 0700 temp dir.
        match tempfile::Builder::new()
            .prefix("cartoon-xcresult-")
            .tempdir()
        {
            Ok(guard) => {
                let path = guard.path().join("result.xcresult");
                argv.push("-resultBundlePath".into());
                argv.push(path.display().to_string());
                Prepared {
                    argv,
                    artifact: Some(Artifact::Dir {
                        _guard: guard,
                        path,
                    }),
                }
            }
            // Temp dir creation failed: run unwrapped, parse() will passthrough.
            Err(_) => Prepared {
                argv,
                artifact: None,
            },
        }
    }
    fn parse(&self, _captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        let bundle = prepared
            .artifact_path()
            .or_else(|| user_bundle_path(&prepared.argv))
            .context("no xcresult bundle path")?;
        if !bundle.exists() {
            anyhow::bail!("xcresult bundle missing (build likely failed)");
        }
        let json = run_xcresulttool(&bundle)?;
        let report = parse_summary_json(&json)?;
        // W3: "no tests ran" (build broke, or a filter matched nothing) is not
        // our job — passthrough so the agent sees the real output.
        if report.total == 0 {
            anyhow::bail!("xcresult reported zero tests");
        }
        Ok(ParseOutcome {
            report: super::AdapterReport::Tests(report),
            // xcodebuild's human log was stdout/stderr — replaced by the summary.
            passthrough_stdout: None,
            passthrough_stderr: None,
        })
    }
}

/// Extract a user-supplied `-resultBundlePath` (space or `=` form).
fn user_bundle_path(argv: &[String]) -> Option<PathBuf> {
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix("-resultBundlePath=") {
            return Some(PathBuf::from(v));
        }
        if a == "-resultBundlePath" {
            return it.next().map(PathBuf::from);
        }
    }
    None
}

/// Run `xcrun xcresulttool get test-results summary` against the bundle and
/// return its JSON. Argv is a vector (no shell); the bundle path is passed as
/// `--path=<value>` (N3) so a value beginning with `-` can't pose as a flag.
fn run_xcresulttool(bundle: &Path) -> Result<String> {
    use std::io::Read;
    let mut child = Command::new(XCRUN)
        .args([
            "xcresulttool",
            "get",
            "test-results",
            "summary",
            "--format",
            "json",
        ])
        .arg(format!("--path={}", bundle.display()))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn xcrun xcresulttool")?;

    // Drain stderr on a thread so a >64KB stderr write can't block the child
    // while we read stdout and then wait() — the classic pipe-buffer deadlock.
    let mut err_pipe = child.stderr.take().expect("stderr piped");
    let err_thread = std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = err_pipe.read_to_end(&mut sink);
    });

    // Bounded read: at most MAX_SUMMARY_BYTES+1 ever enters memory.
    let mut buf = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout piped")
        .take(MAX_SUMMARY_BYTES + 1)
        .read_to_end(&mut buf)?;
    let status = child.wait()?;
    let _ = err_thread.join();
    if !status.success() {
        anyhow::bail!("xcresulttool exited with failure");
    }
    if buf.len() as u64 > MAX_SUMMARY_BYTES {
        anyhow::bail!("xcresult summary exceeds {MAX_SUMMARY_BYTES} bytes");
    }
    String::from_utf8(buf).context("xcresulttool output not UTF-8")
}

#[derive(Deserialize)]
struct Summary {
    #[serde(rename = "totalTestCount", default)]
    total: u64,
    #[serde(rename = "passedTests", default)]
    passed: u64,
    #[serde(rename = "failedTests", default)]
    failed: u64,
    #[serde(rename = "skippedTests", default)]
    skipped: u64,
    #[serde(rename = "startTime", default)]
    start_time: f64,
    #[serde(rename = "finishTime", default)]
    finish_time: f64,
    #[serde(rename = "testFailures", default)]
    test_failures: Vec<SummaryFailure>,
}

#[derive(Deserialize)]
struct SummaryFailure {
    #[serde(rename = "failureText", default)]
    failure_text: String,
    #[serde(rename = "targetName", default)]
    target_name: String,
    #[serde(rename = "testIdentifierString", default)]
    test_identifier: String,
    #[serde(rename = "testName", default)]
    test_name: String,
}

/// Pure parse of the `test-results summary` JSON into the shared report. No
/// I/O — the CI-testable seam. Lenient: unknown fields ignored, missing fields
/// default, so minor schema drift degrades gracefully rather than erroring.
pub fn parse_summary_json(json: &str) -> Result<TestReport> {
    let s: Summary = serde_json::from_str(json).context("invalid xcresult summary json")?;
    let duration_s = (s.finish_time - s.start_time).max(0.0);
    let failures = s
        .test_failures
        .into_iter()
        .map(|f| {
            let id_tail = if f.test_identifier.is_empty() {
                f.test_name
            } else {
                f.test_identifier
            };
            let id = if f.target_name.is_empty() {
                id_tail
            } else {
                format!("{}.{}", f.target_name, id_tail)
            };
            Failure {
                id,
                // Summary carries no file:line — only a test:// URL. Empty loc,
                // consistent with swift-test's xunit.
                loc: String::new(),
                msg: f.failure_text,
                trace: Vec::new(),
            }
        })
        .collect();
    Ok(TestReport {
        runner: "xcodebuild-test",
        total: s.total,
        passed: s.passed,
        failed: s.failed,
        skipped: s.skipped,
        duration_s,
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn fixture(name: &str) -> String {
        let path = format!(
            "{}/tests/fixtures/xcodebuild/{}",
            env!("CARGO_MANIFEST_DIR"),
            name
        );
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn detects_test_action() {
        assert!(XcodebuildTest.detect(&argv(&["xcodebuild", "test"])));
        assert!(XcodebuildTest.detect(&argv(&["xcodebuild", "-project", "X", "test"])));
        assert!(!XcodebuildTest.detect(&argv(&["xcodebuild", "build"])));
        assert!(!XcodebuildTest.detect(&argv(&["swift", "test"])));
    }

    #[test]
    fn prepare_injects_bundle_under_tempdir() {
        let p = XcodebuildTest.prepare(argv(&["xcodebuild", "test", "-scheme", "App"]));
        let i = p
            .argv
            .iter()
            .position(|a| a == "-resultBundlePath")
            .unwrap();
        assert!(p.argv[i + 1].ends_with("result.xcresult"));
        // path is a not-yet-existing child of an existing 0700 temp dir.
        let bundle = Path::new(&p.argv[i + 1]);
        assert!(!bundle.exists());
        assert!(bundle.parent().unwrap().exists());
        assert!(p.artifact.is_some());
    }

    #[test]
    fn prepare_respects_user_bundle_path() {
        for form in [
            argv(&["xcodebuild", "test", "-resultBundlePath", "mine.xcresult"]),
            argv(&["xcodebuild", "test", "-resultBundlePath=mine.xcresult"]),
        ] {
            let p = XcodebuildTest.prepare(form);
            assert_eq!(
                p.argv
                    .iter()
                    .filter(|a| a.starts_with("-resultBundlePath"))
                    .count(),
                1,
                "must not inject a second bundle path"
            );
            assert!(p.artifact.is_none(), "user's bundle is not ours to delete");
        }
    }

    #[test]
    fn parses_mixed_summary() {
        let r = parse_summary_json(&fixture("summary-mixed.json")).unwrap();
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (4, 2, 2, 0));
        assert_eq!(r.runner, "xcodebuild-test");
        assert!((r.duration_s - 14.121).abs() < 0.01, "got {}", r.duration_s);
        assert_eq!(r.failures.len(), 2);
        let f = &r.failures[0];
        assert_eq!(f.id, "SwiftDemoTests.greetingMatches()");
        assert!(f.msg.starts_with("Expectation failed"));
        assert!(f.loc.is_empty());
        assert_eq!(
            r.failures[1].id,
            "SwiftDemoTests.GreeterXCTests/testGreetingExact()"
        );
    }

    #[test]
    fn parses_all_pass_summary() {
        let r = parse_summary_json(&fixture("summary-all-pass.json")).unwrap();
        assert_eq!((r.total, r.passed, r.failed), (3, 3, 0));
        assert!(r.failures.is_empty());
    }

    #[test]
    fn zero_test_summary_parses_but_total_is_zero() {
        // The pure parser is total-agnostic; parse() turns total==0 into a
        // passthrough. This guards the W3 discriminator input.
        let r = parse_summary_json(&fixture("summary-zero-tests.json")).unwrap();
        assert_eq!(r.total, 0);
    }

    #[test]
    fn malformed_json_is_error_not_panic() {
        assert!(parse_summary_json("{not json").is_err());
        assert!(parse_summary_json("").is_err());
    }

    #[test]
    fn overflowing_timestamps_do_not_panic_render() {
        // Two finite-but-extreme timestamps whose DIFFERENCE overflows to
        // +inf (both values parse fine; the subtraction is the hazard).
        let json = r#"{"totalTestCount":1,"passedTests":1,"failedTests":0,
            "skippedTests":0,"startTime":-1e308,"finishTime":1e308,"testFailures":[]}"#;
        let r = parse_summary_json(json).unwrap();
        assert!(!r.duration_s.is_finite(), "precondition: diff overflowed");
        // report::render must clamp it rather than panic on a non-finite f64.
        let out = super::super::report::render(&r, 5, None);
        assert!(out.contains("duration_s: 0"));
    }

    #[test]
    fn user_bundle_path_extracted_both_forms() {
        assert_eq!(
            user_bundle_path(&argv(&["xcodebuild", "-resultBundlePath", "a.xcresult"])),
            Some(PathBuf::from("a.xcresult"))
        );
        assert_eq!(
            user_bundle_path(&argv(&["xcodebuild", "-resultBundlePath=b.xcresult"])),
            Some(PathBuf::from("b.xcresult"))
        );
        assert_eq!(user_bundle_path(&argv(&["xcodebuild", "test"])), None);
    }
}
