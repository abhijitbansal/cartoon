# cartoon

**Token-optimized output for any CLI.** Prefix `cartoon` onto a command and
its output becomes [TOON](https://github.com/toon-format/toon) — a compact
structured format built for LLM agents. Same exit codes, same behavior,
~70%+ fewer tokens on test runs.

This package installs the `cartoon` binary (prebuilt per platform via
optionalDependencies — no Rust toolchain needed).

```bash
npm install -g cartoon-wrap
```

## Use

```bash
cartoon pytest                 # asymmetric test report in TOON
cartoon npx jest src/          # same for jest
cartoon python -m unittest     # same for unittest
cartoon aws ec2 describe-instances --output json   # any JSON CLI → TOON
cartoon --fast pytest          # opt-in: parallel via pytest-xdist
cartoon stats --since 7d       # how many tokens you've saved
cartoon logs --last --stdout   # full raw output of the newest run
```

Failing test run, before (~4800 tokens) vs after (~300 tokens):

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
```

## Guarantees

- Exit codes always mirrored — `cartoon pytest && deploy` behaves like
  `pytest && deploy`.
- If parsing fails, the original output passes through untouched.
- A transform must pay for itself (tokens counted, footer included) or the
  original is emitted byte-identically.
- Every run's full raw output is archived locally and linked via a
  `raw_log:` footer — nothing is ever lost.

## For coding agents

Teach Claude Code, Codex, Cursor & co. to wrap commands automatically —
skills and a Claude Code plugin ship in the repo:
[github.com/abhijitbansal/cartoon](https://github.com/abhijitbansal/cartoon#for-agents-claude-code-codex-copilot-cursor-).

Full documentation: [github.com/abhijitbansal/cartoon](https://github.com/abhijitbansal/cartoon)

MIT
