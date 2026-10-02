# Cartoon whole-repo review — 2026-10-02

**Scope:** `main` at `8c18510` (0.6.0).
**Method:** five parallel review lenses: hook security, core pipeline, adapters, ladder/archive/TOON, and packaging/docs/product. Every finding below was reproduced by running the built binary, real tools (pytest 9, jest 30, vitest 3/5, go 1.24, cargo 1.97, ruff, mypy, tsc) or the live registries. A finding marked *(code)* was confirmed by reading the code only. The 2026-09-05 review's items are not repeated unless their fix is incomplete.

**Local gate at HEAD (Linux, Python 3.11):** fmt and clippy are clean. `cargo test` had **two failures**:
- `junit_flag_renders_a_test_report_for_any_command`: a real bug, fixed on this branch (§7.1).
- `e2e_unittest_failing_suite`: a real adapter weakness (§1.9), not fixed here.

## Verdict

The architecture is sound: the adapter trait, the net-savings guard, the raw-log archive and fail-open hooks. The weak spot is the core promise, "never lose what the agent needs". Several paths turn a **failed run into a clean-looking report**, and those are the bugs that cost an agent the most: it trusts the summary and moves on. Fix §1 before any new adapter or feature.

Second: the most common install path (`uv tool install` on Ubuntu 22.04 / Debian 12 / python:* images) **does not work** (§2.1).

---

## 1. P0: output that misreports failure

| # | What | Where | Repro (observed) | Fix |
|---|---|---|---|---|
| 1.1 | **JSON tail swallows everything before it, in the default safe tier.** `detect_json` returns the last parseable document and only that is encoded. | `src/fallback.rs:14-28`, `src/app.rs` `transform` | 50 `ok` lines + `test_case_51 ... FAILED` + `{"coverage": 81.5}` → output is `coverage: 81.5` + raw_log. The failure is gone. NDJSON keeps only the last record. | Accept JSON only when the whole trimmed stdout is one document; otherwise ladder the prefix and TOON the tail. Parse NDJSON line by line into an array. |
| 1.2 | **The report never carries the exit code.** When an adapter parses `failed: 0` but the command exited non-zero, the raw streams are dropped. This is the root of 1.3–1.8. | `src/app.rs` `run_with_adapter` | See below. | Central rule: if `code != 0` and the report shows no failures or errors, add `exit_code` and pass stdout/stderr through, or take the generic path. Render `exit_code` whenever it is non-zero. |
| 1.3 | jest / vitest 3: **suite-level errors ignored** (missing module, syntax error in a test file, unhandled async error). | `adapters/jest.rs:44,68`, `vitest.rs:29` | `total 30, passed 30, failed 0`, exit 1. The error text is gone. | Emit a Failure per `testResults[i]` with `status=="failed"` and no failed assertion; use `numRuntimeErrorTestSuites`. |
| 1.4 | **vitest 5 adapter does not work**: `--reporter=json` now writes `.vitest/json/output.json` into the user's repo. | `adapters/vitest.rs` | The agent sees `JSON report written to …` + exit 1, with no failure detail. | Inject `--reporter=default --reporter=json --outputFile.json=<tmp>`. |
| 1.5 | pytest: `pytest.exit(...)` mid-run and `--cov-fail-under` failures are reported as all-pass. | `adapters/pytest.rs:67-75` | `16 passed, 0 failed`, exit 3. The reason is lost. | 1.2 covers it. |
| 1.6 | go test: a test killed by `-timeout`, `TestMain` + `os.Exit`, and **Go ≥1.24 `build-output` events** (compiler errors keyed by `ImportPath`, not `Package`). | `adapters/go_test.rs:57,138,228` | Timeout → `1 passed`, exit 1. `undefined: x` → only `[build failed]`, compiler line absent. | Treat `run` with no terminal event as a failure; map `ImportPath` and include `build-output`. |
| 1.7 | swift-build / xcodebuild-build: a single warning masks a linker/signing failure. | `swift_build.rs:43`, `xcodebuild_build.rs:44` *(probe)* | `errors: 0, warnings: 1`, exit 1, the `ld:` error dropped. | Use `errors == 0`, not `matched == 0`. |
| 1.8 | unittest: `unexpected successes=N` not parsed. | `adapters/unittest.rs:99-116` | `34 passed, 0 failed`, exit 1. | Parse it. |
| 1.9 | unittest: **drops the assertion diff and the user's custom message** (`: unexpected roles for alpha`); report repeats absolute paths. | `adapters/unittest.rs:135-155` | 6-failure fixture: report 1146 tokens vs raw 1116, so the guard passes it through and `e2e_unittest_failing_suite` fails on Python 3.11. | Relativize paths to cwd in `report.rs` (helps every adapter); don't repeat `msg` as the last trace line; keep the diff tail. |
| 1.10 | cargo test: **`loc` and `msg` always empty on current Rust** (`thread 'x' (12345) panicked at …`). With `RUST_BACKTRACE` set, the 20-line trace is all `core::panicking` frames. | `adapters/cargo_test.rs:129-140` | `"tests::bad","",""` | Regex `^thread '.*'(?: \(\d+\))? panicked at `; filter std/core frames. |
| 1.11 | cargo test `--nocapture`: `failed: 1`, empty failures list. | `cargo_test.rs:261` | | Add a Failure for every name under `failures:`. |
| 1.12 | **Traces cut to the first N lines with no marker.** pytest keeps 20 setup lines and drops the `>`/`E` lines; pre-commit drops violations 22–40. | `adapters/report.rs:74` | | Prefer `E`/`>`/panic lines and the tail; always emit `+N lines omitted (raw_log)`. |
| 1.13 | Duplicate failure ids (same test name in two files) overwrite each other's trace. | `report.rs:70` (Map keyed by id) | jest: 2 rows, 1 trace. | Key traces by row index or `id@loc`. |
| 1.14 | `--max-tokens` cuts by position, not importance; a mid-log `ERROR` is dropped while `step N compiled` survives. Marker can exceed the ceiling. | `src/budget.rs` | `--max-tokens 300` on a 300-line log drops line 150's error. | Reserve budget around `is_error_line` hits first. |

Also lossy, but lower: rustc/cargo-build `help:` and span labels dropped (`ladder/diagnostics.rs:86`, `cargo_build.rs:238`); pytest captured stdout of failing tests lost (inject `-o junit_logging=all`); tsc elaboration lines and location-less `TS5083` errors dropped; mypy column is 0-based (`mypy.rs:151`, off by one vs mypy's own output).

## 2. P0: install is broken for a large share of users

| # | What | Evidence | Fix |
|---|---|---|---|
| 2.1 | **x86_64 Linux binaries need glibc 2.39** (built natively on ubuntu-24.04). They fail on Ubuntu 22.04, Debian 12, RHEL 9, AL2023 and `python:*` images, which is where agents run. | PyPI 0.6.0 x86_64 wheel is `manylinux_2_39`; npm `cartoon-wrap-linux-x64` needs `GLIBC_2.39`. No sdist, no musllinux. aarch64 is fine (`manylinux_2_17`). | `release.yml`: build `x86_64/aarch64-unknown-linux-musl` (static) for tarballs and npm; maturin `manylinux: 2014` + musllinux; publish an sdist. |
| 2.2 | **Windows npm channel is dead.** `cartoon-wrap-win32-x64` is npm's `0.0.1-security` placeholder; `release.yml:204-213` swallows the publish failure; the wrapper tells Windows users to "reinstall with optional deps". | npm registry. | Move platform packages to a scope (`@cartoon-wrap/win32-x64`); fix the error message to point at `cargo install`. |
| 2.3 | npm wrapper: SIGTERM to node orphans the child; spawn errors (ENOENT, no exec bit) exit 1 silently; no `libc` field. | `packages/npm/cartoon-wrap/bin/cartoon.js:23-29` | Async `spawn`, forward INT/TERM/HUP, re-raise the child's signal, print `result.error`. |

## 3. P1: security

| # | What | Where | Fix |
|---|---|---|---|
| 3.1 | **Hook auto-approves code-loading flags.** All of these return `permissionDecision: "allow"`: `go test -exec /tmp/x`, `go test -toolexec /tmp/x`, `cargo test --config build.rustc-wrapper=/tmp/x`, `make -f /tmp/x.mk`, `make SHELL=/tmp/x`, `jest --config /tmp/j.js`, `mypy --config-file /tmp/x`, `gradle test -I /tmp/init.gradle`. Each runs a binary or script from **outside** the project with no prompt. `MUTATING_TOKENS` covers only ruff, eslint and swiftlint; `eslint -c` is gated but `jest --config` is not. `PYTEST_ADDOPTS` is in `SAFE_ENV_PREFIXES`. | `src/hook/mod.rs:111-135` | Per-tool flag *allowlist* (as `UV_HOOK_SAFE_FLAGS` does for uv), or at minimum deny `-exec`, `-toolexec`, `--config`, `-f`/`--file`/`--makefile`, `SHELL=`, `-I`/`--init-script`, `--config-file`, `-p` for pytest, `+toolchain`; any path argument outside the project → no rewrite. Drop `PYTEST_ADDOPTS`. |
| 3.2 | Hook parser is a blocklist over a quote-unaware splitter (`split_segments` + `split_whitespace`), while the approved string runs under a real shell. Today this fails closed for the cases tried, but it is fragile by construction. *(code)* | `src/hook/mod.rs:337-429,550` | Tokenize with `shell_words::split`; refuse unless every char is in `[A-Za-z0-9 _./=:@,+%-]`; fail closed if re-joining doesn't reproduce the input. Add a proptest/fuzz target for this. |
| 3.3 | **`instructions install` destroys a non-UTF-8 CLAUDE.md/AGENTS.md** (read error is treated as "absent"). `hook install` has the same pattern for `settings.json` on EACCES/EISDIR *(code)*. Settings writes are non-atomic. | `src/instructions.rs:181`, `src/hook/install.rs:256,282,334,386` | Treat only `ErrorKind::NotFound` as absent; write to a temp file in the same dir and rename. |
| 3.4 | Raw logs are world-readable (dirs 0755, files 0644). `meta.json` stores full argv + cwd, so logs of `env`, `curl -H "Authorization: …"` etc. are readable by any local user. Run ids reuse an existing dir (no O_EXCL); `\| 1` halves the salt space. | `src/archive.rs:36,157-161` | `DirBuilder::mode(0o700)`, files `0o600`, `create_dir` + retry on `AlreadyExists`. |
| 3.5 | Release supply chain: third-party actions pinned to mutable tags in jobs with `id-token: write`; `npm@latest` and unpinned `cross` in release; no workflow-level `permissions:`; tarballs have no SHA256SUMS or provenance. | `.github/workflows/release.yml` | Pin by SHA + Dependabot; `permissions: {}` at top; `actions/attest-build-provenance`; checksums file. |

## 4. P1: robustness of the run pipeline

| # | What | Repro | Fix |
|---|---|---|---|
| 4.1 | **Ctrl-C / SIGTERM / agent timeout loses everything and orphans the child.** No signal handling in `src/`. | `timeout 1 cartoon sh -c 'echo partial; sleep 5'` prints nothing (bare prints `partial`); `kill -TERM` leaves `sleep` reparented to PID 1; adapter temp files leak. | Spawn the child in its own process group; forward INT/TERM/HUP; keep reading until it exits; emit what was captured flagged `interrupted`, archive it, exit with the child's status. |
| 4.2 | **`cmd \| head` panics with exit 101** (EPIPE) and replaces the child's exit code; `>/dev/full` too. | `cartoon seq 1 200000 \| head -1` → panic, rc 101, no ledger entry. | `write_all` on a locked stdout; treat `BrokenPipe` as success; record stats before emitting. |
| 4.3 | **stderr is never compressed and ignores `--max-tokens`.** Most compiler/build noise (gcc, cargo, make, docker, curl) is on stderr. | `cartoon --max-tokens 50 sh -c 'seq 1 20000 >&2'` → 108,894 bytes. | Ladder stderr too and budget the total emitted output. |
| 4.4 | stdout/stderr fully buffered, order lost; prompts look like hangs; no liveness for long runs. | `sh -c 'echo out1; echo ERR1 >&2; echo out2'` → `out1 out2 ERR1`. | Read both pipes into one ordered, stream-tagged event log teed to the archive; stderr heartbeat for runs past N seconds. |
| 4.5 | **`--raw` / passthrough is not byte-identical**, though README says so 4×. | `cartoon --raw printf 'a\xffb'` → `a EF BF BD b`; `cartoon cat x.gz \| gunzip` fails; `printf 'caf\xe9' \| cartoon -` exits 2. | Keep `Vec<u8>` in `Captured`; lossy text only for transforms; `--raw` inherits stdio and tees. |
| 4.6 | **One typo in a committed `.cartoon.toml` stops every wrapped command from running** (`level = "aggresive"` → exit 2, command never runs), and `doctor` reports the file `ok`. Breaks fail-open. | | A bad level from config warns and uses `safe`; only `--compress` is fatal. Validate levels and unknown keys in `config::check`. |
| 4.7 | **~170 ms startup on every wrapped command**: `tiktoken o200k_base()` is built per run, even for empty output. | `cartoon true` 172 ms median vs 1.3 ms bare; 5.5 ms with `tokenizer="approx"`. | Skip tokenization for empty/small output; use `len/4` when the guard decision is clear; only build o200k when it's close. |
| 4.8 | Memory ~12× output with no cap. | `seq 1 10000000` (79 MB) → 938 MB RSS, 16.6 s. | Spool to the archive; keep a bounded head+tail window in memory; drop the `captured.stdout.clone()`. |
| 4.9 | `-c 'pytest \| tail -5 > f.txt'` and `\| tail -2 && echo X` silently drop the redirect / trailing command. Parse-failure path never discloses `pipe_filter_dropped`. | `f.txt` never created; `X` never runs. | Only elide the filter when the RHS has no shell syntax. |
| 4.10 | Minor exit/UX: EACCES → exit 2 not 126 and the cause is not printed (`{e}` vs `{e:#}`); `cartoon --rwa tool` → "command not found: --rwa" exit 127; `CARTOON_MAX_TOKENS=abc` silently ignored; `ingest --help` opens a file called `--help`; `learn --help` prints stats usage; `doctor`/`init --help` run the command; top-level help lists only `stats \| adapters`. | | Small, independent fixes. Consider clap subcommands with an external-subcommand fallback. |

## 5. P2: the "non-lossy" safe tier is lossy

| # | What | Where |
|---|---|---|
| 5.1 | Progress collapse treats lines that match after digit-stripping as frames: `GPU0 util: 12%` / `GPU1 util: 99%` / `GPU2 util: 7%` → only the last survives. A `\r` frame absorbs an unrelated pending `%` line (`Coverage total: 45%` lost). `rsplit('\r').next()` keeps an empty segment, so `\r\r\n` endings blank **every** line and a final `FATAL …\r` disappears. | `ladder/progress.rs:55-70` |
| 5.2 | `collapse_blanks` trims trailing whitespace: corrupts `git diff` context lines and turns whitespace-only changes into apparent no-ops. | `ladder/safe.rs:27` |
| 5.3 | `strip_ansi` misses OSC 8 hyperlinks (gcc emits them, URL survives as tokens), `\e(B`, truecolor `38:2:…`. | `ladder/safe.rs:7` |

Add a property test: every non-blank, non-ANSI input line appears in safe output or is covered by a disclosed `(xN)` / progress marker.

**Aggressive tier.** `window_errors` misses `panicked`, `npm ERR!`, `Segmentation fault`, `Killed`, pytest `E   …`, `Permission denied` (`window.rs:14-17`). Near-dup templating collapses `FAILED test_v{i} - assert {200|500|401}` into one line plus `(x10 similar)` (`near_dups.rs:35-48`). `window_errors` runs after `extract_diagnostics` and cuts the table it built, leaving orphan rows (`ladder/mod.rs:58`). One shared, broader `is_error_line` should protect lines in all three stages.

**TOON encoder vs spec.** These cases are wrong (`toon/encode.rs`):
- `[{}, {}]` emits zero items.
- Control characters (ESC, NUL) are emitted raw.
- `a[0]` and `{x}` mid-string are unquoted.
- Keys `my-key`, `1abc` and `-x` are unquoted.
- `1e6` becomes `1000000.0`.
- Integers above 2^64 lose precision.

Vendor the upstream conformance fixtures.

**Archive.** A run larger than `max_archive_mb` prunes every older run, which breaks `raw_log` pointers the agent still holds. Archive-write failure still emits a dangling `raw_log` pointer on the lossy path (`app.rs:152-160`).

## 6. Process, docs, packaging

1. **CI is off "to save Actions minutes", but the repo is public, so standard runners are free.** Re-enable `ci.yml` with `concurrency` + `cancel-in-progress`, `paths-ignore: [docs/**, '**.md']`, `permissions: contents: read`. Add a weekly job that installs the *latest* jest/vitest/go/rust/ruff/mypy/tsc. Three adapter breakages in §1 (vitest 5, Go 1.24 `build-output`, Rust's thread-id panic line) are upstream format drift that the fixture tests can't see.
2. **The release workflow runs no tests.** Add a `test` job (fmt, clippy, test) to `needs:` of `build` and `pypi-wheels`.
3. **Missing tools pass silently.** e2e tests return early when a tool is missing (jest isn't installed here, so `e2e_jest_failing_suite` "passed" without running). Add `CARTOON_E2E_STRICT=1` to turn SKIP into a panic, and set it in CI.
4. **The crate publishes the whole repo** (158 files, 1.14 MB, including docs/assets and plans). Add `include = ["src/**", "Cargo.toml", "Cargo.lock", "README.md", "LICENSE"]`, plus `[profile.release] strip = true, lto = "thin", codegen-units = 1` (binary is 9.8 MB, downloaded by every npm/PyPI user).
5. **`doctor` gives wrong answers:**
   - It is blind to the plugin hook: plugin users are told to `hook install`, which would add a duplicate hook.
   - It lists `vitest` and `cargo nextest` as "no adapter". Both have adapters; the probes just omit the `run` subcommand (`doctor.rs:16-33`).
6. **Docs drift:**
   - "byte-identical" claims (see 4.5).
   - "CI-measured" corpus numbers (CI is off).
   - "Twenty runners" on the site vs 19 in `cartoon adapters`.
   - README invites contributions for cargo/go/rspec, which already ship.
   - RELEASING.md claims jobs are independent (they aren't), contradicts itself on the version source of truth, and says "5 platform packages" (there are 4).
   - `plugin.json` doesn't disclose that installing the plugin auto-approves `make`, `pytest` and `npm test`.
7. `pyproject.toml`: no `[project.urls]`, classifiers or keywords; `requires-python` duplicated.
8. `stats.jsonl` grows without bound and is parsed twice per `stats`. `learn` emits invalid TOML for path commands (`[command../b.sh]`), points at a non-existent `cartoon.toml`, and recommends aggressive for `cat`.

## 7. Changes on this branch

1. `fix(junit)`: `harvest_junit` compared file mtimes against `SystemTime::now()` exactly. On Linux a file the child writes often reads as *older* (26/50 subprocess-written files in a measurement), so `--junit` discarded fresh results. 2 s slack added.

## 8. What would make it markedly better (product)

1. **Correctness before breadth.** Do §1.1, the exit-code rule (§1.2), and the trace budgeting with omitted-lines markers (§1.12) first. One "cartoon said green, it wasn't" incident costs more adoption than any new adapter wins.
2. **Install everywhere:** static musl binaries, an `install.sh`, `cargo binstall` metadata, a Homebrew tap.
3. **An honest headline number.** `benchmarks/run.sh` over ~5 real OSS repos per ecosystem with injected failures, measured against what agents actually run (`pytest -q --tb=short`, `cargo test -q`, `jest --silent`), not `-v`. Results as JSON with the README table generated from them. Best of all: an agent-task benchmark ("same fix rate, X% fewer tokens, Y fewer turns").
4. **Streaming + interruption safety** (§4.1, §4.4): agents run with timeouts, and today a timeout yields nothing.
5. **`cartoon last` / `cartoon diff`:** fixed / still failing / new since the previous run of the same command. The archive already has the data, and it matches the edit-run-fix loop.
6. **Wider detection through wrappers:** `npm run test`, `pnpm|yarn jest|vitest`, `poetry|hatch|pdm run pytest`, `bun test`, `deno test`. Then gradle/maven surefire harvest (the JUnit parser exists), `dotnet test`, `golangci-lint`.
7. **`cartoon mcp`** (run / logs_grep / stats tools) for agents without a hook (Cursor, Codex, Windsurf).
8. **Plugin onboarding:** a SessionStart hook that warns once when the binary is missing, and a single `cartoon setup` (hook + instructions).

## 9. Checked and fine

- **Hook:** fails open on malformed stdin JSON. `wrap_scripts` matches only ever deny. `cd`/`export`/`source` compounds are not approved. Single-quote escaping in the rewrite is correct. Refuses to clobber a foreign Copilot `cartoon.json`. Rejects JSONC/non-object settings rather than overwriting.
- **`hooks/hooks.json`:** matches the current schema and exits 0 silently when `cartoon` is absent.
- **Exit codes:** mirrored for normal exits, >255, death by signal (128+N) and not-found (127). Args after the command reach the child untouched. No pipe deadlock. Children get the default SIGPIPE disposition.
- **Adapter parse-failure fallback:** works for missing JUnit files, non-JSON output, `--help` and `--version`.
- **Guards:** exact-dup counts are disclosed, and savings were never negative in any test.

## 10. Status after the fix pass (same branch)

Everything in §1–§6 is fixed on `claude/fervent-faraday-uzqqid`, with a regression test per item. Gate: fmt and clippy are clean; `cargo test` passes 736 tests with 0 failures. The original reproductions were re-run against the release build. Measured results:

- **Startup:** `cartoon true` went from 172–215 ms to about 5 ms.
- **Memory:** `cartoon seq 1 5000000` went from 498 MB / 7.6 s to 65 MB / 0.6 s. Above 4 MiB, output is windowed; see the README guarantees.
- **Benchmarks** (`benchmarks/run.py`, 600 tests with 20 failing):

  | Runner | Verbose baseline | Quiet baseline |
  |---|---|---|
  | pytest | 82.7% | 0.2% |
  | unittest | 89.2% | 14.8% |
  | cargo test | 81.1% (was 43.3%) | 72.2% (was 16.9%) |
  | go test | 93.3% | 0.2% (was −78%) |

  No row is negative.
- **Hook:** none of the §3.1 bypasses get `allow` any more; benign forms still rewrite.

Added beyond the review list:
- Adapters can provide a native-output baseline (`Adapter::native_stdout`), so injected `-json` / `--junit-xml` never makes cartoon cost more than the bare command.
- `poetry`/`pdm`/`hatch`/`pipenv`/`rye run` wrappers are detected.

Behaviour changes to note in the changelog:
- **Hook:**
  - `make NAME=value`, non-`run` pre-commit subcommands, `..` or out-of-project paths, and `;`/`|`/`||` compounds now go to the normal prompt instead of being auto-approved.
  - `PYTEST_ADDOPTS` is no longer a safe prefix.
- **TOON output follows spec 4.1:**
  - an empty array prints as `key: []`;
  - uniform objects print as keyed tables;
  - strings containing brackets are quoted.
- **Safe tier:** plain `Downloading 10%` lines without `\r` are no longer collapsed.
- **Unknown cartoon flags** are an error (exit 2), not "command not found".
- **npm platform packages** moved to the `@cartoon-wrap/` scope.

Still open:
- **Maintainer actions:**
  - re-enable the `ci` workflow in the Actions UI;
  - create the `@cartoon-wrap` npm org, bootstrap-publish the scoped packages and configure trusted publishing;
  - cut a release (the glibc/musl and npm fixes only reach users then);
  - bump the version (still 0.6.0; given the behaviour changes, 0.7.0).
- **Partial:**
  - TOON integers beyond u64 are emitted as their f64 approximation, because serde_json's `arbitrary_precision` feature isn't enabled.
  - Transformed (non-passthrough) output is still written stdout first, then stderr; arrival order is preserved only in `--raw` and passthrough.
- **Product items from §8, not started:**
  - `cartoon last`/`diff`;
  - `cartoon mcp`;
  - gradle/maven surefire auto-harvest, `dotnet test`, `golangci-lint`;
  - `npm run test` resolution;
  - a Homebrew tap;
  - a SessionStart "binary missing" plugin hook;
  - an agent-task (fix-rate) benchmark.
