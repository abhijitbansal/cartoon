use super::report::{Failure, TestReport};
use super::{basename, is_python_module, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};

pub struct Pytest;

impl Adapter for Pytest {
    fn name(&self) -> &'static str {
        "pytest"
    }
    fn matches(&self) -> &'static str {
        "pytest | python -m pytest"
    }
    fn detect(&self, argv: &[String]) -> bool {
        argv.first()
            .map(|a| basename(a) == "pytest")
            .unwrap_or(false)
            || is_python_module(argv, "pytest")
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
        Prepared { argv, artifact }
    }
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        let path = prepared
            .artifact_path()
            .context("pytest adapter has no junit artifact")?;
        let xml = std::fs::read_to_string(&path).context("junit xml missing")?;
        let report = parse_junit(&xml)?;
        Ok(ParseOutcome {
            report,
            // stdout was pytest's human report — consumed. stderr may hold
            // user warnings the agent needs.
            passthrough_stdout: None,
            passthrough_stderr: (!captured.stderr.is_empty()).then(|| captured.stderr.clone()),
        })
    }
}

pub fn parse_junit(xml: &str) -> Result<TestReport> {
    let doc = roxmltree::Document::parse(xml).context("invalid junit xml")?;
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
            .map(|l| l + 1); // junit line attr is 0-based
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
            let msg = fail
                .attribute("message")
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            let trace = super::report::trim_trace(fail.text().unwrap_or(""));
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
        runner: "pytest",
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
}
