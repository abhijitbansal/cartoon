# benchmarks — verifying cartoon at scale

## Token savings: `run.py`

`run.py` measures what an agent actually reads (stdout + stderr) with and
without cartoon, for pytest, unittest, cargo test and go test. Each suite is
generated deterministically (600 tests, 20 injected failures) and every
command is measured against **two baselines**: the verbose form, and the
quiet form agents usually run (`pytest -q --tb=short`, `python -m unittest`,
`cargo test -q`, `go test`). Tokens are counted with cartoon's own o200k
tokenizer. Runners that are not installed are skipped and listed.

```bash
cargo build --release
python3 benchmarks/run.py            # writes benchmarks/results/results.{json,md}
```

Latest run (`benchmarks/results/results.json`; regenerate after any adapter
change):

cartoon 0.6.0, Linux x86_64; 600 tests per suite, 20 failing. Tokens: o200k, stdout + stderr.

| suite | baseline | command | baseline tokens | cartoon tokens | saved | exit (raw/cartoon) |
|---|---|---|---:|---:|---:|---|
| pytest | verbose | `pytest -v tests` | 14,217 | 2,464 | **82.7%** | 1/1 |
| pytest | quiet | `pytest -q --tb=short tests` | 1,895 | 1,891 | **0.2%** | 1/1 |
| unittest | verbose | `python3 -m unittest -v` | 14,380 | 1,549 | **89.2%** | 1/1 |
| unittest | quiet | `python3 -m unittest` | 1,820 | 1,550 | **14.8%** | 1/1 |
| cargo test | verbose | `cargo test` | 16,327 | 3,083 | **81.1%** | 101/101 |
| cargo test | quiet | `cargo test -q` | 11,101 | 3,082 | **72.2%** | 101/101 |
| go test | verbose | `go test -v ./...` | 11,893 | 795 | **93.3%** | 1/1 |
| go test | quiet | `go test ./...` | 424 | 423 | **0.2%** | 1/1 |

How to read it:

- **Against verbose output cartoon saves 81–93%**; that is where the
  "~70%" headline in the README comes from. It is not a universal number.
- **Against the quiet baselines agents usually run** the result depends on
  the runner: `cargo test -q` still saves 72% (its quiet mode keeps every
  panic and backtrace), unittest 15%, while `pytest -q --tb=short` and plain
  `go test` are already terse and cartoon's net-savings guard hands back
  (essentially) the native output — ~0%, never negative.
- Exit codes match in every row.
- The injected failures are short. Long tracebacks (where cartoon trims
  frames) and passing suites (where cartoon prints only counts) favour
  cartoon more; tiny suites favour it less.

## Agent fix rate: `agent_bench/`

Token counts per command don't show whether an agent still fixes the bug.
[`agent_bench/`](agent_bench/README.md) runs Claude Code headless on five
small broken repos (pytest, cargo test, go test, tsc, ruff), with and without
cartoon's hook. For each session it records: solved, turns, tokens, cost and
wall time. `--verify-tasks` and `--dry-run` are free; `--run` makes paid API
calls.

## Generate a dummy suite

`gen_dummy_suite.py` writes a deterministic, stdlib-only pytest suite (runs
identically under a project venv or a bare `pytest`):

```bash
python benchmarks/gen_dummy_suite.py --out /tmp/uvproj/tests \
    --total 3000 --fail 100 --per-file 50
```

## Run it through cartoon (uv wrapper + bare)

```bash
cd /tmp/uvproj
printf '[project]\nname="uvproj"\nversion="0.1.0"\nrequires-python=">=3.9"\ndependencies=["pytest"]\n' > pyproject.toml
uv venv && uv pip install pytest

cartoon uv run pytest tests -v --tb=short   # uv auto-picks .venv
cartoon pytest -v --tb=short                # bare pytest on PATH
cartoon stats                               # cumulative tokens saved
```

## What was verified (3000 tests, 100 failing)

Both commands produce an identical structured TOON report — `summary` counts
plus all 100 failures with locations, messages, and trimmed tracebacks — and
mirror pytest's exit code (1):

| command | tokens in | tokens out | reduction | exit |
|---|---|---|---|---|
| `cartoon uv run pytest tests -v --tb=short` | 71,354 | 10,701 | **85.0%** | 1 |
| `cartoon pytest -v --tb=short`              | 71,318 | 10,669 | **85.0%** | 1 |

`uv run pytest` resolves to the project `.venv` (here pytest 9.1.1); bare
`pytest` uses whatever is on `PATH` (here 9.0.2) — the wrapper is transparent
to both. Raw output for the run was ~274 KB / 3,631 lines; the cartoon report
was ~31 KB / 210 lines, with the full raw log archived under
`~/.local/state/cartoon/runs/<id>/`.

Detection covers `uv run pytest`, `uvx pytest`, `uv tool run pytest`,
`uv run -m pytest`, `uv run python -m pytest`, and uv-level options in between
(`uv run --no-sync pytest`, `uv run --python 3.12 pytest`). The auto-wrap hook
recognizes the same forms so an agent's bare `uv run pytest` is wrapped
automatically — except package-adding flags (`uv run --with <pkg> …`), which
the hook leaves for the normal permission prompt.
