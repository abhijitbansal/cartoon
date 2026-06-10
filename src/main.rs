use clap::Parser;

fn main() {
    let cli = cartoon::cli::Cli::parse();
    let code = match cartoon::cli::parse_mode(cli) {
        Ok(cartoon::cli::Mode::Wrap {
            argv,
            heuristic,
            raw,
        }) => cartoon::app::run_wrap(&argv, heuristic, raw).unwrap_or_else(|e| {
            eprintln!("cartoon: {e}");
            2
        }),
        Ok(cartoon::cli::Mode::Stats { .. }) => {
            println!("(stats not implemented yet)");
            0
        }
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
