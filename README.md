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

## Use

```bash
cartoon pytest                 # asymmetric test report in TOON
cartoon jest src/              # same for jest
cartoon python -m unittest     # same for unittest
cartoon aws ec2 describe-instances --output json   # any JSON CLI → TOON
cartoon --heuristic make       # lossy compression for plain text (opt-in)
cartoon --raw pytest           # escape hatch: no transformation
cartoon stats --since 7d       # how many tokens you've saved
cartoon adapters               # list built-in adapters
cartoon --tag api pytest       # tag the archived run
cartoon logs                   # list archived raw logs
cartoon logs --last --stdout   # full raw output of the newest run
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
  warning on stderr). Information is never silently lost.
- A transform must pay for itself: if the TOON rendering (footer included)
  wouldn't beat the original token count, the original is emitted
  byte-identically. Savings are never negative.
- Heuristic (lossy) mode is off unless you ask for it.

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
heuristic = false    # default for lossy fallback
tokenizer = "o200k"  # or "approx" (bytes/4) for zero-cost estimates
trace_lines = 20     # per-failure traceback cap
keep_runs = 50       # archived raw logs to keep (0 disables)
max_archive_mb = 50  # max total archive size
```

Stats live in `~/.local/state/cartoon/stats.jsonl`.

## Adapters

| Adapter | Trigger | Source |
|---|---|---|
| pytest | `pytest`, `python -m pytest` | injected `--junit-xml` |
| unittest | `python -m unittest` | stderr text parse |
| jest | `jest`, `npx jest` | injected `--json` |

No adapter match → JSON auto-detection → optional heuristic → passthrough.

Want another runner (cargo test, go test, vitest, rspec)? See
[CONTRIBUTING.md](CONTRIBUTING.md) — adapters are one trait impl + fixtures.

## Support

If cartoon saves you tokens, a ⭐ on this repo helps other agent users find
it — and `cartoon stats` will tell you exactly how much it earned one.

## License

MIT
