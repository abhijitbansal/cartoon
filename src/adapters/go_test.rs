//! `go-test` adapter — parses `go test -json` line-delimited events.
use super::report::{Failure, TestReport};
use super::{basename, Adapter, AdapterReport, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{bail, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

pub struct GoTest;

impl Adapter for GoTest {
    fn name(&self) -> &'static str {
        "go-test"
    }
    fn matches(&self) -> &'static str {
        "go test (-json)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        // Benchmark result lines are the point of a `-bench` run, not a
        // pass/fail count our report shape can express — leave it unwrapped.
        if argv
            .iter()
            .any(|a| a == "-bench" || a.starts_with("-bench="))
        {
            return false;
        }
        matches!(argv, [first, second, ..] if basename(first) == "go" && second == "test")
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        // `-json` must land among `go test`'s own flags, never after
        // `-args` — everything following `-args` is forwarded verbatim to
        // the test binary, where `-json` would mean something else (or
        // nothing at all). Insert right before `-args` when present, else
        // right after `test`. Never remove or reorder the user's own args.
        if !argv.iter().any(|a| a == "-json" || a == "--json") {
            let insert_at = argv
                .iter()
                .position(|a| a == "-args")
                .unwrap_or_else(|| argv.len().min(2));
            argv.insert(insert_at, "-json".into());
        }
        Prepared {
            argv,
            artifact: None,
        }
    }
    fn parse(&self, captured: &Captured, _prepared: &Prepared) -> Result<ParseOutcome> {
        parse_go_test(captured)
    }
}

#[derive(Deserialize)]
struct GoEvent {
    #[serde(rename = "Action")]
    action: String,
    #[serde(rename = "Package", default)]
    package: String,
    #[serde(rename = "Test", default)]
    test: Option<String>,
    #[serde(rename = "Elapsed", default)]
    elapsed: f64,
    #[serde(rename = "Output", default)]
    output: Option<String>,
}

/// Per-package bookkeeping needed to tell a real build/setup failure (no
/// test ever ran) apart from an ordinary test failure.
#[derive(Default)]
struct PackageState {
    /// Package-level (no `Test`) `Output` text, in event order.
    output: Vec<String>,
    /// Whether any event carried a `Test` field for this package.
    had_test_event: bool,
    /// Whether a package-level (no `Test`) `fail` action occurred.
    had_fail: bool,
}

fn package_entry<'a>(
    packages: &'a mut HashMap<String, PackageState>,
    order: &mut Vec<String>,
    name: &str,
) -> &'a mut PackageState {
    if !packages.contains_key(name) {
        order.push(name.to_string());
    }
    packages.entry(name.to_string()).or_default()
}

const BUILD_FAIL_MARKERS: [&str; 2] = ["[build failed]", "[setup failed]"];

fn parse_go_test(captured: &Captured) -> Result<ParseOutcome> {
    let mut any_parsed = false;
    let mut test_output: HashMap<(String, String), Vec<String>> = HashMap::new();
    let mut packages: HashMap<String, PackageState> = HashMap::new();
    let mut package_order: Vec<String> = Vec::new();
    // Terminal (pass/fail/skip) events that carried a `Test` field, in
    // encounter order — kept separate so a second pass can drop parent
    // tests that turn out to have subtests (see leaf-only counting below).
    let mut terminal: Vec<(String, String, String)> = Vec::new();
    // Lines that never parsed as a `go test -json` event at all (e.g. a
    // panic's raw stack trace interleaved with the JSON stream) — kept for
    // passthrough rather than silently dropped.
    let mut raw_lines: Vec<String> = Vec::new();
    let mut duration_s = 0.0f64;

    for raw_line in captured.stdout.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let ev: GoEvent = match serde_json::from_str(line) {
            Ok(ev) => ev,
            Err(e) => {
                // An object with an `Action` key that no longer fits GoEvent
                // is shape drift — return it rather than under-count.
                let looks_like_event = serde_json::from_str::<serde_json::Value>(line)
                    .is_ok_and(|v| v.get("Action").is_some());
                if looks_like_event {
                    bail!("go test -json shape mismatch: {e}");
                }
                raw_lines.push(format!("{raw_line}\n"));
                continue;
            }
        };
        any_parsed = true;

        let pkg = package_entry(&mut packages, &mut package_order, &ev.package);
        match &ev.test {
            Some(test_name) => {
                pkg.had_test_event = true;
                if let Some(out) = &ev.output {
                    test_output
                        .entry((ev.package.clone(), test_name.clone()))
                        .or_default()
                        .push(out.clone());
                }
                if matches!(ev.action.as_str(), "pass" | "fail" | "skip") {
                    terminal.push((ev.package.clone(), test_name.clone(), ev.action.clone()));
                }
            }
            None => {
                if let Some(out) = &ev.output {
                    pkg.output.push(out.clone());
                }
                match ev.action.as_str() {
                    // Packages run in parallel, so the run's wall-clock
                    // duration is bounded by the slowest one, not their sum.
                    "pass" => duration_s = duration_s.max(ev.elapsed),
                    "fail" => {
                        pkg.had_fail = true;
                        duration_s = duration_s.max(ev.elapsed);
                    }
                    _ => {}
                }
            }
        }
    }

    if !any_parsed {
        bail!("no `go test -json` event lines found in output");
    }

    // A parent of `t.Run` subtests reports its own terminal pass/fail/skip
    // event too, which would double-count on top of its children. Count
    // only leaves — tests with no other test named `<name>/…`.
    let has_child = |pkg: &str, name: &str| {
        let prefix = format!("{name}/");
        terminal
            .iter()
            .any(|(p, n, _)| p == pkg && n.starts_with(&prefix))
    };

    let mut total = 0u64;
    let mut passed = 0u64;
    let mut failed = 0u64;
    let mut skipped = 0u64;
    let mut failures = Vec::new();

    for (pkg_name, test_name, action) in &terminal {
        if has_child(pkg_name, test_name) {
            continue;
        }
        total += 1;
        match action.as_str() {
            "pass" => passed += 1,
            "skip" => skipped += 1,
            "fail" => {
                failed += 1;
                let key = (pkg_name.clone(), test_name.clone());
                let lines = test_output.get(&key).cloned().unwrap_or_default();
                failures.push(build_failure(pkg_name, test_name, &lines));
            }
            _ => {}
        }
    }

    // A package that fails to build or set up never emits a single `Test`
    // event — its compiler/setup errors would otherwise vanish into a
    // silent "0 tests ran". Surface one Failure per such package, always
    // alongside its raw output, regardless of what the rest of the run did.
    let mut build_failure_packages: Vec<String> = Vec::new();
    for pkg_name in &package_order {
        let state = &packages[pkg_name];
        let looks_failed_textually = state
            .output
            .iter()
            .any(|o| BUILD_FAIL_MARKERS.iter().any(|m| o.contains(m)));
        if (state.had_fail && !state.had_test_event) || looks_failed_textually {
            build_failure_packages.push(pkg_name.clone());
        }
    }
    for pkg_name in &build_failure_packages {
        total += 1;
        failed += 1;
        failures.push(build_package_failure(pkg_name, &packages[pkg_name].output));
    }

    let mut passthrough_parts: Vec<String> = Vec::new();
    for pkg_name in &build_failure_packages {
        passthrough_parts.extend(packages[pkg_name].output.iter().cloned());
    }
    let has_build_failure_output = !passthrough_parts.is_empty();

    // Fallback for a failure the heuristics above don't attribute to any
    // specific package: nothing ran and the process still failed, so show
    // everything rather than a silent "0 tests ran".
    let unexplained_failure = !captured.status.success() && total == 0;
    if unexplained_failure && !has_build_failure_output {
        for pkg_name in &package_order {
            passthrough_parts.extend(packages[pkg_name].output.iter().cloned());
        }
    }
    if has_build_failure_output || unexplained_failure {
        passthrough_parts.extend(raw_lines.iter().cloned());
    }
    let passthrough_stdout = (!passthrough_parts.is_empty()).then(|| passthrough_parts.concat());
    let passthrough_stderr = (!captured.stderr.is_empty()).then(|| captured.stderr.clone());

    Ok(ParseOutcome {
        report: AdapterReport::Tests(TestReport {
            runner: "go-test",
            total,
            passed,
            failed,
            skipped,
            duration_s,
            failures,
        }),
        passthrough_stdout,
        passthrough_stderr,
    })
}

const MARKER_PREFIXES: &[&str] = &[
    "=== RUN",
    "=== PAUSE",
    "=== CONT",
    "=== NAME",
    "--- FAIL",
    "--- PASS",
    "--- SKIP",
];

fn is_marker(line: &str) -> bool {
    MARKER_PREFIXES.iter().any(|m| line.starts_with(m))
}

/// Match go's `    file_test.go:12: message` failure-output convention.
fn parse_go_loc(line: &str) -> Option<(String, String)> {
    static LOC: OnceLock<regex::Regex> = OnceLock::new();
    let re = LOC.get_or_init(|| regex::Regex::new(r"^(\S+\.go):(\d+):\s?(.*)$").unwrap());
    let caps = re.captures(line)?;
    let file = caps.get(1)?.as_str();
    let ln = caps.get(2)?.as_str();
    let rest = caps
        .get(3)
        .map(|m| m.as_str().trim())
        .unwrap_or("")
        .to_string();
    Some((format!("{file}:{ln}"), rest))
}

fn short_pkg(package: &str) -> &str {
    package.rsplit('/').next().unwrap_or(package)
}

fn build_failure(package: &str, test: &str, output_lines: &[String]) -> Failure {
    let id = format!("{}.{test}", short_pkg(package));

    let mut loc = String::new();
    let mut msg = String::new();
    let mut found_loc = false;
    let mut trace = Vec::new();

    for chunk in output_lines {
        for raw in chunk.lines() {
            let trimmed = raw.trim();
            if trimmed.is_empty() || is_marker(trimmed) {
                continue;
            }
            if !found_loc {
                if let Some((file_loc, rest)) = parse_go_loc(trimmed) {
                    loc = file_loc;
                    msg = rest;
                    found_loc = true;
                }
            }
            trace.push(trimmed.to_string());
        }
    }

    Failure {
        id,
        loc,
        msg,
        trace,
    }
}

/// A package that never ran a single test — its output is go's compiler or
/// `TestMain`/setup diagnostics, e.g. `# pkg` followed by `file.go:5:2:
/// undefined: foo`. There's no test name to key off, so the id is just the
/// package, and the first non-banner line becomes the message.
fn build_package_failure(package: &str, output_lines: &[String]) -> Failure {
    let mut lines: Vec<String> = Vec::new();
    for chunk in output_lines {
        for raw in chunk.lines() {
            let t = raw.trim();
            if !t.is_empty() {
                lines.push(t.to_string());
            }
        }
    }
    // The `# <pkg>` banner just names the package, not the error.
    let msg_idx = lines.iter().position(|l| !l.starts_with('#'));
    let msg = msg_idx.map(|i| lines[i].clone()).unwrap_or_default();
    let trace = lines
        .into_iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != msg_idx)
        .map(|(_, l)| l)
        .collect();

    Failure {
        id: short_pkg(package).to_string(),
        loc: String::new(),
        msg,
        trace,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn captured(stdout: &str, stderr: &str, success: bool) -> Captured {
        Captured {
            stdout: stdout.into(),
            stderr: stderr.into(),
            status: std::process::ExitStatus::from_raw(if success { 0 } else { 256 }),
        }
    }

    fn tests_of(out: &ParseOutcome) -> &TestReport {
        match &out.report {
            AdapterReport::Tests(r) => r,
            AdapterReport::Value(_) => panic!("expected test report"),
        }
    }

    // --- detect ---

    #[test]
    fn detects_go_test_variants() {
        assert!(GoTest.detect(&argv(&["go", "test"])));
        assert!(GoTest.detect(&argv(&["go", "test", "./..."])));
        assert!(GoTest.detect(&argv(&["/usr/local/go/bin/go", "test", "./..."])));
    }

    #[test]
    fn rejects_non_test_go_subcommands() {
        assert!(!GoTest.detect(&argv(&["go", "build", "./..."])));
        assert!(!GoTest.detect(&argv(&["go", "vet", "./..."])));
        assert!(!GoTest.detect(&argv(&["go", "run", "main.go"])));
        assert!(!GoTest.detect(&argv(&["go"])));
    }

    #[test]
    fn declines_bench_invocations() {
        assert!(!GoTest.detect(&argv(&["go", "test", "-bench", "."])));
        assert!(!GoTest.detect(&argv(&["go", "test", "-bench=.", "./..."])));
    }

    // --- prepare ---

    #[test]
    fn prepare_inserts_json_right_after_test() {
        let p = GoTest.prepare(argv(&["go", "test", "./...", "-run", "TestX"]));
        assert_eq!(
            p.argv,
            argv(&["go", "test", "-json", "./...", "-run", "TestX"])
        );
    }

    #[test]
    fn prepare_appends_json_when_no_further_args() {
        let p = GoTest.prepare(argv(&["go", "test"]));
        assert_eq!(p.argv, argv(&["go", "test", "-json"]));
    }

    #[test]
    fn prepare_respects_existing_json_flag() {
        let p = GoTest.prepare(argv(&["go", "test", "-json", "./..."]));
        assert_eq!(p.argv, argv(&["go", "test", "-json", "./..."]));
    }

    #[test]
    fn prepare_respects_existing_double_dash_json_flag() {
        let p = GoTest.prepare(argv(&["go", "test", "--json", "./..."]));
        assert_eq!(p.argv, argv(&["go", "test", "--json", "./..."]));
    }

    #[test]
    fn prepare_inserts_json_before_args_separator() {
        let p = GoTest.prepare(argv(&["go", "test", "-run", "TestX", "-args", "-v"]));
        assert_eq!(
            p.argv,
            argv(&["go", "test", "-run", "TestX", "-json", "-args", "-v"])
        );
    }

    // --- parse: all pass ---

    const ALL_PASS: &str = "\
{\"Action\":\"run\",\"Package\":\"example.com/m/calc\",\"Test\":\"TestAdd\"}
{\"Action\":\"run\",\"Package\":\"example.com/m/calc\",\"Test\":\"TestAdd/subtest\"}
{\"Action\":\"pass\",\"Package\":\"example.com/m/calc\",\"Test\":\"TestAdd/subtest\",\"Elapsed\":0.01}
{\"Action\":\"pass\",\"Package\":\"example.com/m/calc\",\"Test\":\"TestAdd\",\"Elapsed\":0.02}
{\"Action\":\"pass\",\"Package\":\"example.com/m/calc\",\"Elapsed\":0.05}
{\"Action\":\"run\",\"Package\":\"example.com/m/util\",\"Test\":\"TestUtilA\"}
{\"Action\":\"pass\",\"Package\":\"example.com/m/util\",\"Test\":\"TestUtilA\",\"Elapsed\":0.01}
{\"Action\":\"run\",\"Package\":\"example.com/m/util\",\"Test\":\"TestUtilB\"}
{\"Action\":\"pass\",\"Package\":\"example.com/m/util\",\"Test\":\"TestUtilB\",\"Elapsed\":0.02}
{\"Action\":\"pass\",\"Package\":\"example.com/m/util\",\"Elapsed\":0.07}
";

    #[test]
    fn parses_all_pass_counts_and_duration() {
        let c = captured(ALL_PASS, "", true);
        let prepared = GoTest.prepare(argv(&["go", "test", "./..."]));
        let out = GoTest.parse(&c, &prepared).unwrap();
        let r = tests_of(&out);
        // TestAdd is the parent of TestAdd/subtest — only the leaf plus the
        // two independent util tests count, so 3 not 4.
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (3, 3, 0, 0));
        // Packages run in parallel: duration is the slowest package
        // (0.07), not the sum of both (0.12).
        assert!((r.duration_s - 0.07).abs() < 1e-9, "got {}", r.duration_s);
        assert!(r.failures.is_empty());
        assert!(out.passthrough_stdout.is_none());
    }

    #[test]
    fn parent_test_excluded_when_it_has_subtests() {
        const PARENT_AND_SUBTESTS: &str = "\
{\"Action\":\"run\",\"Package\":\"example.com/m/calc\",\"Test\":\"TestGroup\"}
{\"Action\":\"run\",\"Package\":\"example.com/m/calc\",\"Test\":\"TestGroup/sub1\"}
{\"Action\":\"pass\",\"Package\":\"example.com/m/calc\",\"Test\":\"TestGroup/sub1\",\"Elapsed\":0.01}
{\"Action\":\"run\",\"Package\":\"example.com/m/calc\",\"Test\":\"TestGroup/sub2\"}
{\"Action\":\"pass\",\"Package\":\"example.com/m/calc\",\"Test\":\"TestGroup/sub2\",\"Elapsed\":0.01}
{\"Action\":\"pass\",\"Package\":\"example.com/m/calc\",\"Test\":\"TestGroup\",\"Elapsed\":0.02}
{\"Action\":\"pass\",\"Package\":\"example.com/m/calc\",\"Elapsed\":0.02}
";
        let c = captured(PARENT_AND_SUBTESTS, "", true);
        let prepared = GoTest.prepare(argv(&["go", "test", "./..."]));
        let out = GoTest.parse(&c, &prepared).unwrap();
        let r = tests_of(&out);
        assert_eq!(r.total, 2);
        assert_eq!(r.passed, 2);
    }

    // --- parse: mixed fail + skip ---

    const MIXED: &str = "\
{\"Action\":\"run\",\"Package\":\"example.com/m/calc\",\"Test\":\"TestAdd\"}
{\"Action\":\"output\",\"Package\":\"example.com/m/calc\",\"Test\":\"TestAdd\",\"Output\":\"=== RUN   TestAdd\\n\"}
{\"Action\":\"output\",\"Package\":\"example.com/m/calc\",\"Test\":\"TestAdd\",\"Output\":\"    calc_test.go:12: Add(1,2) = 4, want 3\\n\"}
{\"Action\":\"output\",\"Package\":\"example.com/m/calc\",\"Test\":\"TestAdd\",\"Output\":\"--- FAIL: TestAdd (0.00s)\\n\"}
{\"Action\":\"fail\",\"Package\":\"example.com/m/calc\",\"Test\":\"TestAdd\",\"Elapsed\":0}
{\"Action\":\"output\",\"Package\":\"example.com/m/calc\",\"Output\":\"FAIL\\n\"}
{\"Action\":\"fail\",\"Package\":\"example.com/m/calc\",\"Elapsed\":0.01}
{\"Action\":\"run\",\"Package\":\"example.com/m/x\",\"Test\":\"TestSkip\"}
{\"Action\":\"output\",\"Package\":\"example.com/m/x\",\"Test\":\"TestSkip\",\"Output\":\"=== RUN   TestSkip\\n\"}
{\"Action\":\"output\",\"Package\":\"example.com/m/x\",\"Test\":\"TestSkip\",\"Output\":\"    x_test.go:8: skipping on CI\\n\"}
{\"Action\":\"output\",\"Package\":\"example.com/m/x\",\"Test\":\"TestSkip\",\"Output\":\"--- SKIP: TestSkip (0.00s)\\n\"}
{\"Action\":\"skip\",\"Package\":\"example.com/m/x\",\"Test\":\"TestSkip\",\"Elapsed\":0}
{\"Action\":\"pass\",\"Package\":\"example.com/m/x\",\"Elapsed\":0.02}
";

    #[test]
    fn parses_mixed_fail_and_skip() {
        let c = captured(MIXED, "warning: go version mismatch\n", false);
        let prepared = GoTest.prepare(argv(&["go", "test", "./..."]));
        let out = GoTest.parse(&c, &prepared).unwrap();
        let r = tests_of(&out);
        assert_eq!((r.total, r.failed, r.skipped), (2, 1, 1));
        assert_eq!(r.failures.len(), 1);
        let f = &r.failures[0];
        assert_eq!(f.id, "calc.TestAdd");
        assert_eq!(f.loc, "calc_test.go:12");
        assert_eq!(f.msg, "Add(1,2) = 4, want 3");
        // calc did run a test (it just failed), so it's not a build/setup
        // failure — nothing to surface on top of the parsed report.
        assert!(out.passthrough_stdout.is_none());
        assert_eq!(
            out.passthrough_stderr.as_deref(),
            Some("warning: go version mismatch\n")
        );
    }

    // --- parse: build failure ---

    const BUILD_FAILURE: &str = "\
{\"Action\":\"output\",\"Package\":\"example.com/m/calc\",\"Output\":\"# example.com/m/calc\\n\"}
{\"Action\":\"output\",\"Package\":\"example.com/m/calc\",\"Output\":\"calc.go:5:2: undefined: foo\\n\"}
{\"Action\":\"output\",\"Package\":\"example.com/m/calc\",\"Output\":\"FAIL\\texample.com/m/calc [build failed]\\n\"}
{\"Action\":\"fail\",\"Package\":\"example.com/m/calc\",\"Elapsed\":0}
";

    #[test]
    fn build_failure_is_counted_and_passes_compiler_output_through() {
        let c = captured(BUILD_FAILURE, "", false);
        let prepared = GoTest.prepare(argv(&["go", "test", "./..."]));
        let out = GoTest.parse(&c, &prepared).unwrap();
        let r = tests_of(&out);
        assert_eq!(r.total, 1);
        assert_eq!(r.failed, 1);
        assert_eq!(r.failures.len(), 1);
        assert_eq!(r.failures[0].id, "calc");
        assert_eq!(r.failures[0].msg, "calc.go:5:2: undefined: foo");
        let stdout = out.passthrough_stdout.expect("build errors passed through");
        assert!(stdout.contains("undefined: foo"), "got: {stdout}");
        assert!(out.passthrough_stderr.is_none());
    }

    #[test]
    fn one_package_fails_to_build_among_passing_packages() {
        const MIXED_BUILD_FAILURE: &str = "\
{\"Action\":\"run\",\"Package\":\"example.com/m/a\",\"Test\":\"TestA\"}
{\"Action\":\"pass\",\"Package\":\"example.com/m/a\",\"Test\":\"TestA\",\"Elapsed\":0.01}
{\"Action\":\"pass\",\"Package\":\"example.com/m/a\",\"Elapsed\":0.01}
{\"Action\":\"run\",\"Package\":\"example.com/m/b\",\"Test\":\"TestB\"}
{\"Action\":\"pass\",\"Package\":\"example.com/m/b\",\"Test\":\"TestB\",\"Elapsed\":0.01}
{\"Action\":\"pass\",\"Package\":\"example.com/m/b\",\"Elapsed\":0.01}
{\"Action\":\"output\",\"Package\":\"example.com/m/calc\",\"Output\":\"# example.com/m/calc\\n\"}
{\"Action\":\"output\",\"Package\":\"example.com/m/calc\",\"Output\":\"calc.go:5:2: undefined: foo\\n\"}
{\"Action\":\"output\",\"Package\":\"example.com/m/calc\",\"Output\":\"FAIL\\texample.com/m/calc [build failed]\\n\"}
{\"Action\":\"fail\",\"Package\":\"example.com/m/calc\",\"Elapsed\":0}
";
        let c = captured(MIXED_BUILD_FAILURE, "", false);
        let prepared = GoTest.prepare(argv(&["go", "test", "./..."]));
        let out = GoTest.parse(&c, &prepared).unwrap();
        let r = tests_of(&out);
        assert_eq!(r.passed, 2);
        assert!(r.failed >= 1, "got failed={}", r.failed);
        let stdout = out
            .passthrough_stdout
            .expect("compiler output surfaced even though other packages passed");
        assert!(stdout.contains("undefined: foo"), "got: {stdout}");
    }

    #[test]
    fn raw_non_json_lines_survive_on_unexplained_failure_path() {
        const BUILD_FAILURE_WITH_RAW_PANIC: &str = "\
{\"Action\":\"output\",\"Package\":\"example.com/m/calc\",\"Output\":\"# example.com/m/calc\\n\"}
panic: runtime error: index out of range
{\"Action\":\"output\",\"Package\":\"example.com/m/calc\",\"Output\":\"calc.go:5:2: undefined: foo\\n\"}
{\"Action\":\"fail\",\"Package\":\"example.com/m/calc\",\"Elapsed\":0}
";
        let c = captured(BUILD_FAILURE_WITH_RAW_PANIC, "", false);
        let prepared = GoTest.prepare(argv(&["go", "test", "./..."]));
        let out = GoTest.parse(&c, &prepared).unwrap();
        let stdout = out
            .passthrough_stdout
            .expect("raw and compiler output surfaced");
        assert!(stdout.contains("panic: runtime error"), "got: {stdout}");
        assert!(stdout.contains("undefined: foo"), "got: {stdout}");
    }

    // --- parse: garbage ---

    #[test]
    fn garbage_output_is_error() {
        let c = captured("not json at all\nmore garbage\n", "", false);
        let prepared = GoTest.prepare(argv(&["go", "test"]));
        assert!(GoTest.parse(&c, &prepared).is_err());
    }

    fn drift_status_ok() -> std::process::ExitStatus {
        std::process::Command::new("true").status().unwrap()
    }

    #[test]
    fn event_that_no_longer_fits_the_struct_is_an_error() {
        let stdout = "{\"Time\":\"t\",\"Action\":\"pass\",\"Package\":\"p\",\"Test\":\"T\",\"Elapsed\":\"fast\"}\n";
        let cap = Captured {
            stdout: stdout.into(),
            stderr: String::new(),
            status: drift_status_ok(),
        };
        let prepared = GoTest.prepare(vec!["go".into(), "test".into()]);
        assert!(GoTest.parse(&cap, &prepared).is_err());
    }

    // --- markers ---

    #[test]
    fn markers_include_name_and_skip_prefixes() {
        assert!(is_marker("=== NAME  TestX"));
        assert!(is_marker("--- SKIP: TestX (0.00s)"));
    }
}
