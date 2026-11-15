# cartoon reference

The README has the pitch and the three-line setup. This page has everything
else: install options, every flag, how the hook decides, config, and the
upgrade notes from 0.7.

- [Install](#install)
- [Everyday commands](#everyday-commands)
- [Pipes inside `-c`](#pipes-inside--c)
- [`--junit`](#--junit-any-runner-that-writes-junit-xml)
- [`--max-tokens`](#--max-tokens-a-hard-ceiling)
- [`cartoon last` and `cartoon diff`](#cartoon-last-and-cartoon-diff-the-edit-run-fix-loop)
- [`--merge-streams`](#--merge-streams-keep-stdoutstderr-interleaving)
- [Content sniffing](#content-sniffing)
- [`cartoon doctor`](#cartoon-doctor)
- [How it works](#how-it-works)
- [Guarantees](#guarantees)
- [Raw log archive](#raw-log-archive)
- [Config](#config)
- [Project config: wrapping build and test scripts](#project-config-wrapping-build-and-test-scripts)
- [What the hook will not auto-approve](#what-the-hook-will-not-auto-approve)
- [Disabling and overhead](#disabling-and-overhead)
- [Adapters](#adapters)
- [Upgrading from 0.7](#upgrading-from-07)

## Install

```bash
brew tap abhijitbansal/cartoon https://github.com/abhijitbansal/cartoon
brew install cartoon           # macOS / Linux Homebrew (this repo is the tap)
cargo install cartoon          # build from source
uv tool install cartoon        # or: pipx install cartoon
npm install -g cartoon-wrap    # installs the `cartoon` binary
cargo binstall cartoon         # prebuilt binary via cargo-binstall
curl -fsSL https://raw.githubusercontent.com/abhijitbansal/cartoon/main/install.sh | sh
```

Then run `cartoon setup` once. It installs the Claude Code hook (user scope,
or this project's `.claude/settings.json` with `--project`), writes the
instructions block into `CLAUDE.md` if present, else `AGENTS.md` (skip with
`--no-instructions`), and prints the `cartoon doctor` report. It is
idempotent and never prompts. If the cartoon plugin is enabled, `setup`
leaves `settings.json` alone because the plugin already provides the hook.

Prebuilt binaries cover Linux (x86_64, aarch64), macOS (x86_64, arm64) and
Windows (x86_64). The Linux binaries are static (musl), so they run on any
distro: old glibc, Alpine, `python:*` images. PyPI also ships an sdist, so
`pip` builds from source (needs a Rust toolchain) where no wheel fits.

`install.sh` puts the binary in `~/.local/bin` (override with
`CARTOON_INSTALL_DIR`; pin a release with `CARTOON_VERSION=0.8.0`) after
checking it against the release's `SHA256SUMS`. Release tarballs also carry
build provenance: `gh attestation verify cartoon-<target>.tar.gz -R
abhijitbansal/cartoon`.

The Homebrew formula lives in this repo (`Formula/cartoon.rb`) and installs
the release tarballs; the tap needs its URL because the repo isn't named
`homebrew-cartoon`. Upgrade with `brew upgrade cartoon`.

The Claude Code plugin's hooks need the binary. Without it, the plugin tells
the agent once per version (SessionStart) how to install it, and it hints
when the binary is older than the plugin.

Other agents, the MCP server and the instructions block:
[agents.md](agents.md).

## Everyday commands

```bash
cartoon pytest                 # test report in TOON
cartoon jest src/              # same for jest
cartoon vitest run             # same for vitest (watch mode passes through)
cartoon swift test             # swift test: failing tests, not the build log
cartoon xcodebuild test -scheme App -destination 'platform=iOS Simulator,name=iPhone 16'
cartoon ruff check .           # lint diagnostics as a compact TOON table
cartoon npx tsc --noEmit       # tsc type errors
cartoon make                   # safe tier auto-on: ANSI/progress/dupe collapse
cartoon --compress=aggressive make   # opt-in lossy: level filter, diag tables, windowing
cartoon -c 'cd app && make -j4'      # wrap a shell command string
cartoon ingest ci-run.log      # compress a log you already have
some-cmd | cartoon -           # same, from a pipe
cartoon --raw pytest           # escape hatch: no transformation
cartoon aws ec2 describe-instances --output json   # JSON output is encoded as TOON
cartoon setup                  # install hook + instructions, then run doctor
cartoon stats --since 7d       # tokens saved, measured against what the agent would have seen
cartoon learn                  # mine your own runs for config suggestions
cartoon adapters               # list built-in adapters
cartoon init                   # suggest a .cartoon.toml for project wrapper scripts
cartoon logs                   # list archived raw logs
cartoon logs --last --stdout   # full raw output of the newest run
cartoon logs grep ERROR --last # search a raw log instead of re-reading it
cartoon last                   # re-show the newest run's report, no re-run
cartoon diff                   # fixed / still failing / new vs the previous run
cartoon --junit build/test-results/test gradle test   # any runner that writes JUnit XML
cartoon --max-tokens 1500 make       # hard ceiling: head + tail kept, middle disclosed
cartoon --merge-streams make         # stdout+stderr compressed in arrival order, on stdout
cartoon -c 'pytest -v | tail -5'     # the filter is dropped; the report replaces it
cartoon doctor                 # health report: hook, config, allowlist gaps, ledger damage
```

An unknown or removed flag before the command is an error that names the
accepted flags. It is never ignored and never mistaken for a command name.

## Pipes inside `-c`

Agents write `pytest -v | tail -5` because they want less output, which is
cartoon's whole job. When the string is `<adapter-detected command> |
<display filter>` (`head`, `tail`, `cat`, `less`, `more`, `xcbeautify`,
`xcpretty`), cartoon runs the adapter and drops the filter, disclosing it as
`pipe_filter_dropped: "tail -5"` in the report. When no report replaces the
filter (the report would cost more than the raw output, or the adapter could
not parse the run), a `head`/`tail` line count is applied to what cartoon
prints, with a stderr line naming the filter and the raw log, so the output
is never longer than the pipe would have given. One trailing `2>&1` before
the pipe is stripped and turns `--merge-streams` on, so
`cartoon -c 'xcodebuild test 2>&1 | tail -40'` still exits with
xcodebuild's code. Anything else (`grep`, `wc`, `tee`, `xargs`, `sort`,
redirections, two pipes, a non-adapter command) keeps the plain `sh -c`
behavior, so `pytest | grep -c PASSED` returns the real count and exit
status.

The hook applies this itself to the narrow shapes in
[What the hook will not auto-approve](#what-the-hook-will-not-auto-approve):
a trailing `| head -n N`, `| tail -n N`, `| xcbeautify` or `| xcpretty` on a
command with an adapter. Any other piped compound is left for the normal
permission prompt.

## `--junit`: any runner that writes JUnit XML

`cartoon --junit <file-or-dir> <cmd>` (or `[command.<cmd>] junit = "path"`
in config) renders the XML the command wrote as the same test report pytest
gets: gradle, maven, `dotnet test --logger junit`, phpunit `--log-junit`,
anything. A directory means every `*.xml` inside, merged. A file older than
the run is stale (the build failed before tests ran) and is ignored with a
warning so a green report never hides a red build.

## `--max-tokens`: a hard ceiling

`cartoon --max-tokens N <cmd>` (or `CARTOON_MAX_TOKENS=N`, or `max_tokens`
in config) guarantees no result exceeds N tokens: the head and tail are kept
in whole lines and the middle is replaced by one marker that is itself a
ready-to-run `cartoon logs grep` command. Opt-in, because with a ceiling set
even passthrough output may be cut. That is the point. The raw log is
archived as always.

## `cartoon last` and `cartoon diff`: the edit, run, fix loop

Each adapter run (tests, lint, typecheck, build) stores its structured report
next to the raw log. `cartoon last [--cmd <substring>]` re-shows the newest
run's report without re-running anything (a run no adapter parsed gets a
short summary and its `raw_log` path). After an edit and a re-run,
`cartoon diff` compares the newest adapter run with the previous run of the
same command in the same directory (or `cartoon diff <id-a> <id-b>`):

```text
command: pytest
previous:
  id: 20261005-225301-ae02
  exit: 1
  failed: 2
current:
  id: 20261005-225302-87f5
  exit: 1
  failed: 2
fixed[1]{id,loc}:
  "test_loop.py::test_alpha","test_loop.py:1"
still_failing[1]{id,loc,msg}:
  "test_loop.py::test_beta","test_loop.py:7","AssertionError: beta is broken"
new_failures[1]{id,loc,msg}:
  "test_loop.py::test_gamma","test_loop.py:11","AssertionError: gamma is broken"
```

Tests match by id; diagnostics by file + rule + message, so a warning that
only moved lines is not reported as fixed. The re-run itself already ends
with a one-line `vs_previous: fixed 1; still 1; new 1 (cartoon diff)`
footer. `diff` exits 0, or 1 when there is no comparable pair. Only these
forms are reserved: `cartoon diff a.txt b.txt` still wraps the system diff.

## `--merge-streams`: keep stdout/stderr interleaving

By default a transformed run writes its compressed stdout first, then its
compressed stderr, so a reader of `2>&1` loses where a warning landed among
the progress lines. `cartoon --merge-streams <cmd>` (or `merge_streams =
true` under `[compress]` or a `[command.<name>]`; the flag wins, then the
command entry, then `[compress]`) compresses the two streams **as one text
in arrival order** and writes the result to **stdout only**:

- **Generic output:** the ladder runs over the combined text and the guard
  compares against the combined original. A structured rendering of stdout
  (JSON as TOON, a sniffed or `--junit` report) comes first, followed by
  stderr.
- **Adapter reports:** the report stays first on stdout. Any stderr the
  adapter keeps (an unexplained failure, a tool warning) follows it on
  stdout instead of going to stderr.
- **Passthrough** (nothing paid for itself) is unchanged: both streams are
  replayed byte-exact to their own fds, in arrival order.

It is off by default because it changes fd semantics: with it on,
`cartoon --merge-streams cmd 2>/dev/null` no longer hides the command's
stderr, and a `| grep` sees stderr lines too. cartoon's own notices
(`cartoon: ...`) stay on stderr.

The auto-wrap hook turns it on: its rewrite is `cartoon --merge-streams -c
'<command>'`. Agent shells (Claude Code's Bash tool among them) read a
command's stdout and stderr on separate pipes and show stdout first, so
the model never sees where a warning landed; merged, it does. The fd
caveat above can't bite there: the hook never rewrites a command that
carries a redirection other than a trailing `2>&1`. Typed `cartoon -c`
stays unmerged unless you pass the flag or set `[compress] merge_streams =
true`.

## Content sniffing

Output that arrives without a matching adapter (a `./build.sh` that runs
xcodebuild internally, fastlane's gym log, a runner printing JUnit XML to
stdout) is recognized by shape: xcodebuild build/archive banners get the
diagnostics table, XCTest's `Test Case ... passed/failed` protocol gets the
test report, JUnit XML gets the test report. Parse-only, never changes the
command, and the net-savings guard applies as always.

## `cartoon doctor`

One report for the ways an integration quietly stops saving tokens: hook
installed or not, whether the global and project config parse,
`wrap_scripts` entries that do not exist on disk, allowlisted tools that
have no adapter (ladder compression only), and ledger health (malformed
lines, negative-saved runs, the biggest uncompressed commands). Paste it into
a bug report. `cartoon setup` prints it after installing.

## How it works

Every command (or ingested log) moves through one pipeline; the first
stage that understands the content wins:

1. **Adapter match.** Known runners (pytest, jest, xcodebuild, swift test,
   ...) get their machine-readable format injected (or read back from disk)
   and re-rendered as a compact report. A failing `xcodebuild test` or
   `swift test` run with no readable result falls back to the build
   diagnostics parser, and to the raw output if that finds nothing.
2. **JSON detection.** Any JSON document in stdout is TOON-encoded.
3. **Ladder, safe tier (default).** ANSI stripping, progress-bar
   collapse (only frames that redraw or share one indicator; table rows
   with percentages are never folded), duplicate-line collapse, blank-run
   collapse. Deterministic and non-lossy in practice: CRLF line endings
   are preserved, trailing spaces and tabs are trimmed.
4. **Ladder, aggressive tier (opt-in).** Log-level filtering (INFO/DEBUG
   to counts, WARN+ kept with context), near-duplicate templating,
   compiler-diagnostic extraction into a TOON table (gcc/clang one-liners
   and rustc multi-line blocks), error-anchored windowing.
5. **Net-savings guard.** The transform plus the `raw_log` footer must
   beat the original token count, or the original is emitted
   byte-identically. Trying cartoon is zero-risk by construction.

Every rule is a pure function that no-ops when its pattern is absent, so
plain prose is never mangled. The golden corpus in `tests/corpus.rs` (part of
`cargo test`) asserts the signal lines survive at the aggressive tier: a
chatty service log keeps its ERROR and WARN lines with two lines of context,
a real `cargo build` failure keeps all errors and locations in a TOON table,
and an npm ERESOLVE conflict is emitted as the original because the guard
finds no saving.

## Guarantees

- Exit codes always mirrored: `cartoon pytest && deploy` behaves like
  `pytest && deploy`.
- If parsing fails, the original output passes through untouched (one
  warning on stderr). The safe tier preserves all non-redundant text;
  lossy tiers are opt-in and always leave a `raw_log` pointer to the
  unmodified output. One exception, at any tier: output larger than 4 MiB
  (useless to an agent whole) is cut to its first 512 KiB, every error line
  from the middle (up to 200), and its last 1 MiB, behind a marker stating
  exactly what was omitted; the full text is in `raw_log`, and if the
  archive can't be written the original passes through instead.
- A transform must pay for itself: if the TOON rendering (footer included)
  wouldn't beat the original token count, the original is emitted
  byte-identically. When an adapter injects a machine-readable flag
  (`go test -json`, `--junit-xml`, ...), the guard measures against what
  the command would have printed *without* that flag (reconstructed from
  the machine stream), and emits that native output when the report would
  not beat it, so savings are never negative relative to the command as
  you typed it ([benchmarks](../benchmarks/README.md)). A dropped
  `| head -n N` / `| tail -n N` that no report replaces is applied to the
  emitted output (see [Pipes inside `-c`](#pipes-inside--c)).

## Raw log archive

Every wrapped run keeps its full raw output under
`~/.local/state/cartoon/runs/<run-id>/` (`stdout.log`, `stderr.log`,
`meta.json`). Transformed output ends with a `raw_log:` line pointing at the
archive. If the TOON summary dropped something you need, search it with
`cartoon logs grep <pattern> --last` (capped, disclosed) or fetch it with
`cartoon logs <id>` instead of rerunning. Passthrough and
`--raw` output stay byte-identical (no footer) but are still archived.
Retention is capped (`keep_runs`, default 50; `max_archive_mb`, default 50);
`keep_runs = 0` disables archiving.

## Config

`~/.config/cartoon/config.toml`:

```toml
tokenizer = "o200k"  # or "approx" (bytes/4) for zero-cost estimates
trace_lines = 20     # per-failure traceback cap
keep_runs = 50       # archived raw logs to keep (0 disables)
max_archive_mb = 50  # max total archive size
# max_tokens = 1500  # hard output ceiling (see --max-tokens); unset = none

[compress]
level = "safe"       # default for non-adapter output: safe | aggressive
# merge_streams = true  # stdout+stderr in arrival order, on stdout (see --merge-streams)

[command.docker]
level = "aggressive" # per-command pin; CLI --compress wins over config

[command.gradle]
junit = "build/test-results/test"   # render the JUnit XML gradle writes
```

Compression precedence: `--compress` flag > `[command.<name>]` >
`wrap_scripts` member (aggressive) > `[compress]` > legacy `heuristic = true`
key > safe.

`cartoon stats` reports tokens saved against the raw output, `saved vs what
the agent would see` (the same sum with each run's input capped at 7,500
tokens, about what Claude Code shows whole), and `malformed_lines` when the
ledger holds lines it could not parse. The capped figure is an upper bound:
a run where cartoon added tokens counts as 0, and above the cap the agent
would have seen only a ~2 KB preview, not 7,500 tokens. The ledger
(`~/.local/state/cartoon/stats.jsonl`) is rotated to `stats.jsonl.1` at
5 MiB.
`cartoon learn` sees through `sh -c` runs and explains why a piped command
missed its adapter instead of suggesting a `[command.sh]` pin.

## Project config: wrapping build and test scripts

Some projects build through a wrapper script instead of calling a known
runner directly: an iOS project's `./build.sh` that internally runs
`xcodegen`, then `xcodebuild build`, then `simctl install`. The hook's
allowlist matches on the command's own first word, so `./build.sh` never
matches and its output, including `xcodebuild`'s very verbose compile
log, reaches the agent completely unwrapped.

Declare such scripts in a project-local `.cartoon.toml` (repo root, or any
ancestor up to the `.git` boundary):

```toml
wrap_scripts = ["./build.sh"]
```

A declared script compresses at the **aggressive** tier by default, because
the safe tier does little for this kind of output. Pin
`[command."./build.sh"] level = "safe"` to override. Run `cartoon init`
in the repo to scan for `*.sh` files that mention a known noisy tool
(`xcodebuild`, `swift test`/`build`, `pytest`, `cargo test`/`build`, ...)
and print this snippet ready to paste. The hook matches the script whether
you invoke it as `./build.sh`, `build.sh`, `bash ./build.sh`, or by absolute
path.

**A declared script is never auto-approved.** The built-in allowlist
(`pytest`, `cargo test`, `swift test`, ...) gets a transparent rewrite
(`permissionDecision: "allow"`) because those are vetted, globally-known,
read-mostly tools. A project's own script is arbitrary code: an iOS
`build.sh` can install a build onto a physical device, or push model
weights to one. `.cartoon.toml` is also repo-committed and agent-writable,
so treating a declaration as license to bypass the permission prompt would
turn adding one TOML line into a silent permission-bypass primitive.
Instead, a `wrap_scripts` match always denies with a suggestion: the raw
command is blocked and the agent is told to re-run it as
`cartoon -c './build.sh -d'`, going through the normal permission flow for
that explicit call. Want it frictionless? Allowlist that exact string in
your agent's own permission settings. That is the right layer for that
decision, not cartoon's.

## What the hook will not auto-approve

A rewrite is emitted with `permissionDecision: "allow"`, so it skips
Claude Code's permission prompt for that call. The hook is therefore narrow,
and anything it declines reaches your normal permission flow unchanged. In
every case below the hook prints nothing, except a declared `wrap_scripts`
match, which gets a deny with a re-run suggestion.

**It respects your permission rules.** Claude Code evaluates `ask` and
`deny` rules against the command a hook returns, not the one the agent wrote
(measured on Claude Code 2.1.325: with `Bash(make deploy:*)` in `deny`,
`make deploy` was blocked without the hook and ran with a naive one). So the
hook reads every Bash `ask` and `deny` rule itself, from user settings
(`$CLAUDE_CONFIG_DIR` or `~/.claude`), the project's `.claude/settings.json`
and `settings.local.json` (in the working directory and every ancestor), and
managed settings, and declines when a rule could match the command, any `&&`
segment of it, a segment without its leading `NAME=value` words, or the tool
behind a launcher or path (`xcrun`, `npx`/`bunx`/`pnpx`, `pnpm dlx`,
`yarn dlx`, `uv run`, `uvx`, `python -m`, `poetry run`, `pipenv run`;
`./gradlew` is matched as `gradlew`). `:*` prefixes and `*` globs both
count. When it cannot tell, it declines: an unreadable settings file, a bare
`Bash` rule or a rule syntax it does not understand guards every command,
and with neither `HOME` nor `CLAUDE_CONFIG_DIR` set it never rewrites.

Measured 2026-11-15 on Claude Code 2.1.325 (headless `claude -p`, hook
installed): settings-file `deny` / `ask` rules, `--disallowedTools` and
`dontAsk` permission mode all stopped the command. Residual, not measured
and not readable by the hook: rules passed with `--settings`, remote managed
policy, session rules added with `/permissions`, Agent SDK permission
options, managed-settings locations other than the one file per OS
(drop-in directory, MDM profile), and Windows. Claude Code might not
re-check those against the rewritten command; keep a rule that must hold as
a `deny` in a settings file the hook can read.

**It respects plan mode.** A session in `plan` permission mode never gets a
rewrite.

**It only wraps read-mostly dev-loop commands.** Test runners, linters,
typecheckers and builds. Infra CLIs (docker, kubectl, terraform, gh, aws)
and mutating subcommands (`cargo publish`, `npm install`) are never wrapped.
It refuses the deploy, publish, sign and watch forms it recognises; a name
it does not recognise as one is the residual (see the targets item below).

Further limits, per tool:

- A leading `NAME=value` prefix rides along only for benign names (`CI`,
  `RUST_LOG`, `NO_COLOR`, ...). `PATH=... pytest`, `LD_PRELOAD=... cargo
  test`, `NODE_OPTIONS=... jest`, `PYTEST_ADDOPTS=...` are left alone.
- `ruff` is gated to `ruff check`; `ruff format`, `--fix`, `eslint --fix`,
  `eslint -c <path>`, `swiftlint --fix` / `autocorrect` / `--autocorrect`,
  `cargo clippy --fix`, `jest|vitest -u` / `--updateSnapshot` / `--update`
  (also after `npm|pnpm|yarn|bun test --`), `--init` for jest, vitest,
  eslint, tsc and rspec, and `tsc --clean` are never wrapped.
  `pre-commit` is wrapped for `run` only. `npx`/`bunx`/`pnpx` launch only
  `jest`, `vitest`, `tsc`, `eslint`.
- Flags that load or run code from outside the project are refused per tool:
  `go test -exec/-toolexec`, `cargo --config/-Z/+toolchain`,
  `make -f/-C/NAME=value`, `jest|vitest --config/--setupFiles`,
  `pytest -p <plugin>/-c/--rootdir`, `mypy --config-file`,
  `gradle -I/-D/-P`, `mvn -s/-f`, `dotnet -p:` and the like.
- xcodebuild is an allowlist: only the actions `test`,
  `test-without-building`, `build`, `build-for-testing` and `clean`, known
  build and test flags, and simulator or macOS destinations. Any mode switch
  (`-exportArchive`, `-exportNotarizedApp`, `-importLocalizations`,
  `-downloadPlatform`, `-resolvePackageDependencies`, ...), any other action
  (`archive`, `install`, `analyze`, ...), developer-account flags
  (`-allowProvisioningUpdates`, `-authenticationKey*`), `-Name=value` user
  defaults and a physical-device destination are refused. A run with no
  `-destination` lets xcodebuild choose one (not checked).
- dotnet MSBuild targets (`-t:`, `-target:`, `--target`) must be `Build`,
  `Rebuild`, `Clean`, `Test` or `VSTest`.
- Targets with a side effect are refused. A gradle task is judged by its last
  `:` segment, so `:app:publish` is a publish; it must be a build, test or
  check task with no `publish`, `deploy`, `upload`, `install`, `release`,
  `push`, `connected`, `docker`, `sign`, `notarize`, `ship`, `flight`,
  `distribute` or `submit` in it, and no instrumented device test. A maven
  goal must be a lifecycle phase from `clean` to `verify`, and a profile
  naming a side effect is refused. A named `make` target must look like
  build, test or check and name no side effect. `npm ci` needs
  `--ignore-scripts`. Residual: the names are judged by these words only, so
  a gradle task or make target that ships something under another name (or
  a gradle abbreviation such as `deviceandTest`) still wraps.
- Watch and hang modes are refused: `jest --watch`, `vitest --watch` /
  `watch` / `dev` / `--standalone`, `-w` / `watch` / `dev` after
  `npm|pnpm|yarn|bun test --`, `tsc --watch`, `ruff --watch`, `pytest -f`,
  `gradle --continuous` / `-t` (also bundled, `-qt`) / `--debug-jvm`,
  `cargo doc --open`.
- Only these wrappers around a command are accepted: a trailing `2>&1`, one
  trailing `| head -n N` / `| tail -n N` / `| xcbeautify` / `| xcpretty`,
  and a leading `cd <dir> &&` into a plain relative directory that resolves
  (symlinks followed) inside the project; the rewrite emits it as
  `cd ./<dir>`, so a `CDPATH` can't redirect it. `..`, an absolute `cd`
  target, absolute paths outside the project, `;`, `||`, any other pipe or
  redirection, and anything with `$`,
  backticks, backslashes or globs are never rewritten. Only `&&` compounds
  of eligible commands are.
- `make` and `pre-commit run` stay allowlisted by explicit decision: they
  are the canonical dev-loop entry points and the agent already has write
  access to the repo.
- A declared `wrap_scripts` entry is never auto-approved (see above).

`cartoon doctor` shows what is installed where and which allowlisted tools
still have no adapter.

## Disabling and overhead

- **Disable for a session.** Set `export CARTOON_NO_WRAP=1` in the
  environment your agent runs in. Any non-empty value disables the hook;
  unset to re-enable.
- **Disable permanently.** `cartoon hook uninstall [--project]`, and
  `cartoon instructions uninstall` to drop the directive. `cartoon hook
  status` shows what is active.
- **One command raw.** `cartoon --raw <cmd>` runs it untouched.
- **Overhead.** The per-command hook check is a tiny, fail-open process
  (negligible). When wrapping, cartoon **buffers** output (the report prints
  when the command finishes, not live) and adds parse and encode time
  proportional to output size, small next to the command itself. Use
  `--raw` when you want live streaming.

## Adapters

| Adapter | Trigger | Source |
|---|---|---|
| xcodebuild-test | `xcodebuild test` / `test-without-building` | injected `-resultBundlePath`, parsed via `xcresulttool ... test-results summary` (Xcode 16+); a failing run with no readable result falls back to build diagnostics |
| swift-test | `swift test` | injected `--xunit-output` + `--parallel` (merges XCTest + Swift Testing files); a failing run with no xunit falls back to build diagnostics |
| xcodebuild-build | `xcodebuild build` / `archive` / `-exportArchive` / `-exportNotarizedApp` (archive and export win over a test action) | stdout/stderr diagnostics parse |
| swift-build | `swift build` | stdout/stderr text parse |
| pytest | `pytest`, `python -m pytest`, `uv run [-m] pytest`, `uvx pytest`, `poetry`/`pdm`/`hatch`/`pipenv`/`rye run pytest` | injected `--junit-xml` |
| unittest | `python -m unittest`, `uv run [python] -m unittest` | stderr text parse |
| jest | `jest`, `npx jest` | injected `--json` |
| vitest | `vitest run` (watch mode passes through) | injected `--reporter=json` |
| ruff | `ruff check` | injected `--output-format json` |
| eslint | `eslint`, `npx eslint` | injected `--format json` |
| tsc | `tsc`, `npx tsc` (not `--watch`) | injected `--pretty false` |
| pre-commit | `pre-commit`, `pre-commit run ...` | stdout text parse (`--color=never` injected) |
| cargo-test | `cargo test`, `cargo nextest run` | stable text parse (never nightly JSON) |
| cargo-build | `cargo build`, `cargo check`, `cargo clippy` | injected `--message-format=json` (before `--`) |
| go-test | `go test` | injected `-json` |
| mypy | `mypy`, `python -m mypy`, `uv run mypy` | injected `--output json` |
| phpunit | `phpunit`, `vendor/bin/phpunit` | injected `--log-junit` |
| rspec | `rspec`, `bundle exec rspec` | injected `--format json --out <file>` |
| swiftlint | `swiftlint`, `swiftlint lint` (never `--fix`/`autocorrect`) | injected `--reporter json` |
| gradle | `gradle`/`./gradlew` `test`, `check`, `build`, `*Test` tasks (not `--continuous`) | nothing injected: the `build/test-results/**/TEST-*.xml` files this run wrote (every module), plus javac/kotlinc errors and failed tasks from the console |
| maven | `mvn`/`./mvnw` `test`, `verify`, `package`, `install` (not `-DskipTests`) | nothing injected: the `target/{surefire,failsafe}-reports/TEST-*.xml` files this run wrote (every module), plus compiler errors and failed goals |
| dotnet-test | `dotnet test` (VSTest; not Microsoft.Testing.Platform) | injected `--logger trx --results-directory <temp>` (a user's own trx logger / results directory is kept), one `.trx` per test project, plus MSBuild errors |
| golangci-lint | `golangci-lint run` | injected `--output.json.path=stdout` (v2) or `--out-format json` (v1), picked by `--version` |
| pkg-script | `npm test`, `npm run test`, `pnpm test`, `yarn test`, `bun run test` when package.json's `test` script is a plain `jest ...` / `vitest ...` (no `&&`, pipes, quotes, `pretest`/`posttest`; a bare `vitest` only when it would not watch) | the jest / vitest adapter's flags, forwarded to the script (after `--` for npm) |

No adapter match: content sniffing (xcodebuild / XCTest / JUnit shapes),
then JSON auto-detection, then the compression ladder (safe tier by default,
aggressive opt-in), then passthrough when nothing pays for itself.
Hook-allowlisted tools with no adapter (`make`, `dotnet build`, `bun test`,
an `npm test` whose script is not plain jest/vitest, ...) get the ladder
only; `cartoon doctor` lists them.

Want another runner (`bun test`, `deno test`, ...)? See
[CONTRIBUTING.md](../CONTRIBUTING.md): adapters are one trait impl plus
fixtures.

## Upgrading from 0.7

0.8.0 is a breaking release. It narrows cartoon to coding agents driving
Xcode, Swift and the common test runners from Claude Code, and hardens the
hook. Removed flags and commands now exit with code 2 and an error that
names what to use instead.

| 0.7 | 0.8 |
|---|---|
| `cartoon hook install --copilot`, `--vscode`, and the Copilot CLI / VS Code Copilot Chat hook | Removed. `cartoon hook uninstall --copilot` still deletes a Copilot hook file written before 0.8. Use the instructions block or the MCP server (see [agents.md](agents.md)) in other agents. |
| `cartoon hook install --deny`, `cartoon hook rewrite --deny-mode` | Removed. The hook rewrites (allow) or stays silent. A `wrap_scripts` match still denies with a re-run suggestion. An old `--deny-mode` entry prints a note and exits 0 (it no longer wraps); `cartoon hook status` calls it stale. An old allow-mode entry (`Bash\|run_in_terminal` matcher) still wraps and is reported as an older entry. Re-run `cartoon hook install` to refresh either; it touches only cartoon's own hook object. |
| `cartoon shim ...`, `CARTOON_NO_SHIM` | Removed. Use the hook; `cartoon setup` installs it. |
| `cartoon --fast pytest` | Removed. Pass the flag yourself: `cartoon pytest -n auto`. |
| `cartoon --heuristic` | Removed. Use `--compress=aggressive`. The `heuristic = true` config key still works. |
| `cartoon --tag <name>` | Removed. Find runs with `cartoon logs` or `cartoon last --cmd <substring>`. |
| `cartoon hook install --instructions` plus `cartoon instructions install` plus `cartoon doctor` | `cartoon setup`. |
| Hook rewrote anything on the allowlist, including commands your `ask` / `deny` rules cover | The hook reads your Bash `ask` / `deny` rules and plan mode and stays silent when they could apply. |
| `-c 'cmd \| grep ...'` could drop the `grep` | Only `head`, `tail`, `cat`, `less`, `more`, `xcbeautify` and `xcpretty` are dropped. |
| A failing `xcodebuild test` or `swift test` with a compile error passed through whole | Falls back to the build diagnostics table. |

Re-run `cartoon setup` after upgrading to refresh the hook entry and the
instructions block.
