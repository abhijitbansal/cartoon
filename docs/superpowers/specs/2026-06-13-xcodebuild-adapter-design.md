# Design: xcodebuild test/build adapters

**Date:** 2026-06-13
**Status:** approved (design + adversarial review folded in)
**Branch:** feat/swift-adapters

## Goal

Give `xcodebuild test` and `xcodebuild build` the same TOON summarization that
`swift test` / `swift build` get today, without regressing any SwiftPM behavior.
`xcodebuild test` emits a `.xcresult` bundle (not SwiftPM's xunit), so it needs a
distinct adapter and parse path.

## Invocation & schema (captured live on Xcode 26.5)

Test results come from the **supported** command:

```
xcrun xcresulttool get test-results summary --format json --path <bundle>
```

The legacy `xcresulttool get --format json` (whole-graph dump) is deprecated on
Xcode 16+ (requires `--legacy`); the `test-results summary` subcommand is its
replacement. Minimum floor: **Xcode 16** (subcommand absent on 15 → xcresulttool
errors → passthrough; safe degradation — see N5).

Real captured summary schema (`tests/fixtures/xcodebuild/summary-mixed.json`):

```jsonc
{
  "totalTestCount": 4, "passedTests": 2, "failedTests": 2,
  "skippedTests": 0, "expectedFailures": 0,
  "result": "Failed",
  "startTime": 1781404…, "finishTime": 1781404…,   // dur = finish - start
  "testFailures": [
    { "failureText": "Expectation failed: …",
      "targetName": "SwiftDemoTests",
      "testIdentifierString": "GreeterXCTests/testGreetingExact()",
      "testName": "testGreetingExact()",
      "testIdentifierURL": "test://…" }
  ]
}
```

Mapping to the shared `TestReport`: counts direct; `duration_s = finishTime -
startTime`; each failure → `{ id: "<targetName>.<testIdentifierString>", loc:
"" , msg: failureText }`. `loc` is empty — summary carries **no file:line**.

### Rejected alternative (N4): `get test-results tests`

`tests` has `sourceCodeContext` (file:line) but returns a large per-node tree.
`summary` is one call, already carries `failureText`, and maps directly to
counts + failures. swift_test's xunit also lacks file:line, so an empty `loc` is
consistent. We accept no-location in exchange for a smaller, simpler parse.
Revisit only if clickable locations become a requirement.

## Components

1. **`src/adapters/diagnostics.rs`** (new shared) — extract swift_build's
   `collect_diagnostics` + regex here, runner-name parameterized. swift_build and
   xcodebuild_build both call it. Behavior-preserving; swift_build tests stay green.
2. **`src/adapters/xcodebuild.rs`** (new) — `xcodebuild_action(argv) -> Option<Action>`
   shared detection helper (N2), used by both adapters below. Returns `Test` if any
   token is `test`/`test-without-building`, else `Build` if `build`/`build-for-testing`,
   else `None`. Skips tokens that are the value of a preceding value-flag
   (`-scheme/-target/-project/-workspace/-destination/-resultBundlePath/-configuration/-sdk/-arch`)
   so `-scheme test` does not false-positive.
3. **`src/adapters/xcodebuild_test.rs`** — `detect` (action == Test), `prepare`
   (inject `-resultBundlePath`), `parse` (run xcresulttool, pure `parse_summary_json`).
4. **`src/adapters/xcodebuild_build.rs`** — `detect` (action == Build), `prepare`
   (no-op), `parse` (shared diagnostics over stdout/stderr separately).
5. **`src/adapters/mod.rs`** — register `XcodebuildTest` then `XcodebuildBuild`
   (test before build); change `Prepared.artifact` to an `Artifact` enum (below).
6. **`src/hook.rs`** — xcodebuild action detection in `wrap_command` (scan-anywhere,
   reuse the same action logic).
7. **`tests/fixtures/xcodebuild/*.json`** — real captures.
8. **README** adapters table.

## `Prepared.artifact` change (N1)

xcodebuild refuses a pre-existing `-resultBundlePath`, so we need a temp **dir**
with a non-existent `result.xcresult` child (not a `NamedTempFile`).

```rust
/// Owns the temp artifact for cleanup AND exposes the path the adapter reads.
/// File: pytest/swift_test write into this file. Dir: xcodebuild writes the
/// bundle at `path` (a non-existent child of `guard`); the dir tree is removed
/// on drop. Keep "thing that cleans up" and "path to read" in sync here.
pub enum Artifact {
    File(tempfile::NamedTempFile),
    Dir { _guard: tempfile::TempDir, path: PathBuf },
}
pub struct Prepared { pub argv: Vec<String>, pub artifact: Option<Artifact> }
impl Prepared {
    pub fn artifact_path(&self) -> Option<PathBuf> { … }  // File→file path, Dir→child path
}
```

pytest/swift_test get a one-line mechanical update to `Some(Artifact::File(f))`.

## Data flow (test)

hook rewrites `xcodebuild … test` → `cartoon -c '…'` → detect (action Test) →
prepare injects `-resultBundlePath <tmpdir>/result.xcresult` **unless** the user
already passed `-resultBundlePath` → run xcodebuild (exit 65 on test-fail is
expected) → parse: resolve bundle path → run xcresulttool (see Security W1) →
read stdout (capped, W2) → `parse_summary_json` → `TestReport` → TOON. TempDir
auto-cleans on `Prepared` drop. User-supplied bundle is used as-is, never deleted.

## Error handling / discriminators

- **Exit code is NOT a discriminator**: `xcodebuild test` exits **65 for both**
  build-failure and test-failure. Key off the bundle/counts instead.
- **`totalTestCount == 0` → passthrough, regardless of `result` (W3)**: "no tests
  ran" (build broke, or filter matched nothing) is not our job; emit raw so the
  agent sees the real failure. Summarize only when `total > 0`.
- Missing bundle, xcresulttool not found / nonzero exit, malformed or truncated
  JSON → `Err` → app.rs raw passthrough. Never panic.

## Security

- **W1 — resolve the toolchain binary, never bare `$PATH`.** cartoon spawns a
  *secondary* tool the user did not type, so a poisoned PATH could hijack it.
  Resolve `xcrun` via `xcode-select -p` → `<dir>/usr/bin/xcrun`, falling back to
  `/usr/bin/xcrun`; invoke that absolute path. Build argv as a vector; **no `sh -c`**.
- **W2 — read cap that does not defeat the feature.** Cap xcresulttool stdout at
  **64 MiB** (summary = counts + failures, never the build log). On the cap →
  `Err` → passthrough. High enough that realistic summaries never truncate.
- **N3 — pass the bundle path as `--path=<value>`** (equals form) so a value
  beginning with `-` cannot masquerade as a flag to xcresulttool.
- **N4 — `failureText` is untrusted text** echoed into the agent's view (same
  prompt-injection surface as all wrapped test output; documented, not widened).
- Temp dir is `tempfile::TempDir` (mkdtemp, 0700, unpredictable), auto-removed.
- No new logging of scheme names / absolute paths beyond the existing raw-log archive.

## Testing (hermetic, no Xcode in CI)

- **Pure `parse_summary_json` fixtures**: `summary-mixed.json` (real 4/2/2,
  dur, two failures with ids+messages), `summary-all-pass.json`,
  `summary-zero-tests.json` (total==0 → discriminator → passthrough signal).
- **`xcodebuild_action`/detect**: position-flexible actions (`xcodebuild -project X
  test`, `xcodebuild build`, `xcodebuild clean test`), `-scheme test` false-positive
  guard, reject `archive`/`-list`/`-showBuildSettings`.
- **`prepare`**: injects `-resultBundlePath` child under a tempdir; respects a
  user-supplied `-resultBundlePath` (no inject, no delete).
- **`xcodebuild_build`**: captured-diagnostic fixture through the shared collector.
- **`hook::wrap_command`**: `xcodebuild test`, `xcodebuild -project X test`,
  `xcodebuild clean test`, `xcodebuild build`; reject `archive`/`-list`.
- **Xcode-gated `#[ignore]` e2e** (run only with `CARTOON_XCODE_E2E=1`): real
  `xcodebuild test` → summary appears. Out of CI.

## Risks

- **Schema drift** (summary schema `0.1.0`): lenient serde (ignore unknown fields,
  `Option` where sane); parse-fail → passthrough, so worst case is raw output.
- **Raw-log bulk**: xcodebuild output is huge; deliberately **no `-quiet`** inject
  (preserves passthrough fidelity; the summary replaces the noise in the agent's
  view and the full log is archived).
- **Compound commands**: existing hook invariant — every segment must be
  allowlisted or the whole command passes through. `xcodebuild test && deploy`
  stays unwrapped. Acceptable (matches today's behavior).

## Out of scope / not done

Do NOT commit or open a PR. Leave the working tree for review.
