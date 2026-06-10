# cartoon — fast mode (opt-in test parallelization) (design)

Date: 2026-06-10
Status: approved (chat 2026-06-10: recommendation accepted verbatim — "do it")
Owner: Abhijit Bansal
Builds on: `2026-06-09-cartoon-toon-cli-wrapper-design.md`, `2026-06-10-cartoon-raw-log-archive-design.md`

## Problem

cartoon saves tokens but not wall clock. Large pytest suites run serially by
default; agents wait minutes for results they read in milliseconds. pytest-xdist
(`-n auto`) gives near-linear speedup but requires the agent to know about it,
verify it's installed, and remember the flag.

Goal: one opt-in flag — `cartoon --fast pytest` — that injects parallelization
when the runner supports it, discloses the injection in the TOON output, and
degrades safely when it can't.

## Core tension (drives every decision)

cartoon's contract is "same behavior, fewer tokens". Parallel execution is NOT
same behavior: test order changes, shared-state tests can flake, session-scoped
fixtures instantiate per worker. Therefore:

- **Never default.** No config key enables it globally. `--fast` is per-call,
  explicit. (Deliberate asymmetry with `heuristic`, which does have a config
  default — heuristic changes *output*, fast changes *execution*.)
- **Always disclosed.** When injection happens, the TOON output says so.
- **Fail safe.** If injection makes the command unrunnable (xdist missing),
  cartoon recovers without the agent noticing more than a stderr note.

## Decisions

| Topic | Decision |
|---|---|
| Surface | Global `--fast` flag, wrap mode only. No config key. |
| pytest | Append `-n auto` in the adapter path (append-only, after junit args). |
| xdist detection | Optimistic injection + bounded fallback (below). No pre-probe — probes cost 200-500 ms on every run to save a failure that almost never happens. |
| jest / unittest / cargo | `--fast` is a silent no-op (jest already parallel by default; unittest has no parallel runner; no cargo adapter). Adapter trait gains the hook so future adapters can opt in. |
| Disclosure | Transformed output gains one line after `runner:`: `fast: -n auto`. Only when injection actually happened (not on no-op runners, not on fallback). |
| `--lf` (failed-first) | **Out of scope.** Auto-injecting `--lf` silently shrinks the test selection on reruns; an agent reading `total: 5` could believe a 700-test suite is green. The agent can pass `--lf` itself (user args pass through untouched). |
| Stats / archive | Unchanged. Disclosure happens via the TOON 'fast:' line only; meta.json keeps recording the user argv. |

## Fallback: xdist missing

pytest exits with code 4 (usage error) and prints
`error: unrecognized arguments: -n` to stderr when xdist isn't installed.
Nothing was executed, so a retry is safe and cheap:

1. `--fast` active AND exit == 4 AND captured stderr contains
   `unrecognized arguments` AND the unrecognized list mentions an arg we
   injected for fast mode → respawn once with fast injection disabled
   (junit injection stays).
2. Use the retry's result for everything downstream (parse, archive, stats,
   exit code).
3. One stderr warning: `cartoon: --fast unavailable (pytest-xdist not
   installed); reran serially`.
4. No `fast:` line in the TOON output (injection did not happen).

Retry is bounded to exactly one attempt and only on this signature. Any other
exit-4 (user's own typo'd args) won't match the injected-arg check and passes
through normally — exit code mirrored as always.

## Adapter trait change (`src/adapters/mod.rs`)

```rust
pub trait Adapter {
    // existing: name, detect, prepare, parse
    /// Extra args that accelerate this runner. Appended after prepare()'s
    /// args when --fast is active. Default: none (no-op).
    fn fast_args(&self) -> Vec<String> {
        Vec::new()
    }
}
```

pytest impl: `vec!["-n".into(), "auto".into()]`. unittest/jest: default.

`prepare()` stays append-only; fast args are appended after junit/json
injection so the final argv reads naturally.

## Pipeline integration (`src/app.rs`)

- `run_wrap(...)` and the CLI carry a `fast: bool` through to the adapter path.
- Adapter path: `argv_final = prepare(argv) + (fast ? fast_args() : [])`.
- After capture: fallback check (above) before parse.
- On successful parse with injection active: report renderer receives
  `fast_note: Option<String>` (the joined injected args, e.g. `"-n auto"`)
  and emits `fast: -n auto` line right after `runner:`.
- Non-adapter paths (json / heuristic / passthrough / --raw): `--fast` ignored,
  zero behavioral change, no disclosure line.

## CLI surface

```
cartoon --fast pytest                  # parallel via -n auto (if xdist present)
cartoon --fast --tag ci pytest -q      # composes with existing flags
cartoon --fast npx jest                # accepted, no-op (jest already parallel)
```

`--fast` joins `--heuristic`/`--raw`/`--tag` as a pre-command global flag.
Ignored by the `stats`/`logs`/`adapters` subcommands (same as the other wrap flags).

## Output example

```
runner: pytest
fast: "-n auto"
summary:
  total: 723
  ...
```

README gains a Fast mode section: what it injects, why it's opt-in, the flake
caveat ("failures under --fast? rerun without it before debugging"), xdist
install hint.

## Error handling

| Failure | Behavior |
|---|---|
| xdist missing | one bounded serial retry + stderr note (see Fallback) |
| Tests fail under --fast | normal failure report; `fast: -n auto` line is the agent's signal to rerun serially before debugging |
| `--fast` with no adapter match | ignored; normal json/heuristic/passthrough path |
| `--fast stats` / `--fast logs` | silently ignored (consistent with --heuristic/--raw/--tag on subcommands) |

## Testing strategy (TDD)

- **Unit**: `fast_args()` per adapter (pytest non-empty, others empty);
  argv assembly order (user args → junit injection → fast args).
- **CLI**: `--fast` parses before command; composes with `--tag`/`--heuristic`;
  rejected for subcommands.
- **E2E (fixture)**: usage-error fallback — fake `pytest` script that exits 4
  with `unrecognized arguments: -n` on first call, succeeds without `-n`;
  assert single retry, stderr note, no `fast:` line, correct exit code.
- **Real-tool E2E**: venv WITH pytest-xdist: `cartoon --fast pytest` on the
  failing fixture → `fast: -n auto` line present, counts identical to serial
  run, exit 1. venv WITHOUT xdist: fallback path end-to-end.
- Coverage 80%+; clippy -D warnings; fmt clean.

## Out of scope (explicit)

- `--lf` / failed-first injection (selection-shrinking; see Decisions).
- jest `--maxWorkers` tuning (already parallel; marginal).
- unittest parallelization (no upstream support; TOON hint considered and
  rejected as noise).
- Auto-installing pytest-xdist (never mutate the user's environment).
- Config default for fast mode (behavior changes must be per-call explicit).

## Success criteria

- `cartoon --fast pytest` on an xdist venv runs parallel, reports identical
  counts to the serial run, and discloses `fast: -n auto`.
- Same command without xdist: one automatic serial retry, correct results,
  one stderr note, no disclosure line.
- `--fast` on jest/unittest/json/passthrough paths: byte-identical behavior
  to the same call without the flag.
- All existing 107 tests stay green.
