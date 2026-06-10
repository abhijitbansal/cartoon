use clap::Parser;

fn main() {
    let cli = cartoon::cli::Cli::parse();
    let code = match cartoon::cli::parse_mode(cli) {
        Ok(cartoon::cli::Mode::Wrap {
            argv,
            heuristic,
            raw,
        }) => {
            let cfg = cartoon::config::load();
            let heuristic_on = heuristic || cfg.heuristic;
            cartoon::app::run_wrap(&argv, heuristic_on, raw, &cfg).unwrap_or_else(|e| {
                eprintln!("cartoon: {e}");
                2
            })
        }
        Ok(cartoon::cli::Mode::Stats { since }) => match cartoon::stats::report(since.as_deref()) {
            Ok(s) => {
                println!("{s}");
                0
            }
            Err(e) => {
                eprintln!("cartoon: {e}");
                2
            }
        },
        Ok(cartoon::cli::Mode::Adapters) => {
            println!("(no adapters yet)");
            0
        }
        Err(e) => {
            eprintln!("cartoon: {e}");
            2
        }
    };
    std::process::exit(code);
}
