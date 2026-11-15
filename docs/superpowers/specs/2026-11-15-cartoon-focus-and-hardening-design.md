# cartoon: critical review, verdict and launch plan (2026-11-15, approved)

Sources: Opus repo review, Opus market research, Fable red-team, Fable review of the draft (this session).
Labels: measured (command named) / reported (from a subagent, not re-run by the orchestrator) / estimated.

## 1. Verdict

- **Engineering: good.** 851 tests passing (reported, `cargo test`). Sound adapter/guard/archive design.
- **Market position: weak.**
  - rtk-ai/rtk: 86,606 stars (measured, `gh api`). Covers nearly every cartoon feature, now incl. TOON and an xcodebuild filter.
  - headroom 79.7k, context-mode 34.1k own adjacent space (reported).
  - rtk clones launched Feb–Apr 2026 plateau at 200–700 stars. lowfat: 156-pt Show HN, 586 stars (reported).
  - Apple niche: xcsift 518★, sweetpad 1.9k★, MobileBuildMCP 6.6k★. Filters reach hundreds; tools owning the build loop reach thousands (reported).
- **Category under pressure.** Copilot ships native selective compression (GitHub blog 2026-09-02). Quesma, 1,740 runs: RTK moved the bill ±5%, pass rate −1–2 pts. Claude Code 2.1.325 persists a 102.6 KB output to disk with a 2 KB head preview (reported, one measurement; threshold between 20.5 KB and 102.6 KB unmeasured).
- **Traction: none.** 1 star; 3 views / 3 uniques in 14 days (measured, `gh api …/traffic/views`). PyPI 11.4k/month is ~85% Linux mirror/bot traffic: never cite it.
- **Where value has been measured** (owner ledger, ~3,300 runs, reflects the owner's workload): 68.9% saved overall; ~98% of saved tokens from xcodebuild/swift adapters. Safe tier 1.8%. cargo 22 rows, 0 saved. `pytest`, `pytest -q`: 0% (reported).

**Call**
- General "save tokens on any CLI" star play: **not worth it.** Expect hundreds of stars at best; likely <150 in 30 days (estimated).
- Owner's daily tool: **keep it, stop widening it.**
- One bounded Xcode/Swift launch: **only after** the product stops missing its own hero scenario (§2.3–2.4).

**Decision rule**
1. By 2026-12-15: hook fixes + failing-run fallback shipped; ledger post published (§6 step 1).
2. Ledger post must drive ≥50 unique visitors in 7 days (`gh api repos/abhijitbansal/cartoon/traffic/views`; baseline 3/14 days). Else stop: maintenance mode, no benchmark, no launch.
3. If it clears: benchmark on ≥40 failing logs from ≥3 projects the owner doesn't own. Launch only if cartoon keeps failing test id + file:line in ≥95% and beats the best cheap baseline (`| tail -40`, `| grep`, `-quiet`, `xcbeautify`, 2 KB head + 1 grep) by ≥20 points.
4. 30 days after launch: <150 stars or <5 external issues/PRs → freeze features **and stop marketing**.

## 2. Blockers (before any public post)

1. **Hook auto-approves side effects** (measured: `mvn test deploy` → `allow`; reported: `gradle build publish`, `make deploy`, `npm ci`, `pytest && make deploy`, `jest --watchAll`). Positional allowlist for gradle/mvn goals; side-effect guard on `make` targets; `npm ci` only with `--ignore-scripts`; deny watch flags. Regression test per case. Small–medium, `src/hook/mod.rs`, security-sensitive.
2. **Permission rules see the rewritten string.** Hooks doc: rules are evaluated "against the input your hook returns". A user `ask`/`deny` rule on `Bash(make deploy*)` likely won't match `cartoon --merge-streams -c 'make deploy'`. Run a live test first; its result picks the design. Strict allowlist (2.1) covers the dangerous cases; document the residual.
3. **Hook misses the hero scenario** (measured: no rewrite for `xcodebuild test … 2>&1 | tail -40`, `cd App && xcodebuild test …`, `swift build 2>&1`; only bare form wraps). Strip trailing `2>&1`; drop `tail`/`head`/`xcbeautify`/`xcpretty` for Apple commands; allow leading `cd <in-project dir> &&`; first-word check instead of `contains("cartoon")`. Medium, security-sensitive lexer. Owner's bash-guard hides this locally, so the owner's hit rate won't transfer.
4. **Failing runs pass through unreduced** (reported: 38/204 failing xcodebuild runs, all exit 65, 1.4k–155k tokens, median ~17k; swift 61/642). Likely cause: `src/adapters/xcodebuild_test.rs:68-84`, `:150-166` bail when no xcresult / zero tests / failed with no failed test (= compile error during `test`). Fix: on bail, fall back to the build-diagnostics parser. Medium.
5. **`-c` pipe drop changes semantics** (`| grep -c`, `| wc`, `| grep -q` dropped → wrong output and exit code). Drop only `head`/`tail`. Small, `src/cli.rs:261-263`.
6. **Hygiene:** clippy red at `src/budget.rs:132` on 1.96 (reported), pin toolchain; answer PR #20 (16 days); #19; stale branches; rebuild stale 0.6.0 `target/release` binary before demos; remove "~70%" from hook deny message, instructions block, plugin.json. Small.

## 3. Product changes (ranked, after blockers)

1. **`cartoon setup`**: merge `hook install --instructions` + `instructions` + `doctor`. Needed before README cites it. Medium.
2. **Cut surface:** Copilot hook + VS Code deny surface, shims, `--fast`, `--heuristic`, `--tag`; npm platform packages (Windows one 404s anyway). Medium.
3. **Plain indented text for tracebacks**, TOON for tables only. Small–medium.
4. `stats`: report savings vs what the agent would see, not raw bytes. Small.
5. Bound `stats.jsonl`. Small.
6. **Gated on decision rule passing:** `cartoon discover` (missed-command scan), `cartoon rerun --failed`.

## 4. Proof

- **Action zero:** raise `keep_runs` / copy failing xcodebuild logs + xcresult bundles out now. Archive holds 50 runs, none xcodebuild (reported).
- **Public fixture Xcode project** with seeded failures (compile error, assertion, crash, timeout). Reproducible by strangers; doubles as the demo. Medium.
- **Benchmark** needs live macOS runs (log + xcresult pairs), not a static log corpus: the adapter reads the bundle via `xcresulttool`. Score what reaches the model after Claude Code's cutoff. It is a proxy; it does not answer Quesma's bill/task-success objection.
- Regenerate `benchmarks/results` for 0.7.0. Small.
- Paid `agent_bench`: skip (5 tasks × k=5 cannot resolve ±5%).

## 5. Positioning

- **Pitch:** "Your agent gets the failing test and file:line from an xcodebuild or swift test run, read from the xcresult bundle. Exit code and full log kept." Don't hang the pitch on "2 KB": Anthropic can change it in one release.
- **For:** iOS/macOS devs running Claude Code against Xcode projects from the terminal.
- **Wedge:** vs rtk: reads xcresult (rtk: zero hits), structured failures + run diff. vs xcsift: hook auto-wraps, no `| xcsift` (true only after blocker 2.3). vs MobileBuildMCP: none if you build via MCP; say so.
- Drop "TOON" and "any CLI" from headline. One number policy: delete "43–89%", "~70%", "12 lines instead of 800".

## 6. Marketing sequence

1. **Ledger post (demand probe, no new code), r/ClaudeAI:** "3,300 wrapped commands: 98% of the saved tokens came from xcodebuild and swift test." Measure visitors for 7 days.
2. If ≥50 uniques: README first screen (pitch, side-by-side of a truncated Claude Code xcodebuild failure vs cartoon report from the fixture project, `cartoon setup`, honest comparison table incl. cheap baselines; rest → docs/, <150 lines), repo description, site hero, 15-second GIF.
3. r/iOSProgramming (check self-promo rules) + Swift Forums showcase + iOS Mastodon: "Claude Code can't see your xcodebuild failures. cartoon reads the xcresult bundle and hands it the failing tests."
4. Listings: awesome-claude-code, Anthropic plugin directory, iOS Dev Weekly.
5. **Show HN last**, only if niche posts clear the bar: "Show HN: Cartoon – xcodebuild failures for coding agents, read from the xcresult bundle". Benchmark table in first comment.
6. Post-launch: answer issues fast; adapter request → fixture first; re-run benchmark on rival releases; check rule at 30 days.

## 7. Order of work

0. Save failing logs + bundles (config change).
1. Live permission-rule test, then hook hardening (2.1, 2.2).
2. Hygiene (2.5, 2.6).
3. Pipe/cd coverage + failing-run fallback (2.3, 2.4).
4. `cartoon setup` + README first screen.
5. Ledger post → probe.
6. Fixture project + benchmark, only if probe clears.
7. Niche launch, then Show HN.

## 8. Owner decisions (2026-11-15)

- Scope on branch `feat/0.8-focus-and-hardening`: all repo work (§2, §3, README). Public posts, benchmark and demo Xcode project stay gated on the demand probe.
- Cut the Copilot CLI hook and the VS Code / deny mode. Breaking: next release is 0.8.0.
- Keep npm publishing; only drop npm from the README's top install block.
- Blocker 2 verified with headless `claude -p` sessions in a scratch dir.
- Not done on this branch (need the owner): replying to PR #20, closing #19, deleting stale remote branches, `gh repo edit` description, any release.
