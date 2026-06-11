# Cartoon v0.2+ Roadmap

**Date:** 2026-06-11
**Source:** five-scout brainstorm (adapters, features, learn design, growth,
competitive research) merged and prioritized. Items marked ✅ shipped on
`feat/v0.2-ladder-and-beyond` the same day.

## Shipped in this branch ✅

1. ✅ **Compression ladder Phase 1** — safe tier (ANSI/progress/dupe/blank
   collapse) is now the DEFAULT for non-adapter output; aggressive tier
   (`--compress=aggressive`) adds log-level filtering, near-dup templating,
   compiler-diagnostic extraction (gcc/clang single-line AND rustc
   multi-line blocks), error-anchored windowing. Golden corpus in CI with
   measured floors: cargo-build-fail 61.5% aggressive, service-log 92.5%.
2. ✅ **Shell-string mode `cartoon -c '<bash string>'`** — Claude Code's
   Bash tool emits compound commands (`cd x && pytest`); simple strings
   split for adapter detection, shell-operator strings run via `sh -c`
   through the generic ladder. Prerequisite for the auto-wrap hook.
3. ✅ **vitest adapter** — jest-shaped JSON reuse; `run` subcommand
   required so watch mode passes through.
4. ✅ **Diagnostics trio: ruff / eslint / tsc adapters** — machine formats
   (`--output-format json`, `--format json`, `--pretty false`), one shared
   output shape `{runner, summary{errors,warnings}, diagnostics[]{loc,
   severity, rule, msg}}`, summary-only on clean runs. Covers the two most
   frequent agent verification loops (typecheck + lint) in JS and Python.
5. ✅ **Archive query: `cartoon logs grep <pattern> [<id>|--last] [-C n]`**
   — the raw_log escape hatch was a cliff (agents would cat 50–200k tokens
   and destroy the savings). Capped at 50 matches.
6. ✅ **`cartoon learn [--since w]`** — mines the local stats ledger:
   token wasters stuck in passthrough/safe (≥3 calls, ≥500 avg tokens) get
   ready-to-paste `[command.X] level="aggressive"` pins; ≥3 consecutive
   failures of the same command get a "read the archived log instead of
   re-running" loop-breaker. All local, no telemetry.

## NOW (next session)

1. ✅ **PreToolUse auto-wrap hook** — SHIPPED same day: `cartoon hook
   rewrite|install|uninstall|status` + `hooks/hooks.json` in the plugin
   (0.2.0). Conservative allowlist because updatedInput requires
   permissionDecision "allow" (bypasses the prompt): dev-loop tools only,
   subcommand-gated package managers, no infra CLIs, shell-state builtins
   pass through, fail-open everywhere.
2. **BENCHMARKS.md + README hero table** — reproducible "pytest: 48,200 →
   1,900 tokens, 96%, $/run at Claude pricing" table; every growth channel
   depends on it; include one modest-savings row for credibility; CI
   regenerates on release.
3. **StatRecord forward-compat fields** — `intended_adapter` /
   `fallback_reason` on StatRecord + failure byte-range spans in archive
   meta, BEFORE the adapter wave grows (retrofitting is expensive). Feeds
   learn's parse-failure rule.

## NEXT (this month)

1. **Run diffing: `--diff` + `cartoon last`** — agent loop is run-edit-run
   5–15× per task; after run one, each full report is ~90% redundant.
   Test-id-level diffs from structured adapters are lossless.
2. **Adapter wave 2:** go test `-json`, cargo build/check/clippy
   `--message-format json`, cargo test/nextest (stable text — never claim
   nightly libtest JSON), playwright JSON. Independent small PRs; let
   `cartoon learn` passthrough data pick the order.
3. **Launch bundle** — VHS demo GIF, tagline ("Stop paying your AI agent
   to read PASSED PASSED PASSED"), why-not-head-tail docs page, Show HN +
   X thread, marketplace/skills.sh listings. Sequenced after benchmarks so
   copy never outruns evidence; pre-write the HN first comment with
   architecture honesty and explicit non-goals.
4. **`--max-tokens` hard output budget** — "no Bash result ever exceeds
   1.5k tokens"; omission markers are ready-to-run `logs grep` commands;
   `CARTOON_MAX_TOKENS` env for the hook to set a global ceiling.
5. **`cartoon doctor` + miss-log nudges** — half-working integrations
   (hook not firing, config typos, shadowed binaries) are where real-world
   savings silently die; doctor output becomes the standard bug-report
   artifact.
6. **Session savings ledger + statusline + `stats --share` card** — the
   retention/viral loop; hook payload session_id makes attribution free.
7. **Downstream-accuracy benchmark (separate `cartoon-bench` repo)** —
   publish "X% fewer tokens, 0pp diagnosis-accuracy loss, raw always
   archived" before skeptics write it for us. The aggressive tier is
   exactly where trust risk concentrates.
8. **learn v2** — `--agent` TOON mode, `--apply`, stats.jsonl rotation.

## LATER / research

- **Adapters, demand-driven:** JVM JUnit-XML harvester (gradle/maven —
  extract shared junit.rs first), phpunit (junit reuse), rspec, swift test
  (junit reuse; Paperix dogfooding), mypy, dotnet TRX, golangci-lint
  (needs version probe), terraform `-json` (interactive-apply safety),
  docker build `--progress=plain`, `gh run view --log` de-prefixer.
- **Config-driven user adapters in cartoon.toml** (tokf pattern) — closes
  the breadth gap without maintainer bandwidth; net-savings guard makes
  user regexes safe. Pairs with **learn propose** (Drain-mining adapter
  candidates from archived raw logs, replay-scored, `--emit-fixture` for
  upstream contribution) — the community adapter flywheel and long-term
  moat.
- **Reach:** stdin pipe mode (`| cartoon -`), PATH shim dir (env-gated),
  MCP server mode via rmcp (covers sandboxed/remote agents where the
  raw_log path is unreadable), GitHub Action + `--ci` annotations, Windows
  first-class (after hook + `-c` are in the matrix).
- **Foundational:** incremental capture in runner.rs → heartbeat mode +
  `--tail` (prevents agent-timeout double-pay; unblocks streaming
  compression). Failure-context bundle (`--bundle` source snippets —
  eliminates Read round-trips).
- **Growth long tail:** real-session case study (with/without plugin,
  end-to-end $ numbers), Cursor/Codex/Windsurf/Aider integration docs,
  badges + curl installer, WASM savings-calculator page, mdBook docs site.
- **Ladder Phase 2 (Drain template mining) and Phase 3 (extractive ONNX
  scorer)** — per the compression-ladder spec; gated on Phase 1 corpus
  numbers from real-world fixtures.

## KILLED (deliberately not doing)

- **shellenv preexec rewriting** — bash DEBUG-trap rewriting is fragile;
  hook + shims + pipe mode cover every reach scenario with less breakage.
- **Watch mode** — agents don't keep panes open; `--diff` delivers the
  value. YAGNI.
- **Cross-run §ref duplicate referencing (sqz-style)** — superseded by
  `--diff`; opaque refs undermine "never lies to your agent" positioning.
- **mocha adapter** — declining runner; jest/vitest + fallback cover it.
- **Package-manager install adapter (npm/pnpm/pip)** — the safe tier
  already eats most install noise generically; revisit only if learn flags
  installs as a top waste signature.
- **Competing with RTK on command breadth** — breadth comes from
  config-driven + community adapters, not a hand-written adapter race.

## Positioning

Cartoon owns **lossless-first, verifiable compression**. Filter tools (RTK,
headroom) must be trusted blindly — dropped lines are gone; Claude Code's
own 30k-char middle-truncation silently discards the middle of the log,
where the first failure usually lives. Cartoon is the only tool in the
category with all four guarantees: (a) every run archives full raw output
with a one-Read escape hatch — compression is reversible; (b) a net-savings
guard — output gets smaller or passes through byte-identical, so trying it
is zero-risk; (c) mirrored exit codes — CI and agent logic never lie;
(d) structure, not deletion — TOON re-encoding preserves every test and
diagnostic as queryable data, and the stats ledger makes savings
self-measuring rather than self-reported. The message everywhere: **"your
agent reads 12 lines instead of 800, and the receipt is always on disk."**
