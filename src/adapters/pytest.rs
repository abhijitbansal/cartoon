use super::report::{Failure, TestReport};
use super::{basename, is_python_module, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};

pub struct Pytest;

/// Flags that make pytest informational (no test session, no junit xml).
const NON_TEST_FLAGS: &[&str] = &[
    "--version",
    "-V",
    "--help",
    "-h",
    "--collect-only",
    "--co",
    "--fixtures",
    "--markers",
];

impl Adapter for Pytest {
    fn name(&self) -> &'static str {
        "pytest"
    }
    fn matches(&self) -> &'static str {
        "pytest | python -m pytest | uv run [-m] pytest | uvx pytest"
    }
    fn detect(&self, full: &[String]) -> bool {
        // Look past a `uv run` / `uvx` wrapper: uv forwards our appended flags
        // straight through to pytest, so detection is the only thing that needs
        // to see the inner command.
        let argv = super::strip_uv_run(full);
        // A shorter slice means a uv wrapper was stripped; only then is a
        // leading `-m pytest` (uv's own module form) a pytest invocation.
        let uv_wrapped = argv.len() != full.len();
        let is_pytest = argv
            .first()
            .map(|a| basename(a) == "pytest")
            .unwrap_or(false)
            || is_python_module(argv, "pytest")
            || (uv_wrapped && super::is_module_run(argv, "pytest"));
        // Informational invocations run no tests, so pytest exits before
        // writing junit xml — injecting it only buys a parse warning.
        is_pytest && !argv.iter().any(|a| NON_TEST_FLAGS.contains(&a.as_str()))
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        let artifact = tempfile::Builder::new()
            .prefix("cartoon-junit-")
            .suffix(".xml")
            .tempfile()
            .ok();
        if let Some(f) = &artifact {
            argv.push(format!("--junit-xml={}", f.path().display()));
            argv.push("--override-ini=junit_family=legacy".into());
        }
        Prepared {
            argv,
            artifact: artifact.map(super::Artifact::File),
        }
    }
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        let path = prepared
            .artifact_path()
            .context("pytest adapter has no junit artifact")?;
        let xml = std::fs::read_to_string(&path).context("junit xml missing")?;
        let report = parse_junit(&xml)?;
        Ok(ParseOutcome {
            report: super::AdapterReport::Tests(report),
            // stdout was pytest's human report — consumed. stderr may hold
            // user warnings the agent needs.
            passthrough_stdout: None,
            passthrough_stderr: (!captured.stderr.is_empty()).then(|| captured.stderr.clone()),
        })
    }
    fn fast_args(&self) -> Vec<String> {
        vec!["-n".into(), "auto".into()]
    }
}

pub fn parse_junit(xml: &str) -> Result<TestReport> {
    parse_junit_named(xml, "pytest")
}

pub fn parse_junit_named(xml: &str, runner: &'static str) -> Result<TestReport> {
    let doc = roxmltree::Document::parse(xml).context("invalid junit xml")?;
    // pytest's junit-xml writes a 0-based `line`; every other producer
    // (phpunit, gradle, swift) follows the spec's 1-based convention.
    let line_offset: i64 = if runner == "pytest" { 1 } else { 0 };
    let mut duration_s = 0.0;
    for suite in doc.descendants().filter(|n| n.has_tag_name("testsuite")) {
        duration_s += suite
            .attribute("time")
            .and_then(|t| t.parse::<f64>().ok())
            .unwrap_or(0.0);
    }
    let (mut total, mut passed, mut failed, mut skipped) = (0u64, 0u64, 0u64, 0u64);
    let mut failures = Vec::new();
    for case in doc.descendants().filter(|n| n.has_tag_name("testcase")) {
        total += 1;
        let name = case.attribute("name").unwrap_or("?");
        let file = case.attribute("file").unwrap_or("");
        let line = case
            .attribute("line")
            .and_then(|l| l.parse::<i64>().ok())
            .map(|l| l + line_offset);
        let id = if file.is_empty() {
            format!("{}.{}", case.attribute("classname").unwrap_or(""), name)
        } else {
            format!("{file}::{name}")
        };
        let fail_node = case
            .children()
            .find(|c| c.has_tag_name("failure") || c.has_tag_name("error"));
        if let Some(fail) = fail_node {
            failed += 1;
            let mut msg = fail
                .attribute("message")
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            let trace = super::report::trim_trace(fail.text().unwrap_or(""));
            // "collection failure" hides the real error (ImportError etc.);
            // promote pytest's `E ...` line — the actual exception — to msg.
            if msg.is_empty() || msg == "collection failure" {
                if let Some(e) = trace.iter().find(|l| l.starts_with("E ")) {
                    msg = e[1..].trim_start().to_string();
                }
            }
            // Producers without a `message` attribute (phpunit, some JVM
            // runners) put the message in the element text, often after a
            // `Class::method` header line: use the first line that is not it.
            if msg.is_empty() {
                let header_suffix = format!("::{name}");
                if let Some(first) = trace.iter().find(|l| !l.ends_with(&header_suffix)) {
                    msg = first.clone();
                }
            }
            let loc = match line {
                Some(l) if !file.is_empty() => format!("{file}:{l}"),
                _ => file.to_string(),
            };
            failures.push(Failure {
                id,
                loc,
                msg,
                trace,
            });
        } else if case.children().any(|c| c.has_tag_name("skipped")) {
            skipped += 1;
        } else {
            passed += 1;
        }
    }
    if total == 0 {
        anyhow::bail!("junit xml contained no testcases");
    }
    Ok(TestReport {
        runner,
        total,
        passed,
        failed,
        skipped,
        duration_s,
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_fixture(name: &str) -> crate::adapters::report::TestReport {
        let path = format!(
            "{}/tests/fixtures/pytest/{}",
            env!("CARGO_MANIFEST_DIR"),
            name
        );
        parse_junit(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn prepare_appends_junit_flag() {
        let p = Pytest.prepare(vec!["pytest".into(), "-q".into()]);
        assert_eq!(p.argv[0], "pytest");
        assert_eq!(p.argv[1], "-q");
        assert!(p.argv[2].starts_with("--junit-xml="));
        assert_eq!(p.argv[3], "--override-ini=junit_family=legacy");
        assert!(p.artifact.is_some());
    }

    #[test]
    fn parses_mixed_results() {
        let r = parse_fixture("mixed.xml");
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (3, 1, 1, 1));
        assert_eq!(r.duration_s, 0.123);
        let f = &r.failures[0];
        assert_eq!(f.id, "tests/test_auth.py::test_expiry");
        assert_eq!(f.loc, "tests/test_auth.py:42"); // 0-based line 41 + 1
        assert_eq!(f.msg, "AssertionError: assert exp < now");
        assert!(f.trace.iter().any(|l| l.contains("assert token.exp")));
    }

    #[test]
    fn parses_all_pass() {
        let r = parse_fixture("all-pass.xml");
        assert_eq!((r.total, r.passed, r.failed), (2, 2, 0));
        assert!(r.failures.is_empty());
    }

    #[test]
    fn empty_xml_is_parse_error() {
        assert!(parse_junit("").is_err());
    }

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn detects_test_invocations() {
        assert!(Pytest.detect(&argv(&["pytest"])));
        assert!(Pytest.detect(&argv(&["pytest", "-q", "tests/"])));
        assert!(Pytest.detect(&argv(&["python", "-m", "pytest"])));
    }

    #[test]
    fn detects_uv_run_invocations() {
        assert!(Pytest.detect(&argv(&["uv", "run", "pytest"])));
        assert!(Pytest.detect(&argv(&["uv", "run", "pytest", "-q", "tests/"])));
        assert!(Pytest.detect(&argv(&["uvx", "pytest"])));
        assert!(Pytest.detect(&argv(&["uv", "tool", "run", "pytest"])));
        assert!(Pytest.detect(&argv(&["uv", "run", "python", "-m", "pytest"])));
    }

    #[test]
    fn detects_uv_run_with_options_and_module_form() {
        // uv's own `-m` module form.
        assert!(Pytest.detect(&argv(&["uv", "run", "-m", "pytest", "tests"])));
        // uv-level options between `run` and the command.
        assert!(Pytest.detect(&argv(&["uv", "run", "--no-sync", "pytest"])));
        assert!(Pytest.detect(&argv(&["uv", "run", "--with", "pytest-xdist", "pytest"])));
        assert!(Pytest.detect(&argv(&["uv", "run", "--", "pytest", "-q"])));
        // A bare `-m pytest` with no uv wrapper is not a real command — don't
        // treat it as pytest (nothing to exec).
        assert!(!Pytest.detect(&argv(&["-m", "pytest"])));
        // Unknown uv option: fail open rather than mis-detect.
        assert!(!Pytest.detect(&argv(&["uv", "run", "--brand-new-flag", "pytest"])));
    }

    #[test]
    fn skips_informational_invocations() {
        for flag in super::NON_TEST_FLAGS {
            assert!(
                !Pytest.detect(&argv(&["pytest", flag])),
                "should skip pytest {flag}"
            );
        }
        assert!(!Pytest.detect(&argv(&["python", "-m", "pytest", "--version"])));
        // informational flags are still skipped behind a uv wrapper
        assert!(!Pytest.detect(&argv(&["uv", "run", "pytest", "--version"])));
    }

    #[test]
    fn collection_failure_msg_promotes_real_error() {
        let xml = r#"<testsuites><testsuite name="pytest" tests="1" time="0.04">
<testcase classname="tests.test_dedup" name="tests.test_dedup" file="tests/test_dedup.py">
<error message="collection failure">tests/test_dedup.py:3: in &lt;module&gt;
    from sift.dedup import cluster_items
E   ModuleNotFoundError: No module named 'sift'</error>
</testcase></testsuite></testsuites>"#;
        let r = parse_junit(xml).unwrap();
        assert_eq!(
            r.failures[0].msg,
            "ModuleNotFoundError: No module named 'sift'"
        );
    }
}
