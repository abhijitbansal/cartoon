# Decisions pending (owner)

Parked during the 0.8 focus-and-hardening branch (`feat/0.8-focus-and-hardening`, 2026-11-15). Each item names the options and the default the branch shipped with. None blocks merging.

## Hook safety residuals

1. **Claude Code `auto` permission mode.** Measured: `dontAsk` mode and `--disallowedTools` still block a hook-allowed command (headless `claude -p`, Claude Code 2.1.325). `auto` mode was not measured. Options: emit nothing in `auto` mode / keep rewriting. Default: keep rewriting.
2. **Rule sources the hook cannot read.** Session rules added with `/permissions`, Agent SDK permission options, managed-settings locations other than the one file per OS, and Windows were not measured. Settings-file rules are measured as enforced. Options: measure each with `claude -p` / document only. Default: documented as a residual in docs/usage.md.
3. **Event `cwd` outside `CLAUDE_PROJECT_DIR`.** The hook still rewrites. Options: emit nothing when cwd is outside the project and added dirs / leave. Default: leave (Claude Code may never let the shell leave allowed dirs).
4. **Gradle camel-case abbreviations** (`gradle conAndTest` runs `connectedAndroidTest`) still wrap; pinned by `gradle_abbreviations_are_a_known_residual`. Options: stricter suffix rule / read gradle's task list / accept. Default: accept, documented.
5. **mvn profiles** (`-P x`) stay allowed; a profile can bind a publish plugin to `package`. Options: refuse `-P` / allow. Default: allow.
6. **`wrap_scripts` entries for `./gradlew` / `./mvnw`** skip the argument policy (`./gradlew publish` would be approved via a declared script). Predates 0.8. Options: run the tool policy on wrap_scripts matches / leave. Default: leave.
7. **xcodebuild without `-destination`** lets xcodebuild pick one, possibly a connected device. Options: require a simulator/macOS destination / leave. Default: leave.
8. **`npm test -- -w 4`** (jest maxWorkers) is now refused because `-w` after `--` counts as watch mode. Safe direction. Options: special-case jest / leave. Default: leave.
9. **Windows.** No managed-settings path; with neither `HOME` nor `CLAUDE_CONFIG_DIR` set the hook never wraps. Default: leave until a Windows user reports it.

## Product and surface

10. **The Copilot flag on `cartoon instructions install`** is the last Copilot surface (it writes `.github/copilot-instructions.md`). Options: remove / keep undocumented. Default: keep undocumented.
11. **`doctor` row for permission rules** (count of Bash ask/deny rules found, unreadable settings files). Default: not added.
12. **Traceback format.** Spec §3.3 said plain indented text; the plan bound TOON list form, which costs +8–12% tokens per traced failure (measured on three pytest fixtures). Default: TOON list form, test bound 115%.
13. **npm platform packages** stay (owner decision 2026-11-15: keep npm, fix later); the Windows package 404s.

## Hygiene

14. **Pin the Rust toolchain** (`rust-toolchain.toml`, spec §2.6). Default: not pinned; CI uses stable.
15. **Split files over 800 lines**: `src/hook/mod.rs`, `src/hook/policy.rs`, `src/cli.rs`, `src/app.rs` (mostly test modules). Default: not split on this branch.

## Before any public post

16. Re-render `docs/assets/og.png` (still shows the old tagline), rerun `benchmarks/` on 0.8.0 (site stat comes from a 0.6.0 run), and confirm the README before/after sample against a real fixture render.
