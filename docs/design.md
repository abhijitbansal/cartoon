# cartoon — TOON wrapper for any CLI (design)

Date: 2026-06-09
Status: draft for review
Owner: Abhijit Bansal

## Problem

Agents (Claude Code etc.) burn tokens reading CLI output formatted for humans:
test runner banners, progress noise, verbose pass listings, ANSI codes. Worst
offender observed: unit test runs — hundreds of "PASSED" lines an agent never
needs. TOON (Token-Oriented Object Notation) is a compact structured format
designed for LLM consumption.

Goal: a plug-and-play wrapper any developer can install (`pip`/`uv`/`npm`/
`cargo`) and prefix onto any CLI command so its output becomes token-optimized
TOON where possible — first-class support for test runners.

## Decisions (made during brainstorming)

| Topic | Decision |
|---|---|
| Language | Rust, single static binary per platform |
| Name | `cartoon` (binary + crates.io + PyPI; npm package alt-named, bin still `cartoon`) |
| Invocation | Prefix mode: `cartoon <cmd> [args...]` |
| Conversion strategy | Built-in adapters → JSON auto-detect fallback → optional heuristic (off by default) → passthrough |
| Adapter architecture | Approach C: compiled-in adapters behind an `Adapter` trait; declarative plugin DSL deferred until demand |
| v1 adapters | pytest, unittest, jest |
| Test-output policy | Asymmetric: passes summarized to counts; failures keep full actionable detail |
| Stats | Per-call token savings + running totals in local state file, `cartoon stats` report |
| Repo | Own dedicated GitHub repo (not inside claude-skills) |
| License | MIT (standard for dev tooling; final call at repo creation) |

## Architecture

```
cartoon [flags] <cmd> [args...]
   │
   ├─ cli        clap. cartoon's own flags appear before <cmd>; everything
   │             after <cmd> is passed through verbatim (no flag collision)
   ├─ adapter    registry: pytest | unittest | jest
   ├─ runner     spawn child process, capture stdout/stderr via pipes,
   │             mirror exit code exactly
   ├─ toon       TOON encoder written in Rust, verified against the official
   │             TOON conformance fixtures
   ├─ heuristic  optional lossy pass: ANSI strip, blank-line collapse,
   │             repeated-line dedupe. OFF by default; --heuristic or config
   └─ stats      token estimate before/after (BPE, o200k by default),
                 append JSONL record per call
```

### Adapter trait

```rust
trait Adapter {
    /// Does this adapter handle the invocation? (inspects argv[0] + args;
    /// handles `python -m pytest`, `npx jest`, `python -m unittest` forms)
    fn detect(&self, invocation: &Invocation) -> bool;

    /// Return a modified invocation that adds machine-readable output
    /// (never removes user-provided args).
    fn prepare(&self, invocation: Invocation) -> PreparedInvocation;

    /// Parse captured output + artifacts into a structured report.
    fn parse(&self, captured: &Captured) -> Result<Report, ParseError>;
}
```

Machine-readable extraction per adapter — scrape structured sources, not
human text, wherever the runner offers one:

| Adapter | Extraction |
|---|---|
| jest | inject `--json` → parse JSON report |
| pytest | inject `--junit-xml=<tmpfile>` (built-in, no plugin needed) → parse XML; assertion detail from XML failure nodes |
| unittest | no machine format → parse text output (regex on the stable `FAIL:`/`ERROR:`/`Ran N tests` structure) |

### Data flow

1. Parse cartoon flags; split off wrapped command.
2. Adapter registry `detect()` — first match wins.
3. `prepare()` injects machine-output flags (temp files cleaned up after).
4. Runner spawns child. Child sees pipes (not a TTY) → runners auto-disable
   color/progress output. stderr captured alongside stdout.
5. Adapter `parse()` → `Report`.
6. Report rendered to TOON, asymmetric policy applied (below).
7. Emit to stdout. Mirror child exit code.
8. Stats record appended (fire-and-forget; stats failure never breaks the call).

Fallback chain when no adapter matches:
- stdout parses as JSON (whole output or trailing JSON document) → encode to TOON.
- else if heuristic enabled → lossy text compression pass.
- else passthrough byte-for-byte.

### Safety rules (non-negotiable)

- Adapter `parse()` failure → emit the ORIGINAL captured output unchanged,
  plus one warning line on stderr (`cartoon: pytest adapter failed to parse,
  passing through`). Never silently lose information.
- Exit code of the child is always mirrored, in every mode, including
  parse-failure passthrough. `cartoon pytest && deploy` behaves identically
  to `pytest && deploy`.
- User-provided args are never removed or reordered; prepare() only appends.
- Signals (SIGINT/SIGTERM) forwarded to the child; cartoon dies after child.

## Test report shape (asymmetric policy)

Passes cost the tokens of one summary line. Failures keep everything an agent
needs to fix the code without rerunning:

```
summary{total,passed,failed,skipped,duration_s}: 48,45,2,1,3.2
failures[2]{id,loc,msg}:
  tests/test_auth.py::test_expiry,tests/test_auth.py:42,"assert exp < now"
  tests/test_user.py::test_create,tests/test_user.py:88,"KeyError: 'email'"
traces:
  tests/test_auth.py::test_expiry: |
    tests/test_auth.py:42 in test_expiry
      assert token.exp < now()   # exp=1717..., now=1717...
```

- Traceback: user-code frames only; framework internals (pytest/_pytest,
  unittest/case.py, jest internals) dropped. Capped lines per failure
  (configurable, default ~20).
- stderr from the child: passed through after the TOON block if non-empty
  (warnings, log noise the agent may need).

## CLI surface

```
cartoon <cmd> [args...]          # wrap mode
cartoon --heuristic <cmd> ...    # enable lossy fallback for this call
cartoon --raw <cmd> ...          # full bypass / escape hatch
cartoon stats [--since 7d]       # savings report
cartoon adapters                 # list adapters and what they match
```

## Config and state

Config `~/.config/cartoon/config.toml` (XDG; created on first run):

```toml
heuristic = false       # default for the lossy fallback
tokenizer = "o200k"     # BPE used for token estimates
trace_lines = 20        # per-failure traceback cap
```

State `~/.local/state/cartoon/stats.jsonl`, append-only, one JSON object per
call:

```json
{"ts":"2026-06-09T18:21:04Z","cmd":"pytest","adapter":"pytest",
 "tokens_in":4812,"tokens_out":312,"saved":4500,"exit":1}
```

`cartoon stats --since 7d`: total tokens saved, call count, per-adapter
breakdown. Append-only JSONL avoids read-modify-write races under parallel
invocations.

Token estimates use a Rust BPE implementation (e.g. tiktoken-rs) — estimates,
not billing truth; good enough for the running score.

## Error handling

| Failure | Behavior |
|---|---|
| Wrapped command not found | exit 127, clear message |
| Adapter parse error | passthrough original output + stderr warning, child exit code |
| TOON encode error (bug) | passthrough original output + stderr warning |
| Stats write failure | ignored (one stderr note in --verbose only) |
| Config unreadable/corrupt | built-in defaults + stderr warning |
| Child killed by signal | mirror conventional 128+N exit code |

## Testing strategy (TDD)

- **TOON encoder**: conformance fixtures from the official TOON repo run as a
  fixture test suite. Encoder is correct when all fixtures pass.
- **Adapters**: recorded real outputs (pytest/unittest/jest across passing,
  failing, erroring, empty-suite, collection-error cases) committed as
  fixtures; parse → snapshot tests (insta crate).
- **Runner**: integration tests spawning real child processes (echo scripts,
  then real pytest/jest in CI) asserting capture, exit-code mirroring,
  signal forwarding.
- **End-to-end**: `cartoon pytest` against a tiny fixture project in CI,
  assert TOON output + exit codes + measured savings > 0.
- Coverage target 80%+ per repo standards.

## Repo, CI, publishing

- New dedicated repo: `github.com/<owner>/cartoon`. MIT license, README with
  agent-focused pitch, CONTRIBUTING for adapter additions.
- GitHub Actions:
  - test job: linux + macos, cargo test + clippy + fmt.
  - release matrix: darwin-arm64, darwin-x64, linux-x64-gnu, linux-arm64-gnu,
    windows-x64. Triggered on tag.
  - publish: crates.io (`cartoon`), PyPI via maturin (`cartoon`), npm via
    per-platform optionalDependencies packages (parent package alt-named —
    e.g. `cartoon-wrap` — because npm `cartoon`/`cartoon-cli` are squatted;
    installed bin is still `cartoon`).
- Name availability verified 2026-06-09: crates.io free, PyPI free, npm taken.

## Out of scope for v1 (explicit)

- Declarative/plugin adapter DSL (Approach B) — revisit on demand.
- Shell shims/aliases and pipe mode (`cmd | cartoon`) — possible later;
  prefix mode covers the agent use case.
- Streaming TOON for long-running commands — v1 buffers, emits at exit.
- TOON decode / round-tripping. Encoder only.
- npm-test/cargo-test/go-test adapters — next after v1 proves the shape.

## Success criteria

- `uv tool install cartoon` / `npm i -g cartoon-wrap` / `cargo install
  cartoon` all yield a working `cartoon` binary.
- `cartoon pytest` on a real failing suite: agent can fix the failure from
  TOON output alone, with ≥70% token reduction vs raw output on typical suites.
- `cartoon <unknown-cli>` never corrupts output or exit codes.
- `cartoon stats` shows cumulative savings.
