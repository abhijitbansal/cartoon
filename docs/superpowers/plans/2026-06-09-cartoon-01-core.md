# cartoon Core Implementation Plan (1 of 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Working `cartoon` binary: wraps any CLI, captures output, mirrors exit codes, converts JSON output to TOON, optional heuristic compression, token-savings stats.

**Architecture:** Single Rust binary (lib + thin main). Pipeline: parse CLI → spawn child via runner → transform captured stdout (JSON→TOON, else heuristic if enabled, else passthrough) → emit + record stats. Test adapters come in plan 2 of 3; this plan delivers everything else.

**Tech Stack:** Rust 2021, clap (derive), serde/serde_json (preserve_order), toml, roxmltree (used in plan 2), regex, chrono, tiktoken-rs, tempfile, anyhow. Dev: assert_cmd, predicates, insta.

**Execution context:** Run in a NEW repo directory (e.g. `~/projects/cartoon`), NOT in claude-skills. Spec lives at `claude-skills/docs/superpowers/specs/2026-06-09-cartoon-toon-cli-wrapper-design.md`; copy it to `docs/design.md` in the new repo (Task 1).

---

### Task 1: Scaffold

**Files:**
- Create: `Cargo.toml`, `src/main.rs`, `src/lib.rs`, `.gitignore`, `docs/design.md`

- [ ] **Step 1: Create repo + cargo project**

```bash
mkdir -p ~/projects/cartoon && cd ~/projects/cartoon
git init
cargo init --name cartoon .
mkdir -p docs
cp ~/projects/claude-skills/docs/superpowers/specs/2026-06-09-cartoon-toon-cli-wrapper-design.md docs/design.md
printf 'target/\n' > .gitignore
```

- [ ] **Step 2: Write Cargo.toml**

```toml
[package]
name = "cartoon"
version = "0.1.0"
edition = "2021"
description = "Token-optimized TOON output wrapper for any CLI"
license = "MIT"
repository = "https://github.com/abhijitbansal/cartoon"

[dependencies]
anyhow = "1"
chrono = "0.4"
clap = { version = "4", features = ["derive"] }
dirs = "5"
regex = "1"
roxmltree = "0.20"
serde = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["preserve_order"] }
tempfile = "3"
tiktoken-rs = "0.6"
toml = "0.8"

[dev-dependencies]
assert_cmd = "2"
insta = "1"
predicates = "3"
tempfile = "3"
```

`preserve_order` is load-bearing: TOON output must keep insertion order (summary before failures).

- [ ] **Step 3: Write src/lib.rs and src/main.rs**

`src/lib.rs` (modules get added task by task; start empty so every commit builds):
```rust
// modules are added task by task
```

`src/main.rs`:
```rust
fn main() {
    println!("cartoon");
}
```

- [ ] **Step 4: Verify build**

Run: `cargo build && cargo test`
Expected: compiles, zero tests pass.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: scaffold cartoon crate"
```

---

### Task 2: CLI parsing

**Files:**
- Create: `src/cli.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing tests**

Create `src/cli.rs` containing ONLY the test module first (TDD red state — it won't compile until Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn mode(args: &[&str]) -> Mode {
        parse_mode(Cli::parse_from(args)).unwrap()
    }

    #[test]
    fn wrap_mode_passes_args_verbatim() {
        let m = mode(&["cartoon", "pytest", "-q", "--maxfail=1"]);
        assert_eq!(
            m,
            Mode::Wrap {
                argv: vec!["pytest".into(), "-q".into(), "--maxfail=1".into()],
                heuristic: false,
                raw: false
            }
        );
    }

    #[test]
    fn heuristic_flag_before_command() {
        let m = mode(&["cartoon", "--heuristic", "ls", "-la"]);
        assert!(matches!(m, Mode::Wrap { heuristic: true, .. }));
    }

    #[test]
    fn stats_subcommand_with_since() {
        let m = mode(&["cartoon", "stats", "--since", "7d"]);
        assert_eq!(m, Mode::Stats { since: Some("7d".into()) });
    }

    #[test]
    fn adapters_subcommand() {
        assert_eq!(mode(&["cartoon", "adapters"]), Mode::Adapters);
    }

    #[test]
    fn no_command_is_error() {
        assert!(parse_mode(Cli::parse_from(["cartoon"])).is_err());
    }
}
```

Add to `src/lib.rs`:
```rust
pub mod cli;
```

- [ ] **Step 2: Run tests, verify failure**

Run: `cargo test cli`
Expected: compile error (Cli, Mode, parse_mode undefined) — that is the failing state.

- [ ] **Step 3: Implement, above the test module in src/cli.rs**

```rust
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "cartoon",
    version,
    about = "Token-optimized TOON output wrapper for any CLI",
    after_help = "Subcommands `stats` and `adapters` are reserved words; \
to wrap a binary literally named `stats`, use: cartoon env stats"
)]
pub struct Cli {
    /// Enable the lossy heuristic fallback for this call
    #[arg(long)]
    pub heuristic: bool,

    /// Bypass cartoon entirely; run the command untouched
    #[arg(long)]
    pub raw: bool,

    /// Command to wrap plus its args (or: stats | adapters)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum Mode {
    Wrap { argv: Vec<String>, heuristic: bool, raw: bool },
    Stats { since: Option<String> },
    Adapters,
}

pub fn parse_mode(cli: Cli) -> anyhow::Result<Mode> {
    if cli.command.is_empty() {
        anyhow::bail!("no command given. usage: cartoon <cmd> [args...]");
    }
    match cli.command[0].as_str() {
        "stats" => Ok(Mode::Stats { since: parse_since(&cli.command[1..])? }),
        "adapters" => Ok(Mode::Adapters),
        _ => Ok(Mode::Wrap { argv: cli.command, heuristic: cli.heuristic, raw: cli.raw }),
    }
}

fn parse_since(args: &[String]) -> anyhow::Result<Option<String>> {
    match args {
        [] => Ok(None),
        [flag, value] if flag == "--since" => Ok(Some(value.clone())),
        _ => anyhow::bail!("usage: cartoon stats [--since <e.g. 7d|24h|30m>]"),
    }
}
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test cli`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs src/lib.rs
git commit -m "feat: CLI parsing with wrap/stats/adapters modes"
```

---

### Task 3: Runner (spawn, capture, exit codes)

**Files:**
- Create: `src/runner.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing tests** (create `src/runner.rs` with only this module; add `pub mod runner;` to lib.rs)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Captured {
        run(&["sh".to_string(), "-c".to_string(), script.to_string()]).unwrap()
    }

    #[test]
    fn captures_stdout_and_stderr_separately() {
        let c = sh("echo out; echo err >&2");
        assert_eq!(c.stdout, "out\n");
        assert_eq!(c.stderr, "err\n");
    }

    #[test]
    fn mirrors_exit_code() {
        let c = sh("exit 3");
        assert_eq!(exit_code(&c.status), 3);
    }

    #[test]
    fn missing_command_is_not_found_error() {
        let err = run(&["definitely-not-a-real-binary-xyz".to_string()]).unwrap_err();
        let io = err.downcast_ref::<std::io::Error>().unwrap();
        assert_eq!(io.kind(), std::io::ErrorKind::NotFound);
    }
}
```

- [ ] **Step 2: Run, verify compile failure**

Run: `cargo test runner`
Expected: FAIL (unresolved types).

- [ ] **Step 3: Implement above the tests**

```rust
use anyhow::{Context, Result};
use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};

pub struct Captured {
    pub stdout: String,
    pub stderr: String,
    pub status: ExitStatus,
}

/// Spawn argv[0] with argv[1..], capture both streams, wait for exit.
/// Non-UTF8 output is converted lossily (documented v1 limitation).
pub fn run(argv: &[String]) -> Result<Captured> {
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run {}", argv[0]))?;

    let mut err_pipe = child.stderr.take().expect("stderr piped");
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_pipe.read_to_end(&mut buf);
        buf
    });

    let mut out_buf = Vec::new();
    child
        .stdout
        .take()
        .expect("stdout piped")
        .read_to_end(&mut out_buf)?;

    let status = child.wait()?;
    let err_buf = err_thread.join().expect("stderr reader panicked");

    Ok(Captured {
        stdout: String::from_utf8_lossy(&out_buf).into_owned(),
        stderr: String::from_utf8_lossy(&err_buf).into_owned(),
        status,
    })
}

/// Child exit code; signal death maps to conventional 128+N (unix).
pub fn exit_code(status: &ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    1
}
```

Note: anyhow's `downcast_ref` searches the context chain, so the `with_context` wrapper does not break the NotFound test.

Signal handling (spec rule "signals forwarded, cartoon dies after child"): the child shares cartoon's process group, so terminal-sent SIGINT/SIGTERM reach it directly; cartoon then waits and mirrors 128+N via `exit_code`. No explicit forwarding code needed for v1 — do not add any.

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test runner`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add src/runner.rs src/lib.rs
git commit -m "feat: child process runner with capture and exit-code mirroring"
```

---

### Task 4: TOON encoder — scalars and quoting

**Files:**
- Create: `src/toon/mod.rs`, `src/toon/encode.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing tests** (bottom of new `src/toon/encode.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scalars() {
        assert_eq!(encode(&json!(42)), "42");
        assert_eq!(encode(&json!(3.5)), "3.5");
        assert_eq!(encode(&json!(true)), "true");
        assert_eq!(encode(&json!(null)), "null");
        assert_eq!(encode(&json!("plain")), "plain");
    }

    #[test]
    fn strings_quoted_when_ambiguous() {
        assert_eq!(encode(&json!("")), "\"\"");
        assert_eq!(encode(&json!("42")), "\"42\"");
        assert_eq!(encode(&json!("true")), "\"true\"");
        assert_eq!(encode(&json!("a, b")), "\"a, b\"");
        assert_eq!(encode(&json!("k: v")), "\"k: v\"");
        assert_eq!(encode(&json!(" padded")), "\" padded\"");
        assert_eq!(encode(&json!("line\nbreak")), "\"line\\nbreak\"");
        assert_eq!(encode(&json!("say \"hi\"")), "\"say \\\"hi\\\"\"");
    }
}
```

`src/toon/mod.rs`:
```rust
pub mod encode;
pub use encode::encode;
```

Add to `src/lib.rs`:
```rust
pub mod toon;
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test toon`
Expected: FAIL (encode undefined).

- [ ] **Step 3: Implement scalars in src/toon/encode.rs**

```rust
use serde_json::Value;

/// Encode a JSON value as TOON. Returns lines joined by '\n', no trailing newline.
pub fn encode(value: &Value) -> String {
    let mut lines: Vec<String> = Vec::new();
    match value {
        Value::Object(_) | Value::Array(_) => container_lines(value, &mut lines),
        v => lines.push(scalar(v)),
    }
    lines.join("\n")
}

fn container_lines(_value: &Value, _lines: &mut Vec<String>) {
    unimplemented!("Tasks 5 and 6")
}

pub(crate) fn scalar(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            if needs_quotes(s) {
                quote(s)
            } else {
                s.to_string()
            }
        }
        _ => unreachable!("scalar() called on container"),
    }
}

fn needs_quotes(s: &str) -> bool {
    s.is_empty()
        || s.trim() != s
        || matches!(s, "true" | "false" | "null")
        || s.parse::<f64>().is_ok()
        || s.contains([',', ':', '"', '\\', '\n', '\r', '\t'])
        || s.starts_with(['-', '[', ']', '{', '}', '#'])
}

pub(crate) fn quote(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test toon`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add src/toon/ src/lib.rs
git commit -m "feat: TOON scalar encoding with quoting rules"
```

---

### Task 5: TOON encoder — objects and nesting

**Files:**
- Modify: `src/toon/encode.rs`

- [ ] **Step 1: Add failing tests to the tests module**

```rust
    #[test]
    fn flat_object() {
        let v = json!({"a": 1, "b": "hi", "c": true, "d": null});
        assert_eq!(encode(&v), "a: 1\nb: hi\nc: true\nd: null");
    }

    #[test]
    fn nested_objects_indent_two_spaces() {
        let v = json!({"outer": {"inner": {"k": "v"}}, "next": 1});
        assert_eq!(encode(&v), "outer:\n  inner:\n    k: v\nnext: 1");
    }

    #[test]
    fn empty_object_value() {
        assert_eq!(encode(&json!({"e": {}})), "e: {}");
    }

    #[test]
    fn keys_with_special_chars_are_quoted() {
        assert_eq!(encode(&json!({"a key": 1})), "\"a key\": 1");
    }
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test toon`
Expected: FAIL — `unimplemented!` panic in `container_lines`.

- [ ] **Step 3: Implement objects**

Replace `container_lines` with:

```rust
fn container_lines(value: &Value, lines: &mut Vec<String>) {
    match value {
        Value::Object(map) => object_lines(map, 0, lines),
        Value::Array(arr) => array_lines(None, arr, 0, lines),
        _ => unreachable!(),
    }
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

fn object_lines(map: &serde_json::Map<String, Value>, depth: usize, lines: &mut Vec<String>) {
    for (k, v) in map {
        let key = key_str(k);
        match v {
            Value::Object(m) if m.is_empty() => {
                lines.push(format!("{}{}: {{}}", indent(depth), key))
            }
            Value::Object(m) => {
                lines.push(format!("{}{}:", indent(depth), key));
                object_lines(m, depth + 1, lines);
            }
            Value::Array(arr) => array_lines(Some(&key), arr, depth, lines),
            v => lines.push(format!("{}{}: {}", indent(depth), key, scalar(v))),
        }
    }
}

fn array_lines(_key: Option<&str>, _arr: &[Value], _depth: usize, _lines: &mut Vec<String>) {
    unimplemented!("Task 6")
}

fn key_str(k: &str) -> String {
    let plain = !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if plain {
        k.to_string()
    } else {
        quote(k)
    }
}
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test toon`
Expected: object tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/toon/encode.rs
git commit -m "feat: TOON object encoding with nesting"
```

---

### Task 6: TOON encoder — arrays + fixture harness

**Files:**
- Modify: `src/toon/encode.rs`
- Create: `tests/toon_fixtures.rs`, `tests/fixtures/toon/*.json` (8 files)

- [ ] **Step 1: Add failing unit tests**

```rust
    #[test]
    fn primitive_array_inline() {
        assert_eq!(encode(&json!({"tags": ["a", "b", "c"]})), "tags[3]: a,b,c");
    }

    #[test]
    fn empty_array() {
        assert_eq!(encode(&json!({"xs": []})), "xs[0]:");
    }

    #[test]
    fn uniform_object_array_is_tabular() {
        let v = json!({"users": [{"id": 1, "name": "Alice"}, {"id": 2, "name": "Bob"}]});
        assert_eq!(encode(&v), "users[2]{id,name}:\n  1,Alice\n  2,Bob");
    }

    #[test]
    fn mixed_array_is_list() {
        let v = json!({"items": [1, {"a": 2}, [3]]});
        assert_eq!(encode(&v), "items[3]:\n  - 1\n  - a: 2\n  - [1]: 3");
    }

    #[test]
    fn root_array() {
        let v = json!([{"id": 1}, {"id": 2}]);
        assert_eq!(encode(&v), "[2]{id}:\n  1\n  2");
    }
```

- [ ] **Step 2: Run, verify `unimplemented!` failures**

Run: `cargo test toon`

- [ ] **Step 3: Implement arrays**

Replace the `array_lines` stub:

```rust
fn array_lines(key: Option<&str>, arr: &[Value], depth: usize, lines: &mut Vec<String>) {
    let head = |suffix: &str| match key {
        Some(k) => format!("{}{}{}", indent(depth), k, suffix),
        None => format!("{}{}", indent(depth), suffix),
    };
    if arr.is_empty() {
        lines.push(head("[0]:"));
        return;
    }
    if arr.iter().all(is_scalar) {
        let row = arr.iter().map(scalar).collect::<Vec<_>>().join(",");
        lines.push(format!("{} {}", head(&format!("[{}]:", arr.len())), row));
        return;
    }
    if let Some(fields) = tabular_fields(arr) {
        let header = fields.iter().map(|f| key_str(f)).collect::<Vec<_>>().join(",");
        lines.push(head(&format!("[{}]{{{}}}:", arr.len(), header)));
        for item in arr {
            let obj = item.as_object().expect("tabular item is object");
            let row = fields
                .iter()
                .map(|f| scalar(&obj[f]))
                .collect::<Vec<_>>()
                .join(",");
            lines.push(format!("{}{}", indent(depth + 1), row));
        }
        return;
    }
    lines.push(head(&format!("[{}]:", arr.len())));
    for item in arr {
        let mut item_lines: Vec<String> = Vec::new();
        match item {
            Value::Object(m) => object_lines(m, 0, &mut item_lines),
            Value::Array(a) => array_lines(None, a, 0, &mut item_lines),
            v => item_lines.push(scalar(v)),
        }
        for (i, l) in item_lines.iter().enumerate() {
            let bullet = if i == 0 { "- " } else { "  " };
            lines.push(format!("{}{}{}", indent(depth + 1), bullet, l));
        }
    }
}

fn is_scalar(v: &Value) -> bool {
    !matches!(v, Value::Object(_) | Value::Array(_))
}

/// Same keys in same order, all values scalar → tabular form.
fn tabular_fields(arr: &[Value]) -> Option<Vec<String>> {
    let first = arr.first()?.as_object()?;
    if first.is_empty() {
        return None;
    }
    let fields: Vec<String> = first.keys().cloned().collect();
    for item in arr {
        let obj = item.as_object()?;
        if obj.len() != fields.len() {
            return None;
        }
        for f in &fields {
            if !obj.get(f).map(is_scalar).unwrap_or(false) {
                return None;
            }
        }
    }
    Some(fields)
}
```

- [ ] **Step 4: Run unit tests, verify pass**

Run: `cargo test toon`
Expected: all encoder tests pass.

- [ ] **Step 5: Add fixture harness**

`tests/toon_fixtures.rs`:
```rust
use serde_json::Value;
use std::fs;

#[test]
fn toon_fixtures() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/toon");
    let mut checked = 0;
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let case: Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let expected = case["expected"].as_str().unwrap();
        let got = cartoon::toon::encode(&case["input"]);
        assert_eq!(got, expected, "fixture {:?}", path.file_name().unwrap());
        checked += 1;
    }
    assert!(checked >= 8, "only {checked} fixtures ran");
}
```

Create 8 fixture files in `tests/fixtures/toon/`, each `{"input": ..., "expected": "..."}`:

`01-scalars.json`:
```json
{"input": {"a": 1, "b": "hi", "c": true, "d": null}, "expected": "a: 1\nb: hi\nc: true\nd: null"}
```
`02-quoting.json`:
```json
{"input": {"s": "hello, world", "n": "42", "e": "", "pad": " x"}, "expected": "s: \"hello, world\"\nn: \"42\"\ne: \"\"\npad: \" x\""}
```
`03-nested.json`:
```json
{"input": {"outer": {"inner": {"k": "v"}}}, "expected": "outer:\n  inner:\n    k: v"}
```
`04-primitive-array.json`:
```json
{"input": {"tags": ["a", "b", "c"]}, "expected": "tags[3]: a,b,c"}
```
`05-tabular.json`:
```json
{"input": {"users": [{"id": 1, "name": "Alice"}, {"id": 2, "name": "Bob"}]}, "expected": "users[2]{id,name}:\n  1,Alice\n  2,Bob"}
```
`06-mixed-list.json`:
```json
{"input": {"items": [1, {"a": 2}, [3]]}, "expected": "items[3]:\n  - 1\n  - a: 2\n  - [1]: 3"}
```
`07-root-array.json`:
```json
{"input": [{"id": 1}, {"id": 2}], "expected": "[2]{id}:\n  1\n  2"}
```
`08-empty-array.json`:
```json
{"input": {"xs": []}, "expected": "xs[0]:"}
```

- [ ] **Step 6: Run all tests**

Run: `cargo test`
Expected: all pass including `toon_fixtures`.

- [ ] **Step 7 (non-blocking): Vendor official TOON conformance fixtures**

Check https://github.com/toon-format/toon for its test fixture directory. If convertible, add converted cases to `tests/fixtures/toon/` (same `{input, expected}` shape, numbered `1xx-*.json`). Where official expectations differ from our encoder, fix the encoder, not the fixture. If impractical right now, file a repo issue titled "vendor official TOON conformance fixtures" and move on — our 8 fixtures pin the documented v1 behavior.

- [ ] **Step 8: Commit**

```bash
git add src/toon/encode.rs tests/
git commit -m "feat: TOON array encoding (inline, tabular, list) + fixture harness"
```

---

### Task 7: JSON fallback + wrap pipeline

**Files:**
- Create: `src/fallback.rs`, `src/app.rs`, `src/heuristic.rs` (stub), `tests/e2e_wrap.rs`
- Modify: `src/lib.rs`, `src/main.rs`

- [ ] **Step 1: Write failing fallback tests** (bottom of new `src/fallback.rs`; add `pub mod fallback;` to lib.rs)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_output_json_object() {
        assert!(detect_json("{\"a\": 1}").is_some());
    }

    #[test]
    fn trailing_json_after_log_lines() {
        let out = "warming up...\nconnecting...\n{\"result\": [1, 2]}";
        let v = detect_json(out).unwrap();
        assert_eq!(v["result"][0], 1);
    }

    #[test]
    fn plain_text_is_none() {
        assert!(detect_json("all 48 tests passed").is_none());
    }

    #[test]
    fn bare_scalar_json_is_none() {
        assert!(detect_json("42").is_none());
    }
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test fallback`

- [ ] **Step 3: Implement src/fallback.rs**

```rust
use serde_json::Value;

const MAX_PARSE_ATTEMPTS: usize = 20;

/// Detect a JSON object/array in stdout: either the whole (trimmed) output,
/// or a trailing document starting at some line (CLIs often log before the payload).
pub fn detect_json(stdout: &str) -> Option<Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut attempts = 0;
    let mut offset = 0;
    for line in trimmed.split_inclusive('\n') {
        let candidate = &trimmed[offset..];
        offset += line.len();
        if !candidate.starts_with(['{', '[']) {
            continue;
        }
        attempts += 1;
        if attempts > MAX_PARSE_ATTEMPTS {
            return None;
        }
        if let Ok(v) = serde_json::from_str::<Value>(candidate) {
            if v.is_object() || v.is_array() {
                return Some(v);
            }
        }
    }
    None
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test fallback`
Expected: 4 passed.

- [ ] **Step 5: Implement src/app.rs + heuristic stub**

`src/heuristic.rs` (stub so the pipeline compiles; real implementation in Task 8):
```rust
/// Lossy text compression. Real implementation in the heuristic task.
pub fn compress(text: &str) -> String {
    text.to_string()
}
```

`src/app.rs`:
```rust
use crate::{fallback, heuristic, runner, toon};
use anyhow::Result;

/// Run the wrapped command, transform stdout, mirror the exit code.
pub fn run_wrap(argv: &[String], heuristic_on: bool, raw: bool) -> Result<i32> {
    let captured = match runner::run(argv) {
        Ok(c) => c,
        Err(e) => {
            let not_found = e
                .downcast_ref::<std::io::Error>()
                .map(|io| io.kind() == std::io::ErrorKind::NotFound)
                .unwrap_or(false);
            if not_found {
                eprintln!("cartoon: command not found: {}", argv[0]);
                return Ok(127);
            }
            return Err(e);
        }
    };
    let code = runner::exit_code(&captured.status);
    if raw {
        print!("{}", captured.stdout);
        eprint!("{}", captured.stderr);
        return Ok(code);
    }
    let (out, _mode) = transform(&captured.stdout, heuristic_on);
    print!("{out}");
    if !out.is_empty() && !out.ends_with('\n') {
        println!();
    }
    eprint!("{}", captured.stderr);
    Ok(code)
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

Add to `src/lib.rs`:
```rust
pub mod app;
pub mod heuristic;
```

- [ ] **Step 6: Wire src/main.rs**

```rust
use clap::Parser;

fn main() {
    let cli = cartoon::cli::Cli::parse();
    let code = match cartoon::cli::parse_mode(cli) {
        Ok(cartoon::cli::Mode::Wrap { argv, heuristic, raw }) => {
            cartoon::app::run_wrap(&argv, heuristic, raw).unwrap_or_else(|e| {
                eprintln!("cartoon: {e}");
                2
            })
        }
        Ok(cartoon::cli::Mode::Stats { .. }) => {
            println!("(stats not implemented yet)");
            0
        }
        Ok(cartoon::cli::Mode::Adapters) => {
            println!("(no adapters yet)");
            0
        }
        Err(e) => {
            eprintln!("cartoon: {e}");
            2
        }
    };
    std::process::exit(code);
}
```

- [ ] **Step 7: Write E2E tests — tests/e2e_wrap.rs**

```rust
use assert_cmd::Command;
use predicates::str::contains;

fn cartoon() -> Command {
    Command::cargo_bin("cartoon").unwrap()
}

#[test]
fn json_output_becomes_toon() {
    cartoon()
        .args(["sh", "-c", r#"echo '{"a": 1, "tags": ["x", "y"]}'"#])
        .assert()
        .success()
        .stdout(contains("a: 1"))
        .stdout(contains("tags[2]: x,y"));
}

#[test]
fn plain_output_passes_through_with_exit_code() {
    cartoon()
        .args(["sh", "-c", "echo plain text; exit 4"])
        .assert()
        .code(4)
        .stdout(contains("plain text"));
}

#[test]
fn raw_flag_bypasses_transform() {
    cartoon()
        .args(["--raw", "sh", "-c", r#"echo '{"a": 1}'"#])
        .assert()
        .success()
        .stdout(contains(r#"{"a": 1}"#));
}

#[test]
fn missing_command_exits_127() {
    cartoon()
        .args(["definitely-not-a-real-binary-xyz"])
        .assert()
        .code(127);
}
```

- [ ] **Step 8: Run all tests, verify pass**

Run: `cargo test`
Expected: all pass.

- [ ] **Step 9: Commit**

```bash
git add src/ tests/e2e_wrap.rs
git commit -m "feat: wrap pipeline with JSON-to-TOON fallback and passthrough"
```

---

### Task 8: Heuristic compressor

**Files:**
- Modify: `src/heuristic.rs`, `tests/e2e_wrap.rs`

- [ ] **Step 1: Write failing tests** (bottom of `src/heuristic.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_codes() {
        assert_eq!(compress("\x1b[32mPASS\x1b[0m ok"), "PASS ok");
    }

    #[test]
    fn collapses_blank_runs_to_one() {
        assert_eq!(compress("a\n\n\n\nb"), "a\n\nb");
    }

    #[test]
    fn dedupes_identical_consecutive_lines() {
        assert_eq!(compress("same\nsame\nsame\nend"), "same\n  (x3)\nend");
    }

    #[test]
    fn trims_trailing_whitespace() {
        assert_eq!(compress("line   \nnext"), "line\nnext");
    }
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test heuristic`
Expected: FAIL (stub returns input unchanged).

- [ ] **Step 3: Implement**

Replace the `compress` stub:

```rust
use regex::Regex;
use std::sync::OnceLock;

/// Lossy compression for unstructured output: strip ANSI, trim trailing
/// whitespace, collapse blank runs, collapse repeated identical lines.
pub fn compress(text: &str) -> String {
    static ANSI: OnceLock<Regex> = OnceLock::new();
    let ansi = ANSI.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap());
    let stripped = ansi.replace_all(text, "");

    let mut out: Vec<String> = Vec::new();
    let mut prev: Option<String> = None;
    let mut repeat = 0usize;
    let mut blank_run = 0usize;
    for raw in stripped.lines() {
        let line = raw.trim_end().to_string();
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        if prev.as_deref() == Some(line.as_str()) {
            repeat += 1;
            continue;
        }
        if repeat > 0 {
            out.push(format!("  (x{})", repeat + 1));
            repeat = 0;
        }
        out.push(line.clone());
        prev = Some(line);
    }
    if repeat > 0 {
        out.push(format!("  (x{})", repeat + 1));
    }
    out.join("\n")
}
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo test heuristic`
Expected: 4 passed.

- [ ] **Step 5: E2E check of the flag**

Add to `tests/e2e_wrap.rs`:
```rust
#[test]
fn heuristic_flag_compresses_repeats() {
    cartoon()
        .args(["--heuristic", "sh", "-c", "for i in 1 2 3 4 5; do echo tick; done"])
        .assert()
        .success()
        .stdout(contains("(x5)"));
}
```

Run: `cargo test`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add src/heuristic.rs tests/e2e_wrap.rs
git commit -m "feat: heuristic lossy compressor behind --heuristic flag"
```

---

### Task 9: Config, paths, stats, subcommands

**Files:**
- Create: `src/paths.rs`, `src/config.rs`, `src/stats.rs`
- Modify: `src/lib.rs`, `src/app.rs`, `src/main.rs`, `tests/e2e_wrap.rs`

- [ ] **Step 1: Implement src/paths.rs** (small pure helpers; covered via the E2E test in Step 7)

```rust
use std::path::PathBuf;

pub fn config_file() -> Option<PathBuf> {
    base("XDG_CONFIG_HOME", ".config").map(|d| d.join("cartoon/config.toml"))
}

pub fn stats_file() -> Option<PathBuf> {
    base("XDG_STATE_HOME", ".local/state").map(|d| d.join("cartoon/stats.jsonl"))
}

fn base(env: &str, fallback: &str) -> Option<PathBuf> {
    if let Ok(v) = std::env::var(env) {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    dirs::home_dir().map(|h| h.join(fallback))
}
```

(Deliberately XDG-style on ALL platforms incl. macOS — CLI convention, matches spec's `~/.config` / `~/.local/state` paths.)

- [ ] **Step 2: Write failing config tests** (bottom of new `src/config.rs`; add `pub mod config;` + `pub mod paths;` to lib.rs)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = Config::default();
        assert!(!c.heuristic);
        assert_eq!(c.tokenizer, "o200k");
        assert_eq!(c.trace_lines, 20);
    }

    #[test]
    fn partial_toml_overrides() {
        let c: Config = toml::from_str("heuristic = true").unwrap();
        assert!(c.heuristic);
        assert_eq!(c.tokenizer, "o200k");
    }

    #[test]
    fn bad_toml_falls_back_to_defaults() {
        let c = parse_or_default("not [ valid toml", "/tmp/x");
        assert!(!c.heuristic);
    }
}
```

- [ ] **Step 3: Implement src/config.rs**

```rust
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub heuristic: bool,
    pub tokenizer: String,
    pub trace_lines: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self { heuristic: false, tokenizer: "o200k".into(), trace_lines: 20 }
    }
}

pub fn load() -> Config {
    let Some(path) = crate::paths::config_file() else {
        return Config::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => parse_or_default(&s, &path.display().to_string()),
        Err(_) => Config::default(), // no config file is normal
    }
}

fn parse_or_default(s: &str, path: &str) -> Config {
    toml::from_str(s).unwrap_or_else(|e| {
        eprintln!("cartoon: invalid config {path}: {e}; using defaults");
        Config::default()
    })
}
```

Run: `cargo test config`
Expected: 3 passed.

- [ ] **Step 4: Write failing stats tests** (bottom of new `src/stats.rs`; add `pub mod stats;` to lib.rs)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approx_estimate_is_quarter_of_bytes() {
        assert_eq!(estimate_tokens("abcdefgh", "approx"), 2);
    }

    #[test]
    fn o200k_estimate_counts_real_tokens() {
        let n = estimate_tokens("the quick brown fox jumps over the lazy dog", "o200k");
        assert!(n >= 5 && n <= 15, "got {n}");
    }

    #[test]
    fn since_parses_units() {
        assert_eq!(parse_since("7d").unwrap(), chrono::Duration::days(7));
        assert_eq!(parse_since("24h").unwrap(), chrono::Duration::hours(24));
        assert_eq!(parse_since("30m").unwrap(), chrono::Duration::minutes(30));
        assert!(parse_since("7x").is_err());
    }

    #[test]
    fn aggregate_sums_and_groups() {
        let recs = vec![
            StatRecord { ts: "2026-06-09T10:00:00Z".into(), cmd: "pytest".into(), adapter: "pytest".into(), tokens_in: 100, tokens_out: 10, saved: 90, exit: 0 },
            StatRecord { ts: "2026-06-09T11:00:00Z".into(), cmd: "ls".into(), adapter: "passthrough".into(), tokens_in: 5, tokens_out: 5, saved: 0, exit: 0 },
        ];
        let v = aggregate(&recs);
        assert_eq!(v["calls"], 2);
        assert_eq!(v["tokens_saved"], 90);
        assert_eq!(v["by_adapter"]["pytest"]["saved"], 90);
    }
}
```

- [ ] **Step 5: Implement src/stats.rs**

```rust
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Debug, Serialize, Deserialize)]
pub struct StatRecord {
    pub ts: String,
    pub cmd: String,
    pub adapter: String,
    pub tokens_in: usize,
    pub tokens_out: usize,
    pub saved: i64,
    pub exit: i32,
}

pub fn estimate_tokens(text: &str, tokenizer: &str) -> usize {
    match tokenizer {
        "approx" => text.len() / 4,
        _ => {
            use std::sync::OnceLock;
            static BPE: OnceLock<tiktoken_rs::CoreBPE> = OnceLock::new();
            BPE.get_or_init(|| tiktoken_rs::o200k_base().expect("bundled tokenizer"))
                .encode_with_special_tokens(text)
                .len()
        }
    }
}

/// Build + append a record. Failures are swallowed: stats must never break a call.
pub fn record_call(
    argv: &[String],
    adapter: &str,
    original: &str,
    emitted: &str,
    exit: i32,
    tokenizer: &str,
) {
    let tokens_in = estimate_tokens(original, tokenizer);
    let tokens_out = estimate_tokens(emitted, tokenizer);
    let rec = StatRecord {
        ts: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        cmd: argv.first().cloned().unwrap_or_default(),
        adapter: adapter.to_string(),
        tokens_in,
        tokens_out,
        saved: tokens_in as i64 - tokens_out as i64,
        exit,
    };
    let Some(path) = crate::paths::stats_file() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    if let Ok(line) = serde_json::to_string(&rec) {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

pub fn parse_since(s: &str) -> Result<Duration> {
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = num.parse().context("--since wants <number><d|h|m>, e.g. 7d")?;
    match unit {
        "d" => Ok(Duration::days(n)),
        "h" => Ok(Duration::hours(n)),
        "m" => Ok(Duration::minutes(n)),
        _ => anyhow::bail!("--since wants <number><d|h|m>, e.g. 7d"),
    }
}

pub fn aggregate(recs: &[StatRecord]) -> Value {
    let mut by_adapter: Map<String, Value> = Map::new();
    let mut total_saved = 0i64;
    for r in recs {
        total_saved += r.saved;
        let entry = by_adapter
            .entry(r.adapter.clone())
            .or_insert_with(|| json!({"calls": 0, "saved": 0}));
        entry["calls"] = json!(entry["calls"].as_i64().unwrap() + 1);
        entry["saved"] = json!(entry["saved"].as_i64().unwrap() + r.saved);
    }
    json!({
        "calls": recs.len(),
        "tokens_saved": total_saved,
        "by_adapter": Value::Object(by_adapter),
    })
}

/// The `cartoon stats` report — output is itself TOON (dogfooding).
pub fn report(since: Option<&str>) -> Result<String> {
    let cutoff: Option<DateTime<Utc>> = match since {
        Some(s) => Some(Utc::now() - parse_since(s)?),
        None => None,
    };
    let Some(path) = crate::paths::stats_file() else {
        return Ok("calls: 0".into());
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let recs: Vec<StatRecord> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .filter(|r: &StatRecord| match cutoff {
            None => true,
            Some(c) => DateTime::parse_from_rfc3339(&r.ts)
                .map(|t| t.with_timezone(&Utc) >= c)
                .unwrap_or(false),
        })
        .collect();
    Ok(crate::toon::encode(&aggregate(&recs)))
}
```

Run: `cargo test stats`
Expected: 4 passed.

- [ ] **Step 6: Wire stats + config into app and main**

`src/app.rs` — change `run_wrap` to take config and record stats:

```rust
use crate::{config::Config, fallback, heuristic, runner, stats, toon};
use anyhow::Result;

pub fn run_wrap(argv: &[String], heuristic_on: bool, raw: bool, cfg: &Config) -> Result<i32> {
    let captured = match runner::run(argv) {
        Ok(c) => c,
        Err(e) => {
            let not_found = e
                .downcast_ref::<std::io::Error>()
                .map(|io| io.kind() == std::io::ErrorKind::NotFound)
                .unwrap_or(false);
            if not_found {
                eprintln!("cartoon: command not found: {}", argv[0]);
                return Ok(127);
            }
            return Err(e);
        }
    };
    let code = runner::exit_code(&captured.status);
    if raw {
        print!("{}", captured.stdout);
        eprint!("{}", captured.stderr);
        return Ok(code);
    }
    let (out, mode) = transform(&captured.stdout, heuristic_on);
    print!("{out}");
    if !out.is_empty() && !out.ends_with('\n') {
        println!();
    }
    eprint!("{}", captured.stderr);
    let original = format!("{}{}", captured.stdout, captured.stderr);
    let emitted = format!("{}{}", out, captured.stderr);
    stats::record_call(argv, mode, &original, &emitted, code, &cfg.tokenizer);
    Ok(code)
}
```

(`transform` unchanged.)

`src/main.rs` — final version:

```rust
use clap::Parser;

fn main() {
    let cli = cartoon::cli::Cli::parse();
    let code = match cartoon::cli::parse_mode(cli) {
        Ok(cartoon::cli::Mode::Wrap { argv, heuristic, raw }) => {
            let cfg = cartoon::config::load();
            let heuristic_on = heuristic || cfg.heuristic;
            cartoon::app::run_wrap(&argv, heuristic_on, raw, &cfg).unwrap_or_else(|e| {
                eprintln!("cartoon: {e}");
                2
            })
        }
        Ok(cartoon::cli::Mode::Stats { since }) => {
            match cartoon::stats::report(since.as_deref()) {
                Ok(s) => {
                    println!("{s}");
                    0
                }
                Err(e) => {
                    eprintln!("cartoon: {e}");
                    2
                }
            }
        }
        Ok(cartoon::cli::Mode::Adapters) => {
            println!("(no adapters yet)");
            0
        }
        Err(e) => {
            eprintln!("cartoon: {e}");
            2
        }
    };
    std::process::exit(code);
}
```

- [ ] **Step 7: E2E for stats (isolated from real home via XDG override)**

Add to `tests/e2e_wrap.rs`:

```rust
#[test]
fn stats_records_and_reports() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().to_str().unwrap();
    cartoon()
        .env("XDG_STATE_HOME", state)
        .args(["sh", "-c", r#"echo '{"a": 1}'"#])
        .assert()
        .success();
    cartoon()
        .env("XDG_STATE_HOME", state)
        .args(["stats"])
        .assert()
        .success()
        .stdout(contains("calls: 1"));
}
```

- [ ] **Step 8: Run everything**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: green. Fix clippy/fmt findings before committing.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat: config, XDG paths, token-savings stats with TOON report"
```

---

**Plan 1 exit criteria:** `cargo install --path .` then `cartoon sh -c 'echo {"a":1}'` prints `a: 1`; `cartoon stats` shows the call; plain commands pass through with exit codes intact.
