#!/usr/bin/env python3
"""Reproducible token-savings benchmark for cartoon's test-runner adapters.

For each runner it generates a deterministic suite with injected failures,
then measures what the agent would read (stdout + stderr) for:

  * the verbose baseline        (pytest -v, unittest -v, cargo test, go test -v)
  * the quiet baseline agents actually run
                                (pytest -q --tb=short, unittest default,
                                 cargo test -q, go test default)
  * cartoon wrapping each of those

Tokens are counted with cartoon's own tokenizer (o200k, via `cartoon ingest`
and its stats ledger) so every number uses the same counter. Runners that are
not installed are skipped and listed in the output.

    python3 benchmarks/run.py                       # cartoon from target/release or PATH
    python3 benchmarks/run.py --cartoon ./target/release/cartoon --out benchmarks/results

Writes <out>/results.json and <out>/results.md (the table in benchmarks/README.md).
"""
from __future__ import annotations

import argparse
import json
import os
import pathlib
import platform
import shutil
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import gen_dummy_suite  # noqa: E402

TOTAL = 600  # tests per suite
FAIL = 20  # injected failures per suite


# --- suite generators -------------------------------------------------------


def gen_pytest(root: pathlib.Path) -> None:
    gen_dummy_suite.gen(root / "tests", TOTAL, FAIL, 50)


def gen_unittest(root: pathlib.Path) -> None:
    every = TOTAL // FAIL
    for f in range(TOTAL // 50):
        lines = ["import unittest\n\n\nclass Dummy%dTest(unittest.TestCase):\n" % f]
        for k in range(50):
            i = f * 50 + k
            if i % every == 0:
                lines.append(
                    f"    def test_fail_{i}(self):\n"
                    f"        data = {{'id': {i}}}\n"
                    f"        self.assertEqual(data['id'] + 1, {i})\n\n"
                )
            else:
                lines.append(f"    def test_pass_{i}(self):\n        self.assertEqual({i} + 0, {i})\n\n")
        (root / f"test_dummy_{f:03d}.py").write_text("".join(lines))


def gen_cargo(root: pathlib.Path) -> None:
    every = TOTAL // FAIL
    (root / "src").mkdir(parents=True)
    (root / "Cargo.toml").write_text(
        '[package]\nname = "dummy"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n'
    )
    body = ["pub fn add(a: u64, b: u64) -> u64 {\n    a + b\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n"]
    for i in range(TOTAL):
        if i % every == 0:
            body.append(f"    #[test]\n    fn fail_{i}() {{\n        assert_eq!(add({i}, 1), {i}, \"off by one\");\n    }}\n\n")
        else:
            body.append(f"    #[test]\n    fn pass_{i}() {{\n        assert_eq!(add({i}, 0), {i});\n    }}\n\n")
    body.append("}\n")
    (root / "src" / "lib.rs").write_text("".join(body))


def gen_go(root: pathlib.Path) -> None:
    every = TOTAL // FAIL
    (root / "go.mod").write_text("module dummy\n\ngo 1.21\n")
    (root / "dummy.go").write_text("package dummy\n\nfunc Add(a, b int) int { return a + b }\n")
    for f in range(TOTAL // 50):
        lines = ['package dummy\n\nimport "testing"\n\n']
        for k in range(50):
            i = f * 50 + k
            if i % every == 0:
                lines.append(
                    f"func TestFail{i}(t *testing.T) {{\n"
                    f'\tif got := Add({i}, 1); got != {i} {{\n\t\tt.Errorf("Add({i}, 1) = %d, want {i}", got)\n\t}}\n}}\n\n'
                )
            else:
                lines.append(f"func TestPass{i}(t *testing.T) {{\n\tif Add({i}, 0) != {i} {{\n\t\tt.Fatal()\n\t}}\n}}\n\n")
        (root / f"dummy_{f:03d}_test.go").write_text("".join(lines))


# Each suite: (generator, required tool, warm-up command, [(label, argv)]).
# The first argv of each pair is the verbose baseline, the second the quiet
# one agents actually run.
SUITES = {
    "pytest": (gen_pytest, "pytest", None, [
        ("verbose", ["pytest", "-v", "tests"]),
        ("quiet", ["pytest", "-q", "--tb=short", "tests"]),
    ]),
    "unittest": (gen_unittest, "python3", None, [
        ("verbose", ["python3", "-m", "unittest", "-v"]),
        ("quiet", ["python3", "-m", "unittest"]),
    ]),
    "cargo test": (gen_cargo, "cargo", ["cargo", "test", "--no-run", "-q"], [
        ("verbose", ["cargo", "test"]),
        ("quiet", ["cargo", "test", "-q"]),
    ]),
    "go test": (gen_go, "go", ["go", "test", "-run", "^$", "./..."], [
        ("verbose", ["go", "test", "-v", "./..."]),
        ("quiet", ["go", "test", "./..."]),
    ]),
}


# --- measurement ------------------------------------------------------------


class Counter:
    """Counts tokens with cartoon's tokenizer: `cartoon ingest` records the
    input's token count in an isolated stats ledger."""

    def __init__(self, cartoon: str, env: dict, scratch: pathlib.Path):
        self.cartoon, self.env, self.scratch = cartoon, env, scratch
        self.ledger = pathlib.Path(env["XDG_STATE_HOME"]) / "cartoon" / "stats.jsonl"

    def __call__(self, text: str) -> int:
        if not text:
            return 0
        f = self.scratch / "count.txt"
        f.write_text(text)
        subprocess.run([self.cartoon, "ingest", str(f)], env=self.env, capture_output=True, check=True)
        return json.loads(self.ledger.read_text().splitlines()[-1])["tokens_in"]


def run(argv: list[str], cwd: pathlib.Path, env: dict) -> tuple[int, str]:
    p = subprocess.run(argv, cwd=cwd, env=env, capture_output=True, text=True, errors="replace")
    return p.returncode, p.stdout + p.stderr


def tool_version(tool: str) -> str:
    try:
        out = subprocess.run([tool, "--version"] if tool != "go" else ["go", "version"],
                             capture_output=True, text=True).stdout
        return out.strip().splitlines()[0] if out.strip() else "?"
    except OSError:
        return "missing"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    default = HERE.parent / "target" / "release" / "cartoon"
    ap.add_argument("--cartoon", default=str(default) if default.exists() else shutil.which("cartoon"))
    ap.add_argument("--out", type=pathlib.Path, default=HERE / "results")
    args = ap.parse_args()
    if not args.cartoon:
        sys.exit("cartoon binary not found: build with `cargo build --release` or pass --cartoon")
    cartoon = str(pathlib.Path(args.cartoon).resolve())

    scratch = pathlib.Path(tempfile.mkdtemp(prefix="cartoon-bench-"))
    env = dict(os.environ, XDG_STATE_HOME=str(scratch / "state"), XDG_CONFIG_HOME=str(scratch / "config"),
               NO_COLOR="1", CARGO_TERM_COLOR="never", CARTOON_NO_WRAP="1")
    count = Counter(cartoon, env, scratch)
    version = subprocess.run([cartoon, "--version"], capture_output=True, text=True).stdout.strip()

    rows, skipped, tools = [], [], {}
    for name, (gen, tool, warm, cmds) in SUITES.items():
        if shutil.which(tool) is None:
            skipped.append(name)
            continue
        tools[name] = tool_version(tool if name != "unittest" else "python3")
        root = scratch / name.replace(" ", "_")
        root.mkdir()
        gen(root)
        if warm:
            run(warm, root, env)  # compile once: measure test output, not the build
        for label, argv in cmds:
            raw_exit, raw = run(argv, root, env)
            wrapped_exit, wrapped = run([cartoon, *argv], root, env)
            raw_t, wrapped_t = count(raw), count(wrapped)
            rows.append({
                "suite": name,
                "baseline": label,
                "command": " ".join(argv),
                "baseline_tokens": raw_t,
                "cartoon_tokens": wrapped_t,
                "saved_pct": round(100 * (raw_t - wrapped_t) / raw_t, 1) if raw_t else 0.0,
                "exit_baseline": raw_exit,
                "exit_cartoon": wrapped_exit,
            })
            print(f"{name:10} {label:8} {raw_t:>7} -> {wrapped_t:>6}  exit {raw_exit}/{wrapped_exit}", file=sys.stderr)

    result = {
        "cartoon": version,
        "platform": f"{platform.system()} {platform.machine()}",
        "suite_size": {"tests": TOTAL, "failing": FAIL},
        "tools": tools,
        "skipped": skipped,
        "rows": rows,
    }
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    md = [
        f"{version}, {result['platform']}; {TOTAL} tests per suite, {FAIL} failing. "
        "Tokens: o200k, stdout + stderr.",
        "",
        "| suite | baseline | command | baseline tokens | cartoon tokens | saved | exit (raw/cartoon) |",
        "|---|---|---|---:|---:|---:|---|",
    ]
    for r in rows:
        md.append(
            f"| {r['suite']} | {r['baseline']} | `{r['command']}` | {r['baseline_tokens']:,} | "
            f"{r['cartoon_tokens']:,} | **{r['saved_pct']}%** | {r['exit_baseline']}/{r['exit_cartoon']} |"
        )
    if skipped:
        md += ["", f"Skipped (runner not installed): {', '.join(skipped)}."]
    (args.out / "results.md").write_text("\n".join(md) + "\n")
    print("\n".join(md))
    shutil.rmtree(scratch, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
