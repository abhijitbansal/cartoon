# cartoon Fast Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in `--fast` flag that injects `-n auto` (pytest-xdist) into pytest runs, discloses the injection in the TOON output, and falls back to a single serial retry when xdist is missing.

**Architecture:** The `Adapter` trait gains a `fast_args()` hook (default empty; pytest returns `["-n","auto"]`). The CLI gains a global `--fast` flag carried through `Mode::Wrap` into `app::run_wrap` / `run_with_adapter`, where fast args are appended AFTER `prepare()`'s injection. A bounded fallback detects pytest's exit-4 "unrecognized arguments" signature mentioning an injected arg and respawns once without fast args. The report renderer gains an optional `fast_note` that emits a `fast: -n auto` line right after `runner:`.

**Tech Stack:** Rust 2021, clap derive, existing adapter registry, assert_cmd/predicates for E2E.

**Spec:** `docs/superpowers/specs/2026-06-10-cartoon-fast-mode-design.md` — read it before starting.

**Baseline:** 107 tests green at commit 0566d51. `cargo` lives at `~/.cargo/bin/cargo` (export `PATH="$HOME/.cargo/bin:$PATH"` first in every shell).

---

### Task 1: `fast_args()` adapter hook

**Files:**
- Modify: `src/adapters/mod.rs` (trait, ~line 31)
- Modify: `src/adapters/pytest.rs` (impl, ~line 8)
- Tests: inline `#[cfg(test)]` in `src/adapters/mod.rs`

- [ ] **Step 1: Write the failing tests** — append inside the existing `mod tests` in `src/adapters/mod.rs`:

```rust
#[test]
fn pytest_fast_args_inject_xdist() {
    let pytest = registry()
        .into_iter()
        .find(|a| a.name() == "pytest")
        .unwrap();
    assert_eq!(pytest.fast_args(), vec!["-n".to_string(), "auto".to_string()]);
}

#[test]
fn other_adapters_have_no_fast_args() {
    for a in registry() {
        if a.name() != "pytest" {
            assert!(a.fast_args().is_empty(), "{} should be a no-op", a.name());
        }
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p cartoon --lib adapters::tests -- fast_args`
Expected: compile FAIL — `no method named fast_args`

- [ ] **Step 3: Implement** — in `src/adapters/mod.rs`, add to `trait Adapter` (after the `parse` method):

```rust
    /// Extra args that accelerate this runner, appended after prepare()'s
    /// injection when --fast is active. Default: none (silent no-op).
    fn fast_args(&self) -> Vec<String> {
        Vec::new()
    }
```

In `src/adapters/pytest.rs`, add to `impl Adapter for Pytest`:

```rust
    fn fast_args(&self) -> Vec<String> {
        vec!["-n".into(), "auto".into()]
    }
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p cartoon --lib adapters`
Expected: all pass (existing + 2 new)

- [ ] **Step 5: Commit**

```bash
git add src/adapters/mod.rs src/adapters/pytest.rs
git commit -m "feat: add fast_args() adapter hook, pytest injects -n auto"
```

---

### Task 2: `--fast` CLI flag through Mode::Wrap

**Files:**
- Modify: `src/cli.rs` (Cli struct ~line 11, Mode enum ~line 32, parse_mode ~line 75, existing tests that pattern-match `Mode::Wrap` with struct literals: `wrap_mode_passes_args_verbatim` ~line 129, `tag_flags_collect_into_wrap_mode` ~line 187)
- Modify: `src/main.rs` (Wrap arm destructure + run_wrap call)
- NOTE: `src/app.rs` gains the `fast` parameter in THIS task (signature only, value unused until Task 3) so the crate compiles at every commit.

- [ ] **Step 1: Write the failing tests** — append inside `mod tests` in `src/cli.rs`:

```rust
#[test]
fn fast_flag_before_command() {
    let m = mode(&["cartoon", "--fast", "pytest", "-q"]);
    assert!(matches!(m, Mode::Wrap { fast: true, .. }));
}

#[test]
fn fast_composes_with_tag_and_heuristic() {
    let m = mode(&["cartoon", "--fast", "--tag", "ci", "--heuristic", "make"]);
    assert!(matches!(
        m,
        Mode::Wrap {
            fast: true,
            heuristic: true,
            ..
        }
    ));
}

#[test]
fn fast_defaults_off() {
    let m = mode(&["cartoon", "pytest"]);
    assert!(matches!(m, Mode::Wrap { fast: false, .. }));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p cartoon --lib cli`
Expected: compile FAIL — `Mode::Wrap` has no field `fast`

- [ ] **Step 3: Implement.** In `src/cli.rs`:

Add to `struct Cli` after the `raw` field:

```rust
    /// Opt-in acceleration: inject parallelization args for runners that
    /// support it (pytest: -n auto via pytest-xdist). Disclosed in output.
    #[arg(long)]
    pub fast: bool,
```

Add `fast: bool` to `Mode::Wrap`:

```rust
    Wrap {
        argv: Vec<String>,
        heuristic: bool,
        raw: bool,
        tags: Vec<String>,
        fast: bool,
    },
```

In `parse_mode`, the wrap arm becomes:

```rust
        _ => Ok(Mode::Wrap {
            argv: cli.command,
            heuristic: cli.heuristic,
            raw: cli.raw,
            tags: cli.tags,
            fast: cli.fast,
        }),
```

Fix the two struct-literal equality tests by adding `fast: false` to their expected values:

```rust
            Mode::Wrap {
                argv: vec!["pytest".into(), "-q".into(), "--maxfail=1".into()],
                heuristic: false,
                raw: false,
                tags: vec![],
                fast: false
            }
```

```rust
            Mode::Wrap {
                argv: vec!["pytest".into()],
                heuristic: false,
                raw: false,
                tags: vec!["api".into(), "ci".into()],
                fast: false
            }
```

In `src/app.rs`, extend `run_wrap`'s signature (insert `fast: bool` before `cfg`) and thread it to `run_with_adapter`, which accepts and ignores it until Task 3:

```rust
pub fn run_wrap(
    argv: &[String],
    heuristic_on: bool,
    raw: bool,
    tags: &[String],
    fast: bool,
    cfg: &Config,
) -> Result<i32> {
    if !raw {
        if let Some(adapter) = adapters::find_adapter(argv) {
            return run_with_adapter(adapter.as_ref(), argv, tags, fast, cfg);
        }
    }
```

```rust
fn run_with_adapter(
    adapter: &dyn adapters::Adapter,
    argv: &[String],
    tags: &[String],
    _fast: bool,
    cfg: &Config,
) -> Result<i32> {
```

In `src/main.rs`, the Wrap arm becomes:

```rust
        Ok(cartoon::cli::Mode::Wrap {
            argv,
            heuristic,
            raw,
            tags,
            fast,
        }) => {
            let cfg = cartoon::config::load();
            let heuristic_on = heuristic || cfg.heuristic;
            cartoon::app::run_wrap(&argv, heuristic_on, raw, &tags, fast, &cfg).unwrap_or_else(
                |e| {
                    eprintln!("cartoon: {e}");
                    2
                },
            )
        }
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p cartoon`
Expected: ALL tests pass (existing + 3 new)

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs src/main.rs src/app.rs
git commit -m "feat: add --fast flag, threaded through Mode::Wrap into the pipeline"
```

---

### Task 3: fast injection, disclosure line, xdist-missing fallback

**Files:**
- Modify: `src/adapters/report.rs` (`render` ~line 24 + its test callers)
- Modify: `src/app.rs` (`run_with_adapter`)
- Tests: inline in `src/adapters/report.rs`

- [ ] **Step 1: Write the failing render tests** — append inside `mod tests` in `src/adapters/report.rs`:

```rust
#[test]
fn fast_note_renders_after_runner() {
    let out = render(&sample(), 20, Some("-n auto"));
    let runner_idx = out.find("runner: pytest").unwrap();
    let fast_idx = out.find("fast: -n auto").expect("fast line present");
    let summary_idx = out.find("summary:").unwrap();
    assert!(runner_idx < fast_idx && fast_idx < summary_idx, "got:\n{out}");
}

#[test]
fn no_fast_note_no_fast_line() {
    let out = render(&sample(), 20, None);
    assert!(!out.contains("fast:"), "got:\n{out}");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p cartoon --lib report`
Expected: compile FAIL — `render` takes 2 arguments but 3 were supplied

- [ ] **Step 3: Implement render change.** In `src/adapters/report.rs`, change `render`'s signature and insert the `fast` key between `runner` and `summary` (the crate enables serde_json `preserve_order`, so map order is emission order):

```rust
/// Asymmetric rendering: passes cost one summary block; failures keep
/// id/loc/msg rows plus trimmed traces. `fast_note` discloses injected
/// acceleration args (e.g. "-n auto") right after the runner line.
pub fn render(report: &TestReport, trace_lines: usize, fast_note: Option<&str>) -> String {
    let mut root = Map::new();
    root.insert("runner".into(), json!(report.runner));
    if let Some(f) = fast_note {
        root.insert("fast".into(), json!(f));
    }
    root.insert(
        "summary".into(),
        // ... rest unchanged
```

Update every existing `render(` call in report.rs tests to pass `None` as the third argument: `render(&sample(), 20, None)`, `render(&r, 20, None)`, `render(&r, 5, None)`, `render(&sample(), 0, None)`.

- [ ] **Step 4: Implement pipeline.** In `src/app.rs`, replace the head of `run_with_adapter` (everything before `let run = archive::record(...)`) with:

```rust
fn run_with_adapter(
    adapter: &dyn adapters::Adapter,
    argv: &[String],
    tags: &[String],
    fast: bool,
    cfg: &Config,
) -> Result<i32> {
    let prepared = adapter.prepare(argv.to_vec());
    let fast_args = if fast { adapter.fast_args() } else { Vec::new() };
    let mut argv_run = prepared.argv.clone();
    argv_run.extend(fast_args.iter().cloned());
    let mut fast_note = (!fast_args.is_empty()).then(|| fast_args.join(" "));
    let mut captured = match runner::run(&argv_run) {
        Ok(c) => c,
        Err(e) => return not_found_or_err(e, argv),
    };
    let mut code = runner::exit_code(&captured.status);
    // Bounded fallback: pytest exits 4 (usage error) when xdist is missing.
    // Nothing executed, so one serial retry is safe. Only on the exact
    // signature mentioning an arg WE injected — a user's own typo'd args
    // won't match and pass through normally.
    if fast_note.is_some()
        && code == 4
        && captured.stderr.contains("unrecognized arguments")
        && fast_args.iter().any(|a| captured.stderr.contains(a.as_str()))
    {
        eprintln!("cartoon: --fast unavailable (pytest-xdist not installed?); reran serially");
        fast_note = None;
        captured = match runner::run(&prepared.argv) {
            Ok(c) => c,
            Err(e) => return not_found_or_err(e, argv),
        };
        code = runner::exit_code(&captured.status);
    }
    let run = archive::record(argv, adapter.name(), &captured, code, tags, cfg);
```

And change the one `render` call in the `Ok(...)` arm to:

```rust
            let mut out = adapters::report::render(&report, cfg.trace_lines, fast_note.as_deref());
```

(Everything below — footer append, emit, stats — stays byte-for-byte as it is today.)

- [ ] **Step 5: Run full suite**

Run: `cargo test -p cartoon`
Expected: ALL pass

- [ ] **Step 6: clippy + fmt**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: clean

- [ ] **Step 7: Commit**

```bash
git add src/adapters/report.rs src/app.rs
git commit -m "feat: --fast injects adapter fast args with disclosure and serial fallback"
```

---

### Task 4: E2E tests (fallback fixture + real pytest-xdist), CI, README

**Files:**
- Create: `tests/e2e_fast.rs`
- Modify: `.github/workflows/ci.yml` (the step that installs pytest: add `pytest-xdist`)
- Modify: `README.md` (Use section + new Fast mode section)
- Modify: `docs/superpowers/specs/2026-06-10-cartoon-fast-mode-design.md` (one-cell correction)

- [ ] **Step 1: Write the E2E tests** — create `tests/e2e_fast.rs`:

```rust
use assert_cmd::Command;
use predicates::prelude::*;

/// Fake pytest that exits 4 on `-n` (xdist missing) and succeeds without it,
/// writing minimal junit xml to the --junit-xml path cartoon injected.
const FAKE_PYTEST: &str = r#"#!/bin/sh
junit=""
saw_n=0
for a in "$@"; do
  case "$a" in
    --junit-xml=*) junit="${a#--junit-xml=}" ;;
    -n) saw_n=1 ;;
  esac
done
if [ "$saw_n" = "1" ]; then
  echo "usage: pytest [options] [file_or_dir] [...]" >&2
  echo "pytest: error: unrecognized arguments: -n auto" >&2
  exit 4
fi
cat > "$junit" <<'XML'
<?xml version="1.0" encoding="utf-8"?>
<testsuites><testsuite name="pytest" tests="2" failures="0" skipped="0" time="0.01">
<testcase classname="t" name="test_a" file="tests/t.py" line="1" time="0.005"/>
<testcase classname="t" name="test_b" file="tests/t.py" line="5" time="0.005"/>
</testsuite></testsuites>
XML
echo "2 passed"
exit 0
"#;

fn setup_fake_pytest(dir: &std::path::Path) -> std::path::PathBuf {
    let bin = dir.join("pytest");
    std::fs::write(&bin, FAKE_PYTEST).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

#[test]
fn fast_falls_back_serially_when_xdist_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let fake = setup_fake_pytest(tmp.path());
    Command::cargo_bin("cartoon")
        .unwrap()
        .env("XDG_STATE_HOME", state.path())
        .args(["--fast", fake.to_str().unwrap()])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("runner: pytest"))
        .stdout(predicate::str::contains("total: 2"))
        .stdout(predicate::str::contains("fast:").not())
        .stderr(predicate::str::contains("--fast unavailable"));
}

#[test]
fn without_fast_flag_fake_pytest_never_sees_dash_n() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let fake = setup_fake_pytest(tmp.path());
    Command::cargo_bin("cartoon")
        .unwrap()
        .env("XDG_STATE_HOME", state.path())
        .args([fake.to_str().unwrap()])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("total: 2"))
        .stdout(predicate::str::contains("fast:").not())
        .stderr(predicate::str::contains("--fast unavailable").not());
}

fn xdist_available() -> bool {
    std::process::Command::new("python3")
        .args(["-c", "import xdist"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Real-tool test: requires pytest + pytest-xdist on PATH (CI installs both).
#[test]
fn real_pytest_fast_discloses_and_counts_match() {
    if !xdist_available() {
        eprintln!("skipping: pytest-xdist not importable");
        return;
    }
    let proj = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    std::fs::write(
        proj.path().join("test_demo.py"),
        "def test_ok():\n    assert True\n\ndef test_bad():\n    assert 1 == 2\n",
    )
    .unwrap();
    Command::cargo_bin("cartoon")
        .unwrap()
        .env("XDG_STATE_HOME", state.path())
        .current_dir(proj.path())
        .args(["--fast", "pytest"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("runner: pytest"))
        .stdout(predicate::str::contains("fast: -n auto"))
        .stdout(predicate::str::contains("total: 2"))
        .stdout(predicate::str::contains("failed: 1"));
}
```

- [ ] **Step 2: Run to verify**

Run: `cargo test --test e2e_fast`
Expected: PASS (Tasks 1-3 already landed the implementation; these tests are the acceptance gate. If any fail, the earlier tasks have a bug — fix there, not here. The real-tool test self-skips when xdist is absent locally.)

- [ ] **Step 3: CI — install xdist.** In `.github/workflows/ci.yml`, find the step that installs pytest (a `pip install` line) and add `pytest-xdist` to it, keeping everything else on that line exactly as found:

```yaml
      - run: pip install pytest pytest-xdist
```

- [ ] **Step 4: README.** In `README.md` "Use" block, add after the `cartoon logs --last --stdout` line:

```bash
cartoon --fast pytest          # opt-in: parallel via pytest-xdist (-n auto)
```

Add a new section after "Raw log archive":

```markdown
## Fast mode

`cartoon --fast pytest` appends `-n auto` so [pytest-xdist] runs the suite in
parallel. Strictly opt-in — parallel execution is NOT "same behavior" (test
order changes; shared-state tests can flake), so cartoon never enables it on
its own and always discloses it with a `fast: -n auto` line in the report.
Failures under `--fast`? Rerun without it before debugging. If pytest-xdist
isn't installed, cartoon retries serially once and notes it on stderr.
Other runners: no-op (jest is already parallel; unittest has no parallel
runner).

[pytest-xdist]: https://pypi.org/project/pytest-xdist/
```

- [ ] **Step 5: Spec corrections.** In `docs/superpowers/specs/2026-06-10-cartoon-fast-mode-design.md`, two fixes:
  1. Decisions table row "Stats / archive" claims fast-ness is "visible in archived argv (meta.json already records final argv)" — wrong: meta.json records the USER argv. Replace that cell's text with: `Unchanged. Disclosure happens via the TOON 'fast:' line only; meta.json keeps recording the user argv.`
  2. Error-handling table row "`--fast stats` / `--fast logs`" says "clap error (flag belongs to wrap mode)" — wrong: all wrap flags (`--heuristic`, `--raw`, `--tag`) are global and silently ignored on subcommands today; `--fast` matches that existing behavior. Replace that cell's text with: `silently ignored (consistent with --heuristic/--raw/--tag on subcommands)`. Also update the "CLI surface" paragraph's last sentence from "Not valid for `stats`/`logs`/`adapters` subcommands (same as other wrap flags)." to "Ignored by the `stats`/`logs`/`adapters` subcommands (same as the other wrap flags)."

- [ ] **Step 6: Full suite + lints**

Run: `cargo test -p cartoon && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all green, clean

- [ ] **Step 7: Commit**

```bash
git add tests/e2e_fast.rs .github/workflows/ci.yml README.md docs/superpowers/specs/2026-06-10-cartoon-fast-mode-design.md
git commit -m "test: fast-mode e2e (fallback fixture + real xdist), CI installs xdist, README fast mode"
```

---

## Verification (controller, after all tasks)

1. `cargo test -p cartoon` — everything green (expect 112+ tests).
2. Real xdist check: `pip install pytest-xdist` into `/tmp/cartoon-verify/venv`, then `cartoon --fast pytest` against `/tmp/cartoon-verify/mi` — `fast: -n auto` line present, counts match the earlier serial run (723 total / 1 failed), exit 1, faster wall clock.
3. Same venv minus xdist (`pip uninstall -y pytest-xdist`): fallback fires, one stderr note, no `fast:` line, identical counts.
4. Push to main; CI must be green on both runners.
