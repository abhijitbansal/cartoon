# Contributing

## Dev setup

```bash
cargo test                                   # unit + fixture + e2e tests
cargo clippy --all-targets -- -D warnings
cargo fmt
```

E2E tests for pytest/jest self-skip when the runner isn't installed locally.

GitHub CI is intentionally disabled (Actions runner minutes cost money the
maintainer does not want to pay). The three commands above ARE the gate: run
them before every commit and say so in the PR. `cargo test` never touches
your real `~/.local/state/cartoon` archive — every e2e test points
`XDG_STATE_HOME` at a temp dir (`tests/isolation_lint.rs` enforces it).

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
- Never remove or reorder user-provided args in `prepare`.
- Exit codes are sacred.
