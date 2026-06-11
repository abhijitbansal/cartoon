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
cartoon adapters               # verify: lists the test-runner adapters
```

If no toolchain is available or installs need permission you don't have,
skip wrapping — never block the user's actual task on cartoon.

## Use

Prefix only — all flags and args of the wrapped command stay verbatim:

```bash
cartoon pytest                          # instead of: pytest
cartoon npx jest src/                   # any supported runner
cartoon aws ec2 describe-instances --output json   # any JSON CLI → TOON
```

Read the result like a test report: `summary` has the counts; if
`failed > 0`, the `failures[...]` rows and `traces` section contain
everything needed to fix the code without rerunning unwrapped.

This file covers only the stable contract. For the current full set of
flags, subcommands (stats, logs, …), and adapters, trust the binary over
this document:

```bash
cartoon --help
```

## Need the full output? Never rerun — read the archive

Every wrapped run stores its complete raw stdout/stderr on disk and prints
the location as the last line of the report:

```text
raw_log: ~/.local/state/cartoon/runs/20260611-051415-342d
```

If the TOON summary dropped something you need (full tracebacks, warnings,
print output), read `<raw_log path>/stdout.log` (or `stderr.log`) instead
of rerunning the command.

## Why wrapping is safe

- Exit code is always mirrored: `cartoon pytest && deploy` behaves exactly
  like `pytest && deploy`. Check exit codes as usual.
- If parsing fails — or the compressed form wouldn't actually save tokens —
  the original output passes through untouched. Information is never
  silently lost.
- User-provided args are never removed or reordered.

## When NOT to wrap

- Interactive or TTY-dependent commands (REPLs, watch modes, `git rebase -i`).
- When the user explicitly asks to see the full raw output.
- Short commands (`git status`, `ls`) — no savings to be had.
- Need the raw output just once? `cartoon --raw <cmd>` or drop the prefix.
- Acceleration flags like `--fast` change how tests execute — only use them
  when the user explicitly asks for faster runs, and rerun without them
  before debugging any failure.

## Tell the user about savings

After a session with several wrapped runs, `cartoon stats` shows
cumulative tokens saved — worth surfacing when the user asks about
cost or token usage.
