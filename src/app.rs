use crate::adapters::{self, ParseOutcome};
use crate::ladder::CompressLevel;
use crate::{archive, budget, config::Config, fallback, runner, sniff, stats, toon};
use anyhow::Result;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Per-run options for `run_wrap` (everything except the command itself).
pub struct WrapOpts {
    pub level: CompressLevel,
    pub raw: bool,
    pub tags: Vec<String>,
    pub fast: bool,
    /// JUnit XML file or directory to render as a test report after the run.
    pub junit: Option<PathBuf>,
    /// A pure output filter dropped from a `-c` pipeline (disclosed).
    pub dropped_filter: Option<String>,
}

pub fn run_wrap(argv: &[String], opts: &WrapOpts, cfg: &Config) -> Result<i32> {
    // Adapter path: detect first, because prepare() must extend argv.
    if !opts.raw {
        if let Some(adapter) = adapters::find_adapter(argv) {
            return run_with_adapter(adapter.as_ref(), argv, opts, cfg);
        }
    }
    let started = std::time::SystemTime::now();
    let captured = match runner::run(argv) {
        Ok(c) => c,
        Err(e) => return not_found_or_err(e, argv),
    };
    let code = runner::exit_code(&captured.status);
    if opts.raw {
        // Escape hatch: byte-identical output, no footer, no stats — but archived.
        archive::record(argv, "raw", &captured, code, &opts.tags, cfg);
        print!("{}", captured.stdout);
        eprint!("{}", captured.stderr);
        return Ok(code);
    }
    if let Some(path) = &opts.junit {
        if let Some(rendered) = harvest_junit(path, &argv[0], started, cfg) {
            return emit_candidate(argv, &captured, code, (rendered, "junit"), &opts.tags, cfg);
        }
    }
    transform_emit_record(argv, &captured, code, opts.level, &opts.tags, cfg)
}

/// `--junit <path>` / `[command.X] junit`: render the JUnit XML the command
/// wrote as a test report. A directory means every `*.xml` in it (gradle
/// writes one per class), merged. A file older than this run is stale (the
/// command failed before writing it) and is ignored with a warning.
fn harvest_junit(
    path: &Path,
    runner_name: &str,
    started: std::time::SystemTime,
    cfg: &Config,
) -> Option<String> {
    let files: Vec<PathBuf> = if path.is_dir() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(path)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("xml"))
            .collect();
        v.sort();
        v
    } else if path.is_file() {
        vec![path.to_path_buf()]
    } else {
        eprintln!(
            "cartoon: --junit path {} not found; using the compression ladder instead",
            path.display()
        );
        return None;
    };
    let fresh: Vec<&PathBuf> = files
        .iter()
        .filter(|f| {
            std::fs::metadata(f)
                .and_then(|m| m.modified())
                .is_ok_and(|t| t >= started)
        })
        .collect();
    if fresh.is_empty() {
        eprintln!(
            "cartoon: no JUnit file under {} was written by this run (stale results ignored); using the compression ladder instead",
            path.display()
        );
        return None;
    }
    // The report carries the wrapped command's name as its runner label.
    let runner: &'static str = Box::leak(runner_name.to_string().into_boxed_str());
    let reports: Vec<_> = fresh
        .iter()
        .filter_map(|f| {
            let xml = std::fs::read_to_string(f).ok()?;
            adapters::pytest::parse_junit_named(&xml, runner)
                .map_err(|e| eprintln!("cartoon: skipping {}: {e}", f.display()))
                .ok()
        })
        .collect();
    let merged = adapters::report::merge(reports)?;
    Some(adapters::report::render(&merged, cfg.trace_lines, None))
}

/// Shared tail of the non-adapter flow: content-sniff or transform under the
/// ladder, then `emit_candidate`. Used by wrapped runs and by `ingest`.
fn transform_emit_record(
    argv: &[String],
    captured: &runner::Captured,
    code: i32,
    level: CompressLevel,
    tags: &[String],
    cfg: &Config,
) -> Result<i32> {
    // Output that arrived without a matching argv0 (a wrapper script running
    // xcodebuild, JUnit XML on stdout) still gets a structured rendering.
    let candidate = match sniff::sniff(&captured.stdout, &captured.stderr, code) {
        Some(c) => c,
        None => transform(&captured.stdout, level),
    };
    emit_candidate(argv, captured, code, candidate, tags, cfg)
}

/// Apply the net-savings guard (footer included), archive, emit under the
/// optional token ceiling, record stats.
fn emit_candidate(
    argv: &[String],
    captured: &runner::Captured,
    code: i32,
    (candidate, tmode): (String, &'static str),
    tags: &[String],
    cfg: &Config,
) -> Result<i32> {
    // Reserve the archive slot first so the raw_log footer (part of the
    // emitted output) can be counted in the net-savings guard below.
    let reserved = archive::reserve(cfg);
    let Guarded {
        out,
        mode,
        in_tokens,
        err_tokens,
        out_tokens,
    } = guard_with_footer(
        candidate,
        tmode,
        captured,
        reserved.as_ref().map(|r| r.dir.as_path()),
        cfg,
    );
    let run =
        reserved.and_then(|r| archive::write_reserved(r, argv, mode, captured, code, tags, cfg));
    let run_id = run.as_ref().map(|r| r.id.as_str());
    let (out, out_tokens) = capped(out, out_tokens, cfg, run_id);
    if mode == "passthrough" {
        // Byte-identical guarantee (unless a --max-tokens ceiling is set):
        // no trailing-newline normalization.
        print!("{out}");
        eprint!("{}", captured.stderr);
    } else {
        emit(&out, &captured.stderr);
    }
    stats::record_counts(
        argv,
        mode,
        in_tokens + err_tokens,
        out_tokens + err_tokens,
        code,
        run_id,
    );
    Ok(code)
}

/// Result of the net-savings guard: what to emit, how to label it in stats,
/// and the token counts the guard already computed (reused by stats).
struct Guarded {
    out: String,
    mode: &'static str,
    in_tokens: usize,
    err_tokens: usize,
    out_tokens: usize,
}

/// Append the `raw_log` footer to a transformed candidate and apply the
/// net-savings guard against the captured stdout. Each stream is tokenized
/// exactly once (a 1M-token log used to be tokenized four times).
fn guard_with_footer(
    candidate: String,
    tmode: &'static str,
    captured: &runner::Captured,
    raw_log_dir: Option<&Path>,
    cfg: &Config,
) -> Guarded {
    let tok = cfg.tokenizer.as_str();
    let in_tokens = stats::estimate_tokens(&captured.stdout, tok);
    let err_tokens = stats::estimate_tokens(&captured.stderr, tok);
    let (out, mode, out_tokens) = if tmode == "passthrough" {
        (candidate, tmode, in_tokens)
    } else {
        let footer = raw_log_dir
            .map(|d| {
                format!(
                    "\n{}",
                    toon::encode(&json!({ "raw_log": d.display().to_string() }))
                )
            })
            .unwrap_or_default();
        let with_footer = format!("{candidate}{footer}");
        // A transform must pay for itself, footer included — otherwise the
        // original is emitted untouched (still archived).
        let cand_tokens = stats::estimate_tokens(&with_footer, tok);
        if pays_for_itself(cand_tokens, in_tokens) {
            (with_footer, tmode, cand_tokens)
        } else {
            (captured.stdout.clone(), "passthrough", in_tokens)
        }
    };
    Guarded {
        out,
        mode,
        in_tokens,
        err_tokens,
        out_tokens,
    }
}

/// Enforce `max_tokens` when set; returns the (possibly cut) text and its
/// token count so stats stay honest. Identity when no ceiling is configured.
fn capped(out: String, out_tokens: usize, cfg: &Config, run_id: Option<&str>) -> (String, usize) {
    match cfg.max_tokens {
        Some(max) if out_tokens > max => {
            let cut = budget::cap_tokens(&out, max, &cfg.tokenizer, run_id);
            let n = stats::estimate_tokens(&cut, &cfg.tokenizer);
            (cut, n)
        }
        _ => (out, out_tokens),
    }
}

/// `cartoon ingest (<file> | -)` — run an EXISTING log through the same
/// flow as a wrapped command: JSON detect → ladder → net-savings guard →
/// raw-log archive → stats. Exit code is 0 (nothing executed) unless the
/// source can't be read.
pub fn run_ingest(
    source: &str,
    level: CompressLevel,
    tags: &[String],
    cfg: &Config,
) -> Result<i32> {
    let content = if source == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| anyhow::anyhow!("cannot read stdin: {e}"))?;
        buf
    } else {
        std::fs::read_to_string(source).map_err(|e| anyhow::anyhow!("cannot read {source}: {e}"))?
    };
    let argv = vec!["ingest".to_string(), source.to_string()];
    let captured = runner::Captured::synthetic(content, String::new());
    transform_emit_record(&argv, &captured, 0, level, tags, cfg)
}

fn run_with_adapter(
    adapter: &dyn adapters::Adapter,
    argv: &[String],
    opts: &WrapOpts,
    cfg: &Config,
) -> Result<i32> {
    let tags = &opts.tags;
    let fast = opts.fast;
    let prepared = adapter.prepare(argv.to_vec());
    let fast_args = if fast {
        adapter.fast_args()
    } else {
        Vec::new()
    };
    let mut argv_run = prepared.argv.clone();
    argv_run.extend(fast_args.iter().cloned());
    let mut fast_note = (!fast_args.is_empty()).then(|| fast_args.join(" "));
    let mut captured = match runner::run(&argv_run) {
        Ok(c) => c,
        Err(e) => return not_found_or_err(e, argv),
    };
    let mut code = runner::exit_code(&captured.status);
    // Bounded fallback: pytest exits 4 (usage error) when xdist is missing.
    // Nothing executed, so one serial retry is safe. Only on the exact
    // signature naming an arg WE injected in the unrecognized-arguments
    // list — a user's own typo'd args won't match and pass through.
    if fast_note.is_some() && code == 4 && fast_args_rejected(&captured.stderr, &fast_args) {
        eprintln!("cartoon: --fast unavailable (pytest-xdist not installed?); reran serially");
        fast_note = None;
        captured = match runner::run(&prepared.argv) {
            Ok(c) => c,
            Err(e) => return not_found_or_err(e, argv),
        };
        code = runner::exit_code(&captured.status);
    }
    let run = archive::record(argv, adapter.name(), &captured, code, tags, cfg);
    match adapter.parse(&captured, &prepared) {
        Ok(ParseOutcome {
            report,
            passthrough_stdout,
            passthrough_stderr,
        }) => {
            let mut out = report.render(cfg.trace_lines, fast_note.as_deref());
            if let Some(r) = &run {
                out.push_str(&format!(
                    "\n{}",
                    toon::encode(&json!({ "raw_log": r.dir.display().to_string() }))
                ));
            }
            if let Some(f) = &opts.dropped_filter {
                // `cartoon -c 'pytest | tail -5'`: the report replaces the
                // filter's job; say so rather than silently ignoring it.
                out.push_str(&format!(
                    "\n{}",
                    toon::encode(&json!({ "pipe_filter_dropped": f }))
                ));
            }
            let extra_out = passthrough_stdout.unwrap_or_default();
            let extra_err = passthrough_stderr.unwrap_or_default();
            let tok = cfg.tokenizer.as_str();
            let in_tokens = stats::estimate_tokens(&captured.stdout, tok)
                + stats::estimate_tokens(&captured.stderr, tok);
            let emitted_tokens = stats::estimate_tokens(&out, tok)
                + stats::estimate_tokens(&extra_out, tok)
                + stats::estimate_tokens(&extra_err, tok);
            let run_id = run.as_ref().map(|r| r.id.as_str());
            // Net-savings guard, same rule as the ladder path: a report that
            // costs more tokens than the raw output (tiny suites, `-q` runs)
            // is replaced by the original streams, byte-identical.
            if !pays_for_itself(emitted_tokens, in_tokens) {
                let (raw_out, n) = capped(captured.stdout.clone(), in_tokens, cfg, run_id);
                print!("{raw_out}");
                eprint!("{}", captured.stderr);
                stats::record_counts(argv, "passthrough", in_tokens, n, code, run_id);
                return Ok(code);
            }
            let (out, report_tokens) = capped(out, emitted_tokens, cfg, run_id);
            emit(&out, "");
            if !extra_out.is_empty() {
                print!("{extra_out}");
            }
            eprint!("{extra_err}");
            stats::record_counts(argv, adapter.name(), in_tokens, report_tokens, code, run_id);
            Ok(code)
        }
        Err(e) => {
            // Safety rule: never lose information. The captured streams still
            // carry the injected machine format, so fall back to the generic
            // ladder + guard rather than dumping them raw: the guard emits the
            // original byte-identically when nothing pays for itself.
            eprintln!(
                "cartoon: {} adapter failed to parse ({e}); compressing generically",
                adapter.name()
            );
            let (candidate, tmode) = transform(&captured.stdout, opts.level);
            let Guarded {
                out,
                mode,
                in_tokens,
                err_tokens,
                out_tokens,
            } = guard_with_footer(
                candidate,
                tmode,
                &captured,
                run.as_ref().map(|r| r.dir.as_path()),
                cfg,
            );
            let run_id = run.as_ref().map(|r| r.id.as_str());
            let (out, out_tokens) = capped(out, out_tokens, cfg, run_id);
            if mode == "passthrough" {
                print!("{out}");
                eprint!("{}", captured.stderr);
            } else {
                emit(&out, &captured.stderr);
            }
            stats::record_counts(
                argv,
                mode,
                in_tokens + err_tokens,
                out_tokens + err_tokens,
                code,
                run_id,
            );
            Ok(code)
        }
    }
}

/// True when the runner's usage error names one of the args WE injected.
/// pytest prints `unrecognized arguments: <tok> [<tok>...]` but may list only
/// the first offending token (e.g. `-n` without `auto`), so match exact
/// whitespace-separated tokens, not the joined string.
fn fast_args_rejected(stderr: &str, fast_args: &[String]) -> bool {
    stderr.lines().any(|line| {
        line.split("unrecognized arguments:")
            .nth(1)
            .map(|rest| {
                rest.split_whitespace()
                    .any(|tok| fast_args.iter().any(|a| a == tok))
            })
            .unwrap_or(false)
    })
}

fn not_found_or_err(e: anyhow::Error, argv: &[String]) -> Result<i32> {
    let not_found = e
        .downcast_ref::<std::io::Error>()
        .map(|io| io.kind() == std::io::ErrorKind::NotFound)
        .unwrap_or(false);
    if not_found {
        eprintln!("cartoon: command not found: {}", argv[0]);
        return Ok(127);
    }
    Err(e)
}

/// The net-savings guard: a candidate rendering must beat the original's
/// token count, or the original is emitted byte-identically.
pub fn pays_for_itself(candidate_tokens: usize, original_tokens: usize) -> bool {
    candidate_tokens < original_tokens
}

fn emit(out: &str, err: &str) {
    print!("{out}");
    if !out.is_empty() && !out.ends_with('\n') {
        println!();
    }
    eprint!("{err}");
}

pub fn transform(stdout: &str, level: CompressLevel) -> (String, &'static str) {
    if let Some(json) = fallback::detect_json(stdout) {
        return (toon::encode(&json), "json");
    }
    let compressed = crate::ladder::compress(stdout, level);
    // The ladder's line-join drops a trailing newline; treat that as unchanged.
    if compressed == stdout || format!("{compressed}\n") == stdout {
        return (stdout.to_string(), "passthrough");
    }
    (compressed, level.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rejected_when_only_first_token_listed() {
        let stderr = "ERROR: usage: pytest [options]\npytest: error: unrecognized arguments: -n\n  inifile: /x/pyproject.toml\n";
        assert!(fast_args_rejected(stderr, &args(&["-n", "auto"])));
    }

    #[test]
    fn rejected_when_both_tokens_listed() {
        let stderr = "pytest: error: unrecognized arguments: -n auto\n";
        assert!(fast_args_rejected(stderr, &args(&["-n", "auto"])));
    }

    #[test]
    fn not_rejected_by_dash_n_inside_a_path() {
        let stderr =
            "pytest: error: unrecognized arguments: --bogus\nhint: see tests/test-n-gram.py\n";
        assert!(!fast_args_rejected(stderr, &args(&["-n", "auto"])));
    }

    #[test]
    fn not_rejected_without_marker() {
        assert!(!fast_args_rejected(
            "some other exit-4 error",
            &args(&["-n", "auto"])
        ));
    }

    #[test]
    fn guard_rejects_candidates_that_do_not_shrink() {
        assert!(pays_for_itself(10, 100));
        assert!(!pays_for_itself(100, 10));
        assert!(!pays_for_itself(50, 50), "equal is not a win");
    }

    #[test]
    fn transform_safe_passthrough_when_no_rule_fires() {
        let (out, mode) = transform("plain prose line", CompressLevel::Safe);
        assert_eq!(out, "plain prose line");
        assert_eq!(mode, "passthrough");
    }

    #[test]
    fn transform_safe_reports_safe_mode_when_rules_fire() {
        let (out, mode) = transform("\x1b[32mok\x1b[0m\n\n\n\nend", CompressLevel::Safe);
        assert_eq!(mode, "safe");
        assert!(out.contains("ok"));
    }

    #[test]
    fn transform_json_still_wins() {
        let (_, mode) = transform("{\"a\": 1}", CompressLevel::Safe);
        assert_eq!(mode, "json");
    }
}
