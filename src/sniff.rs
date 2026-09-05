//! Content-based fallback for output that arrived without a matching argv0:
//! a `./build.sh` that runs xcodebuild internally, fastlane's gym log, a
//! runner that printed JUnit XML to stdout. Runs only on the no-adapter
//! path, is parse-only (never changes the child's argv), and its rendering
//! goes through the same net-savings guard as everything else.
use crate::adapters::report::{Failure, TestReport};
use regex::Regex;
use std::sync::OnceLock;

/// Rendered TOON plus a mode label for stats, or None when nothing matched.
pub fn sniff(stdout: &str, stderr: &str, exit: i32) -> Option<(String, &'static str)> {
    if let Some(r) = sniff_xctest(stdout, stderr) {
        return Some((
            crate::adapters::report::render(&r, 20, None),
            "sniff-xctest",
        ));
    }
    if let Some(v) = sniff_xcodebuild(stdout, stderr, exit) {
        return Some((crate::toon::encode(&v), "sniff-xcodebuild"));
    }
    if let Some(r) = sniff_junit(stdout) {
        return Some((crate::adapters::report::render(&r, 20, None), "sniff-junit"));
    }
    None
}

fn looks_like_xcodebuild(text: &str) -> bool {
    text.contains("** BUILD FAILED **")
        || text.contains("** BUILD SUCCEEDED **")
        || text.contains("Build settings from command line")
        || text.contains("** ARCHIVE FAILED **")
        || text.contains("** ARCHIVE SUCCEEDED **")
}

/// clang/swift diagnostics inside an xcodebuild-shaped log. A failed build
/// with zero diagnostics (signing, linker, missing scheme) is left to the
/// ladder so the real error is not hidden behind an empty table.
fn sniff_xcodebuild(stdout: &str, stderr: &str, exit: i32) -> Option<serde_json::Value> {
    if !looks_like_xcodebuild(stdout) && !looks_like_xcodebuild(stderr) {
        return None;
    }
    let (mut diags, mut errors, mut warnings) = crate::adapters::diagnostics::collect(stdout);
    let (d2, e2, w2) = crate::adapters::diagnostics::collect(stderr);
    diags.extend(d2);
    errors += e2;
    warnings += w2;
    if exit != 0 && errors + warnings == 0 {
        return None;
    }
    Some(crate::adapters::diagnostics::build_value(
        "xcodebuild-build",
        diags,
        errors,
        warnings,
    ))
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

/// XCTest's text protocol: `Test Case '-[Suite test]' passed|failed|skipped
/// (…)`, `path:line: error: -[Suite test] : msg`, and the final
/// `Executed N tests, with M failures … in T seconds` line.
fn sniff_xctest(stdout: &str, stderr: &str) -> Option<TestReport> {
    static EXECUTED: OnceLock<Regex> = OnceLock::new();
    static CASE: OnceLock<Regex> = OnceLock::new();
    static ERR: OnceLock<Regex> = OnceLock::new();
    let executed = re(
        &EXECUTED,
        r"Executed (\d+) tests?, with (\d+) failures? \(\d+ unexpected\) in ([0-9.]+) \(",
    );
    let case = re(
        &CASE,
        r"Test Case '-\[(\S+) (\S+)\]' (passed|failed|skipped)",
    );
    let err = re(
        &ERR,
        r"^(?P<loc>\S+:\d+): error: -\[(?P<suite>\S+) (?P<test>\S+)\] : (?P<msg>.*)$",
    );
    let text = if stdout.contains("Executed ") {
        stdout
    } else {
        stderr
    };
    let summary = executed.captures_iter(text).last()?;
    let total: u64 = summary[1].parse().ok()?;
    let failed_reported: u64 = summary[2].parse().ok()?;
    let duration_s: f64 = summary[3].parse().unwrap_or(0.0);

    let mut passed = 0u64;
    let mut failed = 0u64;
    let mut skipped = 0u64;
    let mut failures: Vec<Failure> = Vec::new();
    for c in case.captures_iter(text) {
        match &c[3] {
            "passed" => passed += 1,
            "skipped" => skipped += 1,
            _ => {
                failed += 1;
                failures.push(Failure {
                    id: format!("{}/{}", &c[1], &c[2]),
                    loc: String::new(),
                    msg: String::new(),
                    trace: Vec::new(),
                });
            }
        }
    }
    for line in text.lines() {
        if let Some(c) = err.captures(line.trim_end()) {
            let id = format!("{}/{}", &c["suite"], &c["test"]);
            if let Some(f) = failures.iter_mut().find(|f| f.id == id) {
                if f.loc.is_empty() {
                    f.loc = c["loc"].to_string();
                    f.msg = c["msg"].to_string();
                } else {
                    f.trace.push(format!("{}: {}", &c["loc"], &c["msg"]));
                }
            }
        }
    }
    // The summary line is authoritative for counts; per-case lines fill in
    // detail (nested suites print several "Executed" lines — we took the last).
    let failed = failed.max(failed_reported);
    let passed = if passed + failed + skipped == total {
        passed
    } else {
        total.saturating_sub(failed + skipped)
    };
    Some(TestReport {
        runner: "xcodebuild-test",
        total,
        passed,
        failed,
        skipped,
        duration_s,
        failures,
    })
}

/// JUnit/xUnit XML printed straight to stdout.
fn sniff_junit(stdout: &str) -> Option<TestReport> {
    let t = stdout.trim_start();
    if !(t.starts_with("<?xml") || t.starts_with("<testsuite")) {
        return None;
    }
    if !t.contains("<testsuite") {
        return None;
    }
    crate::adapters::pytest::parse_junit_named(t, "junit").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_xcodebuild_build_diagnostics_from_a_wrapper_script_log() {
        let log = "note: Building targets\n/Users/d/App/A.swift:18:9: error: cannot find 'tokn' in scope\n        tokn = refresh()\n** BUILD FAILED **\n";
        let (out, mode) = sniff(log, "", 65).unwrap();
        assert_eq!(mode, "sniff-xcodebuild");
        assert!(out.contains("A.swift:18:9"), "{out}");
        assert!(out.contains("errors: 1"), "{out}");
    }

    #[test]
    fn sniffs_xctest_summary_lines() {
        let log = "Test Suite 'All tests' started\nTest Case '-[AppTests.T testA]' passed (0.001 seconds).\nTest Case '-[AppTests.T testB]' failed (0.002 seconds).\n/Users/d/App/Tests/T.swift:12: error: -[AppTests.T testB] : XCTAssertEqual failed: (\"1\") is not equal to (\"2\")\nExecuted 2 tests, with 1 failure (0 unexpected) in 0.003 (0.010) seconds\n** TEST FAILED **\n";
        let (out, mode) = sniff(log, "", 65).unwrap();
        assert_eq!(mode, "sniff-xctest");
        assert!(out.contains("failed: 1"), "{out}");
        assert!(out.contains("passed: 1"), "{out}");
        assert!(out.contains("AppTests.T/testB"), "{out}");
        assert!(out.contains("T.swift:12"), "{out}");
        assert!(out.contains("XCTAssertEqual failed"), "{out}");
    }

    #[test]
    fn sniffs_junit_xml_on_stdout() {
        let xml = "<?xml version=\"1.0\"?><testsuite name=\"s\" tests=\"1\" time=\"0.1\"><testcase classname=\"c\" name=\"t\"/></testsuite>";
        let (out, mode) = sniff(xml, "", 0).unwrap();
        assert_eq!(mode, "sniff-junit");
        assert!(
            out.contains("runner: junit") && out.contains("total: 1"),
            "{out}"
        );
    }

    #[test]
    fn does_not_sniff_plain_text_or_unexplained_failures() {
        assert!(sniff("hello\nworld\n", "", 0).is_none());
        assert!(sniff("** BUILD FAILED **\n", "ld: symbol not found", 65).is_none());
        assert!(sniff("<html><body>not junit</body></html>", "", 0).is_none());
    }

    #[test]
    fn xcodebuild_success_without_diagnostics_still_summarizes() {
        let log = "Build settings from command line:\n    SDKROOT = iphoneos\nCompileSwift normal arm64 A.swift\n** BUILD SUCCEEDED **\n";
        let (out, mode) = sniff(log, "", 0).unwrap();
        assert_eq!(mode, "sniff-xcodebuild");
        assert!(out.contains("errors: 0"), "{out}");
    }
}
