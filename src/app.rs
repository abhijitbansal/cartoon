use crate::adapters::{self, ParseOutcome};
use crate::{archive, config::Config, fallback, heuristic, runner, stats, toon};
use anyhow::Result;
use serde_json::json;

pub fn run_wrap(
    argv: &[String],
    heuristic_on: bool,
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
    let (mut out, mode) = transform(&captured.stdout, heuristic_on);
    let run = archive::record(argv, mode, &captured, code, tags, cfg);
    if mode != "passthrough" {
        if let Some(r) = &run {
            out.push_str(&format!(
                "\n{}",
                toon::encode(&json!({ "raw_log": r.dir.display().to_string() }))
            ));
        }
    }
    if mode == "passthrough" {
        // Byte-identical guarantee: no trailing-newline normalization.
        print!("{out}");
        eprint!("{}", captured.stderr);
    } else {
        emit(&out, &captured.stderr);
    }
    let original = format!("{}{}", captured.stdout, captured.stderr);
    let emitted = format!("{}{}", out, captured.stderr);
    stats::record_call(
        argv,
        mode,
        &original,
        &emitted,
        code,
        &cfg.tokenizer,
        run.as_ref().map(|r| r.id.as_str()),
    );
    Ok(code)
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
    // signature mentioning an arg WE injected — a user's own typo'd args
    // won't match and pass through normally.
    if fast_note.is_some()
        && code == 4
        && captured.stderr.contains("unrecognized arguments")
        && fast_args
            .iter()
            .any(|a| captured.stderr.contains(a.as_str()))
    {
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
            let mut out = adapters::report::render(&report, cfg.trace_lines, fast_note.as_deref());
            if let Some(r) = &run {
                out.push_str(&format!(
                    "\n{}",
                    toon::encode(&json!({ "raw_log": r.dir.display().to_string() }))
                ));
            }
            let extra_out = passthrough_stdout.unwrap_or_default();
            let extra_err = passthrough_stderr.unwrap_or_default();
            emit(&out, "");
            if !extra_out.is_empty() {
                print!("{extra_out}");
            }
            eprint!("{extra_err}");
            let original = format!("{}{}", captured.stdout, captured.stderr);
            let emitted = format!("{}{}{}", out, extra_out, extra_err);
            stats::record_call(
                argv,
                adapter.name(),
                &original,
                &emitted,
                code,
                &cfg.tokenizer,
                run.as_ref().map(|r| r.id.as_str()),
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

fn emit(out: &str, err: &str) {
    print!("{out}");
    if !out.is_empty() && !out.ends_with('\n') {
        println!();
    }
    eprint!("{err}");
}

pub fn transform(stdout: &str, heuristic_on: bool) -> (String, &'static str) {
    if let Some(json) = fallback::detect_json(stdout) {
        return (toon::encode(&json), "json");
    }
    if heuristic_on {
        return (heuristic::compress(stdout), "heuristic");
    }
    (stdout.to_string(), "passthrough")
}
