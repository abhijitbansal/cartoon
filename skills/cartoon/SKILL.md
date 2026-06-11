---
name: cartoon
description: Save input tokens by wrapping CLI commands in `cartoon`. Use whenever running test suites (pytest, jest, unittest), JSON-emitting CLIs (aws, gh, kubectl with --output json), or any command expected to produce long, noisy output. Also covers installing cartoon when it is not present.
license: MIT
---

# cartoon — token-optimized CLI output

`cartoon` is a wrapper binary. Prefix it onto a command and the output is
re-rendered as [TOON](https://github.com/toon-format/toon), a compact
format built for LLM consumption: test passes collapse to counts, failures
keep full actionable detail (location, message, user-code traceback).
Typical test runs shrink ~70%+. Exit codes and behavior are unchanged.

## Check it is installed (once per session)

```bash
command -v cartoon
```

If missing, install with the first toolchain available, then verify:

```bash
uv tool install cartoon        # preferred when uv exists
pipx install cartoon           # Python fallback
npm install -g cartoon-wrap    # Node (installs the `cartoon` binary)
cargo install cartoon          # Rust
cartoon adapters               # verify: lists pytest, unittest, jest
```

If no toolchain is available or installs need permission you don't have,
skip wrapping — never block the user's actual task on cartoon.

## Use

Prefix only — all flags and args of the wrapped command stay verbatim:

```bash
cartoon pytest                          # instead of: pytest
cartoon python -m pytest tests/ -x      # any pytest invocation
cartoon npx jest src/                   # jest
cartoon python -m unittest              # unittest
cartoon aws ec2 describe-instances --output json   # any JSON CLI → TOON
cartoon --heuristic make                # lossy compression for plain text
cartoon stats --since 7d                # report tokens saved
```

Read the result like a test report: `summary` has the counts; if
`failed > 0`, the `failures[...]` rows and `traces` section contain
everything needed to fix the code without rerunning unwrapped.

## Why wrapping is safe

- Exit code is always mirrored: `cartoon pytest && deploy` behaves exactly
  like `pytest && deploy`. Check exit codes as usual.
- If parsing fails, the original output passes through untouched with one
  stderr warning. Information is never silently lost.
- User-provided args are never removed or reordered.

## When NOT to wrap

- Interactive or TTY-dependent commands (REPLs, watch modes, `git rebase -i`).
- When the user explicitly asks to see the full raw output.
- Short commands (`git status`, `ls`) — no savings to be had.
- Need the raw output just once? `cartoon --raw <cmd>` or drop the prefix.

## Tell the user about savings

After a session with several wrapped runs, `cartoon stats` shows
cumulative tokens saved — worth surfacing when the user asks about
cost or token usage.
