# cartoon — raw log archive + retrieval (design)

Date: 2026-06-10
Status: approved (brainstorm 2026-06-10)
Owner: Abhijit Bansal
Builds on: `2026-06-09-cartoon-toon-cli-wrapper-design.md` (v0.1.0, all three plans complete)

## Problem

cartoon's TOON output is deliberately lossy about passes and aggressively trims
traces. When an LLM agent suspects the summary dropped something it needs (a
warning above a failure, output from a passing test, a trimmed frame), today it
must rerun the command — paying the tokens AND the wall clock again, and the
rerun may not reproduce.

Goal: archive the full raw output of every wrapped run, labeled and timestamped,
and tell the agent where to find it — so "request the whole log" is a cheap
`cat` away instead of a rerun.

## Decisions (made during brainstorming)

| Topic | Decision |
|---|---|
| Retrieval | TOON footer pointer + `cartoon logs` subcommand. Local MCP server deferred until demand. |
| Scope | ALL wrapped runs archived — adapter, json, heuristic, passthrough, and `--raw`. |
| Footer | Only on transformed output (adapter / json / heuristic). Passthrough and `--raw` stay byte-identical — no footer. |
| Storage | Per-run directory; no central index (race-free pruning, agent-cat-able). |
| Retention | Capped + config-tunable: `keep_runs` (default 50) and `max_archive_mb` (default 50); prune oldest on each new write. |
| Labels | Auto metadata (ts, argv, mode, exit, cwd) + repeatable `--tag <t>` flag. |
| Failure policy | Same contract as stats: archive failures swallowed, never break a call; footer printed only if the write succeeded. |

## Storage layout

```
~/.local/state/cartoon/runs/            # XDG_STATE_HOME, beside stats.jsonl
  20260610-051203-ab12/                 # run-id (see below)
    stdout.log                          # raw captured stdout, byte-for-byte
    stderr.log                          # raw captured stderr, byte-for-byte
    meta.json                           # metadata, one JSON object
```

- **run-id**: `YYYYMMDD-HHMMSS-<4 hex chars>` UTC. Lexicographic order == time
  order; suffix avoids collisions for parallel invocations in the same second.
- **meta.json** fields:

```json
{
  "id": "20260610-051203-ab12",
  "ts": "2026-06-10T05:12:03Z",
  "argv": ["pytest", "-q"],
  "mode": "pytest",
  "exit": 1,
  "cwd": "/Users/abhijit/proj",
  "tags": ["api", "ci"],
  "stdout_bytes": 48211,
  "stderr_bytes": 1024
}
```

- `mode` is the same string recorded in stats: adapter name (`pytest` |
  `unittest` | `jest`) or `json` | `heuristic` | `passthrough` | `raw`.
- No central index file. `cartoon logs` enumerates run dirs and reads
  meta.json per dir — cheap at the retention cap, and pruning never rewrites
  shared state (no read-modify-write races under parallel runs).

## New module: `src/archive.rs`

```rust
pub struct RunRef { pub id: String, pub dir: PathBuf }

/// Write stdout/stderr/meta for this run. All failures swallowed → None.
/// Prunes oldest runs beyond retention caps after a successful write.
pub fn record(
    argv: &[String],
    mode: &str,
    captured: &Captured,
    exit: i32,
    tags: &[String],
    cfg: &Config,
) -> Option<RunRef>;

/// List runs, newest first, optionally filtered by tag.
pub fn list(tag: Option<&str>) -> Vec<RunMeta>;

/// Load one run's meta + raw streams by id (or the newest with `--last`).
pub fn load(id: &str) -> Result<(RunMeta, String, String)>;
```

Pruning: after each successful `record`, enumerate run dirs sorted by id;
delete oldest while count > `keep_runs` OR total bytes > `max_archive_mb` MB.
Deletion is idempotent; concurrent pruners are harmless.

## Pipeline integration (`src/app.rs`)

Order within `run_wrap` / `run_with_adapter`, after capture + transform,
before emit:

1. `archive::record(...)` → `Option<RunRef>`.
2. If `Some(run)` AND the output was transformed (adapter ok / json /
   heuristic): append one footer line to the TOON output:

```
raw_log: "/Users/abhijit/.local/state/cartoon/runs/20260610-051203-ab12"
```

   Rendered through the TOON encoder's quoting rules (path contains `/` and
   sometimes `:`). Points at the run **directory**.
3. Passthrough, `--raw`, and adapter-parse-failure passthrough: archive still
   written (when possible) but NO footer — output stays byte-identical to the
   child's. (Parse-failure passthrough is an information-preservation path;
   modifying it would violate safety rule 1.)
4. Stats record gains a `run_id` field (nullable) tying ledger ↔ archive.
   Existing stats lines without the field must continue to parse (serde
   default).

## CLI surface

```
cartoon --tag api --tag ci pytest      # repeatable --tag, before <cmd>
cartoon logs                           # list recent runs (newest first)
cartoon logs --tag api                 # filter by tag
cartoon logs <id>                      # meta + BOTH raw streams
cartoon logs <id> --stdout             # one stream only
cartoon logs <id> --stderr
cartoon logs --last                    # newest run (combinable with --stdout/--stderr)
```

- `logs` joins `stats`/`adapters` as a reserved first word; after_help text
  updated (`cartoon env logs` wraps a literal `logs` binary).
- `cartoon logs` list output is TOON (dogfooding), tabular where uniform:

```
runs[2]{id,ts,cmd,mode,exit,tags}:
  20260610-051203-ab12,2026-06-10T05:12:03Z,pytest,pytest,1,"api,ci"
  20260610-050907-9c3e,2026-06-10T05:09:07Z,ls,passthrough,0,""
```

- `cartoon logs <id>` prints meta as TOON, then `--- stdout ---` and
  `--- stderr ---` delimited raw sections (raw bytes untouched within
  sections). Exit 0; unknown id → exit 2 with clear message.

## Config additions (`config.toml`)

```toml
keep_runs = 50          # max archived runs
max_archive_mb = 50     # max total archive size
```

Both serde-defaulted; existing configs keep working. `keep_runs = 0` disables
archiving entirely (and therefore footers).

## Error handling

| Failure | Behavior |
|---|---|
| Archive dir unwritable | run proceeds normally, no footer, no error (mirror stats policy) |
| Partial write (e.g. stdout.log ok, meta.json fails) | best-effort cleanup of the run dir; treated as record failure → None |
| `cartoon logs <unknown-id>` | exit 2, message lists `cartoon logs` to discover ids |
| Corrupt meta.json in a run dir | dir skipped in listings (one stderr note), `load` by id errors cleanly |
| Prune deletion error | ignored; retried implicitly on next run |

## Testing strategy (TDD)

- **archive unit tests**: record→load round-trip (bytes identical, meta
  fields), run-id lexicographic ordering, tag filtering, prune by count, prune
  by size, `keep_runs = 0` disables, partial-write cleanup.
- **CLI tests**: `--tag` parsing (repeatable, before cmd), `logs` mode parsing
  (bare / id / --last / --tag / --stdout / --stderr), reserved-word behavior.
- **E2E** (XDG_STATE_HOME-isolated): transformed run → footer present, path
  exists, files contain original output; passthrough and `--raw` runs →
  byte-identical output (no footer) yet archived; `cartoon logs` lists them;
  `cartoon logs --last --stdout` returns the raw stream.
- **Real-tool E2E**: `cartoon pytest <failing fixture>` → follow the
  `raw_log:` path and assert the archived stdout contains pytest's original
  human report (the info the TOON summary dropped).
- Coverage target 80%+ per repo standards; clippy -D warnings; fmt clean.

## Out of scope (explicit)

- Local MCP server (`cartoon mcp`) — revisit when a harness can't use
  cat/subcommand.
- Full-text search across archives, log compression, remote sync.
- Streaming archive writes for long-running commands (v1 buffers, writes at
  exit — same as the wrap pipeline itself).

## Success criteria

- `cartoon pytest` (failing suite) output ends with a `raw_log:` line whose
  directory contains the byte-exact original stdout/stderr.
- An agent reading only the TOON output can recover the full raw log with one
  `cat`/`cartoon logs` call — no rerun.
- Passthrough and `--raw` output remain byte-identical to the unwrapped
  command, while still appearing in `cartoon logs`.
- Archive never exceeds configured caps; a wrapped call never fails or slows
  perceptibly because of archiving.
