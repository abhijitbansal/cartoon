# Cartoon Generic-Log Compression Ladder — Design

**Date:** 2026-06-11
**Status:** Approved (design); implementation deferred to a future release
**Scope:** Three phases — deterministic heuristic ladder, Drain template mining,
optional extractive ML line scorer

## Problem

Cartoon's adapters (pytest, jest, unittest, JSON CLIs) cover structured
output. Everything else hits the fallback path, where today only the opt-in
`--heuristic` flag applies a minimal lossy pass (ANSI strip, blank collapse,
exact-duplicate collapse in `src/heuristic.rs`). Generic CLI output — make,
npm, docker, gradle, CI logs — still reaches the agent nearly verbatim, and
agents pay input tokens for every line, on every subsequent API call in the
conversation.

## Goal

Compress arbitrary CLI output through a tiered ladder:

- **Safe tier (new default when no adapter matches):** deterministic,
  non-lossy-in-practice rules applied automatically.
- **Aggressive tier (opt-in):** lossy deterministic rules, including Drain
  template mining (phase 2).
- **Model tier (opt-in, phase 3):** extractive ML line scorer on top of the
  aggressive rules.

The raw log archive remains the universal escape hatch at every tier.

## Decisions (settled during brainstorming)

| Question | Decision |
| --- | --- |
| Phase coverage | All 3 phases in one spec; one implementation plan per phase |
| Activation | Safe tier auto-applies when no adapter matches; lossy tiers opt-in |
| Model provenance | Pluggable ONNX scorer interface; default = existing open small model (LLMLingua-2 family), downloaded on demand |
| Eval rigor | Golden corpus with signal-retention + token-floor assertions, in CI |
| CLI surface | Single `--compress=safe\|aggressive\|model` axis + `cartoon.toml` per-command pins; `--heuristic` aliased |
| Architecture | Rule pipeline: one pure function per rule, `CompressLevel` enum selects subset, fixed order |

## CLI and config surface

- `--compress=safe|aggressive|model`
  - `safe` — default when no adapter matches: ANSI strip, progress collapse,
    exact-duplicate collapse, blank collapse.
  - `aggressive` — adds level filtering, near-duplicate templating,
    compiler-diagnostic extraction, error-anchored windowing, and (phase 2)
    Drain template mining.
  - `model` — adds the extractive line scorer (phase 3). Implies the
    aggressive rules run first.
- `--heuristic` becomes an alias for `--compress=aggressive`. Deprecation
  note in `--help` and README; the flag is not removed.
- `--raw` unchanged: disables all transformation (and remains byte-identical,
  no footer).
- Config (`cartoon.toml`):

  ```toml
  [compress]
  level = "safe"            # global default

  [command.docker]
  level = "aggressive"      # per-command pin; CLI flag wins over config
  ```

- Disclosure: any tier that changes bytes appends the existing `raw_log:`
  footer. A run where no rule fired stays byte-identical (no footer), same as
  today's passthrough.

## Architecture

`src/heuristic.rs` grows into a `src/ladder/` module:

- One pure function per rule (`strip_ansi`, `collapse_progress`,
  `collapse_repeats`, `collapse_blanks`, `filter_levels`,
  `collapse_near_dups`, `extract_diagnostics`, `window_errors`, `drain`,
  `model_score`).
- `CompressLevel { Safe, Aggressive, Model }` selects the rule subset.
- Rules apply in a fixed, documented order.
- The existing `compress()` decomposes into rules 1–4 with behavior
  preserved.

Pipeline placement is unchanged: adapter match → JSON adapter → ladder
(level-dependent) → raw passthrough. Exit-code mirroring and the raw archive
are untouched. The phase-3 model is one more rule behind an optional
feature/installed-model check.

## Phase 1 — deterministic rules

| # | Rule | Tier | Behavior |
| --- | --- | --- | --- |
| 1 | `strip_ansi` | safe | Strip ANSI escape sequences (existing) |
| 2 | `collapse_progress` | safe | `\r`-rewritten frames, spinner/percentage sequences → final state only |
| 3 | `collapse_repeats` | safe | Exact consecutive duplicates → `(xN)` (existing) |
| 4 | `collapse_blanks` | safe | Blank runs → single blank (existing) |
| 5 | `filter_levels` | aggressive | Detect `timestamp LEVEL msg` shapes; DEBUG/INFO collapse to counts; WARN/ERROR kept verbatim with ±2 lines context |
| 6 | `collapse_near_dups` | aggressive | Lines identical after normalizing numbers/ids/paths → `template (xN)` with variable ranges |
| 7 | `extract_diagnostics` | aggressive | `file:line:col: severity: msg` (gcc/clang/rustc/tsc/eslint shape) → TOON diagnostics table |
| 8 | `window_errors` | aggressive | Keep head N + tail N + windows around error keywords; elide middle with `(skipped K lines, see raw_log)` markers |

Safety property: every rule emits its input unchanged when its pattern is
absent. Each rule carries a "does not fire on plain prose" test, so generic
text is never mangled by a rule that misidentified it.

## Phase 2 — Drain template mining

- Online log-template clustering (Drain3 algorithm), ported to Rust within
  the ladder module; no heavyweight new dependencies.
- Runs in the aggressive tier after `collapse_near_dups`, and only when the
  log meets a minimum line count (initial threshold: 200 lines) — small logs
  gain nothing from clustering.
- Output: a TOON `templates[N]{count,template,sample_vars}` section, with
  WARN/ERROR lines still preserved verbatim ahead of it.
- Tunables (tree depth, similarity threshold) start as fixed constants;
  promoted to config only if golden-corpus results demand it.

## Phase 3 — extractive model tier

- **Extractive, never abstractive.** The model scores lines for relevance;
  output is a budget-driven top-K selection of original lines. It cannot
  invent text, so it cannot hallucinate a log line.
- **Interface:** ONNX line scorer — input is a line plus small context
  features; output is a keep-score. `model.path` in config points at any
  ONNX file with the matching signature (pluggable).
- **Default model:** an existing open small encoder (LLMLingua-2 family),
  fetched via `cartoon model install`. Weights are never bundled in the
  binary.
- **Fallback chain:** `--compress=model` without an installed model (or on
  load failure/timeout) warns on stderr and falls back to the aggressive
  tier; aggressive falls back to safe; safe falls back to raw passthrough.
- **Runtime:** `ort` vs `candle` decided in the phase-3 implementation plan.
- **Gate:** the model tier may not become default-installable until it passes
  the same golden corpus as the deterministic tiers.

## Evaluation — golden corpus

- `tests/corpus/` holds real captured logs: make, npm install, docker build,
  cargo build, gradle, CI runs — passing and failing variants.
- Each fixture has a manifest of must-survive signal lines (errors,
  warnings).
- Two assertion classes, run in CI:
  1. **Signal retention** — every manifest line survives compression at each
     tier.
  2. **Token floor** — tokenizer-based count reduced by at least a
     per-fixture-class percentage.
- Phase 2 and phase 3 are gated on corpus numbers from the prior phase:
  phase 2 starts only after phase 1 corpus results exist; phase 3 proceeds
  only if phases 1–2 leave a measurable token margin on the table.

## Guarantees (updated wording)

- Exit codes always mirrored — unchanged.
- Raw log archive — unchanged, and now more load-bearing: every lossy tier's
  output ends with the `raw_log:` pointer.
- "Information never silently lost" becomes: *the safe tier preserves all
  non-redundant text; lossy tiers are opt-in and always leave a raw_log
  pointer to the unmodified output.*

## Testing

- TDD per rule: unit tests per pure function, including no-fire-on-prose
  cases.
- Golden corpus integration tests in CI.
- End-to-end wraps of real commands, following the existing fast-mode e2e
  pattern.
- Repo standard 80%+ coverage applies.

## Out of scope

- Training a custom model (revisit only if the off-the-shelf default fails
  the corpus).
- LLM-judge replay harness (possible phase-3 offline addition; not in CI).
- Streaming/incremental compression (cartoon buffers output today; no change).
