//! `phpunit` adapter — injects `--log-junit` and parses the resulting JUnit
//! xml with the shared parser (see `pytest::parse_junit_named`).
use super::pytest::parse_junit_named;
use super::{basename, Adapter, AdapterReport, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use std::path::PathBuf;

pub struct Phpunit;

/// Flags that run no tests, so phpunit never writes junit xml.
const NON_TEST_FLAGS: &[&str] = &[
    "--version",
    "--help",
    "--list-tests",
    "--list-tests-xml",
    "--list-test-files",
    "--list-suites",
    "--list-groups",
    "--generate-configuration",
    "--migrate-configuration",
    "--check-version",
    "--atleast-version",
    "--warm-coverage-cache",
];

impl Adapter for Phpunit {
    fn name(&self) -> &'static str {
        "phpunit"
    }
    fn matches(&self) -> &'static str {
        "phpunit | vendor/bin/phpunit (--log-junit)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        let Some(first) = argv.first() else {
            return false;
        };
        let is_phpunit = basename(first) == "phpunit"
            || (basename(first).starts_with("php")
                && skip_php_options(argv)
                    .first()
                    .map(|a| basename(a) == "phpunit")
                    .unwrap_or(false));
        if !is_phpunit {
            return false;
        }
        // Informational invocations run no tests, so phpunit exits before
        // writing junit xml — injecting it only buys a parse warning. A
        // user-supplied --log-junit is fine: prepare()/parse() read it
        // directly instead of injecting a second one.
        !argv.iter().any(|a| NON_TEST_FLAGS.contains(&a.as_str()))
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        if log_junit_flag_present(&argv) {
            // User already wants the file themselves; parse() reads their
            // path directly rather than injecting a second one that would
            // silently steal it.
            return Prepared {
                argv,
                artifact: None,
            };
        }
        let artifact = tempfile::Builder::new()
            .prefix("cartoon-phpunit-")
            .suffix(".xml")
            .tempfile()
            .ok();
        if let Some(f) = &artifact {
            // A trailing `--` separator means later args are positional
            // (test file paths); appending after it would hand phpunit
            // `--log-junit`/the path as positional args instead of flags, so
            // insert before it rather than appending blindly.
            let insert_at = argv.iter().position(|a| a == "--").unwrap_or(argv.len());
            argv.insert(insert_at, f.path().display().to_string());
            argv.insert(insert_at, "--log-junit".into());
        }
        Prepared {
            argv,
            artifact: artifact.map(super::Artifact::File),
        }
    }
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        let path = prepared
            .artifact_path()
            .or_else(|| user_log_junit_path(&prepared.argv))
            .context("phpunit adapter has no junit artifact")?;
        let xml = std::fs::read_to_string(&path).context("junit xml missing")?;
        let mut report = parse_junit_named(&xml, "phpunit")?;
        // PHPUnit nests per-class <testsuite> elements inside one top-level
        // <testsuite> whose own `time` is already the rolled-up total; the
        // shared parser sums `time` over every descendant testsuite, double
        // counting that rollup. Prefer the top-level suite's own time.
        if let Some(time) = top_level_suite_duration(&xml) {
            report.duration_s = time;
        }
        // Nonzero exit with no failed test (fatal error, no tests executed,
        // risky/incomplete under --fail-on-*): keep the human report.
        let unexplained = !captured.status.success() && report.failed == 0;
        Ok(ParseOutcome {
            report: AdapterReport::Tests(report),
            // stdout was phpunit's human report (dots/progress + summary) —
            // consumed unless the failure is otherwise unexplained. stderr
            // may hold deprecation notices or config errors.
            passthrough_stdout: (unexplained && !captured.stdout.is_empty())
                .then(|| captured.stdout.clone()),
            passthrough_stderr: (!captured.stderr.is_empty()).then(|| captured.stderr.clone()),
        })
    }
}

/// Skip php CLI interpreter options that can precede the script argument, so
/// `php -d memory_limit=-1 vendor/bin/phpunit` still finds `phpunit` at the
/// front: `-d <val>` / `-d<val>` (ini override), `-c <file>` / `-c<file>`
/// (config file), `-n` (no php.ini). Assumes `argv[0]` is the interpreter.
fn skip_php_options(argv: &[String]) -> &[String] {
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "-n" => i += 1,
            "-d" | "-c" => i += 2,
            a if a.starts_with("-d") || a.starts_with("-c") => i += 1,
            _ => break,
        }
    }
    &argv[i..]
}

fn log_junit_flag_present(argv: &[String]) -> bool {
    argv.iter()
        .any(|a| a == "--log-junit" || a.starts_with("--log-junit="))
}

/// Extract the path from a user-supplied `--log-junit <path>` or
/// `--log-junit=<path>`, when we didn't inject our own.
fn user_log_junit_path(argv: &[String]) -> Option<PathBuf> {
    for (i, a) in argv.iter().enumerate() {
        if let Some(v) = a.strip_prefix("--log-junit=") {
            return Some(PathBuf::from(v));
        }
        if a == "--log-junit" {
            return argv.get(i + 1).map(PathBuf::from);
        }
    }
    None
}

/// PHPUnit's junit xml wraps per-class suites in one top-level `<testsuite>`
/// whose own `time` attribute is already the rolled-up total. `None` if the
/// xml doesn't parse or has no testsuite element at all.
fn top_level_suite_duration(xml: &str) -> Option<f64> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    let top = if root.has_tag_name("testsuite") {
        root
    } else {
        root.children().find(|c| c.has_tag_name("testsuite"))?
    };
    top.attribute("time")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 3 passing tests across 2 per-class suites, nested in the single
    // top-level suite real phpunit output wraps everything in.
    const ALL_PASS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites>
  <testsuite name="Project Test Suite" tests="3" assertions="3" failures="0" errors="0" skipped="0" time="0.005">
    <testsuite name="App\Tests\CalcTest" file="/proj/tests/CalcTest.php" tests="2" assertions="2" failures="0" errors="0" skipped="0" time="0.004">
      <testcase name="testAdd" class="App\Tests\CalcTest" classname="App.Tests.CalcTest" file="/proj/tests/CalcTest.php" line="12" assertions="1" time="0.002"/>
      <testcase name="testSubtract" class="App\Tests\CalcTest" classname="App.Tests.CalcTest" file="/proj/tests/CalcTest.php" line="20" assertions="1" time="0.002"/>
    </testsuite>
    <testsuite name="App\Tests\StringTest" file="/proj/tests/StringTest.php" tests="1" assertions="1" failures="0" errors="0" skipped="0" time="0.001">
      <testcase name="testConcat" class="App\Tests\StringTest" classname="App.Tests.StringTest" file="/proj/tests/StringTest.php" line="9" assertions="1" time="0.001"/>
    </testsuite>
  </testsuite>
</testsuites>"#;

    // Mixed: 1 failure (ExpectationFailedException, multi-line message plus a
    // trace-frame line), 1 error (undefined function), 1 skipped, 2 passed —
    // also nested under one top-level suite (see ALL_PASS_XML).
    const MIXED_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites>
  <testsuite name="Project Test Suite" tests="5" assertions="3" failures="1" errors="1" skipped="1" time="0.015">
    <testsuite name="App\Tests\CalcTest" file="/proj/tests/CalcTest.php" tests="3" assertions="2" failures="1" errors="0" skipped="1" time="0.010">
      <testcase name="testAdd" class="App\Tests\CalcTest" classname="App.Tests.CalcTest" file="/proj/tests/CalcTest.php" line="12" assertions="1" time="0.002"/>
      <testcase name="testDivideByZero" class="App\Tests\CalcTest" classname="App.Tests.CalcTest" file="/proj/tests/CalcTest.php" line="30" assertions="1" time="0.003">
        <failure type="PHPUnit\Framework\ExpectationFailedException">App\Tests\CalcTest::testDivideByZero
Failed asserting that 0.0 matches expected 1.0.
This is a second line of detail.

/proj/tests/CalcTest.php:14
</failure>
      </testcase>
      <testcase name="testSkippedFeature" class="App\Tests\CalcTest" classname="App.Tests.CalcTest" file="/proj/tests/CalcTest.php" line="40" assertions="0" time="0.0">
        <skipped/>
      </testcase>
    </testsuite>
    <testsuite name="App\Tests\StringTest" file="/proj/tests/StringTest.php" tests="2" assertions="1" failures="0" errors="1" skipped="0" time="0.005">
      <testcase name="testConcat" class="App\Tests\StringTest" classname="App.Tests.StringTest" file="/proj/tests/StringTest.php" line="9" assertions="1" time="0.002"/>
      <testcase name="testUndefinedFn" class="App\Tests\StringTest" classname="App.Tests.StringTest" file="/proj/tests/StringTest.php" line="18" assertions="0" time="0.003">
        <error type="Error">Error: Call to undefined function App\Tests\str_frobnicate()

/proj/tests/StringTest.php:18
</error>
      </testcase>
    </testsuite>
  </testsuite>
</testsuites>"#;

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detects_direct_and_vendor_invocations() {
        assert!(Phpunit.detect(&argv(&["phpunit"])));
        assert!(Phpunit.detect(&argv(&["vendor/bin/phpunit", "--testsuite", "unit"])));
        assert!(Phpunit.detect(&argv(&["./vendor/bin/phpunit"])));
        assert!(Phpunit.detect(&argv(&["php", "vendor/bin/phpunit"])));
    }

    #[test]
    fn detects_php_with_interpreter_options_before_phpunit() {
        assert!(Phpunit.detect(&argv(&[
            "php",
            "-d",
            "memory_limit=-1",
            "vendor/bin/phpunit"
        ])));
        assert!(Phpunit.detect(&argv(&["php", "-dmemory_limit=-1", "vendor/bin/phpunit"])));
        assert!(Phpunit.detect(&argv(&["php", "-n", "vendor/bin/phpunit"])));
        assert!(Phpunit.detect(&argv(&["php", "-c", "php.ini", "vendor/bin/phpunit"])));
        // Still correctly rejects a bare interpreter invocation.
        assert!(!Phpunit.detect(&argv(&["php", "-v"])));
    }

    #[test]
    fn detects_but_reads_users_own_log_junit_file() {
        assert!(Phpunit.detect(&argv(&["phpunit", "--log-junit", "custom.xml"])));
        assert!(Phpunit.detect(&argv(&["phpunit", "--log-junit=custom.xml"])));

        let dir = tempfile::tempdir().unwrap();
        let user_path = dir.path().join("custom.xml");
        std::fs::write(&user_path, ALL_PASS_XML).unwrap();

        let prepared = Phpunit.prepare(vec![
            "phpunit".into(),
            "--log-junit".into(),
            user_path.display().to_string(),
        ]);
        // No injection: argv untouched, no owned artifact.
        assert_eq!(
            prepared.argv,
            vec![
                "phpunit".to_string(),
                "--log-junit".to_string(),
                user_path.display().to_string(),
            ]
        );
        assert!(prepared.artifact.is_none());

        let cap = Captured::synthetic(String::new(), String::new());
        let out = Phpunit.parse(&cap, &prepared).unwrap();
        let AdapterReport::Tests(report) = out.report else {
            panic!("expected tests report")
        };
        assert_eq!(report.total, 3);
    }

    #[test]
    fn skips_non_phpunit_invocations() {
        assert!(!Phpunit.detect(&argv(&["php", "-v"])));
        assert!(!Phpunit.detect(&argv(&["pytest"])));
    }

    #[test]
    fn skips_informational_invocations() {
        for flag in NON_TEST_FLAGS {
            assert!(
                !Phpunit.detect(&argv(&["phpunit", flag])),
                "should skip phpunit {flag}"
            );
        }
    }

    #[test]
    fn prepare_appends_log_junit_flag_and_artifact() {
        let p = Phpunit.prepare(vec!["phpunit".into(), "--testsuite".into(), "unit".into()]);
        assert_eq!(p.argv[0], "phpunit");
        assert_eq!(p.argv[1], "--testsuite");
        assert_eq!(p.argv[2], "unit");
        assert_eq!(p.argv[3], "--log-junit");
        assert_eq!(p.argv[4], p.artifact_path().unwrap().display().to_string());
        assert!(p.artifact.is_some());
    }

    #[test]
    fn prepare_inserts_log_junit_before_double_dash_separator() {
        let p = Phpunit.prepare(vec![
            "phpunit".into(),
            "--".into(),
            "tests/CalcTest.php".into(),
        ]);
        assert_eq!(p.argv[0], "phpunit");
        assert_eq!(p.argv[1], "--log-junit");
        assert_eq!(p.argv[2], p.artifact_path().unwrap().display().to_string());
        assert_eq!(p.argv[3], "--");
        assert_eq!(p.argv[4], "tests/CalcTest.php");
    }

    #[test]
    fn parses_all_pass_results() {
        let r = parse_junit_named(ALL_PASS_XML, "phpunit").unwrap();
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (3, 3, 0, 0));
        assert!(r.failures.is_empty());
        // Raw duration double-counts the rolled-up outer suite (0.005) plus
        // its two children (0.004 + 0.001) = 0.010; the adapter corrects
        // this post-parse (see corrects_double_counted_duration_from_nested_suites).
        assert!(
            (r.duration_s - 0.010).abs() < 1e-9,
            "duration_s: {}",
            r.duration_s
        );
    }

    #[test]
    fn parses_mixed_results() {
        let r = parse_junit_named(MIXED_XML, "phpunit").unwrap();
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (5, 2, 2, 1));
        assert_eq!(r.failures.len(), 2);

        let failure = &r.failures[0];
        assert_eq!(failure.id, "/proj/tests/CalcTest.php::testDivideByZero");
        // PHPUnit's `line` is already the real (1-based) source line; the
        // shared parser only shifts pytest's 0-based attribute.
        assert_eq!(failure.loc, "/proj/tests/CalcTest.php:30");
        // No `message` attribute: the message is the first line of the text.
        assert!(
            failure
                .msg
                .contains("Failed asserting that 0.0 matches expected 1.0."),
            "msg: {:?}",
            failure.msg
        );
        assert!(failure
            .trace
            .iter()
            .any(|l| l.contains("Failed asserting that 0.0 matches expected 1.0.")));
        assert!(failure
            .trace
            .iter()
            .any(|l| l == "/proj/tests/CalcTest.php:14"));

        let error = &r.failures[1];
        assert_eq!(error.id, "/proj/tests/StringTest.php::testUndefinedFn");
        assert!(error
            .trace
            .iter()
            .any(|l| l.contains("Call to undefined function")));
    }

    #[test]
    fn corrects_double_counted_duration_from_nested_suites() {
        assert_eq!(top_level_suite_duration(ALL_PASS_XML), Some(0.005));
        assert_eq!(top_level_suite_duration(MIXED_XML), Some(0.015));
    }

    #[test]
    fn parse_applies_corrected_duration_to_report() {
        let prepared = Phpunit.prepare(vec!["phpunit".into()]);
        std::fs::write(prepared.artifact_path().unwrap(), MIXED_XML).unwrap();
        let cap = Captured::synthetic(String::new(), String::new());
        let out = Phpunit.parse(&cap, &prepared).unwrap();
        let AdapterReport::Tests(report) = out.report else {
            panic!("expected tests report")
        };
        assert_eq!(report.duration_s, 0.015);
    }

    #[test]
    fn empty_xml_is_parse_error() {
        assert!(parse_junit_named("", "phpunit").is_err());
    }

    #[test]
    fn parse_errors_when_artifact_missing() {
        let prepared = Prepared {
            argv: vec!["phpunit".into()],
            artifact: None,
        };
        let captured = Captured::synthetic(String::new(), String::new());
        assert!(Phpunit.parse(&captured, &prepared).is_err());
    }

    fn status_fail() -> std::process::ExitStatus {
        std::process::Command::new("false").status().unwrap()
    }

    #[test]
    fn fatal_exit_without_failed_tests_keeps_the_human_report() {
        let prepared = Phpunit.prepare(vec!["phpunit".into()]);
        std::fs::write(
            prepared.artifact_path().unwrap(),
            "<testsuites><testsuite name=\"s\" tests=\"1\" time=\"0.1\"><testcase name=\"t\" classname=\"c\"/></testsuite></testsuites>",
        )
        .unwrap();
        let cap = Captured {
            stdout: "PHPUnit 11.2.0\n\nPHP Fatal error:  Uncaught Error: Class \"Foo\" not found\n"
                .into(),
            stderr: String::new(),
            status: status_fail(),
        };
        let out = Phpunit.parse(&cap, &prepared).unwrap();
        assert!(out
            .passthrough_stdout
            .as_deref()
            .is_some_and(|s| s.contains("Fatal error")));
    }
}
