# cartoon Test Adapters Implementation Plan (2 of 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** pytest, unittest, and jest adapters: detect the runner, inject machine-readable output flags, parse into a structured report, render asymmetric TOON (passes = counts, failures = full actionable detail).

**Architecture:** `Adapter` trait (detect → prepare → parse) with a static registry. Adapters scrape structured sources, not human text: pytest via injected `--junit-xml=<tmp>`, jest via injected `--json`, unittest via regex on its stable stderr format. Parse failure always falls back to passthrough of the original output. Builds on plan 1 of 3 (`2026-06-09-cartoon-01-core.md`) — requires plan 1 complete.

**Tech Stack:** roxmltree (junit XML), regex (unittest), serde (jest JSON), tempfile (junit artifact).

**Execution context:** the `cartoon` repo created in plan 1.

---

### Task 1: Report model + asymmetric TOON rendering

**Files:**
- Create: `src/adapters/mod.rs`, `src/adapters/report.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing tests** (bottom of new `src/adapters/report.rs`)

```rust
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
    fn renders_summary_and_failures() {
        let out = render(&sample(), 20);
        assert!(out.contains("runner: pytest"), "got:\n{out}");
        assert!(out.contains("total: 48"));
        assert!(out.contains("failed: 2"));
        assert!(out.contains("failures[2]{id,loc,msg}:"));
        assert!(out.contains("tests/test_auth.py::test_expiry"));
    }

    #[test]
    fn empty_trace_gets_no_traces_entry() {
        let out = render(&sample(), 20);
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
        let out = render(&r, 20);
        assert!(!out.contains("failures"));
        assert!(!out.contains("traces"));
    }

    #[test]
    fn trace_capped_at_limit() {
        let mut r = sample();
        r.failures[0].trace = (0..50).map(|i| format!("line {i}")).collect();
        let out = render(&r, 5);
        assert!(out.contains("line 4"));
        assert!(!out.contains("line 5"));
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
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test report`
Expected: compile failure.

- [ ] **Step 3: Implement src/adapters/report.rs**

```rust
use serde_json::{json, Map, Value};

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

/// Asymmetric rendering: passes cost one summary block; failures keep
/// id/loc/msg rows plus trimmed traces.
pub fn render(report: &TestReport, trace_lines: usize) -> String {
    let mut root = Map::new();
    root.insert("runner".into(), json!(report.runner));
    root.insert(
        "summary".into(),
        json!({
            "total": report.total,
            "passed": report.passed,
            "failed": report.failed,
            "skipped": report.skipped,
            "duration_s": report.duration_s,
        }),
    );
    if !report.failures.is_empty() {
        root.insert(
            "failures".into(),
            Value::Array(
                report
                    .failures
                    .iter()
                    .map(|f| json!({"id": f.id, "loc": f.loc, "msg": f.msg}))
                    .collect(),
            ),
        );
        let traces: Map<String, Value> = report
            .failures
            .iter()
            .filter(|f| !f.trace.is_empty())
            .map(|f| {
                let capped: Vec<&String> = f.trace.iter().take(trace_lines).collect();
                (f.id.clone(), json!(capped))
            })
            .collect();
        if !traces.is_empty() {
            root.insert("traces".into(), Value::Object(traces));
        }
    }
    crate::toon::encode(&Value::Object(root))
}

const NOISE: &[&str] = &[
    "site-packages",
    "/_pytest/",
    "/unittest/case.py",
    "node_modules",
    "/jest-",
    "node:internal",
];

/// Keep user-code frames, drop framework internals, drop blank lines.
pub fn trim_trace(raw: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut skip_frame = false;
    for line in raw.lines() {
        let l = line.trim_end();
        let t = l.trim_start();
        let is_frame_header = t.starts_with("File \"") || t.starts_with("at ");
        if is_frame_header {
            skip_frame = NOISE.iter().any(|n| l.contains(n));
        }
        if !skip_frame && !t.is_empty() {
            lines.push(t.to_string());
        }
    }
    lines
}
```

`src/adapters/mod.rs` (trait/registry filled in Task 2):
```rust
pub mod report;
```

Add to `src/lib.rs`:
```rust
pub mod adapters;
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test report`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add src/adapters/ src/lib.rs
git commit -m "feat: test report model with asymmetric TOON rendering"
```

---

### Task 2: Adapter trait, registry, detection helpers

**Files:**
- Modify: `src/adapters/mod.rs`
- Create: `src/adapters/pytest.rs`, `src/adapters/unittest.rs`, `src/adapters/jest.rs` (stubs)

- [ ] **Step 1: Write failing tests** (bottom of `src/adapters/mod.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn basename_strips_paths() {
        assert_eq!(basename("/usr/bin/pytest"), "pytest");
        assert_eq!(basename("pytest"), "pytest");
    }

    #[test]
    fn python_module_detection() {
        assert!(is_python_module(&argv(&["python3", "-m", "pytest", "-q"]), "pytest"));
        assert!(is_python_module(&argv(&["python", "-m", "unittest"]), "unittest"));
        assert!(!is_python_module(&argv(&["python3", "script.py"]), "pytest"));
    }

    #[test]
    fn registry_has_three_adapters() {
        let names: Vec<&str> = registry().iter().map(|a| a.name()).collect();
        assert_eq!(names, vec!["pytest", "unittest", "jest"]);
    }

    #[test]
    fn find_adapter_matches_pytest() {
        assert_eq!(
            find_adapter(&argv(&["pytest", "-q"])).map(|a| a.name()),
            Some("pytest")
        );
        assert!(find_adapter(&argv(&["ls", "-la"])).is_none());
    }
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test adapters`
Expected: compile failure.

- [ ] **Step 3: Implement trait + registry in src/adapters/mod.rs**

```rust
pub mod jest;
pub mod pytest;
pub mod report;
pub mod unittest;

use crate::runner::Captured;
use anyhow::Result;
use std::path::PathBuf;

/// Invocation after `prepare`: possibly extended argv plus an artifact file
/// the adapter expects the child to write (e.g. junit xml).
pub struct Prepared {
    pub argv: Vec<String>,
    pub artifact: Option<tempfile::NamedTempFile>,
}

impl Prepared {
    pub fn artifact_path(&self) -> Option<PathBuf> {
        self.artifact.as_ref().map(|f| f.path().to_path_buf())
    }
}

/// What the agent should still see besides the TOON report.
/// `None` means the adapter consumed that stream (it WAS the report).
pub struct ParseOutcome {
    pub report: report::TestReport,
    pub passthrough_stdout: Option<String>,
    pub passthrough_stderr: Option<String>,
}

pub trait Adapter {
    fn name(&self) -> &'static str;
    /// Human description of what it matches, for `cartoon adapters`.
    fn matches(&self) -> &'static str;
    fn detect(&self, argv: &[String]) -> bool;
    /// Append machine-output flags. Must never remove or reorder user args.
    fn prepare(&self, argv: Vec<String>) -> Prepared;
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome>;
}

pub fn registry() -> Vec<Box<dyn Adapter>> {
    vec![
        Box::new(pytest::Pytest),
        Box::new(unittest::Unittest),
        Box::new(jest::Jest),
    ]
}

pub fn find_adapter(argv: &[String]) -> Option<Box<dyn Adapter>> {
    registry().into_iter().find(|a| a.detect(argv))
}

pub fn basename(arg: &str) -> &str {
    arg.rsplit(['/', '\\']).next().unwrap_or(arg)
}

pub fn is_python_module(argv: &[String], module: &str) -> bool {
    let first = argv.first().map(String::as_str).unwrap_or("");
    basename(first).starts_with("python")
        && argv.windows(2).any(|w| w[0] == "-m" && w[1] == module)
}
```

Stub `src/adapters/pytest.rs` (real parse in Task 3):

```rust
use super::{basename, is_python_module, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::Result;

pub struct Pytest;

impl Adapter for Pytest {
    fn name(&self) -> &'static str {
        "pytest"
    }
    fn matches(&self) -> &'static str {
        "pytest | python -m pytest"
    }
    fn detect(&self, argv: &[String]) -> bool {
        argv.first().map(|a| basename(a) == "pytest").unwrap_or(false)
            || is_python_module(argv, "pytest")
    }
    fn prepare(&self, argv: Vec<String>) -> Prepared {
        Prepared { argv, artifact: None }
    }
    fn parse(&self, _captured: &Captured, _prepared: &Prepared) -> Result<ParseOutcome> {
        anyhow::bail!("not implemented yet")
    }
}
```

Stub `src/adapters/unittest.rs` — identical shape: struct `Unittest`, name `"unittest"`, matches `"python -m unittest"`, detect:

```rust
    fn detect(&self, argv: &[String]) -> bool {
        super::is_python_module(argv, "unittest")
    }
```

Stub `src/adapters/jest.rs` — struct `Jest`, name `"jest"`, matches `"jest | npx jest"`, detect:

```rust
    fn detect(&self, argv: &[String]) -> bool {
        match argv {
            [first, ..] if super::basename(first) == "jest" => true,
            [first, second, ..]
                if matches!(super::basename(first), "npx" | "bunx") && second == "jest" =>
            {
                true
            }
            _ => false,
        }
    }
```

(Both stubs: `prepare` returns `Prepared { argv, artifact: None }`, `parse` bails `"not implemented yet"`.)

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test adapters`
Expected: trait/registry tests pass; report tests still green.

- [ ] **Step 5: Commit**

```bash
git add src/adapters/
git commit -m "feat: adapter trait, registry, and runner detection"
```

---

### Task 3: pytest adapter

**Files:**
- Modify: `src/adapters/pytest.rs`
- Create: `tests/fixtures/pytest/mixed.xml`, `tests/fixtures/pytest/all-pass.xml`

- [ ] **Step 1: Create junit fixtures**

`tests/fixtures/pytest/mixed.xml` (shape pytest emits with `--junit-xml`; `line` attr is 0-based):
```xml
<?xml version="1.0" encoding="utf-8"?>
<testsuites>
  <testsuite name="pytest" errors="0" failures="1" skipped="1" tests="3" time="0.123">
    <testcase classname="tests.test_auth" name="test_ok" file="tests/test_auth.py" line="9" time="0.001"/>
    <testcase classname="tests.test_auth" name="test_expiry" file="tests/test_auth.py" line="41" time="0.002">
      <failure message="AssertionError: assert exp &lt; now">def test_expiry():
    token = make_token()
&gt;   assert token.exp &lt; now()
E   AssertionError: assert exp &lt; now

tests/test_auth.py:42: AssertionError</failure>
    </testcase>
    <testcase classname="tests.test_auth" name="test_later" file="tests/test_auth.py" line="50" time="0.000">
      <skipped message="not ready"/>
    </testcase>
  </testsuite>
</testsuites>
```

`tests/fixtures/pytest/all-pass.xml`:
```xml
<?xml version="1.0" encoding="utf-8"?>
<testsuites>
  <testsuite name="pytest" errors="0" failures="0" skipped="0" tests="2" time="0.05">
    <testcase classname="tests.test_a" name="test_one" file="tests/test_a.py" line="1" time="0.001"/>
    <testcase classname="tests.test_a" name="test_two" file="tests/test_a.py" line="5" time="0.001"/>
  </testsuite>
</testsuites>
```

- [ ] **Step 2: Write failing tests** (bottom of `src/adapters/pytest.rs`)

```rust
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
```

- [ ] **Step 3: Run, verify failure**

Run: `cargo test pytest`
Expected: FAIL (`parse_junit` undefined, prepare stub has no artifact).

- [ ] **Step 4: Implement**

Replace `src/adapters/pytest.rs` implementation (keep the tests module):

```rust
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
        argv.first().map(|a| basename(a) == "pytest").unwrap_or(false)
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
            passthrough_stderr: (!captured.stderr.is_empty())
                .then(|| captured.stderr.clone()),
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
            failures.push(Failure { id, loc, msg, trace });
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
```

- [ ] **Step 5: Run, verify pass**

Run: `cargo test pytest`
Expected: 4 passed.

- [ ] **Step 6: Commit**

```bash
git add src/adapters/pytest.rs tests/fixtures/pytest/
git commit -m "feat: pytest adapter via injected junit-xml"
```

---

### Task 4: unittest adapter

**Files:**
- Modify: `src/adapters/unittest.rs`
- Create: `tests/fixtures/unittest/mixed.txt`, `tests/fixtures/unittest/all-pass.txt`

- [ ] **Step 1: Create text fixtures** (this is what `python -m unittest` writes to STDERR)

`tests/fixtures/unittest/mixed.txt`:
```
.F.s
======================================================================
FAIL: test_expiry (tests.test_auth.AuthTest.test_expiry)
----------------------------------------------------------------------
Traceback (most recent call last):
  File "/home/user/proj/tests/test_auth.py", line 42, in test_expiry
    self.assertLess(token.exp, now())
AssertionError: 1717000000 not less than 1716000000

----------------------------------------------------------------------
Ran 4 tests in 0.012s

FAILED (failures=1, skipped=1)
```

`tests/fixtures/unittest/all-pass.txt`:
```
....
----------------------------------------------------------------------
Ran 4 tests in 0.008s

OK
```

- [ ] **Step 2: Write failing tests** (bottom of `src/adapters/unittest.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn parse_fixture(name: &str) -> crate::adapters::report::TestReport {
        let path = format!(
            "{}/tests/fixtures/unittest/{}",
            env!("CARGO_MANIFEST_DIR"),
            name
        );
        parse_text(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn parses_mixed_results() {
        let r = parse_fixture("mixed.txt");
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (4, 2, 1, 1));
        assert_eq!(r.duration_s, 0.012);
        let f = &r.failures[0];
        assert_eq!(f.id, "tests.test_auth.AuthTest.test_expiry");
        assert_eq!(f.loc, "/home/user/proj/tests/test_auth.py:42");
        assert_eq!(f.msg, "AssertionError: 1717000000 not less than 1716000000");
    }

    #[test]
    fn parses_all_pass() {
        let r = parse_fixture("all-pass.txt");
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (4, 4, 0, 0));
    }

    #[test]
    fn unrecognized_text_is_error() {
        assert!(parse_text("random program output").is_err());
    }
}
```

- [ ] **Step 3: Run, verify failure**

Run: `cargo test unittest`

- [ ] **Step 4: Implement**

Replace `src/adapters/unittest.rs` (keep tests module):

```rust
use super::report::{trim_trace, Failure, TestReport};
use super::{is_python_module, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use regex::Regex;
use std::sync::OnceLock;

const SEPARATOR: &str =
    "======================================================================";

pub struct Unittest;

impl Adapter for Unittest {
    fn name(&self) -> &'static str {
        "unittest"
    }
    fn matches(&self) -> &'static str {
        "python -m unittest"
    }
    fn detect(&self, argv: &[String]) -> bool {
        is_python_module(argv, "unittest")
    }
    fn prepare(&self, argv: Vec<String>) -> Prepared {
        Prepared { argv, artifact: None } // unittest has no machine format
    }
    fn parse(&self, captured: &Captured, _prepared: &Prepared) -> Result<ParseOutcome> {
        let report = parse_text(&captured.stderr)?;
        Ok(ParseOutcome {
            report,
            // stdout holds user prints — the agent may need them.
            passthrough_stdout: (!captured.stdout.is_empty())
                .then(|| captured.stdout.clone()),
            // stderr WAS the report — consumed.
            passthrough_stderr: None,
        })
    }
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

pub fn parse_text(stderr: &str) -> Result<TestReport> {
    static RAN: OnceLock<Regex> = OnceLock::new();
    static HEADER: OnceLock<Regex> = OnceLock::new();
    static FILE_LINE: OnceLock<Regex> = OnceLock::new();
    static TAIL: OnceLock<Regex> = OnceLock::new();

    let ran = re(&RAN, r"Ran (\d+) tests? in ([0-9.]+)s");
    let caps = ran
        .captures(stderr)
        .context("no 'Ran N tests' line — not unittest output")?;
    let total: u64 = caps[1].parse()?;
    let duration_s: f64 = caps[2].parse()?;

    // Cut the tail off so failure-block parsing never sees "Ran N tests...".
    let body = &stderr[..caps.get(0).map(|m| m.start()).unwrap_or(stderr.len())];

    // tail counts: "FAILED (failures=1, errors=2, skipped=1)" or "OK (skipped=1)"
    let tail = re(&TAIL, r"(?m)^(OK|FAILED)\s*(?:\(([^)]*)\))?");
    let (mut n_fail, mut n_err, mut n_skip) = (0u64, 0u64, 0u64);
    if let Some(t) = tail.captures(stderr) {
        if let Some(details) = t.get(2) {
            for part in details.as_str().split(',') {
                let part = part.trim();
                if let Some(v) = part.strip_prefix("failures=") {
                    n_fail = v.parse().unwrap_or(0);
                } else if let Some(v) = part.strip_prefix("errors=") {
                    n_err = v.parse().unwrap_or(0);
                } else if let Some(v) = part.strip_prefix("skipped=") {
                    n_skip = v.parse().unwrap_or(0);
                }
            }
        }
    }
    let failed = n_fail + n_err;
    let skipped = n_skip;
    let passed = total.saturating_sub(failed + skipped);

    let header = re(&HEADER, r"(?m)^(FAIL|ERROR): (\S+) \(([^)]+)\)");
    let file_line = re(&FILE_LINE, r#"File "([^"]+)", line (\d+)"#);
    let mut failures = Vec::new();
    for block in body.split(SEPARATOR) {
        let Some(h) = header.captures(block) else { continue };
        let id = h[3].to_string();
        let loc = file_line
            .captures_iter(block)
            .filter(|c| !c[1].contains("site-packages") && !c[1].contains("/unittest/"))
            .last()
            .map(|c| format!("{}:{}", &c[1], &c[2]))
            .unwrap_or_default();
        let msg = block
            .lines()
            .rev()
            .find(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with('-')
            })
            .unwrap_or("")
            .trim()
            .to_string();
        let trace = trim_trace(block);
        failures.push(Failure { id, loc, msg, trace });
    }

    Ok(TestReport {
        runner: "unittest",
        total,
        passed,
        failed,
        skipped,
        duration_s,
        failures,
    })
}
```

- [ ] **Step 5: Run, verify pass**

Run: `cargo test unittest`
Expected: 3 passed.

- [ ] **Step 6: Commit**

```bash
git add src/adapters/unittest.rs tests/fixtures/unittest/
git commit -m "feat: unittest adapter parsing stderr text format"
```

---

### Task 5: jest adapter

**Files:**
- Modify: `src/adapters/jest.rs`
- Create: `tests/fixtures/jest/mixed.json`

- [ ] **Step 1: Create fixture** — `tests/fixtures/jest/mixed.json` (shape of `jest --json --testLocationInResults` stdout; synthetic data):

```json
{
  "numTotalTests": 3,
  "numPassedTests": 1,
  "numFailedTests": 1,
  "numPendingTests": 1,
  "numTodoTests": 0,
  "startTime": 1750000000000,
  "testResults": [
    {
      "name": "/home/user/proj/src/auth.test.js",
      "startTime": 1750000000100,
      "endTime": 1750000001300,
      "assertionResults": [
        {
          "fullName": "auth refreshes expired token",
          "status": "failed",
          "location": {"line": 42, "column": 3},
          "failureMessages": [
            "Error: expect(received).toBe(expected)\n\nExpected: true\nReceived: false\n    at Object.<anonymous> (/home/user/proj/src/auth.test.js:43:29)\n    at processTicksAndRejections (node:internal/process/task_queues:95:5)"
          ]
        },
        {
          "fullName": "auth issues token",
          "status": "passed",
          "location": {"line": 10, "column": 3},
          "failureMessages": []
        },
        {
          "fullName": "auth revokes token",
          "status": "pending",
          "location": {"line": 60, "column": 3},
          "failureMessages": []
        }
      ]
    }
  ]
}
```

- [ ] **Step 2: Write failing tests** (bottom of `src/adapters/jest.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn parse_fixture() -> crate::adapters::report::TestReport {
        let path = format!(
            "{}/tests/fixtures/jest/mixed.json",
            env!("CARGO_MANIFEST_DIR")
        );
        parse_json(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn prepare_appends_json_flags() {
        let p = Jest.prepare(vec!["jest".into(), "src/".into()]);
        assert_eq!(
            p.argv,
            vec!["jest", "src/", "--json", "--testLocationInResults"]
        );
    }

    #[test]
    fn parses_mixed_results() {
        let r = parse_fixture();
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (3, 1, 1, 1));
        assert!((r.duration_s - 1.3).abs() < 0.01, "got {}", r.duration_s);
        let f = &r.failures[0];
        assert_eq!(f.id, "auth refreshes expired token");
        assert_eq!(f.loc, "/home/user/proj/src/auth.test.js:42");
        assert_eq!(f.msg, "Error: expect(received).toBe(expected)");
        assert!(f.trace.iter().any(|l| l.contains("Expected: true")));
        // node internals dropped by trim_trace
        assert!(!f.trace.iter().any(|l| l.contains("task_queues")));
    }

    #[test]
    fn non_json_is_error() {
        assert!(parse_json("Tests: 1 failed").is_err());
    }
}
```

- [ ] **Step 3: Run, verify failure**

Run: `cargo test jest`

- [ ] **Step 4: Implement**

Replace `src/adapters/jest.rs` (keep tests module):

```rust
use super::report::{trim_trace, Failure, TestReport};
use super::{basename, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{Context, Result};
use serde::Deserialize;

pub struct Jest;

impl Adapter for Jest {
    fn name(&self) -> &'static str {
        "jest"
    }
    fn matches(&self) -> &'static str {
        "jest | npx jest"
    }
    fn detect(&self, argv: &[String]) -> bool {
        match argv {
            [first, ..] if basename(first) == "jest" => true,
            [first, second, ..]
                if matches!(basename(first), "npx" | "bunx") && second == "jest" =>
            {
                true
            }
            _ => false,
        }
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        argv.push("--json".into());
        argv.push("--testLocationInResults".into());
        Prepared { argv, artifact: None }
    }
    fn parse(&self, captured: &Captured, _prepared: &Prepared) -> Result<ParseOutcome> {
        let report = parse_json(&captured.stdout)?;
        Ok(ParseOutcome {
            report,
            // stdout was the JSON payload; stderr was jest's human report.
            // Both consumed. v1 limitation: console.log output inside tests
            // is not forwarded (it lives inside the jest report).
            passthrough_stdout: None,
            passthrough_stderr: None,
        })
    }
}

#[derive(Deserialize)]
struct JestRoot {
    #[serde(rename = "numTotalTests")]
    total: u64,
    #[serde(rename = "numPassedTests")]
    passed: u64,
    #[serde(rename = "numFailedTests")]
    failed: u64,
    #[serde(rename = "numPendingTests", default)]
    pending: u64,
    #[serde(rename = "numTodoTests", default)]
    todo: u64,
    #[serde(rename = "startTime")]
    start_time: f64,
    #[serde(rename = "testResults")]
    files: Vec<JestFile>,
}

#[derive(Deserialize)]
struct JestFile {
    name: String,
    #[serde(rename = "endTime", default)]
    end_time: f64,
    #[serde(rename = "assertionResults")]
    asserts: Vec<JestAssert>,
}

#[derive(Deserialize)]
struct JestAssert {
    #[serde(rename = "fullName")]
    full_name: String,
    status: String,
    #[serde(rename = "failureMessages", default)]
    failure_messages: Vec<String>,
    #[serde(default)]
    location: Option<JestLoc>,
}

#[derive(Deserialize)]
struct JestLoc {
    line: u64,
}

pub fn parse_json(stdout: &str) -> Result<TestReport> {
    let json_value =
        crate::fallback::detect_json(stdout).context("no JSON document in jest output")?;
    let root: JestRoot =
        serde_json::from_value(json_value).context("jest JSON shape mismatch")?;

    let end_max = root.files.iter().map(|f| f.end_time).fold(0.0_f64, f64::max);
    let duration_s = ((end_max - root.start_time) / 1000.0).max(0.0);

    let mut failures = Vec::new();
    for file in &root.files {
        for a in &file.asserts {
            if a.status != "failed" {
                continue;
            }
            let raw = a.failure_messages.join("\n");
            let clean = strip_ansi(&raw);
            let msg = clean.lines().next().unwrap_or("").to_string();
            let loc = match &a.location {
                Some(l) => format!("{}:{}", file.name, l.line),
                None => file.name.clone(),
            };
            failures.push(Failure {
                id: a.full_name.clone(),
                loc,
                msg,
                trace: trim_trace(&clean),
            });
        }
    }

    Ok(TestReport {
        runner: "jest",
        total: root.total,
        passed: root.passed,
        failed: root.failed,
        skipped: root.pending + root.todo,
        duration_s,
        failures,
    })
}

fn strip_ansi(s: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static ANSI: OnceLock<Regex> = OnceLock::new();
    ANSI.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap())
        .replace_all(s, "")
        .into_owned()
}
```

(`node:internal` is already in the NOISE list from Task 1, so the `task_queues` frame is dropped.)

- [ ] **Step 5: Run, verify pass**

Run: `cargo test jest`
Expected: 3 passed.

- [ ] **Step 6: Commit**

```bash
git add src/adapters/jest.rs tests/fixtures/jest/
git commit -m "feat: jest adapter via injected --json"
```

---

### Task 6: Wire adapters into the pipeline + E2E

**Files:**
- Modify: `src/app.rs`, `src/main.rs`
- Create: `tests/fixtures/e2e/pyproj/test_sample.py`, `tests/e2e_adapters.rs`

- [ ] **Step 1: Rewrite src/app.rs — adapters run BEFORE the child (prepare must mutate argv)**

```rust
use crate::adapters::{self, ParseOutcome};
use crate::{config::Config, fallback, heuristic, runner, stats, toon};
use anyhow::Result;

pub fn run_wrap(argv: &[String], heuristic_on: bool, raw: bool, cfg: &Config) -> Result<i32> {
    // Adapter path: detect first, because prepare() must extend argv.
    if !raw {
        if let Some(adapter) = adapters::find_adapter(argv) {
            return run_with_adapter(adapter.as_ref(), argv, cfg);
        }
    }
    let captured = match runner::run(argv) {
        Ok(c) => c,
        Err(e) => return not_found_or_err(e, argv),
    };
    let code = runner::exit_code(&captured.status);
    if raw {
        print!("{}", captured.stdout);
        eprint!("{}", captured.stderr);
        return Ok(code);
    }
    let (out, mode) = transform(&captured.stdout, heuristic_on);
    emit(&out, &captured.stderr);
    let original = format!("{}{}", captured.stdout, captured.stderr);
    let emitted = format!("{}{}", out, captured.stderr);
    stats::record_call(argv, mode, &original, &emitted, code, &cfg.tokenizer);
    Ok(code)
}

fn run_with_adapter(
    adapter: &dyn adapters::Adapter,
    argv: &[String],
    cfg: &Config,
) -> Result<i32> {
    let prepared = adapter.prepare(argv.to_vec());
    let captured = match runner::run(&prepared.argv) {
        Ok(c) => c,
        Err(e) => return not_found_or_err(e, argv),
    };
    let code = runner::exit_code(&captured.status);
    match adapter.parse(&captured, &prepared) {
        Ok(ParseOutcome { report, passthrough_stdout, passthrough_stderr }) => {
            let out = adapters::report::render(&report, cfg.trace_lines);
            let extra_out = passthrough_stdout.unwrap_or_default();
            let extra_err = passthrough_stderr.unwrap_or_default();
            emit(&out, "");
            if !extra_out.is_empty() {
                print!("{extra_out}");
            }
            eprint!("{extra_err}");
            let original = format!("{}{}", captured.stdout, captured.stderr);
            let emitted = format!("{}{}{}", out, extra_out, extra_err);
            stats::record_call(argv, adapter.name(), &original, &emitted, code, &cfg.tokenizer);
            Ok(code)
        }
        Err(e) => {
            // Safety rule: never lose information. Emit original output.
            eprintln!(
                "cartoon: {} adapter failed to parse ({e}); passing through",
                adapter.name()
            );
            print!("{}", captured.stdout);
            eprint!("{}", captured.stderr);
            Ok(code)
        }
    }
}

fn not_found_or_err(e: anyhow::Error, argv: &[String]) -> Result<i32> {
    let not_found = e
        .downcast_ref::<std::io::Error>()
        .map(|io| io.kind() == std::io::ErrorKind::NotFound)
        .unwrap_or(false);
    if not_found {
        eprintln!("cartoon: command not found: {}", argv[0]);
        return Ok(127);
    }
    Err(e)
}

fn emit(out: &str, err: &str) {
    print!("{out}");
    if !out.is_empty() && !out.ends_with('\n') {
        println!();
    }
    eprint!("{err}");
}

pub fn transform(stdout: &str, heuristic_on: bool) -> (String, &'static str) {
    if let Some(json) = fallback::detect_json(stdout) {
        return (toon::encode(&json), "json");
    }
    if heuristic_on {
        return (heuristic::compress(stdout), "heuristic");
    }
    (stdout.to_string(), "passthrough")
}
```

- [ ] **Step 2: Update `cartoon adapters` in src/main.rs**

Replace the `Mode::Adapters` arm:

```rust
        Ok(cartoon::cli::Mode::Adapters) => {
            for a in cartoon::adapters::registry() {
                println!("{}: {}", a.name(), a.matches());
            }
            0
        }
```

- [ ] **Step 3: Create E2E pytest fixture project**

`tests/fixtures/e2e/pyproj/test_sample.py`:
```python
def test_pass():
    assert 1 + 1 == 2


def test_fail():
    assert 1 + 1 == 3
```

- [ ] **Step 4: Write E2E tests — tests/e2e_adapters.rs**

```rust
use assert_cmd::Command;
use predicates::str::contains;

fn cartoon() -> Command {
    Command::cargo_bin("cartoon").unwrap()
}

fn have(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn e2e_pytest_failing_suite() {
    if !have("pytest") {
        eprintln!("SKIP: pytest not installed");
        return;
    }
    let proj = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/e2e/pyproj");
    let tmp = tempfile::tempdir().unwrap();
    let assert = cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["pytest", proj])
        .assert()
        .code(1); // pytest exit 1 = test failures, mirrored
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("runner: pytest"), "got:\n{out}");
    assert!(out.contains("failed: 1"), "got:\n{out}");
    assert!(out.contains("test_fail"), "got:\n{out}");
}

#[test]
fn e2e_adapters_lists_three() {
    cartoon()
        .args(["adapters"])
        .assert()
        .success()
        .stdout(contains("pytest"))
        .stdout(contains("unittest"))
        .stdout(contains("jest"));
}
```

- [ ] **Step 5: Run everything**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: green (pytest E2E self-skips locally if pytest absent; CI installs it in plan 3 of 3).

- [ ] **Step 6: Manual smoke test**

```bash
cargo run -- pytest tests/fixtures/e2e/pyproj ; echo "exit: $?"
```
Expected: TOON report with `runner: pytest`, `failed: 1`, trace for `test_fail`; `exit: 1`.

- [ ] **Step 7: Commit**

```bash
git add src/ tests/
git commit -m "feat: wire test adapters into wrap pipeline with passthrough safety"
```

---

**Plan 2 exit criteria:** `cartoon pytest <failing suite>` emits asymmetric TOON (counts + failure detail), mirrors exit code, falls back to raw output on parse failure; `cartoon adapters` lists pytest/unittest/jest; an agent can fix a failing test from the TOON output alone.
