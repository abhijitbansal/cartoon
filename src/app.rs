use crate::{config::Config, fallback, heuristic, runner, stats, toon};
use anyhow::Result;

/// Run the wrapped command, transform stdout, mirror the exit code.
pub fn run_wrap(argv: &[String], heuristic_on: bool, raw: bool, cfg: &Config) -> Result<i32> {
    let captured = match runner::run(argv) {
        Ok(c) => c,
        Err(e) => {
            let not_found = e
                .downcast_ref::<std::io::Error>()
                .map(|io| io.kind() == std::io::ErrorKind::NotFound)
                .unwrap_or(false);
            if not_found {
                eprintln!("cartoon: command not found: {}", argv[0]);
                return Ok(127);
            }
            return Err(e);
        }
    };
    let code = runner::exit_code(&captured.status);
    if raw {
        print!("{}", captured.stdout);
        eprint!("{}", captured.stderr);
        return Ok(code);
    }
    let (out, mode) = transform(&captured.stdout, heuristic_on);
    print!("{out}");
    if !out.is_empty() && !out.ends_with('\n') {
        println!();
    }
    eprint!("{}", captured.stderr);
    let original = format!("{}{}", captured.stdout, captured.stderr);
    let emitted = format!("{}{}", out, captured.stderr);
    stats::record_call(argv, mode, &original, &emitted, code, &cfg.tokenizer);
    Ok(code)
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
