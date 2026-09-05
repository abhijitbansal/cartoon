# Cartoon repo review and improvement plan

**Date:** 2026-09-05
**Scope:** whole repo at `feat/wrap-scripts-project-config` (3 commits ahead of
`main`, no PR yet) plus the maintainer's local usage ledger.
**Method:** 91-agent workflow — nine review lenses (core pipeline, hook
security, adapters, ladder, archive/stats/learn, config/init/instructions,
docs drift, tests/CI, code quality), two adversarial verifiers per finding
(correctness/reproduction and impact/severity; a finding survives only if
both say real), five adapter-scouting angles (demand, roadmap, agent-loop,
Apple dogfooding, architecture), two judges, one completeness critic. Every
finding below was then re-read against the source by hand. Verdict counts:
16 findings survived, 19 refuted on impact, 6 additional gaps from the critic
(4 confirmed by hand and kept).

## 1. Where the tokens actually go

The ledger (`~/.local/state/cartoon/stats.jsonl`, 1,084 records) is the
demand signal for everything in this plan.

| Command head | Calls | Tokens in | Tokens saved | Saved % |
|---|---|---|---|---|
| `sh` (i.e. `cartoon -c '…'` fell to `sh -c`) | 637 | 17,719,954 | 100,563 | 0.6% |
| `./build.sh` (iOS wrapper script) | 70 | 3,223,316 | 1,217 | 0.0% |
| `xcodebuild` (adapter fired) | 45 | 2,641,385 | 2,636,606 | 99.8% |
| `swift` (adapter fired) | 24 | 120,839 | 118,621 | 98.2% |
| `ingest` | 85 | ~170k | ~130k | 78.6% |
| `npm test` | 32 | 4,946 | 0 | 0.0% |
| `cargo …` | 10 | 52,477 | 0 | 0.0% |

Two routing bugs, not missing adapters, explain 21 of the 24 million tokens
that went through cartoon almost uncompressed:

1. `needs_shell()` (`src/cli.rs:146`) forces `sh -c` on any `-c` string that
   contains a quote or `=`. `xcodebuild test -destination 'platform=iOS
   Simulator,name=iPhone 17'` therefore never reaches the xcodebuild adapter
   (which saves 99.8% when it fires). The archive confirms it: every
   `sh -c xcodebuild …` run is recorded as `passthrough`.
2. `main.rs:14` loads the global config with `config::load()`, never
   `load_merged(cwd)`. The `[command."./build.sh"] level = "aggressive"` pin
   that the README tells users to write in `.cartoon.toml` is never applied
   at run time, so `./build.sh` runs stay at the safe tier where the same
   content is known to compress 99.5% at aggressive.

Fixing both is a small code change and is worth more than every new adapter
on the roadmap combined for this ledger. Both judges ranked it 10/10.

## 2. Verified findings

Severity after adversarial verification. Each line: what, where, fix.

### Critical

- **Env-prefix skip auto-approves attacker-controlled variables**
  (`src/hook.rs:330`). `wrap_command_with_policy` skips any leading
  `NAME=value` word before matching argv0, so `PATH=/tmp/x pytest`,
  `LD_PRELOAD=… cargo test`, `RUSTC_WRAPPER=… cargo build`,
  `NODE_OPTIONS=--require … jest` are rewritten with
  `permissionDecision: "allow"`, bypassing the prompt the user's own
  permission rules would raise. Fix: replace the blanket skip with a small
  allowlist of benign names (`CI`, `NO_COLOR`, `RUST_LOG`, `RUST_BACKTRACE`,
  `NODE_ENV`, `PYTHONDONTWRITEBYTECODE`, …); any other prefix returns `None`
  so the command falls through to the normal prompt unwrapped.

### High

- **`needs_shell()` over-triggers on quotes and `=`** (`src/cli.rs:146`;
  three lenses converged). Only true shell operators (`| & ; < > ( ) $ \``,
  newline, and a leading `NAME=value` assignment) need `sh -c`. Fix: use a
  quote-aware tokenizer (`shell-words` crate) for strings with quotes only;
  drop `=` from the operator set; keep `sh -c` for real operators. Adds
  adapter detection, per-command config, and correct stats attribution for
  every quoted or `key=value` invocation.
- **Project `.cartoon.toml` pins never applied at run time**
  (`src/main.rs:14`, also `:77` for ingest). Only the hook reads
  `load_merged`. Fix: `load_merged(&current_dir)` in the `Wrap` and `Ingest`
  arms; add an e2e test that a project pin changes the tier. This is a bug in
  the current branch and must land before the wrap_scripts PR opens.
- **`ruff format`, `ruff check --fix`, `eslint --fix` are auto-approved**
  (`src/hook.rs:30`). `ALWAYS` gates on argv0 only. Fix: move `ruff` to
  `SUBCOMMAND` gated to `check`; scan ruff argv for `--fix`, `--fix-only`,
  `--unsafe-fixes` and eslint argv for `--fix`, `-c`, `--config`,
  `--rulesdir` and force the deny-with-suggestion path. Audit `make` and
  `pre-commit` in the same pass (both run arbitrary project code under the
  same unconditional allow) and decide deliberately: keep, or deny-mode only.
- **Progress collapse drops distinct `%` lines at the safe tier**
  (`src/ladder/progress.rs:6`). Any run of lines containing `\d{1,3}\s*%`
  collapses to the last one, so coverage tables, `df`, and test-timing
  percentages lose rows at the tier documented as non-lossy. Fix: collapse
  only runs that redraw (carriage return present in the raw capture) or whose
  percentage is monotonically non-decreasing.
- **Adapter path has no net-savings guard** (`src/app.rs:163`, critic).
  `run_with_adapter` emits report plus footer without comparing against the
  captured output. The live ledger has 58 negative-saved records (pytest
  `-53`, tsc `-50`), so the README's "savings are never negative" is false.
  Fix: apply the same guard as `transform_emit_record`; when the report would
  not pay for itself, emit the original streams and no footer.
- **`cargo test` writes into the real archive and evicts user runs**
  (`tests/e2e_ladder.rs`, `tests/e2e_ingest.rs`, `tests/xcodebuild_e2e.rs`,
  critic; confirmed by grep). These invoke the built binary without
  `XDG_STATE_HOME`, so every test run archives fixtures into
  `~/.local/state/cartoon` and `prune_at` deletes genuine logs to stay under
  `keep_runs`. Fix: one shared `cartoon_cmd(tmp)` test helper that always
  sets `XDG_STATE_HOME` and `XDG_CONFIG_HOME`; a lint test that greps for
  unisolated invocations.
- **Prune deletes the run it just wrote when one run exceeds
  `max_archive_mb`** (`src/archive.rs:252`, critic; confirmed by reading).
  The loop has no floor, so a single 60 MB run is archived, then removed, and
  the emitted `raw_log:` pointer dangles — precisely on the huge outputs
  where the escape hatch matters most. Fix: never prune the newest entry;
  warn on stderr when the cap cannot be met.

### Medium

- **Shell-string runs are recorded as `sh`** (`src/stats.rs:46`,
  `src/logs_cmd.rs` list view). `cartoon learn` and `cartoon logs` cannot see
  the inner command behind 637 runs. Fix: derive the display command from
  argv[2] when argv is `sh -c <string>` (or `cmd /C`); add the
  `intended_adapter` / `fallback_reason` fields the roadmap already calls
  for.
- **`learn` recommends `[command.sh] level="aggressive"`** (`src/learn.rs:58`),
  which would apply to every future shell-string run regardless of content.
  Fix: special-case `sh`/`cmd` heads with an explanatory note until the
  ledger records inner commands.
- **Stats append is not atomic** (`src/stats.rs:69`, critic). `writeln!`
  issues two writes on an `O_APPEND` file; concurrent runs interleave. The
  real ledger has 11 malformed lines that `read_records` silently drops. Fix:
  build the line with its newline and issue a single `write_all`.
- **`npx`/`bunx`/`pnpx` auto-approve any `ALWAYS` tool** (`src/hook.rs:373`).
  `npx pytest`, `npx make` are approved though those are not JS tools. Fix:
  gate `RUNNERS` to `jest`, `vitest`, `tsc`, `eslint`.
- **`wrap_scripts` matches only the literal declared string**
  (`src/hook.rs:363`). `bash ./build.sh`, `sh build.sh`, an absolute path, or
  a `cd dir && ./build.sh` compound all miss silently. Fix: compare on
  canonical path relative to the project root; document what does not match.
- **User-supplied `--junit-xml` is clobbered** (`src/adapters/pytest.rs:51`).
  Fix: detect an existing flag, read the user's file, do not inject.
- **`collapse_near_dups` runs before `extract_diagnostics`**
  (`src/ladder/mod.rs:58`). Three or more compiler diagnostics that differ
  only by line number template into one before the extractor sees them. Fix:
  reorder so diagnostics are extracted first.
- **Archive write failures are silent** (`src/archive.rs:156`). Fix: one
  stderr line, same as the adapter-parse-failure path.
- **Error window misses camelCase exception names** (`src/ladder/window.rs:9`).
  `KeyError:`, `NullPointerException` do not anchor a keep-window. Fix: add
  `[A-Za-z_]\w*(Error|Exception)\b` to the alternation.
- **`hook install --deny` after a plain install is a silent no-op**
  (`src/hook.rs:638`). Fix: replace the entry when the mode differs.

### Low

- Permission-denied exec maps to exit 2 instead of 126 (`src/app.rs:219`).
- `raw_log:` footer is built before the archive write is known to succeed
  (`src/app.rs:47`).
- Safe tier normalizes CRLF to LF and trims trailing whitespace without
  disclosure (`src/ladder/safe.rs:16`); document or preserve.
- Full output is tokenized up to four times per run (`src/app.rs:66`);
  compute once and thread through.
- `.claude-plugin/plugin.json` says 0.5.0, `Cargo.toml` says 0.5.1; nothing
  in CI or `RELEASING.md` checks the plugin manifest. Release workflow
  publishes npm from the tag and crates.io from `Cargo.toml` with no
  tag-vs-manifest gate.
- `src/hook.rs` is 1,243 lines (822 non-test) mixing rewrite policy,
  install/uninstall, and tests; split into `hook/policy.rs`,
  `hook/install.rs`, `hook/mod.rs`.
- `docs/design.md` is a frozen v0.1 draft that lists since-shipped features
  as out of scope; add a banner pointing at the roadmap.

### Project and process

- **CI is disabled.** The `ci` workflow shows `disabled_manually` on GitHub;
  its last run was 2026-06-15. PRs #5 through #10 merged with no CI. The
  matrix was already reduced to `ubuntu-latest` on 2026-06-19, so the cost
  reason is gone. Re-enable it.
- **Open community issues.** #11 asks for a `pre-commit` adapter (already in
  the hook `ALWAYS` list, so its output is wrapped but only ladder-compressed).
  #12 reports that `cartoon -c 'pytest -v | tail -5'` produces the raw tail
  instead of the report — the pipe forces `sh -c`, same family as the
  `needs_shell` bug.
- **Branch status.** `feat/wrap-scripts-project-config` is pushed, has no
  PR, and ships the `main.rs` config bug above. Its manual checklist lives in
  `.scratch/feat-wrap-scripts-project-config-test-checklist.html`.

## 3. Adapters and mechanisms

Judge consensus (two independent judges, scores 0–10) after merging 33
candidates from five angles. Ranking is by expected tokens recovered on this
ledger divided by effort, then by strategic fit.

### Tier 1 — routing fixes that unlock adapters that already exist

| Item | Judge scores | Effort |
|---|---|---|
| `needs_shell` / `shell_argv` quote-aware fix | 10, 10 | medium, one file plus tests |
| `main.rs` uses `load_merged(cwd)` | 9, 9 | small |
| Stats ledger records the inner command for `sh -c` runs | 7, 8 | small |
| Strip `xcrun` and env prefixes before xcodebuild action detection | 5, 5 | small |
| `wrap_scripts` entries default to aggressive unless overridden | 7, 4 | small, on top of the `load_merged` fix |

### Tier 2 — mechanisms that cover many tools at once

| Item | Judge scores | Notes |
|---|---|---|
| `cartoon doctor` | 5, 8 | Static report: hook allowlist entries with no adapter, hook installed but not firing, config keys that do not parse, project pins that do not match any script, ledger health (malformed lines, negative saves). Becomes the bug-report artifact the roadmap wants. |
| Content-sniff fallback stage | 5, 6 | After a run with no argv0 match, try `xcodebuild`/`swift build` diagnostic parsers and JUnit-XML detection on the captured output. Covers `./build.sh`, fastlane, and any wrapper without a new adapter. Parse-only, never changes the child's argv. |
| Generic JUnit-XML harvester | 3, 6 | `--junit <path>` flag or `[command.X] junit = "path"` config so gradle, maven, phpunit, dotnet (junit logger) get the `TestReport` rendering through `parse_junit_named`, which already exists. |
| `--max-tokens` hard ceiling | 6, 6 | Roadmap item; omission markers are ready-to-run `logs grep` commands. |
| `--diff` / `cartoon last` | 3, 7 | Roadmap item; lossless test-id diffs from structured reports. |
| TOML-declared regex adapters | 3, 8 | Judges split: high strategic value, low near-term recovery on this ledger. Defer until the content-sniff stage exists so both share one "user-declared parser" surface. |

### Tier 3 — new adapters, demand order

1. **pre-commit** (issue #11): already auto-wrapped; parse the
   `name….Passed|Failed|Skipped` lines plus failure blocks into a
   `TestReport`. Small.
2. **cargo test / nextest** (stable text, never nightly JSON): already
   allowlisted, currently 0% saved on this ledger. Medium.
3. **cargo build / check / clippy `--message-format=json`**: reuse the
   diagnostics shape from `diagnostics.rs`. Medium.
4. **go test `-json`**: maps onto `TestReport` directly. Small to medium.
5. **mypy `--output json`**: near-copy of `ruff.rs`. Small.
6. **phpunit `--log-junit`** and **rspec `--format json`**: both in
   `ALWAYS` with no adapter; junit reuse makes phpunit tiny. Small.
7. **xcodebuild `archive` / `-exportArchive`**: extend `xcodebuild-build`,
   keep it out of the hook allowlist (mutating). Small.
8. **swiftlint**: `--reporter json`, diagnostics shape. Small.
9. **playwright `--reporter=json`**, **flutter test `--machine`**,
   **go build / vet**, **dotnet test TRX**: medium each, demand-driven order
   via `cartoon learn` once the ledger records inner commands.

### Issue #12 design choice: pipes inside `-c`

Agents write `pytest -v | tail -5` because they want less output. Cartoon
already delivers that. Recommended design: in `-c` mode, when the string is
`<adapter-detected command> | <pure output filter>` where the filter is one
of `head`, `tail`, `grep`, `wc`, `less`, `cat`, run the adapter on the first
stage, drop the filter, and disclose it in the report as
`pipe_filter_dropped: "tail -5"`. Semantics change only for a filter whose
sole purpose was to shrink text the report already shrinks; anything else
(`tee`, `xargs`, `sort`, redirections, unknown commands) keeps today's
`sh -c` behavior. The hook keeps refusing to auto-approve piped compounds;
this applies only when the user or agent invokes `cartoon -c` explicitly.

### Killed

- fastlane in the hook allowlist: lanes deploy and sign; a fastlane adapter
  is fine, auto-approval is not.
- `npm test` resolve-and-reinvoke of the underlying jest/vitest binary:
  fragile script parsing for 5k tokens on this ledger.
- `gh run view --log` de-prefixer: the safe tier already handles it well
  enough; revisit if `learn` flags it.

## 4. Phased plan

One PR per phase. Every phase: failing test first, `cargo test`,
`cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. Phases 2
and 3 also replay archived raw logs through `cartoon ingest` and record the
before/after token counts in the PR description.

**Phase 0 — unblock (this week, before anything else)**
1. Re-enable the `ci` workflow on GitHub.
2. On `feat/wrap-scripts-project-config`: `main.rs` uses `load_merged`;
   e2e test proving a project pin changes the tier; then open the PR.
3. Test isolation helper so `cargo test` never touches the real archive.

**Phase 1 — hook security**
Env-prefix allowlist; ruff subcommand gate plus `--fix` deny; eslint flag
allowlist; `RUNNERS` restricted to JS tools; `make` and `pre-commit`
decision recorded in the hook module doc; regression tests for every case
that today returns `Some(…)` and must return `None`. Bump to 0.6.0 and note
the tightened allowlist in the changelog since some previously auto-approved
commands will start prompting.

**Phase 2 — routing and ledger fidelity (largest token win)**
`needs_shell` rewrite with `shell-words`; `xcrun`/env prefix strip before
xcodebuild detection; inner command in `StatRecord` and `logs` list;
`learn` special-case for `sh`; atomic stats append and tolerant reader that
counts malformed lines; adapter-path net-savings guard; prune floor; archive
write warning; `wrap_scripts` literal-match fix. Measure: re-ingest the 20
largest archived `sh` runs and report recovered tokens.

**Phase 3 — ladder correctness**
Progress collapse gating; diagnostics before near-dups; exception-name
regex; CRLF handling documented or preserved; single tokenization pass. Add
golden-corpus fixtures for each (coverage table, `df` output, three
same-message rustc errors, `KeyError` traceback, CRLF log).

**Phase 4 — mechanisms**
`cartoon doctor`; content-sniff fallback stage (xcodebuild, swift build,
JUnit-XML shapes); generic JUnit harvester flag and config; `--max-tokens`.
Then the `-c` pipe-filter design from section 3 (closes #12).

**Phase 5 — adapter wave**
pre-commit (closes #11), cargo test, cargo build/check/clippy, go test,
mypy, phpunit, rspec, xcodebuild archive, swiftlint. Independent small PRs;
order by `cartoon learn` output once Phase 2 has landed.

**Phase 6 — release hygiene and structure**
Version gate covering `Cargo.toml`, `docs/index.html`,
`.claude-plugin/plugin.json`, `packages/npm/*/package.json`, and the git tag;
split `src/hook.rs`; README adapter table distinguishes adapter-backed from
ladder-only allowlist entries; `docs/design.md` banner; roadmap doc updated
to mark this plan's items.

## 5. Expected outcome

On the current ledger, Phase 2 alone would have routed roughly 17.7 million
`sh` tokens and 3.2 million `./build.sh` tokens through adapters or the
aggressive tier that measure 98–99.8% savings on the same content, against
0.6% and 0.0% today. Phase 1 closes the only critical finding. Phases 0 and
3 restore the two documented guarantees that are currently false (non-lossy
safe tier, never-negative savings) and stop the test suite from deleting the
user's own archived logs.

## 6. Refuted or deferred

Verifiers rejected these on impact; listed so they are not re-raised:
jest lacking a `--watch` guard (hook wraps it regardless, adapter detect is
not the gate); pytest dropping the warnings summary (documented contract,
`raw_log` pointer present); `wrap_scripts` basename collision with built-in
tools (proved monotonic: a declaration can never reach `allow`); XDG
fallback on Windows (nothing else on Windows works yet); uv flag lists
duplicated between hook and adapters (documented, intentional asymmetry);
instructions marker duplicate blocks (advisory text only); e2e self-skip
without CI guard (tools are installed in CI); release tag-vs-manifest gate
(workflow does fail on crates.io mismatch, downgraded to low above).
