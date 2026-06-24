# cartoon

**Token-optimized output for any CLI.** Prefix `cartoon` onto a command and
its output becomes [TOON](https://github.com/toon-format/toon) — a compact
structured format built for LLM agents. Same exit codes, same behavior,
~70%+ fewer tokens on test runs.

A cartoon is a compressed rendering of reality. So is this.

## Why

Agents (Claude Code, Cursor, Codex, ...) read CLI output formatted for
humans: banners, progress noise, hundreds of `PASSED` lines. You pay for
every token. `cartoon` keeps what the agent needs — counts, failures,
tracebacks — and drops the rest.

## Install

```bash
uv tool install cartoon        # or: pipx install cartoon
npm install -g cartoon-wrap    # installs the `cartoon` binary
cargo install cartoon
```

## For agents (Claude Code, Codex, Copilot, Cursor, …)

Teach your agent to use cartoon automatically — and to install it when
missing — with the skills shipped in this repo:

```text
# Claude Code (plugin)
/plugin marketplace add abhijitbansal/cartoon
/plugin install cartoon@cartoon
```

```bash
# Everything else (skills.sh CLI auto-detects 40+ agents)
npx skills add abhijitbansal/cartoon
```

Copy-paste blocks for AGENTS.md / copilot-instructions.md and the full
integration matrix: [docs/agents.md](docs/agents.md).

### Auto-wrap hook (Claude Code, Copilot CLI, VS Code Copilot Chat)

Instructions only *ask* the agent to wrap; a `PreToolUse` hook *guarantees*
it. The hook rewrites noisy shell commands to run under cartoon
automatically — no skill recall needed. One `cartoon hook rewrite` serves
every supported agent; install it where each one looks:

```bash
cartoon hook install              # Claude Code        → ~/.claude/settings.json
cartoon hook install --vscode     # VS Code Copilot Chat (same Claude-format file)
cartoon hook install --copilot    # Copilot CLI        → ~/.copilot/hooks/cartoon.json
cartoon hook install --copilot --project   # repo-shared .github/hooks/cartoon.json
cartoon hook status               # where it's active, per agent
cartoon hook uninstall [--copilot | --vscode] [--project]
```

Per agent:

- **Claude Code** and **Copilot CLI** (≥ v1.0.24) support transparent
  rewrite (`updatedInput`): the wrapped command runs in place.
- **VS Code Copilot Chat** reads the same Claude-format
  `~/.claude/settings.json` (its hooks are Preview), so `--vscode` installs
  there — one entry covers both it and Claude Code. Chat has no rewrite
  field, so the hook *denies* the raw command with a "re-run wrapped"
  suggestion — still deterministic, just one extra round-trip. (Plain
  `cartoon hook install` already covers Chat too; `--vscode` is the explicit,
  VS-Code-labelled form.)
- The repo-shared `--copilot --project` file (`.github/hooks/cartoon.json`)
  is written in **Copilot-CLI format** (camelCase `preToolUse`) for the
  Copilot CLI and coding agent — it is *not* the VS Code Chat format.
- If your Copilot CLI prompts on every rewrite (a known v1.0.24 bug), use
  `cartoon hook install --copilot --deny` for the smoother deny-and-suggest
  flow. `cartoon hook rewrite --deny-mode` forces deny anywhere.

### Cover what the hook can't: the instructions directive

The hook deliberately won't rewrite a **piped** command — `pytest | tail` is
split on `|`, and because a rewrite auto-approves the call, an allowlisted
segment must never smuggle the rest past the prompt, so the whole compound is
left raw. That's safe, but it means a piped test run silently escapes cartoon
(and VS Code Copilot Chat can only deny, never rewrite). The fix the model
*can* act on is an instruction: wrap, and don't pipe.

`cartoon instructions` writes that directive into your agent's instruction
file as an idempotent, marker-delimited block (re-run to update; uninstall
removes exactly it, never your own text):

```bash
cartoon instructions install            # → ./AGENTS.md (cross-agent default)
cartoon instructions install --copilot  # → ./.github/copilot-instructions.md
cartoon instructions install --claude   # → ./CLAUDE.md
cartoon instructions status             # per-file: installed / not installed
cartoon instructions uninstall [--copilot | --claude]
cartoon instructions print              # emit the block (manual paste)
```

Set up both layers at once — and `hook install` will hint about the pipe gap
(and on a terminal, offer to write the directive for you):

```bash
cartoon hook install --instructions     # install the hook AND the directive
```

### No hook? Shell shims (any agent)

For agents without hooks — or as a belt-and-suspenders layer — `cartoon
shim` writes shell functions that shadow the bare tool names and re-invoke
them through cartoon. Functions beat both PATH and venv-local binaries, so
an activated venv's `pytest` is still caught, with no agent cooperation:

```bash
cartoon shim install     # writes ~/.config/cartoon/shims.sh + activation help
cartoon shim print       # emit the functions to stdout (eval/source them)
```

Activate for the non-interactive shells agents spawn with
`export BASH_ENV=~/.config/cartoon/shims.sh`; disable per-shell with
`CARTOON_NO_SHIM=1`. Shims wrap the same allowlist as the hook, but (unlike
the hook) can't see surrounding pipes, so keep them to tools you run bare.

### Copilot tips & limitations

Tips:

- **Confirmation dialog on every wrapped command?** Some Copilot CLI
  v1.0.24 builds prompt even when the hook approves the rewrite. Reinstall
  with `cartoon hook install --copilot --deny` (blocks-and-suggests, no
  modal). `cartoon hook rewrite --deny-mode` forces deny anywhere.
- **Share it with the team / coding agent** by committing
  `.github/hooks/cartoon.json` (`cartoon hook install --copilot --project`).
- `cartoon hook status` reports every surface at once;
  `cartoon shim status` shows the shim file.

Limitations:

- **VS Code Copilot Chat** can only deny, not rewrite (no `updatedInput`
  field exists), so wrapping costs one extra round-trip while the agent
  re-issues the command.
- **Requires Copilot CLI ≥ v1.0.24** for transparent rewrite; older builds
  ignore `updatedInput` — use `--deny` there.
- **Install to `~/.copilot/hooks` or `.github/hooks`**, not a plugin
  directory — plugin-defined Copilot hooks currently don't fire.
- **Shims can't see pipes/redirection** and **path-invoked binaries**
  (`./gradlew`, `./node_modules/.bin/jest`) bypass shell functions entirely.
  Tools whose names aren't valid shell identifiers (`pre-commit`), plus
  `python -m pytest` and `xcodebuild`, aren't shimmed — the hook covers them.

What it wraps: dev-loop commands only — test runners, linters,
typecheckers, builds (`pytest`, `jest`, `vitest`, `tsc`, `eslint`, `ruff`,
`mypy`, `make`, `cargo build|test|check|clippy`, `go test|build|vet`,
`npm test|ci`, …), including those run through uv (`uv run pytest`,
`uvx ruff check`, `uv run -m pytest`). Because a rewrite auto-approves the
call, the allowlist is deliberately conservative: infra CLIs (docker,
kubectl, terraform, gh, aws) and mutating subcommands (`cargo publish`,
`npm install`) are never wrapped, a uv run carrying package-adding flags
(`uv run --with <pkg> …`) is left for the normal prompt, commands that
change shell state (`cd`, `export`, `source`) pass through untouched, and
anything unrecognized is left alone (fail-open). The net-savings guard
still applies — worst case the output is byte-identical.

### Disabling & overhead

- **Disable for a session** — set in the environment your agent runs in:
  `export CARTOON_NO_WRAP=1` (hook) or `export CARTOON_NO_SHIM=1` (shims).
  Any non-empty value disables; unset to re-enable.
- **Disable permanently** — `cartoon hook uninstall [--copilot | --vscode]
  [--project]` and/or `cartoon shim uninstall` (then drop the `BASH_ENV` /
  source line). `cartoon hook status` shows what's active.
- **One command raw** — `cartoon --raw <cmd>` runs it untouched.
- **Overhead** — the per-command hook check is a tiny, fail-open process
  (negligible). When wrapping, cartoon **buffers** output (the report prints
  when the command finishes, not live) and adds parse/encode time
  proportional to output size — milliseconds in practice, dwarfed by the
  command itself. Use `--raw` when you want live streaming.

## Use

```bash
cartoon pytest                 # asymmetric test report in TOON
cartoon jest src/              # same for jest
cartoon vitest run             # same for vitest (watch mode passes through)
cartoon python -m unittest     # same for unittest
cartoon ruff check .           # lint diagnostics as a compact TOON table
cartoon npx eslint src/        # same for eslint
cartoon npx tsc --noEmit       # same for tsc type errors
cartoon aws ec2 describe-instances --output json   # any JSON CLI → TOON
cartoon make                   # safe tier auto-on: ANSI/progress/dupe collapse
cartoon --compress=aggressive make   # opt-in lossy: level filter, diag tables, windowing
cartoon -c 'cd app && make -j4'      # wrap a shell command string
cartoon ingest ci-run.log      # compress a log you already have
some-cmd | cartoon -           # same, from a pipe
cartoon --raw pytest           # escape hatch: no transformation
cartoon stats --since 7d       # how many tokens you've saved
cartoon learn                  # mine your own runs for config suggestions
cartoon adapters               # list built-in adapters
cartoon --tag api pytest       # tag the archived run
cartoon logs                   # list archived raw logs
cartoon logs --last --stdout   # full raw output of the newest run
cartoon logs grep ERROR --last # search a raw log instead of re-reading it
cartoon --fast pytest          # opt-in: parallel via pytest-xdist (-n auto)
```

Failing test run, before (pytest, ~4800 tokens) vs after (~300 tokens):

```
runner: pytest
summary:
  total: 48
  passed: 45
  failed: 2
  skipped: 1
  duration_s: 3.2
failures[2]{id,loc,msg}:
  "tests/test_auth.py::test_expiry","tests/test_auth.py:42",assert exp < now
  "tests/test_user.py::test_create","tests/test_user.py:88","KeyError: 'email'"
traces:
  "tests/test_auth.py::test_expiry"[2]: "tests/test_auth.py:42 in test_expiry",assert token.exp < now()
```

## How it works

Every command (or ingested log) moves through one pipeline; the first
stage that understands the content wins:

1. **Adapter match** — known runners (pytest, jest, vitest, …) get their
   machine-readable format injected and re-rendered as a compact report.
2. **JSON detection** — any JSON document in stdout is TOON-encoded.
3. **Ladder, safe tier (default)** — ANSI stripping, progress-bar
   collapse, duplicate-line collapse, blank-run collapse. Deterministic
   and non-lossy in practice.
4. **Ladder, aggressive tier (opt-in)** — log-level filtering (INFO/DEBUG
   to counts, WARN+ kept with context), near-duplicate templating,
   compiler-diagnostic extraction into a TOON table (gcc/clang one-liners
   and rustc multi-line blocks), error-anchored windowing.
5. **Net-savings guard** — the transform plus the `raw_log` footer must
   beat the original token count, or the original is emitted
   byte-identically. Trying cartoon is zero-risk by construction.

Every rule is a pure function that no-ops when its pattern is absent, so
plain prose is never mangled. Measured on the golden corpus that runs in
CI (token reduction at the aggressive tier, signal lines asserted intact):

| Fixture | Reduction | Signal kept |
|---|---|---|
| chatty service log (156 lines, 1 ERROR) | **−92.5%** | ERROR + WARN verbatim, ±2 lines context |
| real `cargo build` failure (3 errors) | **−61.5%** | all errors + locations in a TOON table |
| npm ERESOLVE conflict | ±0% | guard emits the original — never negative |

## Guarantees

- Exit codes always mirrored — `cartoon pytest && deploy` behaves like
  `pytest && deploy`.
- If parsing fails, the original output passes through untouched (one
  warning on stderr). The safe tier preserves all non-redundant text;
  lossy tiers are opt-in and always leave a `raw_log` pointer to the
  unmodified output.
- A transform must pay for itself: if the TOON rendering (footer included)
  wouldn't beat the original token count, the original is emitted
  byte-identically. Savings are never negative.

## Raw log archive

Every wrapped run keeps its full raw output under
`~/.local/state/cartoon/runs/<run-id>/` (`stdout.log`, `stderr.log`,
`meta.json`). Transformed output ends with a `raw_log:` line pointing at the
archive — if the TOON summary dropped something you need, search it with
`cartoon logs grep <pattern> --last` (capped, disclosed) or fetch it with
`cartoon logs <id>` instead of rerunning. Passthrough and
`--raw` output stay byte-identical (no footer) but are still archived.
Retention is capped (`keep_runs`, default 50; `max_archive_mb`, default 50);
`keep_runs = 0` disables archiving.

## Fast mode

`cartoon --fast pytest` appends `-n auto` so [pytest-xdist] runs the suite in
parallel. Strictly opt-in — parallel execution is NOT "same behavior" (test
order changes; shared-state tests can flake), so cartoon never enables it on
its own and always discloses it with a `fast: "-n auto"` line in the report.
Failures under `--fast`? Rerun without it before debugging. If pytest-xdist
isn't installed, cartoon retries serially once and notes it on stderr.
Other runners: no-op (jest is already parallel; unittest has no parallel
runner).

[pytest-xdist]: https://pypi.org/project/pytest-xdist/

## Config

`~/.config/cartoon/config.toml`:

```toml
tokenizer = "o200k"  # or "approx" (bytes/4) for zero-cost estimates
trace_lines = 20     # per-failure traceback cap
keep_runs = 50       # archived raw logs to keep (0 disables)
max_archive_mb = 50  # max total archive size

[compress]
level = "safe"       # default for non-adapter output: safe | aggressive

[command.docker]
level = "aggressive" # per-command pin; CLI --compress wins over config
```

Compression precedence: `--compress` flag > `--heuristic` (deprecated alias
for aggressive) > `[command.<name>]` > `[compress]` > legacy `heuristic`
key > safe.

Stats live in `~/.local/state/cartoon/stats.jsonl`.

## Adapters

| Adapter | Trigger | Source |
|---|---|---|
| pytest | `pytest`, `python -m pytest`, `uv run [-m] pytest`, `uvx pytest` | injected `--junit-xml` |
| unittest | `python -m unittest`, `uv run [python] -m unittest` | stderr text parse |
| jest | `jest`, `npx jest` | injected `--json` |
| vitest | `vitest run` (watch mode passes through) | injected `--reporter=json` |
| swift-test | `swift test` | injected `--xunit-output` + `--parallel` (merges XCTest + Swift Testing files) |
| xcodebuild-test | `xcodebuild test` | injected `-resultBundlePath`, parsed via `xcresulttool … test-results summary` (Xcode 16+) |
| ruff | `ruff check` | injected `--output-format json` |
| eslint | `eslint`, `npx eslint` | injected `--format json` |
| tsc | `tsc`, `npx tsc` (not `--watch`) | injected `--pretty false` |
| swift-build | `swift build` | stdout/stderr text parse |
| xcodebuild-build | `xcodebuild build` (no test action) | stdout/stderr diagnostics parse |

No adapter match → JSON auto-detection → compression ladder (safe tier by
default, aggressive opt-in) → passthrough when nothing pays for itself.

Want another runner (cargo test, go test, rspec)? See
[CONTRIBUTING.md](CONTRIBUTING.md) — adapters are one trait impl + fixtures.
The roadmap lives in
[docs/superpowers/specs/2026-06-11-cartoon-v02-roadmap.md](docs/superpowers/specs/2026-06-11-cartoon-v02-roadmap.md).

## Support

If cartoon saves you tokens, a ⭐ on this repo helps other agent users find
it — and `cartoon stats` will tell you exactly how much it earned one.

## License

MIT
