# Cartoon Review Fixes (All Phases) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land every phase of the 2026-09-05 repo review on one branch
(`feat/wrap-scripts-project-config`), one commit per phase, ending with a
manual test checklist and a single PR.

**Architecture:** Fix routing first (shell-string tokenizing, project config
at run time), then tighten the hook's auto-approve surface, then the
ledger/archive/ladder correctness items, then add the cross-cutting
mechanisms (doctor, content sniffing, JUnit harvester, token budget, pipe
filter), then the adapter wave, then release hygiene and the `hook.rs`
split. Every task is TDD: failing test, minimal code, green, next.

**Tech Stack:** Rust 2021 (clap, serde, serde_json, regex, roxmltree,
toml, tiktoken-rs, tempfile), new dep `shell-words`; tests with
assert_cmd/predicates; Node scripts for version sync.

**Spec:** `docs/superpowers/specs/2026-09-05-cartoon-repo-review-improvement-plan.md`

## Global Constraints

- CI on GitHub stays **disabled**. Reason (maintainer decision, 2026-09-05):
  GitHub Actions runner minutes cost money the maintainer does not want to
  pay. The local gate replaces it and must be green before every commit:
  `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`. Run them bare (the cartoon hook wraps them).
- Exit codes always mirror the child. A panic in cartoon must never change
  the child's exit code path.
- Output never grows: any new emission path goes through the net-savings
  guard or is an explicit, disclosed opt-in (`--max-tokens`).
- The hook's `permissionDecision: "allow"` bypasses the user's prompt; every
  allowlist change errs toward `None` (leave the command alone).
- Never remove or reorder user-supplied args in `prepare()`; only append or
  insert our flag before a `--` separator.
- One commit per phase. Commit messages end with the session trailers.
- Files stay under 800 lines; functions under 50 where practical.

---

## Phase 0 — Unblock

### Task 0.1: Bring the spec and this plan onto the branch; record the CI decision

**Files:**
- Cherry-pick: commit `98e65af` (spec) from `docs/repo-review-plan-2026-09`
- Modify: `docs/superpowers/specs/2026-09-05-cartoon-repo-review-improvement-plan.md` (Project and process + Phase 0 bullets)
- Modify: `.github/workflows/ci.yml:1` (header comment)
- Modify: `CONTRIBUTING.md` (Dev setup)
- Modify: `docs/RELEASING.md` (Release checklist step 1)

- [ ] **Step 1: Cherry-pick the spec commit**

```bash
git cherry-pick 98e65af
```

- [ ] **Step 2: Replace the "Re-enable it" sentences in the spec**

In section 2 "Project and process", replace the CI bullet with:

```markdown
- **CI is disabled, deliberately.** The `ci` workflow shows
  `disabled_manually` on GitHub since 2026-06-15. Decision (2026-09-05):
  it stays disabled because GitHub Actions runner minutes cost money the
  maintainer does not want to pay. The local gate replaces it:
  `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test` before every commit, and the PR checklist records that
  they ran.
```

In Phase 0, replace step 1 with "Record the CI-disabled decision in
`ci.yml`, `CONTRIBUTING.md`, `RELEASING.md`."

- [ ] **Step 3: Add the header comment to ci.yml**

```yaml
# DISABLED ON GITHUB (manually, since 2026-06-15; decision recorded
# 2026-09-05): Actions runner minutes cost money the maintainer does not
# want to pay. This file is kept as the reference recipe. The local gate is
# the real one — run before every commit:
#   cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
name: ci
```

- [ ] **Step 4: CONTRIBUTING.md — add under "Dev setup"**

```markdown
GitHub CI is intentionally disabled (runner minutes cost money). The three
commands above ARE the gate: run them before every commit and say so in
the PR. `cargo test` never touches your real `~/.local/state/cartoon`
archive — every e2e test sets `XDG_STATE_HOME` to a temp dir.
```

- [ ] **Step 5: RELEASING.md checklist step 1**

Replace "CI green on main; `cargo test` locally." with
"Local gate green on main: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (GitHub CI is disabled by decision; see CONTRIBUTING.md)."

- [ ] **Step 6: Commit (docs commit, precedes Phase 0's code commit)**

```bash
git add docs/superpowers CONTRIBUTING.md docs/RELEASING.md .github/workflows/ci.yml
git commit -m "docs: review spec + all-phases plan; record CI-disabled decision (runner minutes)"
```

### Task 0.2: Apply the project `.cartoon.toml` at run time

**Files:**
- Modify: `src/config.rs` (add `load_for_cwd`)
- Modify: `src/main.rs:14`, `src/main.rs:77`
- Test: `tests/e2e_wrap.rs`

**Interfaces:**
- Produces: `pub fn config::load_for_cwd() -> Config` — `load_merged(cwd)` when the cwd is readable, else `load()`.

- [ ] **Step 1: Write the failing e2e test**

```rust
#[test]
fn project_cartoon_toml_pin_changes_the_tier_at_run_time() {
    // A project pin for argv0 `sh` set to aggressive must filter INFO lines;
    // the default safe tier keeps them. Proves main.rs reads the merged config.
    let state = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(
        repo.path().join(".cartoon.toml"),
        "[command.sh]\nlevel = \"aggressive\"\n",
    )
    .unwrap();
    let mut log = String::new();
    for i in 0..120 {
        log.push_str(&format!("2026-06-11 INFO item {i}\\n"));
    }
    log.push_str("2026-06-11 ERROR boom\\n");
    let out = cartoon()
        .current_dir(repo.path())
        .env("XDG_STATE_HOME", state.path())
        .env("XDG_CONFIG_HOME", state.path())
        .args(["sh", "-c", &format!("printf '{log}'")])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("INFO item 3"), "aggressive pin applied: {stdout}");
    assert!(stdout.contains("ERROR boom"));
}
```

- [ ] **Step 2: Run it — expect FAIL (INFO lines still present)**

```bash
cargo test --test e2e_wrap project_cartoon_toml_pin
```

- [ ] **Step 3: Implement `load_for_cwd` and use it**

`src/config.rs`:
```rust
/// The config a wrapped run should use: global + the project-local
/// `.cartoon.toml` for the current directory. Falls back to global-only
/// when the cwd is unreadable (fail-open).
pub fn load_for_cwd() -> Config {
    match std::env::current_dir() {
        Ok(cwd) => load_merged(&cwd),
        Err(_) => load(),
    }
}
```

`src/main.rs`: both `let cfg = cartoon::config::load();` → `let cfg = cartoon::config::load_for_cwd();`

- [ ] **Step 4: Run the test — expect PASS**

### Task 0.3: Isolate every e2e test from the real archive

**Files:**
- Modify: `tests/e2e_ladder.rs`, `tests/e2e_ingest.rs`, `tests/xcodebuild_e2e.rs` (add `.env("XDG_STATE_HOME", tmp.path())` to every cartoon invocation)
- Create: `tests/isolation_lint.rs`

- [ ] **Step 1: Write the lint test (fails today on three files)**

```rust
//! Every integration test that runs the real cartoon binary must point
//! XDG_STATE_HOME at a temp dir, or `cargo test` archives fixture runs into
//! the developer's real ~/.local/state/cartoon and prunes genuine logs.
#[test]
fn every_e2e_test_isolates_xdg_state_home() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let runs_binary = src.contains("CARGO_BIN_EXE_cartoon") || src.contains("cargo_bin(\"cartoon\")");
        if runs_binary && !src.contains("XDG_STATE_HOME") {
            offenders.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    assert!(offenders.is_empty(), "e2e tests without XDG_STATE_HOME isolation: {offenders:?}");
}
```

- [ ] **Step 2: Run — expect FAIL listing e2e_ladder.rs, e2e_ingest.rs, xcodebuild_e2e.rs**

- [ ] **Step 3: Add isolation to each offender**

Pattern for `std::process::Command` files:
```rust
let state = tempfile::tempdir().unwrap();
let out = Command::new(cartoon_bin())
    .env("XDG_STATE_HOME", state.path())
    .args([...])
```
Apply to every `Command::new(cartoon_bin())` in the three files.

- [ ] **Step 4: Run the full gate; commit Phase 0**

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
git add -A && git commit -m "fix(config,tests): apply project .cartoon.toml pins at run time; isolate e2e tests from the real archive"
```

---

## Phase 1 — Hook security

### Task 1.1: Env-prefix allowlist

**Files:**
- Modify: `src/hook.rs:329-335` (the `while words.peek()...` skip loop)
- Test: `src/hook.rs` tests module

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn env_prefix_only_benign_names_are_auto_wrapped() {
    assert!(wrap_command("CI=1 pytest -q").is_some());
    assert!(wrap_command("RUST_BACKTRACE=1 cargo test").is_some());
    // Anything that changes what gets executed falls through to the prompt.
    assert!(wrap_command("PATH=/tmp/evil:$PATH pytest").is_none());
    assert!(wrap_command("LD_PRELOAD=/tmp/x.so cargo test").is_none());
    assert!(wrap_command("RUSTC_WRAPPER=/tmp/w cargo build").is_none());
    assert!(wrap_command("NODE_OPTIONS=--require=/tmp/x.js jest").is_none());
    assert!(wrap_command("DEVELOPER_DIR=/tmp xcodebuild test -scheme A").is_none());
}
```

- [ ] **Step 2: Implement**

```rust
/// `NAME=value` prefixes the hook may skip past. Anything else that looks
/// like an assignment makes the whole command ineligible: PATH, LD_PRELOAD,
/// RUSTC_WRAPPER, NODE_OPTIONS, DEVELOPER_DIR, ... change what executes,
/// and a rewrite auto-approves the call.
pub const SAFE_ENV_PREFIXES: &[&str] = &[
    "CI", "NO_COLOR", "FORCE_COLOR", "TERM", "LANG", "LC_ALL", "TZ", "DEBUG",
    "RUST_LOG", "RUST_BACKTRACE", "CARGO_TERM_COLOR", "NODE_ENV",
    "PYTHONDONTWRITEBYTECODE", "PYTHONUNBUFFERED", "PYTEST_ADDOPTS",
];

fn is_env_assignment(word: &str) -> Option<&str> {
    let (name, _) = word.split_once('=')?;
    let ok = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit());
    ok.then_some(name)
}
```
Replace the skip loop with:
```rust
while let Some(name) = words.peek().and_then(|w| is_env_assignment(w)) {
    if !SAFE_ENV_PREFIXES.contains(&name) {
        return None;
    }
    words.next();
}
```

- [ ] **Step 3: Run hook tests — PASS**

### Task 1.2: ruff subcommand gate, mutating-flag scan, make/pre-commit decision

**Files:**
- Modify: `src/hook.rs:30-59` (ALWAYS, SUBCOMMAND), add `MUTATING_TOKENS`, module doc
- Test: `src/hook.rs` tests

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn mutating_lint_invocations_are_never_auto_approved() {
    assert!(wrap_command("ruff check .").is_some());
    assert!(wrap_command("ruff format .").is_none());
    assert!(wrap_command("ruff check --fix .").is_none());
    assert!(wrap_command("ruff check --fix-only .").is_none());
    assert!(wrap_command("uvx ruff format .").is_none());
    assert!(wrap_command("eslint src/").is_some());
    assert!(wrap_command("eslint --fix src/").is_none());
    assert!(wrap_command("npx eslint --fix src/").is_none());
    assert!(wrap_command("eslint -c /tmp/evil.js src/").is_none());
    assert!(wrap_command("eslint --rulesdir /tmp/r src/").is_none());
}

#[test]
fn make_and_pre_commit_stay_allowlisted_by_decision() {
    // Deliberate: both are the canonical dev-loop entry points and the agent
    // already holds write access to the repo. Users who disagree install
    // with --deny. Documented in the module doc.
    assert!(wrap_command("make -j4").is_some());
    assert!(wrap_command("pre-commit run --all-files").is_some());
}
```

- [ ] **Step 2: Implement**

Remove `"ruff"` from `ALWAYS`; add `("ruff", &["check"])` to `SUBCOMMAND`.

```rust
/// Tokens (flags or subcommands) that turn an otherwise read-mostly tool
/// into one that rewrites files or loads code from an arbitrary path. Any
/// segment containing one is left alone entirely (`None`): no rewrite, no
/// deny — the user's normal permission flow decides.
pub const MUTATING_TOKENS: &[(&str, &[&str])] = &[
    ("ruff", &["--fix", "--fix-only", "--unsafe-fixes", "--add-noqa", "format"]),
    ("eslint", &["--fix", "--fix-dry-run", "--fix-type", "-c", "--config", "--rulesdir", "--resolve-plugins-relative-to"]),
    ("swiftlint", &["--fix", "autocorrect"]),
];

fn has_mutating_token(base: &str, rest: &[&str]) -> bool {
    MUTATING_TOKENS
        .iter()
        .find(|(tool, _)| *tool == base)
        .is_some_and(|(_, toks)| rest.iter().any(|w| toks.iter().any(|t| w == t || w.starts_with(&format!("{t}=")))))
}
```
In `wrap_command_with_policy`, after computing `base` collect the remaining
words once (`let rest: Vec<&str> = words.clone().collect();`) and add
`if has_mutating_token(base, &rest) { return None; }` before the runner/uv/
is_noisy checks. For the `RUNNERS` branch (`npx eslint --fix`) and the uv
branch, apply the same check to the inner tool: in `is_noisy`'s RUNNERS
arm return false when `has_mutating_token(inner, rest_after_inner)`. Simplest:
resolve `inner_base` and `inner_rest` in `wrap_command_with_policy` for
`RUNNERS`/`uv` prefixes and call `has_mutating_token(inner_base, inner_rest)`.

Module doc addition (top of hook.rs):
```rust
//! Allowlist decisions (2026-09-05): `make` and `pre-commit` execute
//! project-defined recipes yet stay allowlisted — they are the canonical
//! dev-loop entry points and the agent already has write access to the repo.
//! Tools with a mutating *mode* (`ruff format`, `--fix`, `swiftlint
//! autocorrect`) are gated per token in `MUTATING_TOKENS`. Install with
//! `--deny` to turn every rewrite into a suggestion instead.
```

### Task 1.3: `npx`/`bunx`/`pnpx` only wrap JS tools

- [ ] **Step 1: Failing test**

```rust
#[test]
fn runner_prefix_only_wraps_js_tools() {
    assert!(wrap_command("npx jest").is_some());
    assert!(wrap_command("npx vitest run").is_some());
    assert!(wrap_command("npx tsc --noEmit").is_some());
    assert!(wrap_command("npx pytest").is_none());
    assert!(wrap_command("npx make").is_none());
    assert!(wrap_command("bunx pre-commit run").is_none());
}
```

- [ ] **Step 2: Implement**

```rust
/// Tools a JS package runner (`npx`, `bunx`, `pnpx`) may launch and still be
/// auto-wrapped. Deliberately not the whole ALWAYS list: `npx pytest` is not
/// a thing a vetted dev loop does.
pub const RUNNER_TOOLS: &[&str] = &["jest", "vitest", "tsc", "eslint"];
```
In `is_noisy`, RUNNERS arm: `.is_some_and(|n| RUNNER_TOOLS.contains(&n))`.

### Task 1.4: `hook install --deny` switches an existing install's mode

- [ ] **Step 1: Failing test** (pure helper test; extract `upsert_claude_entry(arr: &mut Vec<Value>, deny: bool) -> Upsert` with `enum Upsert { Added, Unchanged, ModeUpdated }`)

```rust
#[test]
fn install_switches_mode_when_entry_exists_with_other_mode() {
    let mut arr = vec![claude_entry(false)];
    assert_eq!(upsert_claude_entry(&mut arr, true), Upsert::ModeUpdated);
    assert!(arr[0]["hooks"][0]["command"].as_str().unwrap().contains("--deny-mode"));
    assert_eq!(upsert_claude_entry(&mut arr, true), Upsert::Unchanged);
    let mut empty = Vec::new();
    assert_eq!(upsert_claude_entry(&mut empty, false), Upsert::Added);
}
```

- [ ] **Step 2: Implement** — `install_claude` calls `upsert_claude_entry`; prints "cartoon hook mode updated (deny-with-suggestion)" / "(transparent rewrite)" on `ModeUpdated` and writes the file; `Unchanged` keeps today's message.

### Task 1.5: `wrap_scripts` matches interpreter-prefixed and path forms

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn wrap_scripts_matches_common_invocation_forms() {
    let scripts = ["./build.sh".to_string()];
    for cmd in ["./build.sh -d", "build.sh -d", "bash ./build.sh -d", "sh build.sh", "/Users/me/repo/build.sh --no-launch"] {
        let (_, force_deny) = wrap_command_with_policy(cmd, &scripts).unwrap_or_else(|| panic!("{cmd} should match"));
        assert!(force_deny, "{cmd} must be deny-only");
    }
    assert!(wrap_command_with_policy("./deploy.sh", &scripts).is_none());
}
```

- [ ] **Step 2: Implement**

```rust
fn script_key(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}
fn matches_wrap_script(first: &str, next: Option<&str>, wrap_scripts: &[String]) -> bool {
    let target = match script_key(first) {
        "sh" | "bash" | "zsh" => match next { Some(n) => n, None => return false },
        _ => first,
    };
    wrap_scripts.iter().any(|s| script_key(s) == script_key(target))
}
```
Replace `if wrap_scripts.iter().any(|s| s == first || s == base)` with
`if matches_wrap_script(first, rest.first().copied(), wrap_scripts)`.
The interpreter case must be checked before `STATE_BUILTINS` rejects `.`/`source` — `sh`/`bash` are not in that list, so the order is fine.

- [ ] **Step 3: Gate + commit Phase 1**

```bash
git commit -am "fix(hook): tighten auto-approve — env-prefix allowlist, ruff/eslint/swiftlint mutating tokens, JS-only npx scope; wrap_scripts matches interpreter and path forms; --deny switches installed mode"
```

---

## Phase 2 — Routing and ledger fidelity

### Task 2.1: Quote-aware `needs_shell` / `shell_argv`

**Files:**
- Modify: `Cargo.toml` (add `shell-words = "1"`)
- Modify: `src/cli.rs:144-182`
- Test: `src/cli.rs` tests

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn quoted_args_and_equals_do_not_force_a_shell() {
    assert_eq!(
        shell_argv("xcodebuild test -destination 'platform=iOS Simulator,name=iPhone 17' -scheme App"),
        vec!["xcodebuild", "test", "-destination", "platform=iOS Simulator,name=iPhone 17", "-scheme", "App"]
    );
    assert_eq!(
        shell_argv("swift build -Xswiftc -strict-concurrency=complete"),
        vec!["swift", "build", "-Xswiftc", "-strict-concurrency=complete"]
    );
    assert_eq!(shell_argv(r#"pytest -k "a and b" tests/"#), vec!["pytest", "-k", "a and b", "tests/"]);
}

#[test]
fn real_shell_syntax_still_forces_sh_c() {
    for s in ["pytest | tail -5", "FOO=1 pytest", "cargo test && echo ok", "ls *.py", "echo $HOME", "pytest > out.txt"] {
        assert_eq!(shell_argv(s)[..2], ["sh", "-c"], "{s}");
    }
    // Unbalanced quote: fail open to the shell rather than guess.
    assert_eq!(shell_argv("pytest -k 'oops")[..2], ["sh", "-c"]);
}
```

- [ ] **Step 2: Implement**

```rust
/// True shell syntax that needs `sh -c`: operators, substitution, globbing,
/// brace/tilde expansion, and a leading `NAME=value` env assignment. Quotes
/// and `=` inside an argument are NOT shell syntax — they are tokenized by
/// `shell_words` so adapters still see the real argv0.
fn needs_shell(s: &str) -> bool {
    let has_operator = s.chars().any(|c| matches!(c, '|' | '&' | ';' | '<' | '>' | '(' | ')' | '$' | '`' | '\n' | '*' | '?' | '[' | '{' | '~'));
    has_operator || leading_env_assignment(s)
}

fn leading_env_assignment(s: &str) -> bool {
    s.split_whitespace().next().is_some_and(|w| {
        w.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    })
}

pub fn shell_argv(s: &str) -> Vec<String> {
    let via_shell = || {
        let sh = if cfg!(windows) { "cmd" } else { "sh" };
        let flag = if cfg!(windows) { "/C" } else { "-c" };
        vec![sh.to_string(), flag.to_string(), s.to_string()]
    };
    if needs_shell(s) {
        return via_shell();
    }
    match shell_words::split(s) {
        Ok(argv) if !argv.is_empty() => argv,
        _ => via_shell(),
    }
}
```
Keep the existing `needs_shell` unit tests that assert operator strings go through the shell; delete any that asserted `=` or quotes force it.

### Task 2.2: Record the inner command for shell-string runs

**Files:**
- Modify: `src/cli.rs` (add `inner_command`), `src/stats.rs` (`StatRecord.inner_cmd`, `record_call`), `src/logs_cmd.rs:101` (`render_list` cmd column)
- Test: `src/cli.rs`, `src/stats.rs`, `src/logs_cmd.rs` tests

**Interfaces:**
- Produces: `pub fn cli::inner_command(argv: &[String]) -> Option<String>` — for `sh -c <string>` / `cmd /C <string>`, the first non-assignment word of the string; `None` otherwise.
- Produces: `StatRecord.inner_cmd: Option<String>` (`#[serde(default)]`).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn inner_command_reads_through_sh_c() {
    let argv = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(inner_command(&argv(&["sh", "-c", "xcodebuild test -scheme A | tail -3"])), Some("xcodebuild".into()));
    assert_eq!(inner_command(&argv(&["sh", "-c", "FOO=1 ./build.sh -d"])), Some("./build.sh".into()));
    assert_eq!(inner_command(&argv(&["pytest", "-q"])), None);
}
```
In `stats.rs`: a `record_call`-shaped unit test is not possible without touching XDG; test `build_record(argv, ...)` (extract the struct construction into `pub fn build_record(...) -> StatRecord`) and assert `inner_cmd == Some("xcodebuild")` for a `sh -c` argv.
In `logs_cmd.rs`: `render_list` on a `RunMeta` with argv `["sh","-c","xcodebuild test"]` shows `cmd: "sh -c xcodebuild"` — implement as `format!("{} -c {}", argv[0], inner)` when inner is present.

- [ ] **Step 2: Implement**

```rust
pub fn inner_command(argv: &[String]) -> Option<String> {
    let is_shell = matches!(crate::adapters::basename(argv.first()?), "sh" | "bash" | "zsh" | "cmd");
    let is_c = matches!(argv.get(1)?.as_str(), "-c" | "/C" | "/c");
    if !is_shell || !is_c {
        return None;
    }
    argv.get(2)?
        .split_whitespace()
        .find(|w| !w.contains('=') || w.starts_with('-') || w.starts_with('.') || w.starts_with('/'))
        .map(String::from)
}
```

### Task 2.3: `learn` explains shell-string waste instead of pinning `[command.sh]`

- [ ] **Step 1: Failing test** in `src/learn.rs`

```rust
#[test]
fn shell_string_runs_get_an_explanation_not_a_sh_pin() {
    let mut recs: Vec<StatRecord> = (0..4).map(|_| rec("sh", "passthrough", 40_000, 0)).collect();
    for r in &mut recs { r.inner_cmd = Some("xcodebuild".into()); }
    let out = render(&recs, None);
    assert!(!out.contains("[command.sh]"), "{out}");
    assert!(out.contains("shell_string") && out.contains("xcodebuild"), "{out}");
}
```

- [ ] **Step 2: Implement** — in the token-waster loop, when `a.cmd == "sh" || a.cmd == "cmd"`, group by `inner_cmd` and push
```rust
json!({
  "kind": "shell_string",
  "inner_cmd": inner,
  "calls": n,
  "avg_tokens_in": avg,
  "action": format!("`{inner}` ran through `sh -c` (a pipe or shell operator in the command string), so its adapter never fired. Run it without the pipe — cartoon already shrinks the output — or fix the operator."),
})
```
and emit no config line for it. Update the existing `rec()` test helper to set `inner_cmd: None`.

### Task 2.4: Atomic stats append; tolerant, counted reader

- [ ] **Step 1: Failing tests** in `src/stats.rs`

```rust
#[test]
fn reader_recovers_concatenated_records_and_counts_malformed_lines() {
    let a = serde_json::to_string(&sample_record()).unwrap();
    let text = format!("{a}{a}\n\n{{not json\n{a}\n");
    let (recs, malformed) = parse_ledger(&text);
    assert_eq!(recs.len(), 3);
    assert_eq!(malformed, 1);
}
```

- [ ] **Step 2: Implement**

```rust
/// Parse ledger text. Concatenated records on one line (a pre-fix
/// interleaved write) are all recovered; lines that are not JSON count as
/// malformed and are skipped.
pub fn parse_ledger(text: &str) -> (Vec<StatRecord>, usize) {
    let mut recs = Vec::new();
    let mut malformed = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let before = recs.len();
        for item in serde_json::Deserializer::from_str(line).into_iter::<StatRecord>() {
            match item {
                Ok(r) => recs.push(r),
                Err(_) => { malformed += 1; break; }
            }
        }
        if recs.len() == before && malformed == 0 { malformed += 1; }
    }
    (recs, malformed)
}
```
`read_records` uses `parse_ledger` and applies the since filter; add
`pub fn read_ledger_health() -> (usize records, usize malformed)` for `doctor`
and `report` (include `malformed_lines` in `stats` output when > 0).
Append becomes one write:
```rust
if let Ok(mut line) = serde_json::to_string(&rec) {
    line.push('\n');
    use std::io::Write;
    let _ = f.write_all(line.as_bytes());
}
```

### Task 2.5: Net-savings guard on the adapter path

**Files:**
- Modify: `src/app.rs` (extract `pays_for_itself`, use in both paths)
- Test: `src/app.rs` unit; `tests/e2e_adapters.rs` (pytest-gated)

- [ ] **Step 1: Failing tests**

```rust
// src/app.rs tests
#[test]
fn guard_rejects_candidates_that_do_not_shrink() {
    assert!(pays_for_itself("ok", "a much longer original output line\n".repeat(20).as_str(), "approx"));
    assert!(!pays_for_itself(&"x".repeat(400), "short", "approx"));
}
```
```rust
// tests/e2e_adapters.rs
#[test]
fn tiny_pytest_run_passes_through_when_report_would_be_bigger() {
    if !have("pytest") { eprintln!("SKIP: pytest not installed"); return; }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("test_one.py"), "def test_ok():\n    assert True\n").unwrap();
    let state = tempfile::tempdir().unwrap();
    let out = cartoon().env("XDG_STATE_HOME", state.path()).current_dir(dir.path())
        .args(["pytest", "-q", "-p", "no:cacheprovider"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1 passed"), "original emitted: {stdout}");
    assert!(!stdout.contains("runner: pytest"), "report would not pay for itself: {stdout}");
}
```

- [ ] **Step 2: Implement**

```rust
/// The net-savings guard: a candidate must beat the original's token count.
pub fn pays_for_itself(candidate: &str, original: &str, tokenizer: &str) -> bool {
    stats::estimate_tokens(candidate, tokenizer) < stats::estimate_tokens(original, tokenizer)
}
```
In `run_with_adapter`'s `Ok` arm build `out` + footer + extras, then:
```rust
let original = format!("{}{}", captured.stdout, captured.stderr);
let emitted = format!("{}{}{}", out, extra_out, extra_err);
if !pays_for_itself(&emitted, &original, &cfg.tokenizer) {
    print!("{}", captured.stdout);
    eprint!("{}", captured.stderr);
    stats::record_call(argv, "passthrough", &original, &original, code, &cfg.tokenizer, run.as_ref().map(|r| r.id.as_str()));
    return Ok(code);
}
```
Replace the inline comparison in `transform_emit_record` with `pays_for_itself`.

### Task 2.6: Prune never deletes the newest run

- [ ] **Step 1: Failing test** in `src/archive.rs` tests

```rust
#[test]
fn prune_keeps_the_run_just_written_even_over_the_size_cap() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = cfg();
    cfg.max_archive_mb = 0; // every byte is over the cap
    let big = "x".repeat(4096);
    let first = record_at(root.path(), &argv(&["a"]), "safe", &captured(&big, ""), 0, &[], &cfg).unwrap();
    let second = record_at(root.path(), &argv(&["b"]), "safe", &captured(&big, ""), 0, &[], &cfg).unwrap();
    assert!(!first.dir.exists(), "older run pruned");
    assert!(second.dir.exists(), "newest run must survive so raw_log never dangles");
}
```

- [ ] **Step 2: Implement** — loop condition `while i + 1 < dirs.len() && (dirs.len() - i > cfg.keep_runs || total > max_bytes)`; after the loop, if `total > max_bytes` print one stderr line: `cartoon: archive is {total_mb} MB, over max_archive_mb={cap}; the newest run is kept so raw_log stays valid`.

### Task 2.7: Archive write failure warns

- [ ] **Step 1: Test** — `write_at` into a root that is a regular file (not a dir) returns `None` and (observed manually) prints a warning; assert `None` in a unit test.
- [ ] **Step 2: Implement** — `if let Err(e) = write_all() { eprintln!("cartoon: could not archive raw output to {}: {e}", dir.display()); let _ = remove_dir_all(&dir); return None; }`

### Task 2.8: `xcrun` prefix before xcodebuild/swift detection

- [ ] **Step 1: Failing tests**

```rust
// src/adapters/xcodebuild.rs
#[test]
fn xcrun_prefix_is_transparent() {
    assert_eq!(action(&argv(&["xcrun", "xcodebuild", "test", "-scheme", "A"])), Some(Action::Test));
}
// src/hook.rs
#[test]
fn xcrun_prefixed_apple_tools_wrap() {
    assert!(wrap_command("xcrun xcodebuild test -scheme A").is_some());
    assert!(wrap_command("xcrun swift test").is_some());
    assert!(wrap_command("xcrun simctl list").is_none());
}
```

- [ ] **Step 2: Implement** — `pub fn strip_xcrun(argv: &[String]) -> &[String]` in `adapters/mod.rs` (returns `&argv[1..]` when `basename(argv[0]) == "xcrun"` and `argv.len() > 1`); `xcodebuild::action`, `SwiftTest::detect`, `SwiftBuild::detect` call it first. In the hook, when `base == "xcrun"`, advance `first`/`base` to the next word before the existing checks.

### Task 2.9: `wrap_scripts` entries default to the aggressive tier

- [ ] **Step 1: Failing test** in `src/config.rs`

```rust
#[test]
fn wrap_script_defaults_to_aggressive_without_explicit_pin() {
    let cfg: Config = toml::from_str(r#"wrap_scripts = ["./build.sh"]"#).unwrap();
    assert_eq!(resolve_level(None, false, "./build.sh", &cfg).unwrap(), CompressLevel::Aggressive);
    let pinned: Config = toml::from_str("wrap_scripts = [\"./build.sh\"]\n[command.\"./build.sh\"]\nlevel = \"safe\"").unwrap();
    assert_eq!(resolve_level(None, false, "./build.sh", &pinned).unwrap(), CompressLevel::Safe);
}
```

- [ ] **Step 2: Implement** — in `resolve_level`, after the `[command]` lookup: `if cfg.wrap_scripts.iter().any(|s| s == argv0) { return Ok(CompressLevel::Aggressive); }`. Update `init::render` to print only `wrap_scripts = [...]` plus one line "declared scripts compress at the aggressive tier by default; add `[command.\"./x.sh\"] level = \"safe\"` to override" and fix its tests. Update README "Project config" accordingly.

- [ ] **Step 3: Gate + commit Phase 2**

```bash
git commit -am "fix(routing,ledger): quote-aware -c tokenizing so adapters fire; inner command in stats/logs; learn explains sh -c waste; atomic ledger append; adapter-path savings guard; prune floor; archive warnings; xcrun prefix; wrap_scripts default aggressive"
```

---

## Phase 3 — Ladder correctness

### Task 3.1: Progress collapse only for redraws or same-template runs

- [ ] **Step 1: Failing tests** in `src/ladder/progress.rs`

```rust
#[test]
fn distinct_percentage_rows_are_not_collapsed() {
    let table = "src/a.py  10  2  80%\nsrc/b.py  20  0  100%\nsrc/c.py  5   5  0%\nTOTAL     35  7  80%";
    assert_eq!(collapse_progress(table), table);
}
#[test]
fn same_template_percentage_run_still_collapses() {
    assert_eq!(collapse_progress("Downloading 10%\nDownloading 55%\nDownloading 100%\nresolved"), "Downloading 100%\nresolved");
}
```

- [ ] **Step 2: Implement** — add
```rust
/// Digits and bar glyphs removed: two frames of one progress indicator
/// normalize to the same template; two rows of a coverage table do not.
fn template(line: &str) -> String {
    line.chars().filter(|c| !matches!(c, '0'..='9' | '=' | '#' | '>' | '-' | '.' | ' ' | '\t')).collect()
}
```
and collapse a non-CR line into the pending frame only when `template(line) == template(pending)`; a line that contained `\r` is always a frame. Keep the existing tests green.

### Task 3.2: Diagnostics survive near-dup templating

- [ ] **Step 1: Failing test** in `src/ladder/mod.rs`

```rust
#[test]
fn three_same_message_diagnostics_all_reach_the_table() {
    let mut log = String::new();
    for i in 0..90 { log.push_str(&format!("compiling unit {i}\n")); }
    for l in [10, 20, 30] { log.push_str(&format!("src/a.c:{l}:5: error: expected ';'\n")); }
    let out = compress(&log, CompressLevel::Aggressive);
    for l in [10, 20, 30] { assert!(out.contains(&format!("src/a.c:{l}:5")), "{out}"); }
}
```

- [ ] **Step 2: Implement** — `collapse_near_dups` skips lines for which `diagnostics::is_diagnostic_line(line)` is true (export that predicate from `ladder/diagnostics.rs`, wrapping its existing regexes); such lines flush the current run and are emitted verbatim.

### Task 3.3: Error window anchors on `KeyError:` / `…Exception`

- [ ] **Step 1: Failing test** in `src/ladder/window.rs`

```rust
#[test]
fn camelcase_exception_names_anchor_a_window() {
    let mut lines: Vec<String> = (0..120).map(|i| format!("line {i}")).collect();
    lines[60] = "KeyError: 'email'".into();
    lines[61] = "java.lang.NullPointerException: x".into();
    let out = window_errors(&lines.join("\n"));
    assert!(out.contains("KeyError") && out.contains("NullPointerException"), "{out}");
}
```

- [ ] **Step 2: Implement** — regex becomes
`(?i)\b(error|err!|fail|failed|failure|exception|panic|fatal|traceback)\b|[A-Za-z_][A-Za-z0-9_]*(Error|Exception)\b`.

### Task 3.4: Preserve CRLF line endings in the safe tier

- [ ] **Step 1: Failing test** in `src/ladder/safe.rs`

```rust
#[test]
fn crlf_input_keeps_crlf_output() {
    assert_eq!(collapse_blanks("a\r\n\r\n\r\nb"), "a\r\n\r\nb");
    assert_eq!(collapse_repeats("x\r\nx\r\ny"), "x\r\n  (x2)\r\ny");
}
```

- [ ] **Step 2: Implement** — `pub(crate) fn line_sep(text: &str) -> &'static str { if text.contains("\r\n") { "\r\n" } else { "\n" } }`; the three safe-tier joins use it; `collapse_blanks` trims only `[' ', '\t']`. Document in README's safe-tier bullet that trailing spaces are trimmed.

### Task 3.5: Tokenize each stream once

- [ ] **Step 1: Test** — `stats::record_counts(argv, adapter, tokens_in, tokens_out, exit, run_id)` builds the same record as `record_call` given precomputed counts (unit test compares `build_record` outputs).
- [ ] **Step 2: Implement** — `transform_emit_record` computes `in_out = estimate(stdout)`, `err = estimate(stderr)`, `cand = estimate(with_footer)` once; guard uses `cand < in_out`; stats gets `tokens_in = in_out + err`, `tokens_out = (cand or in_out) + err`. Same in `run_with_adapter`. Remove the `format!` concatenations.

- [ ] **Step 3: Golden corpus fixtures** — add to `tests/corpus/`: `coverage-table.txt` (safe tier must keep every row), `three-diagnostics.txt` (aggressive keeps 3 locs), `keyerror-traceback.txt` (aggressive keeps `KeyError`), `crlf-log.txt` (safe keeps `\r\n`); assert in `tests/corpus.rs` following its existing fixture pattern.

- [ ] **Step 4: Gate + commit Phase 3**

```bash
git commit -am "fix(ladder): progress collapse only for redraws; diagnostics survive near-dup templating; exception names anchor windows; CRLF preserved; single tokenization pass"
```

---

## Phase 4 — Mechanisms

### Task 4.1: `cartoon doctor`

**Files:**
- Create: `src/doctor.rs`
- Modify: `src/lib.rs`, `src/cli.rs` (reserved word `doctor` → `Mode::Doctor`), `src/main.rs`, `src/hook.rs` (expose `pub fn status_rows() -> Vec<(String, &'static str, bool)>`), `src/stats.rs` (`read_ledger_health`)

**Interfaces:**
- Produces: `pub fn doctor::report() -> String` (TOON) and `pub fn doctor::ladder_only_allowlist() -> Vec<String>`.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn ladder_only_lists_allowlisted_tools_without_an_adapter() {
    let only = ladder_only_allowlist();
    assert!(only.iter().any(|t| t == "make"));
    assert!(!only.iter().any(|t| t == "pytest"));
}
#[test]
fn report_has_every_section() {
    let r = report();
    for k in ["hook:", "config:", "allowlist_without_adapter", "ledger:"] { assert!(r.contains(k), "{r}"); }
}
```

- [ ] **Step 2: Implement**

```rust
pub fn ladder_only_allowlist() -> Vec<String> {
    let probe = |argv: &[&str]| crate::adapters::find_adapter(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>()).is_none();
    let mut out: Vec<String> = crate::hook::ALWAYS.iter().filter(|t| probe(&[t])).map(|t| t.to_string()).collect();
    for (tool, subs) in crate::hook::SUBCOMMAND {
        for s in *subs { if probe(&[tool, s]) { out.push(format!("{tool} {s}")); } }
    }
    out
}
```
`report()` assembles: hook rows (path, surface, installed), config (global path + parse ok, project path if any + parse ok, `wrap_scripts` entries missing on disk relative to cwd), `allowlist_without_adapter`, ledger (records, malformed_lines, negative_saved, top_uncompressed_heads), and `version`. Wire `doctor` as a reserved word in `cli.rs` (add to the `after_help` list and the reserved-words sentence).

### Task 4.2: Content-sniff fallback stage

**Files:**
- Create: `src/sniff.rs`
- Modify: `src/app.rs` (`transform_emit_record` tries `sniff::sniff` before the ladder), `src/lib.rs`

**Interfaces:**
- Produces: `pub fn sniff::sniff(stdout: &str, stderr: &str, exit: i32) -> Option<(String, &'static str)>` — rendered TOON plus mode label (`"sniff-xcodebuild"`, `"sniff-xctest"`, `"sniff-junit"`).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn sniffs_xcodebuild_build_diagnostics_from_a_wrapper_script_log() {
    let log = "note: Building targets\n/Users/d/App/A.swift:18:9: error: cannot find 'tokn' in scope\n        tokn = refresh()\n** BUILD FAILED **\n";
    let (out, mode) = sniff(log, "", 65).unwrap();
    assert_eq!(mode, "sniff-xcodebuild");
    assert!(out.contains("A.swift:18:9") && out.contains("errors: 1"));
}
#[test]
fn sniffs_xctest_summary_lines() {
    let log = "Test Suite 'All tests' started\nTest Case '-[AppTests.T testA]' passed (0.001 seconds).\nTest Case '-[AppTests.T testB]' failed (0.002 seconds).\n/Users/d/App/Tests/T.swift:12: error: -[AppTests.T testB] : XCTAssertEqual failed: (\"1\") is not equal to (\"2\")\nExecuted 2 tests, with 1 failure (0 unexpected) in 0.003 (0.010) seconds\n** TEST FAILED **\n";
    let (out, mode) = sniff(log, "", 65).unwrap();
    assert_eq!(mode, "sniff-xctest");
    assert!(out.contains("failed: 1") && out.contains("testB"));
}
#[test]
fn sniffs_junit_xml_on_stdout() {
    let xml = "<?xml version=\"1.0\"?><testsuite name=\"s\" tests=\"1\" time=\"0.1\"><testcase classname=\"c\" name=\"t\"/></testsuite>";
    assert_eq!(sniff(xml, "", 0).unwrap().1, "sniff-junit");
}
#[test]
fn does_not_sniff_plain_text_or_unexplained_failures() {
    assert!(sniff("hello\nworld\n", "", 0).is_none());
    assert!(sniff("** BUILD FAILED **\n", "ld: symbol not found", 65).is_none());
}
```

- [ ] **Step 2: Implement** — xcodebuild: marker `** BUILD FAILED **` / `** BUILD SUCCEEDED **` / `Build settings from command line` → `adapters::diagnostics::collect` on both streams → `build_value("xcodebuild-build", ...)`; return `None` when exit != 0 and zero diagnostics. xctest: marker `Executed N tests, with M failures` → parse `Test Case '-[Suite test]' passed|failed` lines and `path:line: error: -[Suite test] : msg` lines into a `TestReport { runner: "xcodebuild-test" }` (render via `report::render(&r, 20, None)`). junit: trimmed stdout starts with `<?xml` or `<testsuite` → `parse_junit_named(xml, "junit")`. In `transform_emit_record`: `let (candidate, tmode) = match sniff::sniff(...) { Some((c, m)) => (c, m), None => transform(&captured.stdout, level) };` — the guard and footer logic stay unchanged.

### Task 4.3: Generic JUnit harvester (`--junit <path>` / `[command.X] junit = "path"`)

**Files:**
- Modify: `src/cli.rs` (`--junit` flag on `Cli`, threaded into `Mode::Wrap { junit }`), `src/config.rs` (`CommandCfg.junit: Option<String>`), `src/app.rs` (`run_wrap` reads the file after the run), `src/adapters/report.rs` (`merge`), `src/main.rs`
- Test: `src/adapters/report.rs`, `tests/e2e_wrap.rs`

- [ ] **Step 1: Failing tests**

```rust
// report.rs
#[test]
fn merge_sums_reports() {
    let a = TestReport { runner: "junit", total: 2, passed: 1, failed: 1, skipped: 0, duration_s: 1.0, failures: vec![] };
    let b = TestReport { runner: "junit", total: 3, passed: 3, failed: 0, skipped: 0, duration_s: 0.5, failures: vec![] };
    let m = merge(vec![a, b]).unwrap();
    assert_eq!((m.total, m.passed, m.failed), (5, 4, 1));
}
```
```rust
// e2e_wrap.rs
#[test]
fn junit_flag_renders_a_test_report_for_any_command() {
    let state = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let xml = dir.path().join("r.xml");
    std::fs::write(&xml, "<testsuite tests=\"2\" time=\"0.2\"><testcase classname=\"c\" name=\"ok\"/><testcase classname=\"c\" name=\"bad\"><failure message=\"boom\">trace</failure></testcase></testsuite>").unwrap();
    cartoon().env("XDG_STATE_HOME", state.path())
        .args(["--junit", xml.to_str().unwrap(), "sh", "-c", "echo gradle noise; echo more noise; exit 1"])
        .assert().code(1)
        .stdout(contains("runner: sh")).stdout(contains("failed: 1")).stdout(contains("boom"));
}
```

- [ ] **Step 2: Implement** — `pub fn merge(reports: Vec<TestReport>) -> Option<TestReport>` sums fields and concatenates failures (runner from the first). In `run_wrap` (non-adapter path, after capture): resolve `junit` from the flag or `cfg.command[argv0].junit`; if the path is a file parse it, if a directory parse every `*.xml` inside and merge; runner label = `argv0` (leaked via `Box::leak` into `&'static str`, or change `TestReport.runner` to `String` — prefer `String` and update the adapters, it is a mechanical change). Render, append `raw_log`, run the guard, emit; on parse error print one stderr warning and fall to the ladder. Passthrough stderr as today.

### Task 4.4: `--max-tokens` hard output ceiling

**Files:**
- Create: `src/budget.rs`
- Modify: `src/cli.rs` (`--max-tokens N`), `src/config.rs` (`max_tokens: Option<usize>`), `src/app.rs` (apply at the single `emit` chokepoint), `src/main.rs` (env `CARTOON_MAX_TOKENS`)

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn cap_keeps_head_and_tail_and_discloses() {
    let text: String = (0..1000).map(|i| format!("line {i}\n")).collect();
    let out = cap_tokens(&text, 200, "approx", Some("20260905-1200-abcd"));
    assert!(estimate_tokens(&out, "approx") <= 230, "{}", estimate_tokens(&out, "approx"));
    assert!(out.starts_with("line 0\n"));
    assert!(out.trim_end().ends_with("line 999"));
    assert!(out.contains("omitted") && out.contains("cartoon logs grep"));
}
#[test]
fn cap_is_identity_under_budget() {
    assert_eq!(cap_tokens("small\n", 50, "approx", None), "small\n");
}
```

- [ ] **Step 2: Implement**

```rust
/// Enforce a hard token ceiling: keep the first ~60% and last ~40% of the
/// budget in whole lines, replace the middle with one disclosed marker that
/// is itself a ready-to-run `cartoon logs grep` command.
pub fn cap_tokens(text: &str, max: usize, tokenizer: &str, run_id: Option<&str>) -> String {
    if crate::stats::estimate_tokens(text, tokenizer) <= max { return text.to_string(); }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let cost = |l: &str| crate::stats::estimate_tokens(l, tokenizer).max(1);
    let (head_budget, tail_budget) = (max * 6 / 10, max * 4 / 10);
    let mut head = Vec::new(); let mut used = 0;
    for l in &lines { if used + cost(l) > head_budget { break; } used += cost(l); head.push(*l); }
    let mut tail = Vec::new(); used = 0;
    for l in lines.iter().rev().skip_while(|_| false) { if used + cost(l) > tail_budget || head.len() + tail.len() >= lines.len() { break; } used += cost(l); tail.push(*l); }
    tail.reverse();
    let omitted = lines.len() - head.len() - tail.len();
    let sel = run_id.map(|id| id.to_string()).unwrap_or_else(|| "--last".into());
    let marker = format!("  (omitted {omitted} lines to stay under --max-tokens {max}; cartoon logs grep <pattern> {sel} -C 2)\n");
    format!("{}{}{}", head.concat(), marker, tail.concat())
}
```
Apply in `app.rs` right before printing on every path (`emit`, passthrough print, adapter emit) when `cfg.max_tokens` (flag > `CARTOON_MAX_TOKENS` > config) is set. Document: with a ceiling set, even passthrough may be capped — that is the point of the flag.

### Task 4.5: `-c` pipe-filter elision (closes #12)

**Files:**
- Modify: `src/cli.rs` (`split_pipe_filter`, `Mode::Wrap { dropped_filter: Option<String> }`), `src/app.rs` (disclosure line), `src/main.rs`

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn adapter_command_piped_to_a_pure_filter_drops_the_filter() {
    let (argv, dropped) = shell_argv_with_filter("pytest -v | tail -5");
    assert_eq!(argv, vec!["pytest", "-v"]);
    assert_eq!(dropped.as_deref(), Some("tail -5"));
}
#[test]
fn pipes_that_are_not_pure_filters_or_not_adapters_keep_the_shell() {
    assert_eq!(shell_argv_with_filter("pytest | tee out.txt").0[..2], ["sh", "-c"]);
    assert_eq!(shell_argv_with_filter("echo hi | tail -1").0[..2], ["sh", "-c"]);
    assert_eq!(shell_argv_with_filter("pytest -k 'a|b'").0, vec!["pytest", "-k", "a|b"]);
}
```

- [ ] **Step 2: Implement**

```rust
const PURE_FILTERS: &[&str] = &["head", "tail", "grep", "wc", "cat", "less", "more"];

/// `<adapter cmd> | <pure output filter>` → run the adapter and drop the
/// filter (its only job was to shrink text the report already shrinks).
/// Anything else keeps today's `sh -c` behavior.
pub fn shell_argv_with_filter(s: &str) -> (Vec<String>, Option<String>) {
    if let Ok(tokens) = shell_words::split(s) {
        let bars: Vec<usize> = tokens.iter().enumerate().filter(|(_, t)| *t == "|").map(|(i, _)| i).collect();
        if let [i] = bars[..] {
            let (lhs, rhs) = (&tokens[..i], &tokens[i + 1..]);
            let lhs_str = shell_words::join(lhs);
            if !needs_shell(&lhs_str) && rhs.first().is_some_and(|f| PURE_FILTERS.contains(&f.as_str()))
                && crate::adapters::find_adapter(lhs).is_some()
            {
                return (lhs.to_vec(), Some(shell_words::join(rhs)));
            }
        }
    }
    (shell_argv(s), None)
}
```
`parse_mode` uses it for `-c`; `Mode::Wrap` gains `dropped_filter`; `run_with_adapter` appends `pipe_filter_dropped: "<filter>"` after the `raw_log` line when set. Note `needs_shell` treats `|` as an operator, so `shell_argv(lhs_str)` is only consulted for the lhs.

- [ ] **Step 3: Docs + gate + commit Phase 4**

README: new "Doctor", "--junit", "--max-tokens", "Pipes inside -c" subsections; `cli.rs` after_help lists `doctor`.
```bash
git commit -am "feat: cartoon doctor; content-sniff fallback for wrapper-script output; generic --junit harvester; --max-tokens ceiling; -c pipe-filter elision (closes #12)"
```

---

## Phase 5 — Adapter wave

Each adapter: new file `src/adapters/<name>.rs` with `detect`/`prepare`/`parse`,
inline fixture consts, unit tests for detect (positive/negative), prepare
(flag injection, user flag respected, `--` handling), parse (pass, fail,
skip, empty/garbage → `Err`). Register in `adapters/mod.rs::registry()`,
add a README table row, and add hook allowlist entries only where noted.
Execution note: pre-create every `pub mod` line and a compiling stub per
file first so independent implementers can work in parallel on disjoint
files.

### Task 5.1: pre-commit (closes #11)

- detect: `basename(argv[0]) == "pre-commit"` and (`argv.len() == 1` or `argv[1] == "run"`); not for `install|uninstall|autoupdate|clean|gc|sample-config|try-repo|migrate-config|init-templatedir`.
- prepare: append `--color=never` (pre-commit accepts `--color {auto,always,never}`).
- parse (stdout): regex `^(?P<name>.*?)\.{3,}(?P<status>Passed|Failed|Skipped)$` also matching `(no files to check)Skipped`; a `Failed` hook's detail block runs until the next dotted line: `- hook id: <id>`, `- exit code: <n>`, then output lines. `TestReport { runner: "pre-commit", total, passed, failed, skipped, failures: [Failure { id: name, loc: hook id, msg: first output line, trace: rest }] }`.
- Fixture (passing, from issue #11) and a failing fixture:
```
Ruff check...............................................................Failed
- hook id: ruff
- exit code: 1

src/a.py:10:5: F821 Undefined name `x`
Found 1 error.

Ruff format..............................................................Passed
```

### Task 5.2: cargo test / cargo nextest (stable text)

- detect: `basename == "cargo"` and (`argv[1] == "test"` or (`argv[1] == "nextest"` and `argv[2] == "run"`)).
- prepare: nothing injected (libtest JSON is nightly-only; never claim it).
- parse cargo test (stdout): per target `running N tests`; lines `test <name> ... ok|FAILED|ignored`; `failures:` section with `---- <name> stdout ----` blocks until the next `----` or `failures:` list; `test result: ok|FAILED. P passed; F failed; I ignored; M measured; X filtered out; finished in T s` summed across targets. Doc-tests included. Exit != 0 with zero `test result` lines → compile error → `passthrough_stdout/stderr` set (unexplained failure). Runner `"cargo-test"`.
- parse nextest (stderr is where nextest prints): lines `PASS [ 0.012s] crate::mod::name`, `FAIL [...]`, `SKIP [...]`, `Summary [ 1.234s] 36 tests run: 35 passed, 1 failed, 0 skipped`; failure output between `--- STDOUT: crate::mod::name ---` / `--- STDERR ... ---` blocks. Runner `"cargo-nextest"`.

### Task 5.3: cargo build / check / clippy `--message-format=json`

- detect: `basename == "cargo"` and `argv[1] ∈ {build, check, clippy}`.
- prepare: unless a `--message-format` is present, insert `--message-format=json` immediately before the first `--` token if any, else append (clippy's `-- -D warnings` must stay last).
- parse (stdout JSON lines): `reason == "compiler-message"` and `message.level ∈ {error, warning}`; skip messages with no `spans` and code null (the "N warnings emitted" summaries); loc from the primary span `file_name:line_start:column_start`; `rule` from `message.code.code`; msg `message.message`. Value via `diagnostics::build_value("cargo-build" | "cargo-check" | "cargo-clippy", ...)`. Exit != 0 with zero diagnostics → unexplained → passthrough streams. Human `--message-format` supplied by the user → `Err` → passthrough.

### Task 5.4: go test `-json`

- detect: `basename == "go"` and `argv[1] == "test"`.
- prepare: insert `-json` at index 2 unless present (flags precede packages).
- parse (stdout JSON lines `{Time, Action, Package, Test, Elapsed, Output}`): per `Test` collect `Output` lines; terminal `Action ∈ {pass, fail, skip}` with non-empty `Test` counts; failures msg = first Output line not starting with `=== RUN`/`--- FAIL`, trimmed; duration = sum of package-level `Elapsed` on terminal actions with empty `Test`. Build failures: `Action == "output"` lines with `# pkg` and no test events, exit != 0 → unexplained → passthrough. Runner `"go-test"`.

### Task 5.5: mypy `--output json`

- detect: `basename == "mypy"` (also `python -m mypy`, `uv run mypy`).
- prepare: append `--output json` unless `--output` present.
- parse (stdout JSON lines `{file, line, column, message, hint, code, severity}`): severity `error|note` (notes are not counted); loc `file:line:column`; rule `code`. `diagnostics::build_value("mypy", ...)`. Non-JSON stdout (old mypy) → `Err`.

### Task 5.6: phpunit `--log-junit`

- detect: `basename == "phpunit"` (covers `vendor/bin/phpunit`).
- prepare: tempfile `cartoon-phpunit-*.xml`; append `--log-junit <path>`.
- parse: `parse_junit_named(xml, "phpunit")`; stdout consumed; stderr passthrough when non-empty.

### Task 5.7: rspec `--format json`

- detect: `basename == "rspec"`, or `bundle exec rspec`.
- prepare: tempfile; append `--format json --out <path>` (the default progress formatter keeps printing to stdout).
- parse JSON `{ examples: [{ id, full_description, status, file_path, line_number, exception: { class, message, backtrace } }], summary: { duration, example_count, failure_count, pending_count } }` → `TestReport { runner: "rspec" }`; `pending` → skipped; failure msg = `exception.class: first line of message`; trace = backtrace lines not containing `/gems/`.

### Task 5.8: xcodebuild `archive` / `-exportArchive`

- `xcodebuild::Action` gains `Archive` (tokens `archive`, `-exportArchive`); `XcodebuildBuild::detect` accepts `Build | Archive`; runner label `"xcodebuild-archive"` for the archive case. The hook wraps only `Test | Build` (`subcommand_gating_blocks_mutating_subcommands` keeps `xcodebuild archive` unwrapped — extend it with `xcodebuild -exportArchive`).

### Task 5.9: swiftlint

- detect: `basename == "swiftlint"` and no `autocorrect`/`--fix` token (mutating; the hook already leaves those alone).
- prepare: append `--reporter json` unless `--reporter` present.
- parse JSON array `[{ file, line, character, severity: "Warning"|"Error", rule_id, reason }]` → `diagnostics::build_value("swiftlint", ...)`; severity lowercased.
- hook: add `"swiftlint"` to `ALWAYS` (its `MUTATING_TOKENS` entry exists from Task 1.2).

- [ ] **Register, README table, `cartoon adapters`, gate, commit Phase 5**

```bash
git commit -am "feat(adapters): pre-commit (closes #11), cargo test/nextest, cargo build/check/clippy json, go test -json, mypy json, phpunit junit, rspec json, xcodebuild archive, swiftlint"
```

---

## Phase 6 — Release hygiene and structure

### Task 6.1: Version gate across every manifest; bump to 0.6.0

- Create `scripts/check-versions.mjs`: reads `Cargo.toml` version; compares `docs/index.html` marker, `.claude-plugin/plugin.json` `version`, `.claude-plugin/marketplace.json` plugin `version` (if present); with `--tag vX.Y.Z` also compares the tag. Exit 1 with a per-file diff on mismatch; `--write` fixes the JSON manifests and calls the site sync.
- Create `tests/version_sync.rs`: reads `.claude-plugin/plugin.json` and asserts `version == env!("CARGO_PKG_VERSION")` (the local gate catches drift since CI is off).
- `release.yml`: new first job `verify-version` running `node scripts/check-versions.mjs --tag "$GITHUB_REF_NAME"`; every other job gets `needs: verify-version` (release stays enabled: tags are rare and this is the only place publishing happens).
- Bump `Cargo.toml` to `0.6.0`, run `node scripts/check-versions.mjs --write`, update `RELEASING.md` "Versioning" to name the script.

### Task 6.2: Split `src/hook.rs`

- `git mv src/hook.rs src/hook/mod.rs`; create `src/hook/install.rs` holding `Target`, `target()`, install/uninstall/status/offer_instructions/prompt_yes and their tests; `mod.rs` keeps `run`, `Surface`, rewrite decision, allowlists, `wrap_command*`, `is_noisy`, `uv_wraps_noisy`, `split_segments`, and their tests. Public API unchanged: `hook::run`, `hook::wrap_command`, `hook::wrap_command_with_policy`, `hook::rewrite_decision`, `hook::rewrite_decision_with_scripts`, `hook::ALWAYS`, `hook::SUBCOMMAND`, `hook::RUNNERS`, `hook::UV_HOOK_SAFE_FLAGS`, `hook::status_rows`. `cargo test` proves nothing moved semantically.

### Task 6.3: Docs drift

- README: adapter table adds the nine new rows; a sentence under "What it wraps" distinguishing adapter-backed tools from ladder-only allowlist entries (`make`, `pre-commit` until 5.1 lands, `gradle`, `mvn`, `dotnet`, `npm test`, …) and pointing at `cartoon doctor`; "Project config" reflects the aggressive default; "Config" documents `junit`, `max_tokens`; hook section documents the tightened allowlist (0.6.0) and `--deny` mode switching; "Disabling & overhead" mentions `--max-tokens`.
- `docs/design.md`: banner at the top — "Frozen v0.1 draft (2026-06-09). Several items marked out of scope have shipped; the current roadmap is `docs/superpowers/specs/2026-06-11-cartoon-v02-roadmap.md` and the 2026-09-05 review plan."
- Roadmap spec: mark `--max-tokens`, `doctor`, adapter wave 2 items, content sniffing first slice as shipped with the date; add the CI-disabled decision under "KILLED / deliberately not doing".
- `skills/cartoon/SKILL.md` and `docs/agents.md`: mention `cartoon doctor` as the first troubleshooting step.

- [ ] **Gate + commit Phase 6**

```bash
git commit -am "chore(release,docs): 0.6.0; version gate across manifests + release job; split hook.rs; README/roadmap/design/skill drift"
```

---

## Wrap-up

1. `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` — record counts.
2. Replay: `for d in $(ls -t ~/.local/state/cartoon/runs | head -20); do cartoon ingest ~/.local/state/cartoon/runs/$d/stdout.log; done` — note before/after tokens for the PR body (read-only on the archive apart from new ingest records).
3. Manual test checklist: `.scratch/feat-wrap-scripts-project-config-test-checklist.html` (interactive, localStorage-persisted, per-phase sections, regression tags) plus a markdown twin; the PR body's "Test plan" links the list.
4. Push and open one PR against `main` with the seven commits (docs + six phases), the local-gate results, the replay numbers, and the checklist.
