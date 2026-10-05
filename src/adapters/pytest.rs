use super::report::{Failure, TestReport};
use super::{basename, is_python_module, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use regex::Regex;
use std::path::PathBuf;
use std::sync::OnceLock;

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
    "--setup-plan",
    "--setup-only",
    "--fixtures-per-test",
];

/// Captured stdout/stderr/log lines attached to one failure, at most.
const MAX_CAPTURED_LINES: usize = 10;

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
        if user_junit_path(&argv).is_some() {
            // The user wants the junit file themselves: parse() reads their
            // path rather than injecting a second one that would steal it.
            return Prepared {
                argv,
                artifact: None,
            };
        }
        let artifact = tempfile::Builder::new()
            .prefix("cartoon-junit-")
            .suffix(".xml")
            .tempfile()
            .ok();
        if let Some(f) = &artifact {
            argv.push(format!("--junit-xml={}", f.path().display()));
            argv.push("--override-ini=junit_family=legacy".into());
            // Captured stdout/stderr/logging of each test, so a failing
            // test's prints reach the report (attached to failures only).
            if !argv.iter().any(|a| a.contains("junit_logging")) {
                argv.push("--override-ini=junit_logging=all".into());
            }
        }
        Prepared {
            argv,
            artifact: artifact.map(super::Artifact::File),
        }
    }
    fn native_stdout(&self, user_argv: &[String], captured: &Captured) -> Option<String> {
        // Our injected `--junit-xml` makes pytest print a
        // `generated xml file: …/cartoon-junit-….xml` line the user's own
        // run would not have; the baseline (and any passthrough) omits it.
        if user_junit_path(user_argv).is_some() {
            return None;
        }
        Some(strip_injected_junit_line(&captured.stdout))
    }
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        let path = prepared
            .artifact_path()
            .or_else(|| user_junit_path(&prepared.argv))
            .context("pytest adapter has no junit artifact")?;
        let xml = std::fs::read_to_string(&path).context("junit xml missing")?;
        let mut report = parse_junit(&xml)?;
        // Session-level outcomes the junit xml never records.
        add_session_failures(&mut report, &captured.stdout);
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

/// A user-supplied `--junit-xml`/`--junitxml` path (`=` or two-token form).
fn strip_injected_junit_line(stdout: &str) -> String {
    stdout
        .split_inclusive('\n')
        .filter(|l| !(l.contains("generated xml file:") && l.contains("cartoon-junit-")))
        .collect()
}

fn user_junit_path(argv: &[String]) -> Option<PathBuf> {
    for (i, a) in argv.iter().enumerate() {
        for flag in ["--junit-xml", "--junitxml"] {
            if let Some(v) = a.strip_prefix(&format!("{flag}=")) {
                return Some(PathBuf::from(v));
            }
            if a == flag {
                return argv.get(i + 1).map(PathBuf::from);
            }
        }
    }
    None
}

/// Reasons a run failed that live only in pytest's terminal summary:
/// `pytest.exit()` / KeyboardInterrupt banners (`!!!! … !!!!`) and an unmet
/// `--cov-fail-under`. Each becomes a failure, so a run that stopped
/// early or missed coverage never looks like a clean pass.
pub fn add_session_failures(report: &mut TestReport, stdout: &str) {
    static BANG: OnceLock<Regex> = OnceLock::new();
    let bang = BANG.get_or_init(|| Regex::new(r"^!{3,} (.+?) !{3,}$").unwrap());
    let mut extra = Vec::new();
    for line in stdout.lines().map(str::trim_end) {
        if let Some(c) = bang.captures(line) {
            let reason = c[1].to_string();
            // Already explained by the failures in the report.
            let explained = report.failed > 0
                && ((reason.starts_with("Interrupted: ") && reason.contains("during collection"))
                    || reason.starts_with("stopping after "));
            if !explained {
                extra.push(("(session)", reason));
            }
        } else if line.starts_with("FAIL Required test coverage") {
            extra.push(("(coverage)", line.to_string()));
        }
    }
    for (id, msg) in extra {
        report.failed += 1;
        report.failures.push(Failure {
            id: id.into(),
            loc: String::new(),
            msg,
            trace: Vec::new(),
        });
    }
}

/// Captured output (`<system-out>`/`<system-err>`) of a failed testcase as
/// `[stdout] …` lines: pytest's `--- Captured Out ---` headers become the
/// label, blank lines go. Bounded to the last `MAX_CAPTURED_LINES`.
fn captured_output(case: roxmltree::Node) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for node in case
        .children()
        .filter(|c| c.has_tag_name("system-out") || c.has_tag_name("system-err"))
    {
        let mut label = if node.has_tag_name("system-out") {
            "stdout"
        } else {
            "stderr"
        };
        for line in node.text().unwrap_or("").lines() {
            let t = line.trim_end();
            if t.trim().is_empty() {
                continue;
            }
            if let Some(h) = captured_header(t) {
                label = h;
                continue;
            }
            out.push(format!("[{label}] {t}"));
        }
    }
    if out.len() > MAX_CAPTURED_LINES {
        let extra = out.len() - MAX_CAPTURED_LINES;
        out.drain(..extra);
        out.insert(
            0,
            format!("\u{2026} +{extra} captured lines omitted (see raw_log)"),
        );
    }
    out
}

/// `----- Captured Out -----` → "stdout" (also Err → stderr, Log → log).
fn captured_header(line: &str) -> Option<&'static str> {
    let t = line.trim().trim_matches('-').trim();
    match t {
        "Captured Out" => Some("stdout"),
        "Captured Err" => Some("stderr"),
        "Captured Log" => Some("log"),
        _ => None,
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
        // `pytest.exit()` mid-run leaves a bare `<testcase time="0.000" />`
        // for the interrupted test: not a test that passed.
        let Some(name) = case.attribute("name") else {
            continue;
        };
        total += 1;
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
            let mut trace = super::report::trim_trace(fail.text().unwrap_or(""));
            // "collection failure" hides the real error (ImportError etc.);
            // promote pytest's `E ...` exception line to msg — for a
            // SyntaxError that is `E   SyntaxError: …`, not the `E   File
            // "…", line 2` lines before it.
            if msg.is_empty() || msg == "collection failure" {
                let e_lines = || trace.iter().filter(|l| l.starts_with("E "));
                let is_exc = |l: &&String| is_exception_line(l[1..].trim_start());
                if let Some(e) = e_lines().find(is_exc).or_else(|| e_lines().next()) {
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
            trace.extend(captured_output(case));
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

/// `SyntaxError: …`, `ModuleNotFoundError: …`, `pkg.CustomException: …`.
fn is_exception_line(t: &str) -> bool {
    static EXC: OnceLock<Regex> = OnceLock::new();
    EXC.get_or_init(|| {
        Regex::new(r"^[A-Za-z_][A-Za-z0-9_.]*(Error|Exception|Failure|Exit|Interrupt)\b").unwrap()
    })
    .is_match(t)
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
    fn native_view_drops_only_our_generated_xml_line() {
        let out = "1 failed in 0.01s\n- generated xml file: /tmp/cartoon-junit-ab.xml -\n- generated xml file: mine.xml -\n";
        assert_eq!(
            strip_injected_junit_line(out),
            "1 failed in 0.01s\n- generated xml file: mine.xml -\n"
        );
    }

    #[test]
    fn prepare_appends_junit_flag() {
        let p = Pytest.prepare(vec!["pytest".into(), "-q".into()]);
        assert_eq!(p.argv[0], "pytest");
        assert_eq!(p.argv[1], "-q");
        assert!(p.argv[2].starts_with("--junit-xml="));
        assert_eq!(p.argv[3], "--override-ini=junit_family=legacy");
        assert_eq!(p.argv[4], "--override-ini=junit_logging=all");
        assert!(p.artifact.is_some());
    }

    #[test]
    fn prepare_keeps_a_user_junit_logging_choice() {
        let p = Pytest.prepare(argv(&["pytest", "-o", "junit_logging=system-err"]));
        assert_eq!(
            p.argv
                .iter()
                .filter(|a| a.contains("junit_logging"))
                .count(),
            1,
            "{:?}",
            p.argv
        );
    }

    #[test]
    fn user_junit_xml_path_is_reused_not_stolen() {
        for a in [
            argv(&["pytest", "--junit-xml=out/r.xml"]),
            argv(&["pytest", "--junitxml", "out/r.xml", "-q"]),
        ] {
            let p = Pytest.prepare(a.clone());
            assert_eq!(p.argv, a, "nothing injected");
            assert!(p.artifact.is_none());
            assert_eq!(user_junit_path(&p.argv), Some(PathBuf::from("out/r.xml")));
        }
        assert_eq!(user_junit_path(&argv(&["pytest", "-q"])), None);
    }

    #[test]
    fn parse_reads_the_user_junit_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.xml");
        std::fs::copy(
            format!(
                "{}/tests/fixtures/pytest/mixed.xml",
                env!("CARGO_MANIFEST_DIR")
            ),
            &path,
        )
        .unwrap();
        let prepared = Pytest.prepare(vec![
            "pytest".into(),
            format!("--junitxml={}", path.display()),
        ]);
        let out = Pytest.parse(&captured("", false), &prepared).unwrap();
        match out.report {
            crate::adapters::AdapterReport::Tests(r) => assert_eq!(r.failed, 1),
            _ => panic!("expected a test report"),
        }
    }

    fn captured(stdout: &str, ok: bool) -> Captured {
        let status = std::process::Command::new(if ok { "true" } else { "false" })
            .status()
            .unwrap();
        Captured {
            stdout: stdout.into(),
            stderr: String::new(),
            status,
        }
    }

    fn fixture_text(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/pytest/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn pytest_exit_mid_run_is_a_failure_not_a_pass() {
        // Real pytest 9: `pytest.exit()` in the second of three tests.
        let mut r = parse_fixture("session-exit.xml");
        // The bare <testcase/> pytest leaves for the aborted test is skipped.
        assert_eq!((r.total, r.passed, r.failed), (1, 1, 0));
        add_session_failures(&mut r, &fixture_text("session-exit.stdout.txt"));
        assert_eq!(r.failed, 1);
        assert_eq!(
            r.failures[0].msg,
            "_pytest.outcomes.Exit: database not reachable, aborting run"
        );
    }

    #[test]
    fn unmet_cov_fail_under_is_a_failure() {
        let mut r = parse_fixture("cov-fail-under.xml");
        add_session_failures(&mut r, &fixture_text("cov-fail-under.stdout.txt"));
        assert_eq!((r.passed, r.failed), (1, 1));
        assert_eq!(r.failures[0].id, "(coverage)");
        assert_eq!(
            r.failures[0].msg,
            "FAIL Required test coverage of 95% not reached. Total coverage: 50.00%"
        );
    }

    #[test]
    fn collection_interrupt_banner_is_not_repeated() {
        let mut r = parse_fixture("collect-syntax-error.xml");
        add_session_failures(&mut r, &fixture_text("collect-syntax-error.stdout.txt"));
        assert_eq!(r.failed, 1, "{:?}", r.failures);
    }

    #[test]
    fn collection_syntax_error_msg_is_the_syntax_error_line() {
        let r = parse_fixture("collect-syntax-error.xml");
        let f = &r.failures[0];
        assert_eq!(f.msg, "SyntaxError: '(' was never closed");
        assert!(
            !f.trace.iter().any(|l| l.contains("<frozen")),
            "{:?}",
            f.trace
        );
        assert!(!f.trace.iter().any(|l| l == "???"), "{:?}", f.trace);
    }

    #[test]
    fn failing_test_carries_its_captured_output_passing_one_does_not() {
        let r = parse_fixture("captured-output.xml");
        assert_eq!(r.failures.len(), 1);
        let t = &r.failures[0].trace;
        assert!(
            t.contains(&"[stdout] computed total = 41".to_string()),
            "{t:?}"
        );
        assert!(t.contains(&"[stderr] warning text".to_string()), "{t:?}");
        assert!(!t.iter().any(|l| l.contains("Captured")), "{t:?}");
    }

    #[test]
    fn captured_output_is_bounded() {
        let body: String = (0..30).map(|i| format!("line {i}\n")).collect();
        let xml = format!(
            r#"<testsuite><testcase name="t" file="t.py" line="1"><failure message="boom">E boom</failure><system-out>{body}</system-out></testcase></testsuite>"#
        );
        let r = parse_junit(&xml).unwrap();
        let t = &r.failures[0].trace;
        assert!(t.contains(&"[stdout] line 29".to_string()), "{t:?}");
        assert!(!t.contains(&"[stdout] line 0".to_string()), "{t:?}");
        assert!(
            t.iter().any(|l| l.contains("+20 captured lines omitted")),
            "{t:?}"
        );
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
