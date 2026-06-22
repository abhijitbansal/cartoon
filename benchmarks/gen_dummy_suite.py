#!/usr/bin/env python3
"""Generate a large, self-contained pytest suite for benchmarking cartoon.

The suite is deterministic and uses only the standard library, so it runs
identically under a project venv (`uv run pytest`) or a bare `pytest` on
PATH. Use it to exercise cartoon's pytest/uv adapter at scale and measure
token savings on verbose (`-v`) output.

Example:
    python benchmarks/gen_dummy_suite.py --out /tmp/uvproj/tests \
        --total 3000 --fail 100 --per-file 50

Every Nth test (N = total // fail) is made to fail, cycling through a few
failure shapes (assertion, exception, equality) so the rendered tracebacks
are representative rather than uniform.
"""
from __future__ import annotations

import argparse
import math
import pathlib

# Each failing test cycles through one of these bodies so the report exercises
# more than one traceback shape. Keep them stdlib-only and self-contained.
FAIL_BODIES = [
    "    expected = {i}\n    actual = {i} + 1  # off by one\n    assert actual == expected\n",
    "    data = {{'id': {i}}}\n    assert data['missing'] == {i}  # KeyError\n",
    "    values = [n for n in range({i} % 7)]\n    assert sum(values) == 9999  # arithmetic mismatch\n",
    "    text = 'row-{i}'\n    assert text.startswith('col-')  # string assertion\n",
    "    raise RuntimeError('boom in test {i}')\n",
]

PASS_BODIES = [
    "    assert {i} + 0 == {i}\n",
    "    assert str({i}) == '{i}'\n",
    "    assert sorted([3, 1, 2]) == [1, 2, 3]\n",
    "    assert len('x' * ({i} % 5)) == {i} % 5\n",
]


def gen(out: pathlib.Path, total: int, fail: int, per_file: int) -> tuple[int, int]:
    out.mkdir(parents=True, exist_ok=True)
    fail_every = max(1, total // fail) if fail else total + 1
    n_files = math.ceil(total / per_file)
    written = 0
    failures = 0
    for fidx in range(n_files):
        lines = [f'"""Auto-generated dummy suite file {fidx}."""\n\n']
        for local in range(per_file):
            i = fidx * per_file + local
            if i >= total:
                break
            is_fail = fail and (i % fail_every == 0) and failures < fail
            if is_fail:
                body = FAIL_BODIES[failures % len(FAIL_BODIES)].format(i=i)
                lines.append(f"def test_fail_{i}():\n{body}\n")
                failures += 1
            else:
                body = PASS_BODIES[i % len(PASS_BODIES)].format(i=i)
                lines.append(f"def test_pass_{i}():\n{body}\n")
            written += 1
        (out / f"test_dummy_{fidx:04d}.py").write_text("".join(lines))
    return written, failures


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", required=True, type=pathlib.Path, help="output tests dir")
    ap.add_argument("--total", type=int, default=3000, help="total test functions")
    ap.add_argument("--fail", type=int, default=100, help="approx failing tests")
    ap.add_argument("--per-file", type=int, default=50, help="tests per file")
    args = ap.parse_args()
    written, failures = gen(args.out, args.total, args.fail, args.per_file)
    print(f"wrote {written} tests ({failures} failing) to {args.out}")


if __name__ == "__main__":
    main()
