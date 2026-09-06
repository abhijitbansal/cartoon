//! `cargo test` / `cargo nextest run` adapter.
//!
//! Both are stable TEXT-parsing targets — libtest's JSON output
//! (`-Z unstable-options --format json`) is nightly-only and must never be
//! injected or relied on here.
use super::report::{trim_trace, Failure, TestReport};
use super::{basename, Adapter, AdapterReport, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use regex::{Match, Regex};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

pub struct CargoTest;

impl Adapter for CargoTest {
    fn name(&self) -> &'static str {
        "cargo-test"
    }
    fn matches(&self) -> &'static str {
        "cargo test | cargo nextest run"
    }
    fn detect(&self, argv: &[String]) -> bool {
        cargo_subcommand(argv).is_some()
    }
    fn prepare(&self, argv: Vec<String>) -> Prepared {
        // Neither runner needs an injected flag: cargo test's own stdout
        // report and nextest's own stderr report are both stable text.
        Prepared {
            argv,
            artifact: None,
        }
    }
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        // The invoked subcommand is the primary signal for which format to
        // expect — trusting stream content alone is spoofable by a test
        // that happens to print "test result:" or "Summary [" itself.
        // Content is only a tiebreak for the (should-be-unreachable, since
        // `detect` already gated on this) case where argv shape is neither.
        let kind = cargo_subcommand(&prepared.argv).unwrap_or_else(|| {
            if captured.stderr.contains("Summary [") {
                Kind::Nextest
            } else {
                Kind::Test
            }
        });
        match kind {
            Kind::Test => {
                let report = parse_cargo_test(&captured.stdout)?;
                // stdout WAS the report; stderr held `Compiling`/`Finished`
                // noise plus, sometimes, compiler diagnostics the agent
                // should still see. Anchored to a line start so a crate
                // merely named `my-error-utils` in printed test output
                // can't trip this on the bare substring.
                let passthrough_stderr = diag_re()
                    .is_match(&captured.stderr)
                    .then(|| captured.stderr.clone());
                Ok(ParseOutcome {
                    report: AdapterReport::Tests(report),
                    passthrough_stdout: None,
                    passthrough_stderr,
                })
            }
            Kind::Nextest => {
                let report = parse_nextest(&captured.stderr)?;
                // stderr WAS the report; stdout is whatever the tests
                // themselves printed (nextest captures it separately from
                // its own report).
                Ok(ParseOutcome {
                    report: AdapterReport::Tests(report),
                    passthrough_stdout: (!captured.stdout.is_empty())
                        .then(|| captured.stdout.clone()),
                    passthrough_stderr: None,
                })
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Kind {
    Test,
    Nextest,
}

/// Which cargo test runner `argv` invokes, skipping an optional leading
/// `+toolchain` token (`cargo +nightly test`, `cargo +1.75.0 test`).
fn cargo_subcommand(argv: &[String]) -> Option<Kind> {
    let first = argv.first()?;
    if basename(first) != "cargo" {
        return None;
    }
    let mut rest = &argv[1..];
    if rest.first().is_some_and(|a| a.starts_with('+')) {
        rest = &rest[1..];
    }
    match (
        rest.first().map(String::as_str),
        rest.get(1).map(String::as_str),
    ) {
        (Some("test"), _) => Some(Kind::Test),
        (Some("nextest"), Some("run")) => Some(Kind::Nextest),
        _ => None,
    }
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

/// `warning:`/`error:`/`error[E...]:` at the start of a line — narrower than
/// a bare substring match, which a crate named e.g. `my-error-utils` could
/// trip via its own printed test output.
fn diag_re() -> &'static Regex {
    static DIAG: OnceLock<Regex> = OnceLock::new();
    re(&DIAG, r"(?m)^(warning|error)(\[|:)")
}

/// An optional capture group as `u64`, defaulting to 0 when absent. Unlike
/// `unwrap_or(0)` on a parse failure, a malformed present value still
/// propagates as an error rather than silently reporting zero.
fn opt_u64(m: Option<Match<'_>>) -> Result<u64> {
    Ok(match m {
        Some(m) => m.as_str().parse()?,
        None => 0,
    })
}

/// Byte offset right after `panicked at ` on the genuine
/// `thread '<name>' panicked at ...` line. Anchoring here — rather than a
/// bare substring search for `panicked at ` — keeps a test's own printed
/// output (which may itself contain that text, or an unrelated `.rs:N`
/// reference) from being mistaken for the real panic location.
fn panic_at(block: &str) -> Option<usize> {
    static THREAD: OnceLock<Regex> = OnceLock::new();
    re(&THREAD, r"(?m)^thread '.*' panicked at ")
        .find(block)
        .map(|m| m.end())
}

/// First `path.rs:line[:col]` mentioned from the panic line onward.
fn find_loc(block: &str) -> String {
    static LOC: OnceLock<Regex> = OnceLock::new();
    let Some(start) = panic_at(block) else {
        return String::new();
    };
    re(&LOC, r"\S+\.rs:\d+(?::\d+)?")
        .find(&block[start..])
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

/// The panic message: the line after `panicked at path:line:col:` (current
/// format), or the text between quotes in `panicked at 'msg', path:line`
/// (pre-2021 format).
fn panic_msg(block: &str) -> String {
    let Some(start) = panic_at(block) else {
        return String::new();
    };
    let after = &block[start..];
    let line_end = after.find('\n').unwrap_or(after.len());
    let head = after[..line_end].trim_end();
    if head.ends_with(':') {
        let rest = after[line_end..].strip_prefix('\n').unwrap_or("");
        let msg_end = rest.find('\n').unwrap_or(rest.len());
        return rest[..msg_end].trim().to_string();
    }
    if let Some(quoted) = head.strip_prefix('\'') {
        if let Some(close) = quoted.find('\'') {
            return quoted[..close].trim().to_string();
        }
    }
    String::new()
}

/// Names listed in a trailing `failures:\n    name\n...` summary — as
/// opposed to the `failures:\n\n---- name stdout ----` block-intro form,
/// which is followed by a blank line rather than an indented name.
/// `cargo test -- --show-output` prints the same `---- name stdout ----`
/// header shape for PASSING tests under a `successes:` section; only names
/// that appear in a failures summary are real failures.
fn failing_names(stdout: &str) -> HashSet<String> {
    static SUMMARY: OnceLock<Regex> = OnceLock::new();
    let summary_re = re(&SUMMARY, r"(?m)^failures:\n((?:[ \t]+\S.*\n)+)");
    let mut names = HashSet::new();
    for cap in summary_re.captures_iter(stdout) {
        for line in cap[1].lines() {
            let name = line.trim();
            if !name.is_empty() {
                names.insert(name.to_string());
            }
        }
    }
    names
}

/// `---- <name> stdout ----` headers and the content between them: up to
/// the next such header, or the next `failures:` / `test result:` /
/// `running N tests` line — whichever comes first (a multi-binary run packs
/// several of these in sequence, and an earlier binary's block must not
/// swallow its own summary, `test result:` line, and the next binary's
/// `running N tests` banner). Only headers whose name appears in the
/// trailing failures summary are kept — see `failing_names`.
fn cargo_test_blocks(stdout: &str) -> Vec<(String, String)> {
    static HEADER: OnceLock<Regex> = OnceLock::new();
    static BOUNDARY: OnceLock<Regex> = OnceLock::new();
    let header_re = re(&HEADER, r"(?m)^---- (.+) stdout ----$");
    let boundary_re = re(
        &BOUNDARY,
        r"(?m)^(?:failures:|test result:|running \d+ tests?)",
    );
    let caps: Vec<_> = header_re.captures_iter(stdout).collect();
    let failing = failing_names(stdout);
    caps.iter()
        .enumerate()
        .filter_map(|(i, cap)| {
            let name = cap[1].to_string();
            if !failing.contains(&name) {
                return None;
            }
            let content_start = cap.get(0).unwrap().end();
            let next_header = caps
                .get(i + 1)
                .map(|c| c.get(0).unwrap().start())
                .unwrap_or(stdout.len());
            let next_boundary = boundary_re
                .find_at(stdout, content_start)
                .map(|m| m.start())
                .filter(|&pos| pos < next_header)
                .unwrap_or(next_header);
            Some((
                name,
                stdout[content_start..next_boundary].trim().to_string(),
            ))
        })
        .collect()
}

fn parse_cargo_test(stdout: &str) -> Result<TestReport> {
    static RESULT: OnceLock<Regex> = OnceLock::new();
    // Several `test result:` lines appear (unit / integration / doc-tests
    // each get their own binary run) — sum across all of them.
    let result_re = re(
        &RESULT,
        r"test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; \d+ measured; \d+ filtered out; finished in ([0-9.]+)s",
    );
    let (mut passed, mut failed, mut ignored) = (0u64, 0u64, 0u64);
    let mut duration_s = 0.0f64;
    let mut found = false;
    for c in result_re.captures_iter(stdout) {
        found = true;
        passed += c[1].parse::<u64>()?;
        failed += c[2].parse::<u64>()?;
        ignored += c[3].parse::<u64>()?;
        duration_s += c[4].parse::<f64>()?;
    }
    if !found {
        anyhow::bail!("no 'test result:' line — not cargo test output");
    }

    let failures = cargo_test_blocks(stdout)
        .into_iter()
        .map(|(id, block)| Failure {
            loc: find_loc(&block),
            msg: panic_msg(&block),
            trace: trim_trace(&block),
            id,
        })
        .collect();

    Ok(TestReport {
        runner: "cargo-test",
        total: passed + failed + ignored,
        passed,
        failed,
        skipped: ignored,
        duration_s,
        failures,
    })
}

/// `--- STDOUT: <name> ---` / `--- STDERR: <name> ---` blocks, merged per
/// test name (a failing test can produce both).
fn nextest_blocks(stderr: &str) -> HashMap<String, String> {
    static HEADER: OnceLock<Regex> = OnceLock::new();
    let header_re = re(&HEADER, r"(?m)^--- (?:STDOUT|STDERR): (.+) ---$");
    let caps: Vec<_> = header_re.captures_iter(stderr).collect();
    let mut blocks: HashMap<String, String> = HashMap::new();
    for (i, cap) in caps.iter().enumerate() {
        let name = cap[1].trim().to_string();
        let content_start = cap.get(0).unwrap().end();
        let content_end = caps
            .get(i + 1)
            .map(|c| c.get(0).unwrap().start())
            .unwrap_or(stderr.len());
        let content = stderr[content_start..content_end].trim();
        if content.is_empty() {
            continue;
        }
        let entry = blocks.entry(name).or_default();
        if !entry.is_empty() {
            entry.push('\n');
        }
        entry.push_str(content);
    }
    blocks
}

fn parse_nextest(stderr: &str) -> Result<TestReport> {
    static SUMMARY: OnceLock<Regex> = OnceLock::new();
    // nextest omits the `failed`/`skipped` segments entirely on some runs,
    // adds a `(N slow|leaky|flaky)` annotation after `passed`, and can add
    // `exec failed` / `timed out` segments alongside `failed` — every
    // segment after `passed` is optional, and all three failure-shaped
    // counts fold into `failed`.
    let summary_re = re(
        &SUMMARY,
        r"Summary \[\s*([0-9.]+)s\]\s*\d+ tests? run: (\d+) passed(?: \([^)]*\))?(?:, (\d+) failed)?(?:, (\d+) exec failed)?(?:, (\d+) timed out)?(?:, (\d+) skipped)?",
    );
    let caps = summary_re
        .captures(stderr)
        .context("no 'Summary [' line — not nextest output")?;
    let duration_s: f64 = caps[1].parse()?;
    let passed: u64 = caps[2].parse()?;
    let failed = opt_u64(caps.get(3))? + opt_u64(caps.get(4))? + opt_u64(caps.get(5))?;
    let skipped: u64 = opt_u64(caps.get(6))?;

    static FAIL_LINE: OnceLock<Regex> = OnceLock::new();
    // ABORT / SIGSEGV / SIGABRT / TIMEOUT are terminal statuses that also
    // fold into the summary's `failed` total — give them Failure entries
    // too, not just the plain `FAIL` status.
    let fail_re = re(
        &FAIL_LINE,
        r"(?m)^\s*(?:FAIL|ABORT|SIGSEGV|SIGABRT|TIMEOUT)\s*\[\s*[0-9.]+s\]\s*(\S+ \S+)\s*$",
    );
    let blocks = nextest_blocks(stderr);
    let failures = fail_re
        .captures_iter(stderr)
        .map(|c| {
            let id = c[1].trim().to_string();
            let block = blocks.get(&id).cloned().unwrap_or_default();
            Failure {
                loc: find_loc(&block),
                msg: panic_msg(&block),
                trace: trim_trace(&block),
                id,
            }
        })
        .collect();

    Ok(TestReport {
        runner: "cargo-nextest",
        total: passed + failed + skipped,
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
    use std::os::unix::process::ExitStatusExt;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn captured(stdout: &str, stderr: &str, code: i32) -> Captured {
        Captured {
            stdout: stdout.into(),
            stderr: stderr.into(),
            status: std::process::ExitStatus::from_raw(code << 8),
        }
    }

    const ALL_PASS: &str = "\
running 2 tests
test tests::a ... ok
test tests::b ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

running 1 test
test tests::integration_a ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

   Doc-tests my_crate

running 1 test
test src/lib.rs - module::fn (line 12) ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
";

    // Two binaries, each with its own failure — regression fixture for the
    // block-boundary bug (an earlier binary's block must not swallow its
    // own summary and the next binary's `running N tests` banner).
    const TWO_BINARIES_BOTH_FAILING: &str = "\
running 2 tests
test tests::a_ok ... ok
test tests::a_fail ... FAILED

failures:

---- tests::a_fail stdout ----
thread 'tests::a_fail' panicked at src/a.rs:5:9:
first crate failed here

failures:
    tests::a_fail

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

running 2 tests
test tests::b_ok ... ok
test tests::b_fail ... FAILED

failures:

---- tests::b_fail stdout ----
thread 'tests::b_fail' panicked at src/b.rs:9:5:
second crate failed here

failures:
    tests::b_fail

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";

    // `-- --show-output` prints a `successes:` block with the same
    // `---- name stdout ----` header shape for a PASSING test.
    const SHOW_OUTPUT_WITH_PASS_AND_FAIL: &str = "\
running 2 tests
test tests::t_pass ... ok
test tests::t_fail ... FAILED

successes:

---- tests::t_pass stdout ----
ordinary printed output, not a failure

successes:
    tests::t_pass

failures:

---- tests::t_fail stdout ----
thread 'tests::t_fail' panicked at src/lib.rs:7:1:
boom

failures:
    tests::t_fail

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";

    const WITH_FAILURE: &str = "\
running 3 tests
test tests::t_ok ... ok
test tests::t_fail ... FAILED
test tests::t_skip ... ignored, flaky

failures:

---- tests::t_fail stdout ----
thread 'tests::t_fail' panicked at src/lib.rs:42:9:
assertion `left == right` failed
  left: 2
 right: 3
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    tests::t_fail

test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s
";

    const NEXTEST_MIXED: &str = "\
        PASS [   0.012s] my-crate tests::a
        FAIL [   0.030s] my-crate tests::b
        SKIP [   0.000s] my-crate tests::c
        PASS [   0.500s] my-crate tests::slow (SLOW)
--- STDOUT: my-crate tests::b ---
thread 'tests::b' panicked at src/lib.rs:10:5:
assertion failed: false
--- STDERR: my-crate tests::b ---
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
     Summary [   0.542s] 4 tests run: 2 passed, 1 failed, 1 skipped
";

    // No `failed`/`skipped` segments at all on an all-green run.
    const NEXTEST_ALL_PASS: &str = "\
        PASS [   0.012s] my-crate tests::a
        PASS [   0.013s] my-crate tests::b
        PASS [   0.014s] my-crate tests::c
     Summary [   0.031s] 3 tests run: 3 passed, 0 skipped
";

    const COMPILE_ERROR: &str = "\
error[E0433]: failed to resolve: use of undeclared crate or module `foo`
 --> src/lib.rs:1:5
  |
1 | use foo::bar;
  |     ^^^ use of undeclared crate or module `foo`

error: could not compile `my_crate` (lib test) due to 1 previous error
";

    #[test]
    fn detects_cargo_test_and_nextest_run() {
        assert!(CargoTest.detect(&argv(&["cargo", "test"])));
        assert!(CargoTest.detect(&argv(&["cargo", "nextest", "run"])));
        assert!(CargoTest.detect(&argv(&["/usr/bin/cargo", "test", "--workspace"])));
        assert!(!CargoTest.detect(&argv(&["cargo", "build"])));
        assert!(!CargoTest.detect(&argv(&["cargo", "nextest", "list"])));
        assert!(!CargoTest.detect(&argv(&["go", "test"])));
    }

    #[test]
    fn detects_through_a_toolchain_override() {
        assert!(CargoTest.detect(&argv(&["cargo", "+nightly", "test"])));
        assert!(CargoTest.detect(&argv(&["cargo", "+1.75.0", "nextest", "run"])));
        assert!(!CargoTest.detect(&argv(&["cargo", "+nightly", "build"])));
    }

    #[test]
    fn prepare_leaves_argv_untouched() {
        let p = CargoTest.prepare(argv(&["cargo", "test", "--workspace", "--", "--nocapture"]));
        assert_eq!(
            p.argv,
            argv(&["cargo", "test", "--workspace", "--", "--nocapture"])
        );
        assert!(p.artifact.is_none());
    }

    #[test]
    fn parse_all_pass_sums_across_binaries() {
        let captured = captured(ALL_PASS, "", 0);
        let out = CargoTest
            .parse(&captured, &CargoTest.prepare(argv(&["cargo", "test"])))
            .unwrap();
        let AdapterReport::Tests(r) = out.report else {
            panic!("expected Tests report")
        };
        assert_eq!(r.runner, "cargo-test");
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (4, 4, 0, 0));
        assert!((r.duration_s - 0.08).abs() < 1e-9);
        assert!(out.passthrough_stdout.is_none());
        assert!(out.passthrough_stderr.is_none());
    }

    #[test]
    fn parse_failing_fixture_captures_id_loc_msg_trace() {
        let captured = captured(WITH_FAILURE, "", 101);
        let out = CargoTest
            .parse(&captured, &CargoTest.prepare(argv(&["cargo", "test"])))
            .unwrap();
        let AdapterReport::Tests(r) = out.report else {
            panic!("expected Tests report")
        };
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (3, 1, 1, 1));
        assert_eq!(r.failures.len(), 1);
        let f = &r.failures[0];
        assert_eq!(f.id, "tests::t_fail");
        assert_eq!(f.loc, "src/lib.rs:42:9");
        assert!(f.msg.contains("assertion"), "got: {}", f.msg);
        assert!(
            !f.trace
                .iter()
                .any(|l| l.contains("---- tests::t_fail stdout ----")),
            "{:?}",
            f.trace
        );
    }

    #[test]
    fn ignored_counts_as_skipped() {
        let captured = captured(WITH_FAILURE, "", 101);
        let out = CargoTest
            .parse(&captured, &CargoTest.prepare(argv(&["cargo", "test"])))
            .unwrap();
        let AdapterReport::Tests(r) = out.report else {
            panic!("expected Tests report")
        };
        assert_eq!(r.skipped, 1);
    }

    #[test]
    fn each_binarys_failure_block_stays_bounded_to_its_own_binary() {
        let captured = captured(TWO_BINARIES_BOTH_FAILING, "", 101);
        let out = CargoTest
            .parse(&captured, &CargoTest.prepare(argv(&["cargo", "test"])))
            .unwrap();
        let AdapterReport::Tests(r) = out.report else {
            panic!("expected Tests report")
        };
        assert_eq!((r.total, r.passed, r.failed), (4, 2, 2));
        assert_eq!(r.failures.len(), 2);
        assert_eq!(r.failures[0].id, "tests::a_fail");
        assert_eq!(r.failures[0].loc, "src/a.rs:5:9");
        assert!(r.failures[0]
            .trace
            .iter()
            .any(|l| l.contains("first crate")));
        assert!(!r.failures[0]
            .trace
            .iter()
            .any(|l| l.contains("second crate") || l.contains("running 2 tests")));
        assert_eq!(r.failures[1].id, "tests::b_fail");
        assert_eq!(r.failures[1].loc, "src/b.rs:9:5");
    }

    #[test]
    fn show_output_successes_block_is_not_a_failure() {
        let captured = captured(SHOW_OUTPUT_WITH_PASS_AND_FAIL, "", 101);
        let out = CargoTest
            .parse(&captured, &CargoTest.prepare(argv(&["cargo", "test"])))
            .unwrap();
        let AdapterReport::Tests(r) = out.report else {
            panic!("expected Tests report")
        };
        assert_eq!(r.failures.len(), 1);
        assert_eq!(r.failures[0].id, "tests::t_fail");
    }

    #[test]
    fn stderr_with_warning_passes_through_stderr_with_warning() {
        let captured = captured(ALL_PASS, "warning: unused import: `foo`\n", 0);
        let out = CargoTest
            .parse(&captured, &CargoTest.prepare(argv(&["cargo", "test"])))
            .unwrap();
        assert!(out.passthrough_stderr.unwrap().contains("warning"));
    }

    #[test]
    fn stderr_mentioning_error_word_mid_line_is_not_passed_through() {
        // A crate/test named with "error" in it must not spoof the anchor.
        let captured = captured(ALL_PASS, "   Compiling my-error-utils v0.1.0\n", 0);
        let out = CargoTest
            .parse(&captured, &CargoTest.prepare(argv(&["cargo", "test"])))
            .unwrap();
        assert!(out.passthrough_stderr.is_none());
    }

    #[test]
    fn parses_nextest_fixture() {
        let captured = captured("", NEXTEST_MIXED, 1);
        let out = CargoTest
            .parse(
                &captured,
                &CargoTest.prepare(argv(&["cargo", "nextest", "run"])),
            )
            .unwrap();
        let AdapterReport::Tests(r) = out.report else {
            panic!("expected Tests report")
        };
        assert_eq!(r.runner, "cargo-nextest");
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (4, 2, 1, 1));
        assert_eq!(r.failures.len(), 1);
        assert_eq!(r.failures[0].id, "my-crate tests::b");
        assert_eq!(r.failures[0].loc, "src/lib.rs:10:5");
    }

    #[test]
    fn parses_nextest_all_pass_fixture_with_no_failed_segment() {
        let captured = captured("some test printed this", NEXTEST_ALL_PASS, 0);
        let out = CargoTest
            .parse(
                &captured,
                &CargoTest.prepare(argv(&["cargo", "nextest", "run"])),
            )
            .unwrap();
        let AdapterReport::Tests(r) = out.report else {
            panic!("expected Tests report")
        };
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (3, 3, 0, 0));
        assert!(r.failures.is_empty());
        assert_eq!(
            out.passthrough_stdout.as_deref(),
            Some("some test printed this")
        );
        assert!(out.passthrough_stderr.is_none());
    }

    #[test]
    fn compile_error_with_no_summary_is_err() {
        let captured = captured(COMPILE_ERROR, "", 101);
        let result = CargoTest.parse(&captured, &CargoTest.prepare(argv(&["cargo", "test"])));
        assert!(result.is_err());
    }
}
