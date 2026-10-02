# Contributing

## Dev setup

```bash
cargo test                                   # unit + fixture + e2e tests
cargo clippy --all-targets -- -D warnings
cargo fmt
```

The three commands above are the gate, locally and in CI
(`.github/workflows/ci.yml`, on every push to main and every PR). `cargo
test` never touches your real `~/.local/state/cartoon` archive — every e2e
test points `XDG_STATE_HOME` at a temp dir (`tests/isolation_lint.rs`
enforces it).

Real-tool e2e tests (pytest, jest, ...) skip when the tool isn't installed
locally. Set `CARTOON_E2E_STRICT=1` to make a missing tool fail instead; CI
does, and installs pinned versions of pytest, pytest-xdist, jest, vitest,
tsc, go, ruff and mypy. Apple-only tools (xcodebuild, xcrun, swift) are
never required; `CARTOON_E2E_ALLOW_MISSING=a,b` exempts others by name. Use
the shared `tests/common::have()` for new real-tool tests so strict mode
covers them.

`upstream-drift.yml` runs the same suite weekly against the *latest*
release of every runner: a red run there means an upstream output format
changed and an adapter needs updating.

The npm wrapper has its own test: `node --test
packages/npm/cartoon-wrap/test/wrapper.test.mjs`.

> CI was switched off in GitHub's Actions settings in 2026-06 (to save
> runner minutes, which a public repository does not spend). The workflow
> file alone does not turn it back on: a maintainer must re-enable the
> `ci` workflow under Actions → ci → "Enable workflow".

## Adding an adapter

1. Create `src/adapters/<runner>.rs` implementing the `Adapter` trait
   (`detect` / `prepare` / `parse`). Prefer injecting a machine-readable
   output flag over scraping human text.
2. Record real runner output as fixtures under `tests/fixtures/<runner>/`
   (passing, failing, skipped cases minimum). Strip anything private.
3. Unit-test `parse` against the fixtures; register in
   `src/adapters/mod.rs::registry()`.
4. `parse` errors must be returned, not swallowed — the pipeline falls back
   to passthrough, which is the safety contract.

## Rules

- TDD: failing test first.
- Never remove or reorder user-provided args in `prepare`. Inject by
  appending, or by inserting before the first `--` separator (cargo, mypy,
  phpunit) or before `-args` (go test) — the tokens after those belong to
  the wrapped program, not the runner.
- Exit codes are sacred.
