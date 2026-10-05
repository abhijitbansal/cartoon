#!/usr/bin/env python3
"""Agent fix-rate benchmark: does cartoon change whether, and how cheaply, a
coding agent fixes a failing build?

Each task in tasks/<name>/ is a small repo with a real bug and a failing
check (pytest, cargo test, go test, tsc, ruff). The harness runs Claude Code
headless on a fresh copy of the repo, twice per repetition:

  control    no cartoon hook (CARTOON_NO_WRAP=1 as a belt-and-braces guard)
  treatment  cartoon's PreToolUse hook (`cartoon hook rewrite`) injected
             with --settings, or the whole plugin with --treatment plugin

and records: solved (the task's check passes after the agent stops, with the
protected test files restored from the pristine copy), turns, input/output
tokens, cost, wall time, and how many commands cartoon wrapped.

    python3 benchmarks/agent_bench/bench.py --verify-tasks   # free: bugs fail, fixes pass
    python3 benchmarks/agent_bench/bench.py --dry-run -k 3   # free: set up, print commands
    python3 benchmarks/agent_bench/bench.py --run -k 3 --model sonnet   # PAID
    python3 benchmarks/agent_bench/bench.py --report results/results.json

Writes <out>/results.json and <out>/results.md. See README.md.
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import pathlib
import random
import shlex
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

HERE = pathlib.Path(__file__).resolve().parent
REPO_ROOT = HERE.parents[1]
TASKS_DIR = HERE / "tasks"
ARMS = ("control", "treatment")
# Tools the agent may use without a prompt. Headless mode denies anything
# else, so the agent can read, edit and run commands in its sandbox copy.
ALLOWED_TOOLS = "Bash,Edit,Write,Read,Glob,Grep"


# --- tasks --------------------------------------------------------------------


def load_tasks(names: list[str] | None) -> list[dict]:
    tasks = []
    for d in sorted(p for p in TASKS_DIR.iterdir() if (p / "task.json").is_file()):
        if names and d.name not in names:
            continue
        task = json.loads((d / "task.json").read_text())
        task.update(name=d.name, repo=d / "repo", fix=d / "fix")
        tasks.append(task)
    if names:
        missing = set(names) - {t["name"] for t in tasks}
        if missing:
            sys.exit(f"unknown task(s): {', '.join(sorted(missing))}")
    return tasks


def missing_tools(task: dict) -> list[str]:
    return [t for t in task.get("requires", []) if shutil.which(t) is None]


def copy_repo(task: dict, dest: pathlib.Path) -> pathlib.Path:
    shutil.copytree(task["repo"], dest)
    return dest


def overlay(src: pathlib.Path, dest: pathlib.Path) -> None:
    """Copy every file under src onto dest (the reference fix). Fresh mtimes
    (copy, not copy2) so cargo/go notice the change after a build."""
    for f in src.rglob("*"):
        if f.is_file():
            target = dest / f.relative_to(src)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(f, target)


def restore_protected(task: dict, workdir: pathlib.Path) -> list[str]:
    """Put the pristine protected paths back; return the ones the agent changed."""
    changed = []
    for rel in task.get("protected", []):
        pristine, live = task["repo"] / rel, workdir / rel
        if pristine.is_dir():
            if not live.is_dir() or _tree(pristine) != _tree(live):
                changed.append(rel)
            shutil.rmtree(live, ignore_errors=True)
            if live.exists():
                live.unlink()
            shutil.copytree(pristine, live, copy_function=shutil.copy)
        else:
            if not live.is_file() or live.read_bytes() != pristine.read_bytes():
                changed.append(rel)
            if live.is_dir():
                shutil.rmtree(live)
            shutil.copy(pristine, live)
    return changed


def _tree(root: pathlib.Path) -> dict[str, bytes]:
    return {
        str(f.relative_to(root)): f.read_bytes()
        for f in root.rglob("*")
        if f.is_file() and "__pycache__" not in f.parts
    }


def run_check(task: dict, workdir: pathlib.Path, env: dict | None = None) -> tuple[int, str]:
    proc = subprocess.run(
        ["sh", "-c", task["check"]],
        cwd=workdir,
        env=env,
        capture_output=True,
        text=True,
        timeout=600,
    )
    return proc.returncode, proc.stdout + proc.stderr


def check_env(workdir: pathlib.Path) -> dict:
    """The check never runs through cartoon and keeps build caches local."""
    env = dict(os.environ)
    env.pop("BASH_ENV", None)
    env.update(
        CARTOON_NO_WRAP="1",
        CARTOON_NO_SHIM="1",
        CARGO_TARGET_DIR=str(workdir / "target"),
        PYTHONDONTWRITEBYTECODE="1",
    )
    return env


def verify_tasks(tasks: list[dict]) -> int:
    """Each task's check must fail on the repo and pass with the reference fix."""
    bad = 0
    for task in tasks:
        missing = missing_tools(task)
        if missing:
            print(f"SKIP {task['name']}: missing {', '.join(missing)}")
            continue
        with tempfile.TemporaryDirectory(prefix=f"agent-bench-{task['name']}-") as tmp:
            work = copy_repo(task, pathlib.Path(tmp) / "repo")
            env = check_env(work)
            rc_bug, out_bug = run_check(task, work, env)
            overlay(task["fix"], work)
            rc_fix, out_fix = run_check(task, work, env)
            # The harness must undo edits to tests: tamper, restore, re-check.
            tampered = _tamper(task, work)
            restored = restore_protected(task, work)
            rc_restored, _ = run_check(task, work, env)
        ok = rc_bug != 0 and rc_fix == 0 and rc_restored == 0 and tampered == restored
        bad += not ok
        print(
            f"{'ok  ' if ok else 'FAIL'} {task['name']:<14} "
            f"bug: exit {rc_bug} ({_last_line(out_bug)}) | "
            f"fix: exit {rc_fix} ({_last_line(out_fix)}) | "
            f"tamper detected: {restored}"
        )
        if not ok and rc_fix != 0:
            print(out_fix[-2000:])
    print("all tasks verified" if not bad else f"{bad} task(s) failed verification")
    return 1 if bad else 0


def _tamper(task: dict, work: pathlib.Path) -> list[str]:
    """Simulate an agent editing protected files; returns what it touched."""
    touched = []
    for rel in task.get("protected", []):
        p = work / rel
        target = next((f for f in sorted(p.rglob("*")) if f.is_file()), None) if p.is_dir() else p
        if target is not None:
            target.write_text(target.read_text() + "\n// tampered\n")
            touched.append(rel)
    return touched


def _last_line(text: str) -> str:
    lines = [ln.strip() for ln in text.strip().splitlines() if ln.strip()]
    return (lines[-1][:70] if lines else "no output").replace("\n", " ")


# --- agent runs ---------------------------------------------------------------


def hook_settings(arm: str, cartoon: str, treatment: str) -> dict:
    if arm == "treatment" and treatment == "hook":
        command = f"{shlex.quote(cartoon)} hook rewrite"
        hook = {"type": "command", "command": command, "timeout": 10}
        return {"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [hook]}]}}
    return {}


def agent_command(task: dict, arm: str, args: argparse.Namespace) -> list[str]:
    cmd = [
        args.claude,
        "-p",
        task["prompt"],
        "--output-format",
        "json",
        "--permission-mode",
        "acceptEdits",
        "--allowedTools",
        ALLOWED_TOOLS,
        # Only project settings (none exist in the sandbox copy): the user's
        # own hooks, e.g. a globally installed cartoon hook, stay out of both arms.
        "--setting-sources",
        "project",
        "--settings",
        json.dumps(hook_settings(arm, args.cartoon, args.treatment)),
        "--no-session-persistence",
        "--max-budget-usd",
        str(args.max_budget_usd),
    ]
    if arm == "treatment" and args.treatment == "plugin":
        cmd += ["--plugin-dir", str(REPO_ROOT)]
    if args.model:
        cmd += ["--model", args.model]
    return cmd


def agent_env(arm: str, run_dir: pathlib.Path, workdir: pathlib.Path, cartoon: str) -> dict:
    env = dict(os.environ)
    env.pop("BASH_ENV", None)  # cartoon shell shims, if the user has them
    env.update(
        # The hook rewrites `pytest` to `cartoon -c 'pytest'`, so the binary
        # under test must be the `cartoon` on PATH. Same PATH in both arms.
        PATH=os.pathsep.join([str(pathlib.Path(cartoon).parent), env.get("PATH", "")]),
        XDG_STATE_HOME=str(run_dir / "state"),
        XDG_CONFIG_HOME=str(run_dir / "config"),
        CARGO_TARGET_DIR=str(workdir / "target"),
        CARTOON_NO_SHIM="1",
    )
    if arm == "control":
        env["CARTOON_NO_WRAP"] = "1"
    else:
        env.pop("CARTOON_NO_WRAP", None)
    return env


def plan(tasks: list[dict], k: int, seed: int) -> list[tuple[dict, str, int]]:
    """Every (task, arm, rep), arms in random order within each rep so
    time-of-day drift and API load hit both arms alike."""
    rng = random.Random(seed)
    runs = []
    for rep in range(k):
        for task in tasks:
            arms = list(ARMS)
            rng.shuffle(arms)
            runs += [(task, arm, rep) for arm in arms]
    return runs


def setup_run(task: dict, arm: str, rep: int, work_root: pathlib.Path) -> tuple[pathlib.Path, pathlib.Path]:
    run_dir = work_root / f"{task['name']}-{arm}-{rep}"
    shutil.rmtree(run_dir, ignore_errors=True)
    run_dir.mkdir(parents=True)
    workdir = copy_repo(task, run_dir / "repo")
    if shutil.which("git"):
        # A clean git repo, as agents expect; `git diff` shows their edits.
        for git in (["init", "-q"], ["add", "-A"], ["commit", "-qm", "task"]):
            subprocess.run(
                ["git", "-c", "user.name=bench", "-c", "user.email=bench@localhost", *git],
                cwd=workdir,
                check=True,
                capture_output=True,
            )
    return run_dir, workdir


def run_one(task: dict, arm: str, rep: int, args: argparse.Namespace, work_root: pathlib.Path) -> dict:
    run_dir, workdir = setup_run(task, arm, rep, work_root)
    cmd = agent_command(task, arm, args)
    start = time.monotonic()
    try:
        proc = subprocess.run(
            cmd,
            cwd=workdir,
            env=agent_env(arm, run_dir, workdir, args.cartoon),
            capture_output=True,
            text=True,
            timeout=args.timeout,
        )
        stdout, stderr, rc, timed_out = proc.stdout, proc.stderr, proc.returncode, False
    except subprocess.TimeoutExpired as e:
        stdout = e.stdout.decode() if isinstance(e.stdout, bytes) else (e.stdout or "")
        stderr, rc, timed_out = "", None, True
    wall = time.monotonic() - start
    (run_dir / "agent.stdout.json").write_text(stdout)
    (run_dir / "agent.stderr.txt").write_text(stderr)

    record = {
        "task": task["name"],
        "arm": arm,
        "rep": rep,
        "exit_code": rc,
        "timed_out": timed_out,
        "wall_s": round(wall, 1),
        **parse_agent_json(stdout),
    }
    record["tampered"] = restore_protected(task, workdir)
    check_rc, check_out = run_check(task, workdir, check_env(workdir))
    (run_dir / "check.txt").write_text(check_out)
    record["solved"] = check_rc == 0
    record["cartoon_wrapped"] = count_wrapped(run_dir / "state")
    if not args.keep_workdirs:
        shutil.rmtree(workdir, ignore_errors=True)
    return record


def parse_agent_json(stdout: str) -> dict:
    """Pull usage out of `claude -p --output-format json` (one result object)."""
    try:
        data = json.loads(stdout.strip().splitlines()[-1]) if stdout.strip() else {}
    except (json.JSONDecodeError, IndexError):
        data = {}
    usage = data.get("usage") or {}
    fresh = usage.get("input_tokens") or 0
    cache_write = usage.get("cache_creation_input_tokens") or 0
    cache_read = usage.get("cache_read_input_tokens") or 0
    return {
        "agent_result": data.get("subtype"),
        "agent_is_error": data.get("is_error"),
        "num_turns": data.get("num_turns"),
        "cost_usd": data.get("total_cost_usd"),
        "duration_api_ms": data.get("duration_api_ms"),
        # Everything the model read, cached or not; the cache split is kept.
        "input_tokens": fresh + cache_write + cache_read if usage else None,
        "input_tokens_uncached": fresh if usage else None,
        "cache_creation_input_tokens": cache_write if usage else None,
        "cache_read_input_tokens": cache_read if usage else None,
        "output_tokens": usage.get("output_tokens"),
    }


def count_wrapped(state: pathlib.Path) -> int:
    """Commands cartoon ran during the session (one stats line per run)."""
    stats = state / "cartoon" / "stats.jsonl"
    try:
        return sum(1 for line in stats.read_text().splitlines() if line.strip())
    except OSError:
        return 0


# --- reporting ----------------------------------------------------------------


def _median(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else None


def _mean(xs):
    xs = [x for x in xs if x is not None]
    return statistics.fmean(xs) if xs else None


def summarize(records: list[dict]) -> list[dict]:
    rows = []
    groups: dict[tuple[str, str], list[dict]] = {}
    for r in records:
        groups.setdefault((r["task"], r["arm"]), []).append(r)
        groups.setdefault(("ALL", r["arm"]), []).append(r)
    for (task, arm), rs in sorted(groups.items(), key=lambda kv: (kv[0][0] == "ALL", kv[0])):
        rows.append(
            {
                "task": task,
                "arm": arm,
                "n": len(rs),
                "solved": sum(r["solved"] for r in rs),
                "median_turns": _median(r["num_turns"] for r in rs),
                "median_input_tokens": _median(r["input_tokens"] for r in rs),
                "median_output_tokens": _median(r["output_tokens"] for r in rs),
                "mean_cost_usd": _mean(r["cost_usd"] for r in rs),
                "median_wall_s": _median(r["wall_s"] for r in rs),
                "median_wrapped": _median(r.get("cartoon_wrapped") for r in rs),
                "tampered": sum(bool(r.get("tampered")) for r in rs),
            }
        )
    return rows


def _fmt(v, kind=""):
    if v is None:
        return "–"
    if kind == "usd":
        return f"${v:.3f}"
    if isinstance(v, float) and not v.is_integer():
        return f"{v:,.1f}"
    return f"{int(v):,}"


def _delta(t, c):
    if t is None or c in (None, 0):
        return "–"
    return f"{(t - c) / c:+.0%}"


def render_markdown(meta: dict, rows: list[dict]) -> str:
    out = [
        f"Agent fix-rate benchmark, {meta.get('date', '?')}: {meta.get('agent', 'claude')}"
        f" (model: {meta.get('model') or 'default'}), treatment = cartoon {meta.get('treatment', 'hook')}"
        f", k = {meta.get('k')} per task and arm.",
        "",
        "| task | arm | solved | turns (median) | input tokens (median) | output tokens (median)"
        " | cost (mean) | wall s (median) | cartoon-wrapped cmds | tests edited |",
        "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for r in rows:
        out.append(
            f"| {r['task']} | {r['arm']} | {r['solved']}/{r['n']} | {_fmt(r['median_turns'])}"
            f" | {_fmt(r['median_input_tokens'])} | {_fmt(r['median_output_tokens'])}"
            f" | {_fmt(r['mean_cost_usd'], 'usd')} | {_fmt(r['median_wall_s'])}"
            f" | {_fmt(r['median_wrapped'])} | {r['tampered']} |"
        )
    by = {(r["task"], r["arm"]): r for r in rows}
    if ("ALL", "control") in by and ("ALL", "treatment") in by:
        c, t = by[("ALL", "control")], by[("ALL", "treatment")]
        out += [
            "",
            f"Treatment vs control, all tasks: solved {t['solved']}/{t['n']} vs {c['solved']}/{c['n']};"
            f" median input tokens {_delta(t['median_input_tokens'], c['median_input_tokens'])},"
            f" median turns {_delta(t['median_turns'], c['median_turns'])},"
            f" mean cost {_delta(t['mean_cost_usd'], c['mean_cost_usd'])},"
            f" median wall time {_delta(t['median_wall_s'], c['median_wall_s'])}.",
            "",
            "Medians over few runs are noisy: see README.md (statistical caveats) before quoting a delta.",
        ]
    return "\n".join(out) + "\n"


def write_results(out: pathlib.Path, meta: dict, records: list[dict]) -> None:
    out.mkdir(parents=True, exist_ok=True)
    rows = summarize(records)
    (out / "results.json").write_text(
        json.dumps({"meta": meta, "summary": rows, "runs": records}, indent=2) + "\n"
    )
    (out / "results.md").write_text(render_markdown(meta, rows))


# --- main ---------------------------------------------------------------------


def find_cartoon(explicit: str | None) -> str | None:
    if explicit:
        return str(pathlib.Path(explicit).resolve())
    for p in (REPO_ROOT / "target/release/cartoon", REPO_ROOT / "target/debug/cartoon"):
        if p.is_file():
            return str(p)
    return shutil.which("cartoon")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    mode = ap.add_mutually_exclusive_group(required=True)
    mode.add_argument(
        "--verify-tasks", action="store_true", help="check bugs fail and reference fixes pass (free)"
    )
    mode.add_argument(
        "--dry-run", action="store_true", help="set up workdirs and print the agent commands (free)"
    )
    mode.add_argument("--run", action="store_true", help="run the agent sessions (PAID)")
    mode.add_argument("--report", metavar="RESULTS_JSON", help="re-render results.md from a results.json")
    ap.add_argument("--tasks", help="comma-separated task names (default: all)")
    ap.add_argument("-k", type=int, default=3, help="repetitions per task and arm (default 3)")
    ap.add_argument("--model", help="model alias or name passed to claude --model")
    ap.add_argument(
        "--treatment",
        choices=["hook", "plugin"],
        default="hook",
        help="hook: inject `cartoon hook rewrite` via --settings; plugin: --plugin-dir this repo",
    )
    ap.add_argument("--claude", default="claude", help="claude CLI to run (default: claude on PATH)")
    ap.add_argument("--cartoon", help="cartoon binary (default: target/release, target/debug, then PATH)")
    ap.add_argument("--max-budget-usd", type=float, default=1.0, help="per-session spend cap (default 1.0)")
    ap.add_argument("--timeout", type=int, default=900, help="per-session wall-clock limit in seconds")
    ap.add_argument("--seed", type=int, default=0, help="seed for the arm order")
    ap.add_argument("--out", type=pathlib.Path, default=HERE / "results")
    ap.add_argument("--keep-workdirs", action="store_true", help="keep each run's repo copy for inspection")
    args = ap.parse_args(argv)

    if args.report:
        data = json.loads(pathlib.Path(args.report).read_text())
        sys.stdout.write(render_markdown(data["meta"], summarize(data["runs"])))
        return 0

    tasks = load_tasks(args.tasks.split(",") if args.tasks else None)
    if args.verify_tasks:
        return verify_tasks(tasks)

    cartoon = find_cartoon(args.cartoon)
    if cartoon is None:
        sys.exit("cartoon binary not found: cargo build --release, or pass --cartoon")
    args.cartoon = cartoon
    for task in tasks:
        missing = missing_tools(task)
        if missing:
            sys.exit(f"task {task['name']} needs {', '.join(missing)} (or drop it with --tasks)")

    runs = plan(tasks, args.k, args.seed)
    work_root = args.out / "work"
    print(
        f"{len(runs)} sessions ({len(tasks)} tasks x {len(ARMS)} arms x k={args.k}); "
        f"spend cap ${len(runs) * args.max_budget_usd:.2f} (= sessions x --max-budget-usd)",
        file=sys.stderr,
    )

    if args.dry_run:
        for task, arm, rep in runs:
            run_dir, workdir = setup_run(task, arm, rep, work_root)
            env = agent_env(arm, run_dir, workdir, args.cartoon)
            env_prefix = f'PATH={shlex.quote(str(pathlib.Path(args.cartoon).parent))}:"$PATH" ' + " ".join(
                f"{k}={shlex.quote(env[k])}"
                for k in (
                    "XDG_STATE_HOME",
                    "XDG_CONFIG_HOME",
                    "CARGO_TARGET_DIR",
                    "CARTOON_NO_SHIM",
                    "CARTOON_NO_WRAP",
                )
                if k in env
            )
            print(f"# {task['name']} {arm} rep {rep}")
            print(
                f"(cd {shlex.quote(str(workdir))} && env -u BASH_ENV {env_prefix} {shlex.join(agent_command(task, arm, args))})"
            )
            print(f"#   then: restore {task.get('protected')} and run: {task['check']}")
        print(f"# workdirs set up under {work_root}", file=sys.stderr)
        return 0

    if shutil.which(args.claude) is None:
        sys.exit(f"{args.claude} not found")
    meta = {
        "date": datetime.date.today().isoformat(),
        "agent": subprocess.run([args.claude, "--version"], capture_output=True, text=True).stdout.strip(),
        "cartoon": subprocess.run([cartoon, "--version"], capture_output=True, text=True).stdout.strip(),
        "model": args.model,
        "treatment": args.treatment,
        "k": args.k,
        "seed": args.seed,
        "max_budget_usd": args.max_budget_usd,
        "tasks": {t["name"]: t["description"] for t in tasks},
    }
    records = []
    for i, (task, arm, rep) in enumerate(runs, 1):
        print(f"[{i}/{len(runs)}] {task['name']} {arm} rep {rep} ...", file=sys.stderr, flush=True)
        rec = run_one(task, arm, rep, args, work_root)
        records.append(rec)
        print(
            f"    solved={rec['solved']} turns={rec['num_turns']} in={rec['input_tokens']} "
            f"out={rec['output_tokens']} cost={rec['cost_usd']} wall={rec['wall_s']}s "
            f"wrapped={rec['cartoon_wrapped']}",
            file=sys.stderr,
        )
        write_results(args.out, meta, records)  # partial results survive a crash
    sys.stdout.write((args.out / "results.md").read_text())
    return 0


if __name__ == "__main__":
    sys.exit(main())
