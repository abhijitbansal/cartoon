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
# Claude Code (plugin: skills + /cartoon:caveman terse-output mode)
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
- Heuristic (lossy) mode is off unless you ask for it.

## Config

`~/.config/cartoon/config.toml`:

```toml
heuristic = false    # default for lossy fallback
tokenizer = "o200k"  # or "approx" (bytes/4) for zero-cost estimates
trace_lines = 20     # per-failure traceback cap
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

## License

MIT
