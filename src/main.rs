use clap::Parser;

fn main() {
    let cli = cartoon::cli::Cli::parse();
    let code = match cartoon::cli::parse_mode(cli) {
        Ok(cartoon::cli::Mode::Wrap {
            argv,
            compress,
            heuristic,
            raw,
            tags,
            fast,
        }) => {
            let cfg = cartoon::config::load();
            match cartoon::config::resolve_level(compress.as_deref(), heuristic, &argv[0], &cfg) {
                Ok(level) => cartoon::app::run_wrap(&argv, level, raw, &tags, fast, &cfg)
                    .unwrap_or_else(|e| {
                        eprintln!("cartoon: {e}");
                        2
                    }),
                Err(e) => {
                    eprintln!("cartoon: {e}");
                    2
                }
            }
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
            for a in cartoon::adapters::registry() {
                println!("{}: {}", a.name(), a.matches());
            }
            0
        }
        Ok(cartoon::cli::Mode::Logs(query)) => cartoon::logs_cmd::run(query).unwrap_or_else(|e| {
            eprintln!("cartoon: {e}");
            2
        }),
        Ok(cartoon::cli::Mode::Learn { since }) => cartoon::learn::run(since.as_deref())
            .unwrap_or_else(|e| {
                eprintln!("cartoon: {e}");
                2
            }),
        Ok(cartoon::cli::Mode::Hook { args }) => cartoon::hook::run(&args).unwrap_or_else(|e| {
            eprintln!("cartoon: {e}");
            2
        }),
        Ok(cartoon::cli::Mode::Shim { args }) => cartoon::shim::run(&args).unwrap_or_else(|e| {
            eprintln!("cartoon: {e}");
            2
        }),
        Ok(cartoon::cli::Mode::Instructions { args }) => cartoon::instructions::run(&args)
            .unwrap_or_else(|e| {
                eprintln!("cartoon: {e}");
                2
            }),
        Ok(cartoon::cli::Mode::Ingest {
            source,
            compress,
            tags,
        }) => {
            let cfg = cartoon::config::load();
            match cartoon::config::resolve_level(compress.as_deref(), false, "ingest", &cfg) {
                Ok(level) => {
                    cartoon::app::run_ingest(&source, level, &tags, &cfg).unwrap_or_else(|e| {
                        eprintln!("cartoon: {e}");
                        2
                    })
                }
                Err(e) => {
                    eprintln!("cartoon: {e}");
                    2
                }
            }
        }
        Err(e) => {
            eprintln!("cartoon: {e}");
            2
        }
    };
    std::process::exit(code);
}
