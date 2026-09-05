use crate::adapters::{self, ParseOutcome};
use crate::ladder::CompressLevel;
use crate::{archive, config::Config, fallback, runner, stats, toon};
use anyhow::Result;
use serde_json::json;

pub fn run_wrap(
    argv: &[String],
    level: CompressLevel,
    raw: bool,
    tags: &[String],
    fast: bool,
    cfg: &Config,
) -> Result<i32> {
    // Adapter path: detect first, because prepare() must extend argv.
    if !raw {
        if let Some(adapter) = adapters::find_adapter(argv) {
            return run_with_adapter(adapter.as_ref(), argv, tags, fast, cfg);
        }
    }
    let captured = match runner::run(argv) {
        Ok(c) => c,
        Err(e) => return not_found_or_err(e, argv),
    };
    let code = runner::exit_code(&captured.status);
    if raw {
        // Escape hatch: byte-identical output, no footer, no stats — but archived.
        archive::record(argv, "raw", &captured, code, tags, cfg);
        print!("{}", captured.stdout);
        eprint!("{}", captured.stderr);
        return Ok(code);
    }
    transform_emit_record(argv, &captured, code, level, tags, cfg)
}

/// Shared tail of the non-adapter flow: transform under the ladder, apply
/// the net-savings guard (footer included), archive, emit, record stats.
/// Used by wrapped runs and by `ingest` (existing logs, synthetic capture).
fn transform_emit_record(
    argv: &[String],
    captured: &runner::Captured,
    code: i32,
    level: CompressLevel,
    tags: &[String],
    cfg: &Config,
) -> Result<i32> {
    // Reserve the archive slot first so the raw_log footer (part of the
    // emitted output) can be counted in the net-savings guard below.
    let reserved = archive::reserve(cfg);
    let (candidate, tmode) = transform(&captured.stdout, level);
    // Tokenize each stream exactly once; the guard and the stats record
    // share these counts (a 1M-token log used to be tokenized four times).
    let tok = cfg.tokenizer.as_str();
    let in_tokens = stats::estimate_tokens(&captured.stdout, tok);
    let err_tokens = stats::estimate_tokens(&captured.stderr, tok);
    let (out, mode, out_tokens) = if tmode == "passthrough" {
        (candidate, tmode, in_tokens)
    } else {
        let footer = reserved
            .as_ref()
            .map(|r| {
                format!(
                    "\n{}",
                    toon::encode(&json!({ "raw_log": r.dir.display().to_string() }))
                )
            })
            .unwrap_or_default();
        let with_footer = format!("{candidate}{footer}");
        // Net-savings guard: a transform must pay for itself, footer
        // included — otherwise emit the original untouched (still archived).
        let cand_tokens = stats::estimate_tokens(&with_footer, tok);
        if pays_for_itself(cand_tokens, in_tokens) {
            (with_footer, tmode, cand_tokens)
        } else {
            (captured.stdout.clone(), "passthrough", in_tokens)
        }
    };
    let run =
        reserved.and_then(|r| archive::write_reserved(r, argv, mode, captured, code, tags, cfg));
    if mode == "passthrough" {
        // Byte-identical guarantee: no trailing-newline normalization.
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
        run.as_ref().map(|r| r.id.as_str()),
    );
    Ok(code)
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
    tags: &[String],
    fast: bool,
    cfg: &Config,
) -> Result<i32> {
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
                print!("{}", captured.stdout);
                eprint!("{}", captured.stderr);
                stats::record_counts(argv, "passthrough", in_tokens, in_tokens, code, run_id);
                return Ok(code);
            }
            emit(&out, "");
            if !extra_out.is_empty() {
                print!("{extra_out}");
            }
            eprint!("{extra_err}");
            stats::record_counts(
                argv,
                adapter.name(),
                in_tokens,
                emitted_tokens,
                code,
                run_id,
            );
            Ok(code)
        }
        Err(e) => {
            // Safety rule: never lose information. Emit original output, NO footer.
            eprintln!(
                "cartoon: {} adapter failed to parse ({e}); passing through",
                adapter.name()
            );
            print!("{}", captured.stdout);
            eprint!("{}", captured.stderr);
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
