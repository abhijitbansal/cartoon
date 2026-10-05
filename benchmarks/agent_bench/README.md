# Agent fix-rate benchmark

`run.py` (one level up) measures how many tokens cartoon saves on a single
command. This benchmark asks the question that matters to users: **does an
agent fix a broken build as often with cartoon, and with fewer tokens, turns
and dollars?**

`bench.py` runs Claude Code headless (`claude -p`) on small task repos, once
without cartoon (control) and once with cartoon's hook (treatment), and
checks whether the build is green afterwards.

## Tasks

Each `tasks/<name>/` holds `repo/` (the buggy project), `fix/` (the reference
fix, overlaid file by file) and `task.json` (prompt, check command,
protected paths, required tools).

| task | toolchain | bug | check |
|---|---|---|---|
| `py_roman` | python3 + pytest | `to_roman()` lacks `CM`, so 900s come out non-canonical (218 tests, 16 failing) | `python3 -m pytest` |
| `rust_ringbuf` | cargo | `RingBuffer::get` forgets the modulo after wraparound, so indexing panics (12 tests, 6 failing) | `cargo test` |
| `go_wordfreq` | go | `TopN` breaks count ties in reverse alphabetical order (4 failing subtests) | `go test ./...` |
| `ts_config` | tsc | 5 strict-mode errors: possibly-undefined env vars, `Array.find`, indexed access | `tsc --noEmit -p .`, with no `@ts-ignore`, `as any` or `!` |
| `py_lint` | ruff + pytest | 6 ruff findings, two of them real bugs (a mutable default argument and a misspelled name) | `ruff check .` and `pytest`, with no `noqa` |

"Solved" means the task's `check` exits 0 after the agent stops. Before the
check runs, the harness restores the protected paths (tests, manifests,
tsconfig) from the pristine copy. An agent that "fixes" the build by editing
the tests is therefore scored unsolved, and the edit is counted in the
`tests edited` column.

## Running

```bash
cargo build --release            # the treatment uses target/release/cartoon

# Free: every bug fails its check, every reference fix passes, and edits
# to protected files are detected and undone.
python3 benchmarks/agent_bench/bench.py --verify-tasks

# Free: set up the sandboxes and print the exact claude commands.
python3 benchmarks/agent_bench/bench.py --dry-run -k 5

# PAID: 5 tasks x 2 arms x k sessions.
python3 benchmarks/agent_bench/bench.py --run -k 5 --model sonnet
python3 benchmarks/agent_bench/bench.py --run -k 5 --tasks py_roman,rust_ringbuf

# Re-render the table from saved results.
python3 benchmarks/agent_bench/bench.py --report benchmarks/agent_bench/results/results.json
```

Each session runs in `results/work/<task>-<arm>-<rep>/repo`, a fresh copy
committed to a throwaway git repo:

```
claude -p "<task prompt>" --output-format json
  --permission-mode acceptEdits --allowedTools Bash,Edit,Write,Read,Glob,Grep
  --setting-sources project          # the user's own hooks/plugins stay out of both arms
  --settings '<arm settings>'        # treatment: PreToolUse -> `cartoon hook rewrite`
  --no-session-persistence --max-budget-usd 1.0 [--model M]
```

How the two arms differ:

- **Both arms** get the same prompt, tools and PATH (cartoon's directory
  first). Each run has its own `XDG_STATE_HOME`/`XDG_CONFIG_HOME`, and
  `BASH_ENV` is unset and `CARTOON_NO_SHIM=1`, so cartoon shims can't leak in.
- **Control** gets `--settings '{}'` and `CARTOON_NO_WRAP=1`. That disables
  the hook even if one is configured somewhere unexpected.
- **Treatment** (`--treatment hook`, the default) gets only the PreToolUse
  hook. `--treatment plugin` loads this repo as a plugin
  (`--plugin-dir`), which adds the skill and the SessionStart hook. That
  measures the full plugin experience, including the extra prompt text.
- The arm order is shuffled within each repetition (`--seed`), so drift over
  time affects both arms equally.

**Sandboxing.** `Bash` is allowed without a prompt, so the agent can run
anything as your user. The sandbox directory only scopes the file tools.
Run the paid benchmark in a container or VM with no credentials besides the
API key.

## Results

`--run` writes `results/results.json` and `results/results.md`, and
rewrites both after every session, so an interrupted run keeps its data.

- `results.json` has `meta` (date, claude and cartoon versions, model, k,
  seed, task descriptions), `summary` (one row per task and arm, plus `ALL`)
  and `runs` (one record per session). A run record holds: `solved`,
  `num_turns`, `input_tokens` (fresh + cache-write + cache-read, plus each
  part separately), `output_tokens`, `cost_usd` (Claude Code's
  `total_cost_usd`), `wall_s`, `cartoon_wrapped` (how many commands cartoon
  ran, read from the run's own stats ledger), `tampered` (the protected
  paths the agent changed), `timed_out` and the agent's exit status.
- `results.md` is a table with these columns: solved k/n, median turns,
  median input and output tokens, mean cost, median wall time, median
  wrapped commands, tests edited. A final line gives the treatment-vs-control
  deltas across all tasks.
- Every session's raw `claude` JSON, its stderr and the check output stay
  under `results/work/` (git-ignored). Add `--keep-workdirs` to keep the
  edited repos too.

A treatment row with `cartoon-wrapped cmds = 0` means the hook never fired:
the agent ran nothing cartoon wraps, or the hook was not loaded. Check that
before reading anything into the token numbers.

## Cost estimate

This is a rough estimate; no paid run has been made yet. On these tasks a
session is typically 5–15 turns and 50k–300k input tokens, mostly cache
reads. With a Sonnet-class model that comes to about $0.05–$0.50 per session.

| k | sessions | typical | hard cap (`--max-budget-usd 1.0`) |
|---:|---:|---:|---:|
| 1 | 10 | ~$1–5 | $10 |
| 5 | 50 | ~$3–25 | $50 |
| 10 | 100 | ~$5–50 | $100 |

`--dry-run` prints the hard cap for the chosen options. Wall time is about
1–3 minutes per session, run sequentially.

## Statistical caveats

- **Repeat.** Agent runs are nondeterministic, so one session per arm says
  nothing. Use k ≥ 5 per task and arm, and k ≥ 10 before quoting a number.
- **Fix rate hits a ceiling.** These bugs are deliberately small, so expect
  both arms to solve nearly everything. Then "same fix rate" is the result,
  and the measurable effect is in tokens, turns and cost. With n = 25 per arm,
  a 95% interval on a 96% solve rate spans roughly 80–99%. A small
  difference in fix rate is noise unless k is large.
- **Compare per task, then aggregate.** Token counts differ by an order of
  magnitude between tasks and are heavy-tailed: one session that loops can
  dominate a mean. That is why the table reports medians. For a claim, pair
  the arms by task and repetition, and bootstrap the ratio of
  treatment to control.
- **Cost depends on caching.** `total_cost_usd` depends on prompt-cache hits,
  which depend on timing between sessions. Input tokens (all three kinds
  summed) are the steadier measure of what the model read.
- **The agent decides what to run.** cartoon only helps when the agent runs
  something verbose. An agent that already runs `pytest -q` sees little
  difference (see `run.py`'s quiet baselines). `cartoon_wrapped` shows how
  often the hook actually applied.
- **Pin versions.** Record and keep fixed the model (`--model`), the Claude
  Code version and the cartoon build. `meta` stores the versions, but not
  server-side model updates behind an alias.
- **Small, synthetic tasks.** Five toy repos don't stand in for real
  codebases, where outputs and fix loops are longer. Treat the result as a
  lower bound on the effect, and add tasks from real projects (one directory
  each, same layout) to widen it.
