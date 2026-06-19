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

If it's missing — or you want to set up automatic wrapping (a PreToolUse
hook or shell shims) so you don't prefix `cartoon` by hand — read
[install.md](install.md). If cartoon can't be installed, skip wrapping;
never block the user's actual task on it.

## Use

Prefix only — all flags and args of the wrapped command stay verbatim:

```bash
cartoon pytest                          # instead of: pytest
cartoon npx jest src/                   # any supported runner
cartoon aws ec2 describe-instances --output json   # any JSON CLI → TOON
cartoon make                            # any noisy command: safe compression is automatic
cartoon ingest build.log                # a log that already exists on disk
some-cmd | cartoon -                    # or piped in
```

## Don't pre-truncate noisy commands — wrap them

When a command is noisy (a test run, a build, a JSON CLI), do NOT pipe it to
`head`/`tail`/`grep` to shrink the output. That is lossy in the wrong way: it
keeps an arbitrary slice (often the wrong one — a build's real error sits
mid-log while `tail` shows only `BUILD FAILED`) and the hook will not wrap a
piped command at all, so you lose cartoon entirely.

```bash
xcodebuild build … | tail -15     # WRONG: dumb cut, and wrapping is skipped
cartoon xcodebuild build …        # RIGHT: signal kept, ~70% fewer tokens
```

Wrap first; if you still need a slice of the raw log afterward, use
`cartoon logs grep … --last`. Anything with a dedicated adapter (pytest, jest,
vitest, swift test/build, `xcodebuild test`/`build`, ruff, eslint, tsc) should
be run bare so the auto-wrap hook catches it — never behind a pipe.

Commands without a dedicated adapter still compress: the safe tier (ANSI,
progress, duplicate, blank collapse) applies automatically, and
`--compress=aggressive` adds lossy rules (log-level filtering, diagnostic
tables, error windowing) when the user wants deeper cuts. A net-savings
guard means worst case is byte-identical output — wrapping is never worse.

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
print output), do NOT cat the whole file — that spends the tokens cartoon
just saved. Search it instead:

```bash
cartoon logs grep "ERROR" --last -C 2   # matching lines + context, capped
```

Only read `<raw_log path>/stdout.log` in full when a targeted grep can't
answer the question.

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

## Auto-wrap may already be on

A hook or shell shims may wrap noisy dev commands for you, so commands you
run bare may come back as TOON. Non-noisy commands (`git`, `ls`, `docker`,
…) pass through untouched, and wrapped output is buffered (the report prints
when the command finishes, not live). To turn it off when asked:
`export CARTOON_NO_WRAP=1` (hook) or `CARTOON_NO_SHIM=1` (shims), or
`cartoon --raw <cmd>` for one command. Setup, scope, and uninstall:
[install.md](install.md).

## Tell the user about savings

After a session with several wrapped runs, `cartoon stats` shows
cumulative tokens saved — worth surfacing when the user asks about
cost or token usage.
