use regex::Regex;
use std::sync::OnceLock;

/// True for a line that signals a failure: the one shared predicate the
/// aggressive tier uses to protect lines (windowing anchors, near-dup
/// templating, diagnostics extraction) and that other stages (e.g. the
/// token budget) can reuse.
pub fn is_error_line(line: &str) -> bool {
    static CI: OnceLock<Regex> = OnceLock::new();
    static CS: OnceLock<Regex> = OnceLock::new();
    // Case-insensitive keywords and phrases.
    let ci = CI.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:errors?|fail(?:s|ed|ure|ures)?|exception|panic(?:ked)?|fatal|traceback|assert(?:ion)?|segmentation fault|core dumped|permission denied|no such file or directory|not found|cannot|undefined reference)\b",
        )
        .unwrap()
    });
    // Case-sensitive markers: `npm ERR!` (no `\b` after `!`), `Killed`
    // (OOM / SIGKILL), pytest's `E   ` assertion lines, and identifier-glued
    // names (`KeyError:`, `NullPointerException`) that `\b…\b` alone misses.
    let cs = CS.get_or_init(|| {
        Regex::new(r"\bERR!|\bKilled\b|^E\s{2,}|[A-Za-z_][A-Za-z0-9_]*(?:Error|Exception)\b")
            .unwrap()
    });
    ci.is_match(line) || cs.is_match(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_failure_markers() {
        for line in [
            "thread 'main' panicked at src/main.rs:2:5",
            "panic: runtime error: index out of range",
            "npm ERR! code ELIFECYCLE",
            "npm error code ERESOLVE",
            "Segmentation fault (core dumped)",
            "bash: line 1: 4242 Killed                  ./train",
            "cp: cannot open 'x': Permission denied",
            "ls: x: No such file or directory",
            "sh: 1: foo: not found",
            "make: cannot stat 'out'",
            "/usr/bin/ld: main.o: undefined reference to `foo'",
            "E       assert 500 == 200",
            "E   AssertionError",
            "    assert resp.status == 200",
            "Traceback (most recent call last):",
            "KeyError: 'email'",
            "java.lang.NullPointerException: x",
            "FAILED tests/test_a.py::test_x - assert 1 == 2",
            "--- FAIL: TestFoo (0.00s)",
            "fatal: not a git repository",
            "ERROR connection refused",
            "1 failure",
        ] {
            assert!(is_error_line(line), "should match: {line}");
        }
    }

    #[test]
    fn ignores_ordinary_lines() {
        for line in [
            "Compiling cartoon v0.6.0",
            "test tests::ok ... ok",
            "Every day",
            "step 42: ok",
            "Eventually consistent",
            "error_reporting.rs compiled",
            "skilled workers",
            "INFO worker heartbeat",
        ] {
            assert!(!is_error_line(line), "should not match: {line}");
        }
    }
}
