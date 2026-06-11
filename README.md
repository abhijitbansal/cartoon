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

### Auto-wrap hook (Claude Code)

The plugin ships a `PreToolUse` hook that rewrites noisy Bash commands to
run under cartoon automatically — no skill recall needed. Standalone
install (without the plugin):

```bash
cartoon hook install     # adds the hook to ~/.claude/settings.json
cartoon hook status      # check where it's active
cartoon hook uninstall   # remove it
```

What it wraps: dev-loop commands only — test runners, linters,
typecheckers, builds (`pytest`, `jest`, `vitest`, `tsc`, `eslint`, `ruff`,
`mypy`, `make`, `cargo build|test|check|clippy`, `go test|build|vet`,
`npm test|ci`, …). Because a rewrite auto-approves the call, the allowlist
is deliberately conservative: infra CLIs (docker, kubectl, terraform, gh,
aws) and mutating subcommands (`cargo publish`, `npm install`) are never
wrapped, commands that change shell state (`cd`, `export`, `source`) pass
through untouched, and anything unrecognized is left alone (fail-open).
The net-savings guard still applies — worst case the output is
byte-identical.

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
archive — if the TOON summary dropped something you need, fetch the original
with `cat` or `cartoon logs <id>` instead of rerunning. Passthrough and
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
| pytest | `pytest`, `python -m pytest` | injected `--junit-xml` |
| unittest | `python -m unittest` | stderr text parse |
| jest | `jest`, `npx jest` | injected `--json` |
| vitest | `vitest run` (watch mode passes through) | injected `--reporter=json` |
| ruff | `ruff check` | injected `--output-format json` |
| eslint | `eslint`, `npx eslint` | injected `--format json` |
| tsc | `tsc`, `npx tsc` (not `--watch`) | injected `--pretty false` |

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
