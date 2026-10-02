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
        // Same for `-list` (a listing of names, no test runs) and `-fuzz`
        // (a long-running fuzzer whose progress lines are the output).
        if argv.iter().any(|a| {
            let flag = a.strip_prefix("--").or_else(|| a.strip_prefix('-'));
            flag.is_some_and(|f| {
                ["bench", "list", "fuzz"]
                    .iter()
                    .any(|n| f == *n || f.starts_with(&format!("{n}=")))
            })
        }) {
            return false;
        }
        matches!(argv, [first, second, ..] if basename(first) == "go" && second == "test")
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        // `-json` must land among `go test`'s own flags, never after
        // `-args` — everything following `-args` is forwarded verbatim to
        // the test binary, where `-json` would mean something else (or
        // nothing at all). Insert right before `-args` when present, else
        // right after `test` — or after a leading `-C <dir>` / `-C=dir`,
        // which go requires to be the very first flag. Never remove or
        // reorder the user's own args.
        if !argv.iter().any(|a| a == "-json" || a == "--json") {
            let after_test = match argv.get(2).map(String::as_str) {
                Some("-C" | "--C") => 4,
                Some(a) if a.starts_with("-C=") || a.starts_with("--C=") => 3,
                _ => 2,
            };
            let insert_at = argv
                .iter()
                .position(|a| a == "-args")
                .unwrap_or_else(|| argv.len().min(after_test));
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
    /// Go >=1.24 `build-output` / `build-fail` events are keyed by the
    /// import path being built (e.g. `pkg [pkg.test]`), not `Package`.
    #[serde(rename = "ImportPath", default)]
    import_path: String,
    /// On a package `fail` caused by a build failure, the `ImportPath` whose
    /// `build-output` explains it — possibly a dependency, not the package.
    #[serde(rename = "FailedBuild", default)]
    failed_build: String,
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
    /// Whether a package-level (no `Test`) `pass` action occurred.
    had_pass: bool,
    /// The package `fail` event's `FailedBuild` import path, if any.
    failed_build: Option<String>,
}

/// `pkg [pkg.test]` → `pkg`: go's build-event import paths carry the test
/// variant in a bracketed suffix.
fn strip_variant(import_path: &str) -> &str {
    import_path
        .split_once(" [")
        .map_or(import_path, |(path, _)| path)
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
    // `build-output` text per raw ImportPath.
    let mut build_output: HashMap<String, Vec<String>> = HashMap::new();
    // Tests that emitted `run`, in order — a test killed by `-timeout` (or a
    // crashed binary) never gets its terminal event.
    let mut runs: Vec<(String, String)> = Vec::new();

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

        // Go >=1.24 reports compiler/vet output as separate build events
        // keyed by ImportPath; collect them and attach to the package whose
        // `fail` event names that build (see `FailedBuild`) afterwards.
        if ev.action.starts_with("build-") {
            let entry = build_output.entry(ev.import_path.clone()).or_default();
            if let Some(out) = &ev.output {
                entry.push(out.clone());
            }
            continue;
        }

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
                match ev.action.as_str() {
                    "pass" | "fail" | "skip" => {
                        terminal.push((ev.package.clone(), test_name.clone(), ev.action.clone()))
                    }
                    "run" => runs.push((ev.package.clone(), test_name.clone())),
                    _ => {}
                }
            }
            None => {
                if let Some(out) = &ev.output {
                    pkg.output.push(out.clone());
                }
                match ev.action.as_str() {
                    // Packages run in parallel, so the run's wall-clock
                    // duration is bounded by the slowest one, not their sum.
                    "pass" => {
                        pkg.had_pass = true;
                        duration_s = duration_s.max(ev.elapsed);
                    }
                    "fail" => {
                        pkg.had_fail = true;
                        if !ev.failed_build.is_empty() {
                            pkg.failed_build = Some(ev.failed_build.clone());
                        }
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

    // Attach build output to the failed package that names it — directly
    // via `FailedBuild`, else by matching the stripped import path — ahead
    // of the package's own `FAIL ... [build failed]` line.
    for pkg_name in &package_order {
        let state = packages.get_mut(pkg_name).unwrap();
        let key = state.failed_build.clone().or_else(|| {
            build_output
                .keys()
                .find(|k| strip_variant(k) == pkg_name)
                .cloned()
        });
        if let Some(lines) = key.and_then(|k| build_output.remove(&k)) {
            state.output.splice(0..0, lines);
        }
    }

    // A test with a `run` but no terminal event in a failed package was cut
    // off — `-timeout` panics, a crash, `os.Exit` mid-test. Count it as a
    // failure carrying whatever it printed (e.g. `panic: test timed out`).
    let process_failed = !captured.status.success();
    for (pkg_name, test_name) in &runs {
        let ended = terminal
            .iter()
            .any(|(p, n, _)| p == pkg_name && n == test_name);
        let pkg_failed = packages
            .get(pkg_name)
            .is_some_and(|s| s.had_fail || (!s.had_pass && process_failed));
        if !ended && pkg_failed {
            terminal.push((pkg_name.clone(), test_name.clone(), "fail".into()));
        }
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
    // Packages with at least one attributed test failure.
    let mut pkgs_with_test_failure: Vec<&str> = Vec::new();

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
                pkgs_with_test_failure.push(pkg_name);
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
    // Likewise a package that fails with no failing test (`TestMain` calling
    // `os.Exit(1)`, a goroutine-leak detector) gets a package-level Failure
    // carrying its output.
    let mut build_failure_packages: Vec<String> = Vec::new();
    for pkg_name in &package_order {
        let state = &packages[pkg_name];
        let looks_failed_textually = state
            .output
            .iter()
            .any(|o| BUILD_FAIL_MARKERS.iter().any(|m| o.contains(m)));
        let failed_without_test =
            state.had_fail && !pkgs_with_test_failure.contains(&pkg_name.as_str());
        if failed_without_test || looks_failed_textually {
            total += 1;
            failed += 1;
            failures.push(build_package_failure(pkg_name, &state.output));
            if !state.had_test_event || looks_failed_textually {
                build_failure_packages.push(pkg_name.clone());
            }
        }
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
    // Build output no failed package claimed — show it rather than drop it.
    if process_failed {
        let mut leftover: Vec<_> = build_output.into_iter().collect();
        leftover.sort();
        for (_, lines) in leftover {
            passthrough_parts.extend(lines);
        }
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

/// A goroutine-dump location line: `\t/path/file.go:12 +0x1d` (the
/// `+0x..` offset is absent for inlined frames).
fn frame_location(line: &str) -> Option<(&str, &str)> {
    static FRAME: OnceLock<regex::Regex> = OnceLock::new();
    let re =
        FRAME.get_or_init(|| regex::Regex::new(r"^\t(\S+\.go):(\d+)(?: \+0x[0-9a-f]+)?$").unwrap());
    let caps = re.captures(line)?;
    Some((caps.get(1)?.as_str(), caps.get(2)?.as_str()))
}

/// Go runtime / testing-harness frames carry no signal about the user's bug,
/// nor does any other standard-library frame once GOROOT is known (it is
/// learned from the first runtime/testing frame path, e.g. `/usr/local/go`).
fn is_runtime_frame(file: &str, goroot_src: Option<&str>) -> bool {
    file.contains("/src/runtime/")
        || file.contains("/src/testing/")
        || file == "_testmain.go"
        || goroot_src.is_some_and(|g| file.starts_with(g))
}

fn build_failure(package: &str, test: &str, output_lines: &[String]) -> Failure {
    let id = format!("{}.{test}", short_pkg(package));

    let mut loc = String::new();
    let mut msg = String::new();
    let mut found_loc = false;
    let mut panic_seen = false;
    let mut panic_loc_found = false;
    let mut trace: Vec<String> = Vec::new();

    let raw_lines: Vec<&str> = output_lines.iter().flat_map(|c| c.lines()).collect();
    let goroot_src = raw_lines.iter().find_map(|l| {
        let (file, _) = frame_location(l)?;
        ["/src/runtime/", "/src/testing/"]
            .iter()
            .find_map(|m| file.find(m).map(|i| &file[..i + "/src/".len()]))
    });
    let mut i = 0;
    while i < raw_lines.len() {
        let raw = raw_lines[i];
        // A goroutine-dump frame is a function line followed by its
        // location line; keep or drop the pair together.
        if let Some((file, ln)) = raw_lines.get(i + 1).and_then(|l| frame_location(l)) {
            if frame_location(raw).is_none() && !raw.starts_with('\t') {
                if !is_runtime_frame(file, goroot_src) {
                    // `created by` names where a goroutine was spawned,
                    // not where it failed.
                    if panic_seen && !panic_loc_found && !raw.starts_with("created by ") {
                        loc = format!("{}:{ln}", basename(file));
                        panic_loc_found = true;
                    }
                    trace.push(raw.trim().to_string());
                    trace.push(raw_lines[i + 1].trim().to_string());
                }
                i += 2;
                continue;
            }
        }
        i += 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || is_marker(trimmed) {
            continue;
        }
        // The panic (incl. `-timeout`'s `panic: test timed out`) is what
        // ended the test, so it wins over an earlier `t.Log`/`t.Error` line.
        if let Some(text) = trimmed.strip_prefix("panic: ") {
            if !panic_seen {
                panic_seen = true;
                msg = format!("panic: {}", text.trim_end_matches(" [recovered]"));
            }
        } else if !found_loc && !panic_seen {
            if let Some((file_loc, rest)) = parse_go_loc(trimmed) {
                loc = file_loc;
                msg = rest;
                found_loc = true;
            }
        }
        trace.push(trimmed.to_string());
    }
    // Drop `goroutine N [state]:` headers whose frames were all filtered.
    let is_header = |l: &str| l.starts_with("goroutine ") && l.ends_with(':');
    let mut kept: Vec<String> = Vec::with_capacity(trace.len());
    for (idx, l) in trace.iter().enumerate() {
        let next_is_header_or_end = trace.get(idx + 1).is_none_or(|n| is_header(n));
        if is_header(l) && next_is_header_or_end {
            continue;
        }
        kept.push(l.clone());
    }

    Failure {
        id,
        loc,
        msg,
        trace: kept,
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
    // The `# <pkg>` banner just names the package, not the error; nor do
    // go's own `PASS` / `FAIL\t<pkg>` / `ok` status lines (a `TestMain`
    // exit leaves them around the actual reason).
    let is_status = |l: &str| {
        l.starts_with('#')
            || l == "PASS"
            || l == "FAIL"
            || l.starts_with("FAIL\t")
            || l.starts_with("ok ")
    };
    let msg_idx = lines
        .iter()
        .position(|l| !is_status(l))
        .or_else(|| lines.iter().position(|l| !l.starts_with('#')));
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

    // --- real go 1.24 fixtures ---

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/go-test/{}",
            env!("CARGO_MANIFEST_DIR"),
            name
        ))
        .unwrap()
    }

    fn parse_fixture(name: &str) -> ParseOutcome {
        let c = captured(&fixture(name), "", false);
        let prepared = GoTest.prepare(argv(&["go", "test", "./..."]));
        GoTest.parse(&c, &prepared).unwrap()
    }

    #[test]
    fn real_failing_test_has_loc_and_msg() {
        let out = parse_fixture("fail.jsonl");
        let r = tests_of(&out);
        assert_eq!((r.total, r.passed, r.failed), (2, 1, 1));
        let f = &r.failures[0];
        assert_eq!(f.id, "fail.TestBad");
        assert_eq!(f.loc, "fail_test.go:10");
        assert_eq!(f.msg, "Add(1,1) = 2, want 3");
    }

    #[test]
    fn timed_out_test_without_terminal_event_is_a_failure() {
        let out = parse_fixture("timeout.jsonl");
        let r = tests_of(&out);
        assert_eq!((r.total, r.passed, r.failed), (2, 1, 1), "{r:?}");
        let f = &r.failures[0];
        assert_eq!(f.id, "timeout.TestSlow");
        assert_eq!(f.msg, "panic: test timed out after 1s");
        assert_eq!(f.loc, "timeout_test.go:11");
        // The user's frame survives; runtime/testing/stdlib frames are
        // filtered.
        assert!(
            f.trace.iter().any(|l| l.contains("timeout_test.go:11")),
            "{:?}",
            f.trace
        );
        assert!(
            !f.trace
                .iter()
                .any(|l| l.contains("/usr/local/go1.24.7/src/")),
            "{:?}",
            f.trace
        );
    }

    #[test]
    fn run_without_terminal_event_in_passing_package_is_not_a_failure() {
        // Only a failed package turns a dangling `run` into a failure.
        const DANGLING_OK: &str = "\
{\"Action\":\"run\",\"Package\":\"example.com/m/a\",\"Test\":\"TestA\"}
{\"Action\":\"pass\",\"Package\":\"example.com/m/a\",\"Elapsed\":0.01}
";
        let c = captured(DANGLING_OK, "", true);
        let prepared = GoTest.prepare(argv(&["go", "test"]));
        let out = GoTest.parse(&c, &prepared).unwrap();
        assert_eq!(tests_of(&out).failed, 0);
    }

    #[test]
    fn testmain_exit_with_no_failing_test_is_a_package_failure() {
        let out = parse_fixture("tmain.jsonl");
        let r = tests_of(&out);
        assert_eq!((r.passed, r.failed), (1, 1), "{r:?}");
        let f = &r.failures[0];
        assert_eq!(f.id, "tmain");
        assert_eq!(f.msg, "leak detector: 1 goroutine leaked");
    }

    #[test]
    fn go_1_24_build_output_events_carry_the_compiler_error() {
        let out = parse_fixture("builderr.jsonl");
        let r = tests_of(&out);
        assert_eq!((r.total, r.failed), (1, 1), "{r:?}");
        let f = &r.failures[0];
        assert_eq!(f.id, "builderr");
        assert_eq!(f.msg, "builderr/b.go:3:23: undefined: x");
        let stdout = out
            .passthrough_stdout
            .expect("compiler output passed through");
        assert!(stdout.contains("undefined: x"), "got: {stdout}");
    }

    #[test]
    fn build_output_of_a_failed_dependency_is_attached_via_failed_build() {
        let out = parse_fixture("usesdep.jsonl");
        let r = tests_of(&out);
        assert_eq!(r.failed, 1, "{r:?}");
        let f = &r.failures[0];
        assert_eq!(f.id, "usesdep");
        assert_eq!(f.msg, "dep/d.go:3:23: undefined: y");
    }

    #[test]
    fn vet_failure_reported_as_build_output() {
        let out = parse_fixture("vetbad.jsonl");
        let r = tests_of(&out);
        assert_eq!(r.failed, 1, "{r:?}");
        assert!(
            r.failures[0].msg.contains("fmt.Printf format %d"),
            "{:?}",
            r.failures[0]
        );
    }

    #[test]
    fn panic_msg_and_first_user_frame_are_populated() {
        let out = parse_fixture("panicp.jsonl");
        let r = tests_of(&out);
        assert_eq!(r.failed, 1, "{r:?}");
        let f = &r.failures[0];
        assert_eq!(f.id, "panicp.TestPanics");
        assert_eq!(
            f.msg,
            "panic: runtime error: index out of range [5] with length 0"
        );
        assert_eq!(f.loc, "p_test.go:7");
        assert!(
            !f.trace.iter().any(|l| l.contains("/src/testing/")),
            "{:?}",
            f.trace
        );
        assert!(f.trace.iter().any(|l| l.contains("p_test.go:11")));
    }

    #[test]
    fn declines_list_and_fuzz_invocations() {
        assert!(!GoTest.detect(&argv(&["go", "test", "-list", "."])));
        assert!(!GoTest.detect(&argv(&["go", "test", "-list=Test.*"])));
        assert!(!GoTest.detect(&argv(&["go", "test", "-fuzz", "FuzzX"])));
        assert!(!GoTest.detect(&argv(&["go", "test", "-fuzz=FuzzX", "./p"])));
    }

    #[test]
    fn prepare_inserts_json_after_leading_chdir_flag() {
        let p = GoTest.prepare(argv(&["go", "test", "-C", "sub", "./..."]));
        assert_eq!(p.argv, argv(&["go", "test", "-C", "sub", "-json", "./..."]));
        let p = GoTest.prepare(argv(&["go", "test", "-C=sub", "./..."]));
        assert_eq!(p.argv, argv(&["go", "test", "-C=sub", "-json", "./..."]));
        let p = GoTest.prepare(argv(&["go", "test", "--C", "sub"]));
        assert_eq!(p.argv, argv(&["go", "test", "--C", "sub", "-json"]));
    }

    // --- markers ---

    #[test]
    fn markers_include_name_and_skip_prefixes() {
        assert!(is_marker("=== NAME  TestX"));
        assert!(is_marker("--- SKIP: TestX (0.00s)"));
    }
}
