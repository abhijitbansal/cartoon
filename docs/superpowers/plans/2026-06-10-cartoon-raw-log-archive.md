# cartoon Raw Log Archive Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every wrapped run archives its raw stdout/stderr + metadata under `~/.local/state/cartoon/runs/<run-id>/`; transformed TOON output ends with a `raw_log:` footer pointing at the archive; `cartoon logs` lists and retrieves archives; capped retention.

**Architecture:** New `src/archive.rs` (storage: record/list/load/prune, internal `_at` functions take an explicit root so unit tests never touch env vars) + new `src/logs_cmd.rs` (presentation for the `logs` subcommand). `app.rs` records after transform and appends the footer only when output was already transformed (passthrough/`--raw` stay byte-identical). Stats records gain a nullable `run_id`. Spec: `docs/superpowers/specs/2026-06-10-cartoon-raw-log-archive-design.md`.

**Tech Stack:** Existing deps only (serde/serde_json, chrono, tempfile, anyhow). No new crates — run-id entropy from time + pid.

**Execution context:** repo `/Users/abhijitbansal/projects/cartoon`, branch main, 79 tests green at start. Every cargo command needs `export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"`. Run `cargo fmt` before every commit.

---

### Task 1: Config additions (keep_runs, max_archive_mb)

**Files:**
- Modify: `src/config.rs`

- [ ] **Step 1: Add failing tests** to the tests module in `src/config.rs`:

```rust
    #[test]
    fn archive_defaults() {
        let c = Config::default();
        assert_eq!(c.keep_runs, 50);
        assert_eq!(c.max_archive_mb, 50);
    }

    #[test]
    fn archive_keys_override() {
        let c: Config = toml::from_str("keep_runs = 5\nmax_archive_mb = 10").unwrap();
        assert_eq!(c.keep_runs, 5);
        assert_eq!(c.max_archive_mb, 10);
        assert_eq!(c.tokenizer, "o200k"); // other defaults intact
    }
```

- [ ] **Step 2: Run `cargo test config`** — expect compile FAIL (fields missing).

- [ ] **Step 3: Implement** — add two fields to `Config` and its `Default`:

```rust
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub heuristic: bool,
    pub tokenizer: String,
    pub trace_lines: usize,
    pub keep_runs: usize,
    pub max_archive_mb: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            heuristic: false,
            tokenizer: "o200k".into(),
            trace_lines: 20,
            keep_runs: 50,
            max_archive_mb: 50,
        }
    }
}
```

- [ ] **Step 4: Run `cargo test config`** — expect 5 passed (3 existing + 2 new).

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat: keep_runs and max_archive_mb config keys"
```

---

### Task 2: Archive core — record, load, list

**Files:**
- Create: `src/archive.rs`
- Modify: `src/paths.rs`, `src/lib.rs`

- [ ] **Step 1: Add `runs_dir` to `src/paths.rs`:**

```rust
pub fn runs_dir() -> Option<PathBuf> {
    base("XDG_STATE_HOME", ".local/state").map(|d| d.join("cartoon/runs"))
}
```

- [ ] **Step 2: Write failing tests.** Create `src/archive.rs` containing ONLY this tests module (add `pub mod archive;` to `src/lib.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::Captured;

    fn captured(stdout: &str, stderr: &str) -> Captured {
        use std::process::Command;
        let status = Command::new("true").status().unwrap();
        Captured { stdout: stdout.into(), stderr: stderr.into(), status }
    }

    fn cfg() -> crate::config::Config {
        crate::config::Config::default()
    }

    #[test]
    fn run_ids_are_time_ordered_and_unique() {
        let a = new_run_id();
        let b = new_run_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), "20260610-051203-ab12".len());
        assert!(a <= b, "lexicographic order must follow time: {a} vs {b}");
    }

    #[test]
    fn record_then_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let argv = vec!["pytest".to_string(), "-q".to_string()];
        let cap = captured("OUT bytes\n", "ERR bytes\n");
        let tags = vec!["api".to_string(), "ci".to_string()];
        let run = record_at(tmp.path(), &argv, "pytest", &cap, 1, &tags, &cfg()).unwrap();

        let (meta, out, err) = load_at(tmp.path(), &run.id).unwrap();
        assert_eq!(meta.id, run.id);
        assert_eq!(meta.argv, argv);
        assert_eq!(meta.mode, "pytest");
        assert_eq!(meta.exit, 1);
        assert_eq!(meta.tags, tags);
        assert_eq!(meta.stdout_bytes, 10);
        assert_eq!(meta.stderr_bytes, 10);
        assert_eq!(out, "OUT bytes\n");
        assert_eq!(err, "ERR bytes\n");
        assert!(run.dir.join("meta.json").exists());
    }

    #[test]
    fn list_is_newest_first_and_tag_filtered() {
        let tmp = tempfile::tempdir().unwrap();
        let cap = captured("x", "");
        record_at(tmp.path(), &["a".into()], "passthrough", &cap, 0, &[], &cfg()).unwrap();
        record_at(tmp.path(), &["b".into()], "json", &cap, 0, &["t1".into()], &cfg()).unwrap();

        let all = list_at(tmp.path(), None);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].argv[0], "b", "newest first");

        let tagged = list_at(tmp.path(), Some("t1"));
        assert_eq!(tagged.len(), 1);
        assert_eq!(tagged[0].argv[0], "b");
    }

    #[test]
    fn load_unknown_id_errors() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_at(tmp.path(), "20990101-000000-dead").is_err());
    }

    #[test]
    fn corrupt_meta_is_skipped_in_list() {
        let tmp = tempfile::tempdir().unwrap();
        let cap = captured("x", "");
        record_at(tmp.path(), &["ok".into()], "json", &cap, 0, &[], &cfg()).unwrap();
        let bad = tmp.path().join("20000101-000000-beef");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("meta.json"), "not json").unwrap();
        let all = list_at(tmp.path(), None);
        assert_eq!(all.len(), 1);
    }
}
```

- [ ] **Step 3: Run `cargo test archive`** — expect compile FAIL.

- [ ] **Step 4: Implement** above the tests in `src/archive.rs`:

```rust
use crate::config::Config;
use crate::runner::Captured;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub struct RunRef {
    pub id: String,
    pub dir: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RunMeta {
    pub id: String,
    pub ts: String,
    pub argv: Vec<String>,
    pub mode: String,
    pub exit: i32,
    pub cwd: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
}

/// `YYYYMMDD-HHMMSS-<4 hex>` UTC; lexicographic order == time order.
/// Suffix entropy from subsecond nanos + pid (no rand dependency).
pub fn new_run_id() -> String {
    let now = chrono::Utc::now();
    let nanos = now.timestamp_subsec_nanos() as u64;
    let salt = (nanos ^ (std::process::id() as u64)) & 0xffff;
    format!("{}-{:04x}", now.format("%Y%m%d-%H%M%S"), salt)
}

/// Public wrapper: archive under the XDG runs dir. Failures swallowed → None.
pub fn record(
    argv: &[String],
    mode: &str,
    captured: &Captured,
    exit: i32,
    tags: &[String],
    cfg: &Config,
) -> Option<RunRef> {
    let root = crate::paths::runs_dir()?;
    record_at(&root, argv, mode, captured, exit, tags, cfg)
}

pub fn list(tag: Option<&str>) -> Vec<RunMeta> {
    match crate::paths::runs_dir() {
        Some(root) => list_at(&root, tag),
        None => Vec::new(),
    }
}

pub fn load(id: &str) -> Result<(RunMeta, String, String)> {
    let root = crate::paths::runs_dir().context("no state directory")?;
    load_at(&root, id)
}

/// Newest run id, if any.
pub fn last_id() -> Option<String> {
    list(None).into_iter().next().map(|m| m.id)
}

pub fn record_at(
    root: &Path,
    argv: &[String],
    mode: &str,
    captured: &Captured,
    exit: i32,
    tags: &[String],
    cfg: &Config,
) -> Option<RunRef> {
    if cfg.keep_runs == 0 {
        return None; // archiving disabled
    }
    let id = new_run_id();
    let dir = root.join(&id);
    let meta = RunMeta {
        id: id.clone(),
        ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        argv: argv.to_vec(),
        mode: mode.to_string(),
        exit,
        cwd: std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        tags: tags.to_vec(),
        stdout_bytes: captured.stdout.len() as u64,
        stderr_bytes: captured.stderr.len() as u64,
    };
    let write_all = || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("stdout.log"), &captured.stdout)?;
        std::fs::write(dir.join("stderr.log"), &captured.stderr)?;
        let json = serde_json::to_string_pretty(&meta)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(dir.join("meta.json"), json)?;
        Ok(())
    };
    if write_all().is_err() {
        // Partial write: best-effort cleanup, then report failure.
        let _ = std::fs::remove_dir_all(&dir);
        return None;
    }
    prune_at(root, cfg);
    Some(RunRef { id, dir })
}

pub fn list_at(root: &Path, tag: Option<&str>) -> Vec<RunMeta> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut metas: Vec<RunMeta> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path().join("meta.json")).ok()?;
            match serde_json::from_str::<RunMeta>(&text) {
                Ok(m) => Some(m),
                Err(_) => {
                    eprintln!(
                        "cartoon: skipping corrupt archive entry {}",
                        e.path().display()
                    );
                    None
                }
            }
        })
        .filter(|m| match tag {
            Some(t) => m.tags.iter().any(|x| x == t),
            None => true,
        })
        .collect();
    metas.sort_by(|a, b| b.id.cmp(&a.id)); // newest first
    metas
}

pub fn load_at(root: &Path, id: &str) -> Result<(RunMeta, String, String)> {
    let dir = root.join(id);
    let meta: RunMeta = serde_json::from_str(
        &std::fs::read_to_string(dir.join("meta.json"))
            .with_context(|| format!("no archived run {id} — try `cartoon logs`"))?,
    )
    .with_context(|| format!("corrupt meta for run {id}"))?;
    let stdout = std::fs::read_to_string(dir.join("stdout.log")).unwrap_or_default();
    let stderr = std::fs::read_to_string(dir.join("stderr.log")).unwrap_or_default();
    Ok((meta, stdout, stderr))
}

fn prune_at(_root: &Path, _cfg: &Config) {
    // implemented in the pruning task
}
```

- [ ] **Step 5: Run `cargo test archive`** — expect 5 passed. Then full `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`.

- [ ] **Step 6: Commit**

```bash
git add src/archive.rs src/paths.rs src/lib.rs
git commit -m "feat: archive module — record, load, list raw run logs"
```

---

### Task 3: Retention pruning

**Files:**
- Modify: `src/archive.rs`

- [ ] **Step 1: Add failing tests** to the archive tests module:

```rust
    #[test]
    fn prunes_beyond_keep_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let cap = captured("x", "");
        let mut small = cfg();
        small.keep_runs = 2;
        for name in ["a", "b", "c"] {
            record_at(tmp.path(), &[name.to_string()], "json", &cap, 0, &[], &small).unwrap();
        }
        let all = list_at(tmp.path(), None);
        assert_eq!(all.len(), 2, "oldest pruned");
        assert_eq!(all[1].argv[0], "b", "a was deleted");
    }

    #[test]
    fn prunes_beyond_max_size() {
        let tmp = tempfile::tempdir().unwrap();
        let big = "x".repeat(1024 * 1024); // 1 MiB stdout per run
        let cap = captured(&big, "");
        let mut small = cfg();
        small.keep_runs = 100;
        small.max_archive_mb = 2;
        for name in ["a", "b", "c"] {
            record_at(tmp.path(), &[name.to_string()], "json", &cap, 0, &[], &small).unwrap();
        }
        let all = list_at(tmp.path(), None);
        assert!(all.len() <= 2, "size cap enforced, got {}", all.len());
    }

    #[test]
    fn keep_runs_zero_disables_archiving() {
        let tmp = tempfile::tempdir().unwrap();
        let cap = captured("x", "");
        let mut off = cfg();
        off.keep_runs = 0;
        assert!(record_at(tmp.path(), &["a".into()], "json", &cap, 0, &[], &off).is_none());
        assert!(list_at(tmp.path(), None).is_empty());
    }
```

- [ ] **Step 2: Run `cargo test archive`** — `keep_runs_zero` passes already (guard exists); the two prune tests FAIL.

- [ ] **Step 3: Implement** — replace the `prune_at` stub:

```rust
/// Delete oldest runs while count > keep_runs OR total bytes > max_archive_mb.
/// Errors ignored: deletion is idempotent and retried implicitly next run.
fn prune_at(root: &Path, cfg: &Config) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort(); // run-ids sort oldest-first lexicographically

    let dir_size = |d: &Path| -> u64 {
        std::fs::read_dir(d)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| e.metadata().ok())
                    .map(|m| m.len())
                    .sum()
            })
            .unwrap_or(0)
    };
    let mut sizes: Vec<u64> = dirs.iter().map(|d| dir_size(d)).collect();
    let mut total: u64 = sizes.iter().sum();
    let max_bytes = cfg.max_archive_mb * 1024 * 1024;

    let mut i = 0;
    while i < dirs.len()
        && (dirs.len() - i > cfg.keep_runs || total > max_bytes)
    {
        let _ = std::fs::remove_dir_all(&dirs[i]);
        total = total.saturating_sub(sizes[i]);
        sizes[i] = 0;
        i += 1;
    }
}
```

- [ ] **Step 4: Run `cargo test archive`** — 8 passed. Full `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`.

- [ ] **Step 5: Commit**

```bash
git add src/archive.rs
git commit -m "feat: archive retention pruning by count and total size"
```

---

### Task 4: CLI — --tag flag and logs subcommand parsing

**Files:**
- Modify: `src/cli.rs`, `src/main.rs`

- [ ] **Step 1: Add failing tests** to the cli tests module:

```rust
    #[test]
    fn tag_flags_collect_into_wrap_mode() {
        let m = mode(&["cartoon", "--tag", "api", "--tag", "ci", "pytest"]);
        assert_eq!(
            m,
            Mode::Wrap {
                argv: vec!["pytest".into()],
                heuristic: false,
                raw: false,
                tags: vec!["api".into(), "ci".into()]
            }
        );
    }

    #[test]
    fn logs_bare_lists() {
        assert_eq!(
            mode(&["cartoon", "logs"]),
            Mode::Logs(LogsQuery::List { tag: None })
        );
    }

    #[test]
    fn logs_tag_filter() {
        assert_eq!(
            mode(&["cartoon", "logs", "--tag", "api"]),
            Mode::Logs(LogsQuery::List { tag: Some("api".into()) })
        );
    }

    #[test]
    fn logs_by_id_with_stream() {
        assert_eq!(
            mode(&["cartoon", "logs", "20260610-051203-ab12", "--stdout"]),
            Mode::Logs(LogsQuery::Show {
                sel: RunSel::Id("20260610-051203-ab12".into()),
                stream: StreamSel::Stdout
            })
        );
    }

    #[test]
    fn logs_last_both_streams() {
        assert_eq!(
            mode(&["cartoon", "logs", "--last"]),
            Mode::Logs(LogsQuery::Show { sel: RunSel::Last, stream: StreamSel::Both })
        );
    }

    #[test]
    fn logs_bad_args_error() {
        assert!(parse_mode(Cli::parse_from(["cartoon", "logs", "--nope"])).is_err());
        assert!(parse_mode(Cli::parse_from(["cartoon", "logs", "id1", "id2"])).is_err());
    }
```

NOTE: existing tests construct `Mode::Wrap { .. }` without `tags` — update `wrap_mode_passes_args_verbatim` to include `tags: vec![]`.

- [ ] **Step 2: Run `cargo test cli`** — expect compile FAIL.

- [ ] **Step 3: Implement.** In `src/cli.rs`:

Add to the `Cli` struct (after `raw`):

```rust
    /// Tag this run in the raw-log archive (repeatable)
    #[arg(long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,
```

Update after_help to:

```rust
    after_help = "Subcommands `stats`, `adapters`, and `logs` are reserved words; \
to wrap a binary literally named `stats`, use: cartoon env stats"
```

Replace/extend the mode types and parser:

```rust
#[derive(Debug, PartialEq)]
pub enum Mode {
    Wrap { argv: Vec<String>, heuristic: bool, raw: bool, tags: Vec<String> },
    Stats { since: Option<String> },
    Logs(LogsQuery),
    Adapters,
}

#[derive(Debug, PartialEq)]
pub enum LogsQuery {
    List { tag: Option<String> },
    Show { sel: RunSel, stream: StreamSel },
}

#[derive(Debug, PartialEq)]
pub enum RunSel {
    Id(String),
    Last,
}

#[derive(Debug, PartialEq)]
pub enum StreamSel {
    Both,
    Stdout,
    Stderr,
}

pub fn parse_mode(cli: Cli) -> anyhow::Result<Mode> {
    if cli.command.is_empty() {
        anyhow::bail!("no command given. usage: cartoon <cmd> [args...]");
    }
    match cli.command[0].as_str() {
        "stats" => Ok(Mode::Stats { since: parse_since(&cli.command[1..])? }),
        "adapters" => Ok(Mode::Adapters),
        "logs" => Ok(Mode::Logs(parse_logs(&cli.command[1..])?)),
        _ => Ok(Mode::Wrap {
            argv: cli.command,
            heuristic: cli.heuristic,
            raw: cli.raw,
            tags: cli.tags,
        }),
    }
}

fn parse_logs(args: &[String]) -> anyhow::Result<LogsQuery> {
    const USAGE: &str =
        "usage: cartoon logs [--tag <t>] | cartoon logs (<id> | --last) [--stdout | --stderr]";
    let mut sel: Option<RunSel> = None;
    let mut stream = StreamSel::Both;
    let mut tag: Option<String> = None;
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--last" if sel.is_none() => sel = Some(RunSel::Last),
            "--stdout" if stream == StreamSel::Both => stream = StreamSel::Stdout,
            "--stderr" if stream == StreamSel::Both => stream = StreamSel::Stderr,
            "--tag" => {
                let v = it.next().ok_or_else(|| anyhow::anyhow!(USAGE))?;
                tag = Some(v.clone());
            }
            s if !s.starts_with('-') && sel.is_none() => sel = Some(RunSel::Id(s.to_string())),
            _ => anyhow::bail!(USAGE),
        }
    }
    match (sel, tag) {
        (None, t) if stream == StreamSel::Both => Ok(LogsQuery::List { tag: t }),
        (Some(sel), None) => Ok(LogsQuery::Show { sel, stream }),
        _ => anyhow::bail!(USAGE),
    }
}
```

- [ ] **Step 4: Fix the compile sites that destructure `Mode::Wrap`.** `src/main.rs` won't compile until Task 6 rewires `run_wrap`'s signature, so for THIS task only: bind and discard the new field — `Ok(cartoon::cli::Mode::Wrap { argv, heuristic, raw, tags: _tags })` — and add a temporary arm:

```rust
        Ok(cartoon::cli::Mode::Logs(_)) => {
            println!("(logs not wired yet)");
            0
        }
```

- [ ] **Step 5: Run `cargo test cli`** — 13 passed (7 existing incl. updated wrap test + 6 new). Full `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`.

- [ ] **Step 6: Commit**

```bash
git add src/cli.rs src/main.rs
git commit -m "feat: --tag flag and logs subcommand parsing"
```

---

### Task 5: logs subcommand rendering

**Files:**
- Create: `src/logs_cmd.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing tests.** Create `src/logs_cmd.rs` with ONLY this tests module (add `pub mod logs_cmd;` to lib.rs):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::RunMeta;

    fn meta(id: &str, cmd: &str, tags: &[&str]) -> RunMeta {
        RunMeta {
            id: id.into(),
            ts: "2026-06-10T05:12:03Z".into(),
            argv: vec![cmd.into(), "-q".into()],
            mode: "pytest".into(),
            exit: 1,
            cwd: "/proj".into(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            stdout_bytes: 10,
            stderr_bytes: 0,
        }
    }

    #[test]
    fn list_renders_tabular_toon() {
        let out = render_list(&[meta("20260610-051203-ab12", "pytest", &["api", "ci"])]);
        assert!(out.contains("runs[1]{id,ts,cmd,mode,exit,tags}:"), "got:\n{out}");
        assert!(out.contains("20260610-051203-ab12"));
        assert!(out.contains("\"api,ci\""));
    }

    #[test]
    fn empty_list_renders_zero() {
        assert_eq!(render_list(&[]), "runs[0]:");
    }

    #[test]
    fn show_both_streams_has_sections() {
        let out = render_show(
            &meta("id1", "pytest", &[]),
            "RAW OUT",
            "RAW ERR",
            &crate::cli::StreamSel::Both,
        );
        assert!(out.contains("id: id1"));
        assert!(out.contains("--- stdout ---\nRAW OUT"));
        assert!(out.contains("--- stderr ---\nRAW ERR"));
    }

    #[test]
    fn show_single_stream_is_raw_only() {
        let out = render_show(
            &meta("id1", "pytest", &[]),
            "RAW OUT",
            "RAW ERR",
            &crate::cli::StreamSel::Stdout,
        );
        assert_eq!(out, "RAW OUT");
    }
}
```

- [ ] **Step 2: Run `cargo test logs_cmd`** — expect compile FAIL.

- [ ] **Step 3: Implement** above the tests:

```rust
use crate::archive::{self, RunMeta};
use crate::cli::{LogsQuery, RunSel, StreamSel};
use anyhow::Result;
use serde_json::json;

/// Entry point for `cartoon logs ...`. Returns the process exit code.
pub fn run(query: LogsQuery) -> Result<i32> {
    match query {
        LogsQuery::List { tag } => {
            println!("{}", render_list(&archive::list(tag.as_deref())));
            Ok(0)
        }
        LogsQuery::Show { sel, stream } => {
            let id = match sel {
                RunSel::Id(id) => id,
                RunSel::Last => archive::last_id()
                    .ok_or_else(|| anyhow::anyhow!("no archived runs yet"))?,
            };
            let (meta, stdout, stderr) = archive::load(&id)?;
            println!("{}", render_show(&meta, &stdout, &stderr, &stream));
            Ok(0)
        }
    }
}

pub fn render_list(metas: &[RunMeta]) -> String {
    let rows: Vec<serde_json::Value> = metas
        .iter()
        .map(|m| {
            json!({
                "id": m.id,
                "ts": m.ts,
                "cmd": m.argv.first().cloned().unwrap_or_default(),
                "mode": m.mode,
                "exit": m.exit,
                "tags": m.tags.join(","),
            })
        })
        .collect();
    crate::toon::encode(&json!({ "runs": rows }))
}

pub fn render_show(meta: &RunMeta, stdout: &str, stderr: &str, stream: &StreamSel) -> String {
    match stream {
        StreamSel::Stdout => stdout.to_string(),
        StreamSel::Stderr => stderr.to_string(),
        StreamSel::Both => {
            let head = crate::toon::encode(&json!({
                "id": meta.id,
                "ts": meta.ts,
                "cmd": meta.argv.join(" "),
                "mode": meta.mode,
                "exit": meta.exit,
                "tags": meta.tags.join(","),
            }));
            format!("{head}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}")
        }
    }
}
```

- [ ] **Step 4: Run `cargo test logs_cmd`** — 4 passed. Full suite + clippy + fmt.

- [ ] **Step 5: Commit**

```bash
git add src/logs_cmd.rs src/lib.rs
git commit -m "feat: logs subcommand rendering (TOON list, raw show)"
```

---

### Task 6: Pipeline wiring — archive, footer, stats run_id, E2E

**Files:**
- Modify: `src/app.rs`, `src/main.rs`, `src/stats.rs`
- Create: `tests/e2e_archive.rs`

- [ ] **Step 1: stats run_id field.** In `src/stats.rs`, add to `StatRecord`:

```rust
    #[serde(default)]
    pub run_id: Option<String>,
```

Change `record_call` to accept a new last parameter `run_id: Option<&str>` and set the field with `run_id.map(String::from)`. Update the two `StatRecord` literals in the stats tests to include `run_id: None`.

- [ ] **Step 2: app.rs wiring.** Replace `run_wrap` and `run_with_adapter` in `src/app.rs`:

```rust
use crate::adapters::{self, ParseOutcome};
use crate::{archive, config::Config, fallback, heuristic, runner, stats, toon};
use anyhow::Result;
use serde_json::json;

pub fn run_wrap(
    argv: &[String],
    heuristic_on: bool,
    raw: bool,
    tags: &[String],
    cfg: &Config,
) -> Result<i32> {
    // Adapter path: detect first, because prepare() must extend argv.
    if !raw {
        if let Some(adapter) = adapters::find_adapter(argv) {
            return run_with_adapter(adapter.as_ref(), argv, tags, cfg);
        }
    }
    let captured = match runner::run(argv) {
        Ok(c) => c,
        Err(e) => return not_found_or_err(e, argv),
    };
    let code = runner::exit_code(&captured.status);
    if raw {
        // Escape hatch: byte-identical output, no footer, no stats — but archived.
        archive::record(argv, "raw", &captured, code, tags, cfg);
        print!("{}", captured.stdout);
        eprint!("{}", captured.stderr);
        return Ok(code);
    }
    let (mut out, mode) = transform(&captured.stdout, heuristic_on);
    let run = archive::record(argv, mode, &captured, code, tags, cfg);
    if mode != "passthrough" {
        if let Some(r) = &run {
            out.push_str(&format!(
                "\n{}",
                toon::encode(&json!({ "raw_log": r.dir.display().to_string() }))
            ));
        }
    }
    emit(&out, &captured.stderr);
    let original = format!("{}{}", captured.stdout, captured.stderr);
    let emitted = format!("{}{}", out, captured.stderr);
    stats::record_call(
        argv,
        mode,
        &original,
        &emitted,
        code,
        &cfg.tokenizer,
        run.as_ref().map(|r| r.id.as_str()),
    );
    Ok(code)
}

fn run_with_adapter(
    adapter: &dyn adapters::Adapter,
    argv: &[String],
    tags: &[String],
    cfg: &Config,
) -> Result<i32> {
    let prepared = adapter.prepare(argv.to_vec());
    let captured = match runner::run(&prepared.argv) {
        Ok(c) => c,
        Err(e) => return not_found_or_err(e, argv),
    };
    let code = runner::exit_code(&captured.status);
    let run = archive::record(argv, adapter.name(), &captured, code, tags, cfg);
    match adapter.parse(&captured, &prepared) {
        Ok(ParseOutcome { report, passthrough_stdout, passthrough_stderr }) => {
            let mut out = adapters::report::render(&report, cfg.trace_lines);
            if let Some(r) = &run {
                out.push_str(&format!(
                    "\n{}",
                    toon::encode(&json!({ "raw_log": r.dir.display().to_string() }))
                ));
            }
            let extra_out = passthrough_stdout.unwrap_or_default();
            let extra_err = passthrough_stderr.unwrap_or_default();
            emit(&out, "");
            if !extra_out.is_empty() {
                print!("{extra_out}");
            }
            eprint!("{extra_err}");
            let original = format!("{}{}", captured.stdout, captured.stderr);
            let emitted = format!("{}{}{}", out, extra_out, extra_err);
            stats::record_call(
                argv,
                adapter.name(),
                &original,
                &emitted,
                code,
                &cfg.tokenizer,
                run.as_ref().map(|r| r.id.as_str()),
            );
            Ok(code)
        }
        Err(e) => {
            // Safety rule: never lose information. Emit original output, NO footer.
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
```

(`not_found_or_err`, `emit`, `transform` unchanged.)

- [ ] **Step 3: main.rs.** Update the Wrap arm to pass tags, and wire Logs:

```rust
        Ok(cartoon::cli::Mode::Wrap { argv, heuristic, raw, tags }) => {
            let cfg = cartoon::config::load();
            let heuristic_on = heuristic || cfg.heuristic;
            cartoon::app::run_wrap(&argv, heuristic_on, raw, &tags, &cfg).unwrap_or_else(|e| {
                eprintln!("cartoon: {e}");
                2
            })
        }
```

```rust
        Ok(cartoon::cli::Mode::Logs(query)) => {
            cartoon::logs_cmd::run(query).unwrap_or_else(|e| {
                eprintln!("cartoon: {e}");
                2
            })
        }
```

- [ ] **Step 4: Run `cargo test`** — fix any remaining compile fallout (the e2e suites don't construct Mode directly, so expect green). Expect ~89 tests at this point.

- [ ] **Step 5: E2E tests.** Create `tests/e2e_archive.rs`:

```rust
use assert_cmd::Command;
use predicates::str::contains;

fn cartoon() -> Command {
    Command::cargo_bin("cartoon").unwrap()
}

#[test]
fn transformed_run_gets_footer_and_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let assert = cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["--tag", "e2e", "sh", "-c", r#"echo '{"a": 1}'"#])
        .assert()
        .success()
        .stdout(contains("raw_log:"));
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let path = out
        .lines()
        .find(|l| l.starts_with("raw_log:"))
        .and_then(|l| l.split_once(' '))
        .map(|(_, p)| p.trim().trim_matches('"').to_string())
        .expect("footer path");
    let raw = std::fs::read_to_string(format!("{path}/stdout.log")).unwrap();
    assert_eq!(raw, "{\"a\": 1}\n", "archived stdout is the ORIGINAL json");
    let meta = std::fs::read_to_string(format!("{path}/meta.json")).unwrap();
    assert!(meta.contains("\"e2e\""), "tag recorded");
}

#[test]
fn passthrough_is_byte_identical_but_archived() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["sh", "-c", "echo plain"])
        .assert()
        .success()
        .stdout("plain\n"); // exact: no footer appended
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["logs"])
        .assert()
        .success()
        .stdout(contains("passthrough"));
}

#[test]
fn raw_mode_is_byte_identical_but_archived() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["--raw", "sh", "-c", r#"echo '{"a": 1}'"#])
        .assert()
        .success()
        .stdout("{\"a\": 1}\n");
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["logs"])
        .assert()
        .success()
        .stdout(contains(",raw,"));
}

#[test]
fn logs_last_stdout_returns_raw_stream() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["sh", "-c", r#"echo '{"a": 1}'"#])
        .assert()
        .success();
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["logs", "--last", "--stdout"])
        .assert()
        .success()
        .stdout(contains(r#"{"a": 1}"#));
}

#[test]
fn logs_unknown_id_exits_2() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["logs", "20990101-000000-dead"])
        .assert()
        .code(2);
}

#[test]
fn e2e_pytest_footer_points_at_original_report() {
    let have = std::process::Command::new("pytest")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !have {
        eprintln!("SKIP: pytest not installed");
        return;
    }
    let proj = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/e2e/pyproj");
    let tmp = tempfile::tempdir().unwrap();
    let assert = cartoon()
        .env("XDG_STATE_HOME", tmp.path())
        .args(["pytest", proj])
        .assert()
        .code(1)
        .stdout(contains("raw_log:"));
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    let path = out
        .lines()
        .find(|l| l.starts_with("raw_log:"))
        .and_then(|l| l.split_once(' '))
        .map(|(_, p)| p.trim().trim_matches('"').to_string())
        .unwrap();
    let raw = std::fs::read_to_string(format!("{path}/stdout.log")).unwrap();
    assert!(raw.contains("test_fail"), "original pytest report archived");
    assert!(
        raw.contains("short test summary") || raw.contains("FAILED"),
        "human report detail present: {raw}"
    );
}
```

- [ ] **Step 6: Run everything**

Run: `export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH" && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: ~95 tests green, real pytest E2E NOT skipped.

- [ ] **Step 7: Manual smoke** (include in report):

```bash
XDG_STATE_HOME=$(mktemp -d) sh -c '
  cargo run -q -- --tag demo pytest tests/fixtures/e2e/pyproj; echo "exit: $?";
  cargo run -q -- logs;
  cargo run -q -- logs --last --stdout | head -5'
```

Expected: TOON report ending in `raw_log:` line; exit 1; logs list shows the tagged run; raw pytest output prints.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: raw log archive with TOON footer and logs retrieval"
```

---

### Task 7: Docs

**Files:**
- Modify: `README.md`

- [ ] **Step 1: README updates.** In the Use section add:

```bash
cartoon --tag api pytest       # tag the archived run
cartoon logs                   # list archived raw logs
cartoon logs --last --stdout   # full raw output of the newest run
```

After the Guarantees section add:

```markdown
## Raw log archive

Every wrapped run keeps its full raw output under
`~/.local/state/cartoon/runs/<run-id>/` (`stdout.log`, `stderr.log`,
`meta.json`). Transformed output ends with a `raw_log:` line pointing at the
archive — if the TOON summary dropped something you need, fetch the original
with `cat` or `cartoon logs <id>` instead of rerunning. Passthrough and
`--raw` output stay byte-identical (no footer) but are still archived.
Retention is capped (`keep_runs`, default 50; `max_archive_mb`, default 50);
`keep_runs = 0` disables archiving.
```

Update the Config block to include the new keys:

```toml
heuristic = false    # default for lossy fallback
tokenizer = "o200k"  # or "approx" (bytes/4) for zero-cost estimates
trace_lines = 20     # per-failure traceback cap
keep_runs = 50       # archived raw logs to keep (0 disables)
max_archive_mb = 50  # max total archive size
```

- [ ] **Step 2: Verify + commit**

```bash
cargo test
git add README.md
git commit -m "docs: raw log archive usage and config"
git push
```

---

**Exit criteria (= spec success criteria):** `cartoon pytest` (failing fixture) output ends with a `raw_log:` line whose directory holds byte-exact original stdout/stderr; `cartoon logs --last --stdout` recovers it without a rerun; passthrough/`--raw` output byte-identical yet listed in `cartoon logs`; archive never exceeds configured caps; all tests + clippy + fmt green; CI green after push.
