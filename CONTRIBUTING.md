# Contributing

## Dev setup

```bash
cargo test                                   # unit + fixture + e2e tests
cargo clippy --all-targets -- -D warnings
cargo fmt
```

E2E tests for pytest/jest self-skip when the runner isn't installed locally;
CI runs them all.

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
