---
name: cartoon
description: Keep the failing test and file:line, drop the wall of log, by wrapping noisy dev commands in `cartoon`. Use whenever running xcodebuild, swift test/build, pytest, cargo test, jest, unittest, other test runners, builds and linters, or a JSON-emitting CLI. Also covers installing and setting up cartoon when it is not present.
license: MIT
---

# cartoon — token-optimized CLI output

`cartoon` is a wrapper binary. Prefix it onto a command and the output is
re-rendered as [TOON](https://github.com/toon-format/toon), a compact
format built for LLM consumption: test passes collapse to counts, failures
keep full actionable detail (location, message, user-code traceback).
Verbose test runs shrink a lot (see benchmarks/README.md; far less against already-quiet flags like `pytest -q`). Exit codes and behavior are unchanged.

## Check it is installed (once per session)

```bash
command -v cartoon
```

If it's missing — or you want to set up automatic wrapping (the Claude Code
PreToolUse hook, via `cartoon setup`) so you don't prefix `cartoon` by hand —
read [install.md](install.md). If cartoon can't be installed, skip wrapping;
never block the user's actual task on it.

## Use

Prefix only — all flags and args of the wrapped command stay verbatim:

```bash
cartoon xcodebuild test -scheme App -destination 'platform=iOS Simulator,name=iPhone 16'
cartoon swift test                      # instead of: swift test
cartoon pytest                          # instead of: pytest
cartoon uv run pytest                   # uv picks the project .venv; cartoon still wraps
cartoon uvx ruff check .                # uvx / uv tool run are wrapped too
cartoon npx jest src/                   # any supported runner (see `cartoon adapters`)
cartoon aws ec2 describe-instances --output json   # any JSON CLI → TOON
cartoon make                            # any noisy command: safe compression is automatic
cartoon ingest build.log                # a log that already exists on disk
some-cmd | cartoon -                    # or piped in
```

`uv run pytest` / `uvx pytest` / `uv run -m pytest` are detected just like a
bare `pytest`: cartoon appends pytest's machine-output flag and uv forwards
it through, so wrapping a uv test run is identical to wrapping a bare one.

## Don't pre-truncate noisy commands — wrap them

When a command is noisy (a test run, a build, a JSON CLI), do NOT pipe it to
`head`/`tail`/`grep` to shrink the output. That is lossy in the wrong way: it
keeps an arbitrary slice (often the wrong one — a build's real error sits
mid-log while `tail` shows only `BUILD FAILED`), and the pipe's exit status
is the filter's, not the command's. The hook rewrites a trailing
`| tail -n N` or `| head -n N` on a command with an adapter, but it will not
wrap any other piped command, so you lose cartoon there.

```bash
xcodebuild build … | grep error   # WRONG: dumb cut, wrapping is skipped
cartoon xcodebuild build …        # RIGHT: signal kept, far fewer tokens
```

Wrap first; if you still need a slice of the raw log afterward, use
`cartoon logs grep … --last`. Anything with a dedicated adapter (pytest,
unittest, jest, vitest, ruff, eslint, tsc, mypy, pre-commit, cargo
test/nextest, cargo build/check/clippy, go test, golangci-lint, phpunit,
rspec, swiftlint, gradle, maven, dotnet test, `npm`/`pnpm`/`yarn test` and
`bun run test` when the script is plain jest/vitest, swift test/build,
`xcodebuild test`/`build`/`archive`) should be run bare so
the auto-wrap hook catches it — never behind a pipe. If you must pipe, use
`cartoon -c '<cmd> | tail -5'`: the adapter still fires and the dropped
filter is disclosed. Any other runner that writes JUnit XML gets the same
report via `cartoon --junit <file-or-dir> <cmd>`, and
`cartoon --max-tokens 1500 <cmd>` guarantees a hard ceiling on what you read.

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
answer the question. After fixing and re-running, `cartoon diff` lists what
got fixed / still fails / newly fails; `cartoon last` re-shows a report.

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

## Auto-wrap may already be on

The Claude Code hook may wrap noisy dev commands for you, so commands you
run bare may come back as TOON. Non-noisy commands (`git`, `ls`, `docker`,
…) pass through untouched, and wrapped output is buffered (the report prints
when the command finishes, not live). The hook stays out of the way when a
user permission rule (`ask` / `deny`) could match the command or the session
is in plan mode, so a command that is not rewritten is normal, not an error.
To turn it off when asked: `export CARTOON_NO_WRAP=1`, or
`cartoon --raw <cmd>` for one command. Setup, scope, and uninstall:
[install.md](install.md).

## Tell the user about savings

After a session with several wrapped runs, `cartoon stats` shows
cumulative tokens saved — worth surfacing when the user asks about
cost or token usage.

## Something not shrinking? Run `cartoon doctor`

`cartoon doctor` prints one report: whether the hook is installed, whether the global and project config parse, `wrap_scripts` entries
that do not exist on disk, allowlisted tools that only get ladder
compression (no adapter yet), and ledger health. Run it before filing an
issue and paste the output.
