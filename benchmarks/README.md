# benchmarks — verifying cartoon at scale

A reproducible harness for exercising cartoon's pytest/uv adapter on a large
suite and measuring token savings. Used to verify the uv wrapper end to end.

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
