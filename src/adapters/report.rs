use regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::OnceLock;

#[derive(Debug)]
pub struct TestReport {
    pub runner: &'static str,
    pub total: u64,
    pub passed: u64,
    pub failed: u64,
    pub skipped: u64,
    pub duration_s: f64,
    pub failures: Vec<Failure>,
}

#[derive(Debug)]
pub struct Failure {
    pub id: String,
    pub loc: String,
    pub msg: String,
    pub trace: Vec<String>,
}

/// Sum several reports (one JUnit file per module, gradle-style) into one.
/// Runner label comes from the first. None for an empty list.
pub fn merge(reports: Vec<TestReport>) -> Option<TestReport> {
    let mut iter = reports.into_iter();
    let mut acc = iter.next()?;
    for r in iter {
        acc.total += r.total;
        acc.passed += r.passed;
        acc.failed += r.failed;
        acc.skipped += r.skipped;
        acc.duration_s += r.duration_s;
        acc.failures.extend(r.failures);
    }
    Some(acc)
}

/// Asymmetric rendering: passes cost one summary block; failures keep
/// id/loc/msg rows plus budgeted traces. `fast_note` discloses injected
/// acceleration args (e.g. "-n auto") right after the runner line. Paths
/// under the working directory are printed relative to it.
pub fn render(report: &TestReport, trace_lines: usize, fast_note: Option<&str>) -> String {
    let rel = |s: &str| relativize(s);
    let mut root = Map::new();
    root.insert("runner".into(), json!(report.runner));
    if let Some(f) = fast_note {
        root.insert("fast".into(), json!(f));
    }
    root.insert(
        "summary".into(),
        json!({
            "total": report.total,
            "passed": report.passed,
            "failed": report.failed,
            "skipped": report.skipped,
            // Non-finite durations (e.g. an absurd timestamp in untrusted
            // runner output) would serialize to null/panic; clamp to 0.
            "duration_s": if report.duration_s.is_finite() { report.duration_s } else { 0.0 },
        }),
    );
    if !report.failures.is_empty() {
        root.insert(
            "failures".into(),
            Value::Array(
                report
                    .failures
                    .iter()
                    .map(|f| json!({"id": rel(&f.id), "loc": rel(&f.loc), "msg": rel(&f.msg)}))
                    .collect(),
            ),
        );
        let mut traces = Map::new();
        for (key, f) in trace_keys(&report.failures)
            .into_iter()
            .zip(&report.failures)
        {
            let budgeted = budget_trace(&f.trace, trace_lines);
            if !budgeted.is_empty() {
                let lines: Vec<String> = budgeted.iter().map(|l| rel(l)).collect();
                traces.insert(rel(&key), json!(lines));
            }
        }
        if !traces.is_empty() {
            root.insert("traces".into(), Value::Object(traces));
        }
    }
    crate::toon::encode(&Value::Object(root))
}

/// One unique `traces` key per failure, so two failures sharing an id (the
/// same test name in two files) never overwrite each other's trace: a
/// duplicated id becomes `id@loc`, and a still-duplicated key gets `#N`.
fn trace_keys(failures: &[Failure]) -> Vec<String> {
    let count = |id: &str| failures.iter().filter(|f| f.id == id).count();
    let mut keys: Vec<String> = failures
        .iter()
        .map(|f| {
            if count(&f.id) > 1 && !f.loc.is_empty() {
                format!("{}@{}", f.id, f.loc)
            } else {
                f.id.clone()
            }
        })
        .collect();
    let mut seen = std::collections::HashMap::<String, usize>::new();
    for k in keys.iter_mut() {
        let n = seen.entry(k.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            *k = format!("{k}#{n}");
        }
    }
    keys
}

/// Lines worth keeping first when a trace is over budget: pytest's
/// `>`/`E` lines, jest's `>` code-frame line and Expected/Received, and the
/// exception / panic line itself.
fn is_key_line(t: &str) -> bool {
    static EXC: OnceLock<Regex> = OnceLock::new();
    t.starts_with("E ")
        || t.starts_with('>')
        || t.starts_with("Expected")
        || t.starts_with("Received")
        || t.contains("panicked at")
        || EXC
            .get_or_init(|| {
                Regex::new(r"^[A-Za-z_][A-Za-z0-9_.]*(Error|Exception|Failure|Exit)\b").unwrap()
            })
            .is_match(t)
}

/// Fit a trace into `max` lines. Over budget, keep the most informative
/// lines (see `is_key_line`), then fill from the tail (where the error
/// usually is) rather than the leading setup context, and always say how
/// many lines were dropped, at the first gap.
pub fn budget_trace(trace: &[String], max: usize) -> Vec<String> {
    if trace.len() <= max {
        return trace.to_vec();
    }
    if max == 0 {
        return Vec::new();
    }
    // One slot goes to the omitted-lines marker.
    let mut left = max.saturating_sub(1).max(1);
    let mut keep = vec![false; trace.len()];
    for (i, l) in trace.iter().enumerate() {
        if left == 0 {
            break;
        }
        if is_key_line(l) {
            keep[i] = true;
            left -= 1;
        }
    }
    for i in (0..trace.len()).rev() {
        if left == 0 {
            break;
        }
        if !keep[i] {
            keep[i] = true;
            left -= 1;
        }
    }
    let omitted = keep.iter().filter(|k| !**k).count();
    let mut out = Vec::with_capacity(max);
    let mut marked = false;
    for (l, k) in trace.iter().zip(&keep) {
        if *k {
            out.push(l.clone());
        } else if !marked {
            out.push(format!("\u{2026} +{omitted} lines omitted (see raw_log)"));
            marked = true;
        }
    }
    out
}

/// Working-directory prefixes to strip from printed paths: the canonical
/// cwd and the shell's logical `$PWD` when it differs (symlinked dirs).
fn cwd_prefixes() -> &'static [String] {
    static P: OnceLock<Vec<String>> = OnceLock::new();
    P.get_or_init(|| {
        let mut v: Vec<String> = Vec::new();
        let cwd = std::env::current_dir().ok();
        let pwd = std::env::var_os("PWD").map(std::path::PathBuf::from);
        for d in cwd.into_iter().chain(pwd) {
            let s = d.to_string_lossy().trim_end_matches('/').to_string();
            // Stripping "/" (cwd at the filesystem root) would mangle
            // every absolute path.
            if !s.is_empty() && !v.contains(&s) {
                v.push(s);
            }
        }
        v
    })
}

/// Print paths under the working directory relative to it: a big token
/// win for absolute-path runners (unittest, jest, vitest), shared by every
/// adapter. See `relativize_with`.
pub fn relativize(s: &str) -> String {
    relativize_with(s, cwd_prefixes())
}

/// Strip `<dir>/` (and `file://<dir>/`) wherever it starts a path in `s`:
/// at the start of the string or after a non-path character, so a longer
/// path that merely ends in `<dir>` is left alone.
pub fn relativize_with(s: &str, dirs: &[String]) -> String {
    let mut out = s.to_string();
    for d in dirs {
        let prefix = format!("{d}/");
        if !out.contains(&prefix) {
            continue;
        }
        out = out.replace(&format!("file://{prefix}"), "");
        let mut res = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(i) = rest.find(&prefix) {
            let prev = rest[..i].chars().last().or_else(|| res.chars().last());
            let at_path_start =
                prev.is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-')));
            res.push_str(&rest[..i]);
            if !at_path_start {
                res.push_str(&prefix);
            }
            rest = &rest[i + prefix.len()..];
        }
        res.push_str(rest);
        out = res;
    }
    out
}

/// Recursively relativize every string in a diagnostics `Value` (see
/// `relativize`).
pub fn relativize_value(v: &mut Value) {
    match v {
        Value::String(s) => {
            let r = relativize(s);
            if r != *s {
                *s = r;
            }
        }
        Value::Array(a) => a.iter_mut().for_each(relativize_value),
        Value::Object(o) => o.values_mut().for_each(relativize_value),
        _ => {}
    }
}

const NOISE: &[&str] = &[
    "site-packages",
    "/_pytest/",
    "/unittest/case.py",
    "/importlib/",
    "node_modules",
    "/jest-",
    "node:internal",
    // CPython import machinery: `<frozen importlib._bootstrap>:1204: in …`
    "<frozen ",
];

/// pytest "short" trace style frame header: `path/file.py:126: in import_module`
fn is_short_frame_header(t: &str) -> bool {
    let mut parts = t.splitn(3, ':');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(p), Some(n), Some(rest))
            if !p.is_empty()
                && n.trim().parse::<u64>().is_ok()
                && rest.trim_start().starts_with("in ")
    )
}

/// Keep user-code frames, drop framework internals, drop blank lines and
/// caret-only marker lines (^^^^) that point into dropped source context.
pub fn trim_trace(raw: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut skip_frame = false;
    for line in raw.lines() {
        let l = line.trim_end();
        let t = l.trim_start();
        let is_frame_header =
            t.starts_with("File \"") || t.starts_with("at ") || is_short_frame_header(t);
        if is_frame_header {
            skip_frame = NOISE.iter().any(|n| l.contains(n));
        }
        // Python 3.11+ marks the failing expression with `~~~~^^^^` runs.
        let is_caret_line = !t.is_empty() && t.chars().all(|c| c == '^' || c == '~' || c == ' ');
        let is_traceback_header = t == "Traceback (most recent call last):";
        // pytest prints `???` for source it cannot show (frozen modules).
        let is_unknown_source = t == "???";
        // pytest's `E ` lines are the exception itself: kept even when the
        // frame they follow is framework noise.
        let is_pytest_error = t.starts_with("E ");
        if (!skip_frame || is_pytest_error)
            && !t.is_empty()
            && !is_caret_line
            && !is_traceback_header
            && !is_unknown_source
        {
            lines.push(t.to_string());
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TestReport {
        TestReport {
            runner: "pytest",
            total: 48,
            passed: 45,
            failed: 2,
            skipped: 1,
            duration_s: 3.2,
            failures: vec![
                Failure {
                    id: "tests/test_auth.py::test_expiry".into(),
                    loc: "tests/test_auth.py:42".into(),
                    msg: "assert exp < now".into(),
                    trace: vec![
                        "tests/test_auth.py:42 in test_expiry".into(),
                        "assert token.exp < now()".into(),
                    ],
                },
                Failure {
                    id: "tests/test_user.py::test_create".into(),
                    loc: "tests/test_user.py:88".into(),
                    msg: "KeyError: 'email'".into(),
                    trace: vec![],
                },
            ],
        }
    }

    #[test]
    fn merge_sums_reports() {
        let a = TestReport {
            runner: "junit",
            total: 2,
            passed: 1,
            failed: 1,
            skipped: 0,
            duration_s: 1.0,
            failures: vec![],
        };
        let b = TestReport {
            runner: "junit",
            total: 3,
            passed: 3,
            failed: 0,
            skipped: 0,
            duration_s: 0.5,
            failures: vec![],
        };
        let m = merge(vec![a, b]).unwrap();
        assert_eq!((m.total, m.passed, m.failed), (5, 4, 1));
        assert!((m.duration_s - 1.5).abs() < 1e-9);
        assert!(merge(vec![]).is_none());
    }

    #[test]
    fn renders_summary_and_failures() {
        let out = render(&sample(), 20, None);
        assert!(out.contains("runner: pytest"), "got:\n{out}");
        assert!(out.contains("total: 48"));
        assert!(out.contains("failed: 2"));
        assert!(out.contains("failures[2]{id,loc,msg}:"));
        assert!(out.contains("tests/test_auth.py::test_expiry"));
    }

    #[test]
    fn empty_trace_gets_no_traces_entry() {
        let out = render(&sample(), 20, None);
        // traces section exists (first failure has a trace) but only one key
        let traces_idx = out.find("traces:").expect("traces section");
        let tail = &out[traces_idx..];
        assert!(tail.contains("test_expiry"));
        assert!(!tail.contains("test_create"));
    }

    #[test]
    fn all_pass_renders_no_failures_section() {
        let mut r = sample();
        r.failures.clear();
        r.failed = 0;
        r.passed = 47;
        let out = render(&r, 20, None);
        assert!(!out.contains("failures"));
        assert!(!out.contains("traces"));
    }

    #[test]
    fn trace_capped_at_limit() {
        let mut r = sample();
        r.failures[0].trace = (0..50).map(|i| format!("line {i}")).collect();
        let out = render(&r, 5, None);
        // No key lines: the tail survives, the head is disclosed as omitted.
        assert!(out.contains("line 49"), "got:\n{out}");
        assert!(!out.contains("line 0"), "got:\n{out}");
        assert!(
            out.contains("+46 lines omitted (see raw_log)"),
            "got:\n{out}"
        );
    }

    #[test]
    fn budget_prefers_pytest_e_and_arrow_lines_over_setup_context() {
        let mut trace: Vec<String> = (0..20).map(|i| format!("setup_step_{i}()")).collect();
        trace.push(">       assert total == 42".into());
        trace.push("E       assert 41 == 42".into());
        trace.push("tests/test_x.py:30: AssertionError".into());
        let kept = budget_trace(&trace, 5);
        assert_eq!(kept.len(), 5, "{kept:?}");
        assert!(kept.contains(&">       assert total == 42".to_string()));
        assert!(kept.contains(&"E       assert 41 == 42".to_string()));
        assert!(kept.contains(&"tests/test_x.py:30: AssertionError".to_string()));
        assert!(
            kept.iter().any(|l| l.contains("+19 lines omitted")),
            "{kept:?}"
        );
        assert!(!kept.contains(&"setup_step_0()".to_string()), "{kept:?}");
    }

    #[test]
    fn budget_leaves_short_traces_alone() {
        let trace = vec!["a".to_string(), "b".to_string()];
        assert_eq!(budget_trace(&trace, 2), trace);
        assert!(budget_trace(&trace, 0).is_empty());
    }

    #[test]
    fn duplicate_ids_keep_every_trace() {
        let mut r = sample();
        r.failures[1].id = r.failures[0].id.clone();
        r.failures[1].trace = vec!["second trace line".into()];
        let out = render(&r, 20, None);
        assert!(out.contains("assert token.exp < now()"), "got:\n{out}");
        assert!(out.contains("second trace line"), "got:\n{out}");
        assert!(
            out.contains("test_expiry@tests/test_user.py:88"),
            "got:\n{out}"
        );
    }

    #[test]
    fn duplicate_id_and_loc_get_an_index() {
        let f = |t: &str| Failure {
            id: "t".into(),
            loc: "a.js".into(),
            msg: String::new(),
            trace: vec![t.into()],
        };
        assert_eq!(
            trace_keys(&[f("x"), f("y"), f("z")]),
            vec!["t@a.js", "t@a.js#2", "t@a.js#3"]
        );
    }

    #[test]
    fn relativize_strips_cwd_prefixes_at_path_starts_only() {
        let dirs = vec!["/home/u/proj".to_string()];
        assert_eq!(
            relativize_with("File \"/home/u/proj/tests/t.py\", line 3", &dirs),
            "File \"tests/t.py\", line 3"
        );
        assert_eq!(relativize_with("/home/u/proj/a.py:1", &dirs), "a.py:1");
        assert_eq!(
            relativize_with("at file:///home/u/proj/src/x.test.js:3:9", &dirs),
            "at src/x.test.js:3:9"
        );
        assert_eq!(
            relativize_with("(/home/u/proj/a.js:1) and /home/u/proj/b.js", &dirs),
            "(a.js:1) and b.js"
        );
        // A different directory that merely ends with the cwd is untouched.
        assert_eq!(
            relativize_with("/srv/home/u/proj/a.py", &dirs),
            "/srv/home/u/proj/a.py"
        );
        assert_eq!(
            relativize_with("/home/u/project2/a.py", &dirs),
            "/home/u/project2/a.py"
        );
    }

    #[test]
    fn render_relativizes_paths_under_cwd() {
        let cwd = std::env::current_dir().unwrap().display().to_string();
        let mut r = sample();
        r.failures[0].loc = format!("{cwd}/tests/test_auth.py:42");
        r.failures[0].trace = vec![format!("File \"{cwd}/tests/test_auth.py\", line 42")];
        let out = render(&r, 20, None);
        assert!(!out.contains(&cwd), "got:\n{out}");
        assert!(
            out.contains("File \\\"tests/test_auth.py\\\", line 42"),
            "got:\n{out}"
        );
    }

    #[test]
    fn trim_trace_drops_frozen_importlib_frames_and_unknown_source() {
        let raw = "<frozen importlib._bootstrap>:1204: in _gcd_import\n    ???\n<frozen importlib._bootstrap>:690: in _load_unlocked\n    ???\nE   SyntaxError: '(' was never closed";
        assert_eq!(
            trim_trace(raw),
            vec!["E   SyntaxError: '(' was never closed"]
        );
    }

    #[test]
    fn trim_trace_drops_framework_frames() {
        let raw = "Traceback (most recent call last):\n  File \"/usr/lib/python3/site-packages/_pytest/runner.py\", line 1, in run\n    framework()\n  File \"tests/test_auth.py\", line 42, in test_expiry\n    assert token.exp < now()\nAssertionError: assert exp < now";
        let t = trim_trace(raw);
        let joined = t.join("\n");
        assert!(joined.contains("tests/test_auth.py"), "got: {joined}");
        assert!(!joined.contains("site-packages"));
        assert!(joined.contains("AssertionError"));
    }

    #[test]
    fn zero_trace_lines_omits_traces_section() {
        let out = render(&sample(), 0, None);
        assert!(!out.contains("traces"));
    }

    #[test]
    fn trim_trace_drops_tilde_caret_marker_lines_and_traceback_header() {
        let raw = "Traceback (most recent call last):\n  File \"/p/t.py\", line 2, in f\n    g(x)\n    ~^^^\nValueError: bad\n";
        assert_eq!(
            trim_trace(raw),
            vec!["File \"/p/t.py\", line 2, in f", "g(x)", "ValueError: bad"]
        );
    }

    #[test]
    fn trim_trace_drops_caret_marker_lines() {
        let raw = "tests/test_dedup.py:3: in <module>\n    from sift.dedup import cluster_items\n^^^^^^^^^^^^^^^^^^^^^^^^^\nE   ModuleNotFoundError: No module named 'sift'";
        let t = trim_trace(raw).join("\n");
        assert!(t.contains("from sift.dedup"));
        assert!(!t.contains("^^^"), "got: {t}");
        assert!(t.contains("ModuleNotFoundError"));
    }

    #[test]
    fn trim_trace_drops_short_style_framework_frames() {
        let raw = "../../.local/share/uv/python/cpython-3.11/lib/python3.11/importlib/__init__.py:126: in import_module\n    return _bootstrap._gcd_import(name)\ntests/test_dedup.py:3: in <module>\n    from sift.dedup import cluster_items\nE   ModuleNotFoundError: No module named 'sift'";
        let t = trim_trace(raw).join("\n");
        assert!(!t.contains("importlib"), "got: {t}");
        assert!(!t.contains("_gcd_import"), "got: {t}");
        assert!(t.contains("tests/test_dedup.py:3"));
        assert!(t.contains("ModuleNotFoundError"));
    }

    #[test]
    fn trim_trace_drops_js_noise_frames() {
        let raw = "Error: expect(received).toBe(expected)\n    at Object.<anonymous> (/proj/src/auth.test.js:43:29)\n    at processTicksAndRejections (node:internal/process/task_queues/95:5)";
        let t = trim_trace(raw).join("\n");
        assert!(t.contains("auth.test.js"));
        assert!(!t.contains("node:internal"));
    }

    #[test]
    fn fast_note_renders_after_runner() {
        let out = render(&sample(), 20, Some("-n auto"));
        let runner_idx = out.find("runner: pytest").unwrap();
        // TOON quotes strings starting with '-' to avoid ambiguity, so the
        // value "-n auto" is rendered as: fast: "-n auto"
        let fast_idx = out.find("fast: \"-n auto\"").expect("fast line present");
        let summary_idx = out.find("summary:").unwrap();
        assert!(
            runner_idx < fast_idx && fast_idx < summary_idx,
            "got:\n{out}"
        );
    }

    #[test]
    fn no_fast_note_no_fast_line() {
        let out = render(&sample(), 20, None);
        assert!(!out.contains("fast:"), "got:\n{out}");
    }
}
