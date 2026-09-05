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
            junit,
            max_tokens,
            dropped_filter,
        }) => {
            let mut cfg = cartoon::config::load_for_cwd();
            // Ceiling precedence: flag > CARTOON_MAX_TOKENS > config.
            cfg.max_tokens = max_tokens
                .or_else(|| {
                    std::env::var("CARTOON_MAX_TOKENS")
                        .ok()
                        .and_then(|v| v.trim().parse().ok())
                })
                .or(cfg.max_tokens);
            let junit = junit.or_else(|| cfg.command.get(&argv[0]).and_then(|c| c.junit.clone()));
            match cartoon::config::resolve_level(compress.as_deref(), heuristic, &argv[0], &cfg) {
                Ok(level) => {
                    let opts = cartoon::app::WrapOpts {
                        level,
                        raw,
                        tags,
                        fast,
                        junit: junit.map(std::path::PathBuf::from),
                        dropped_filter,
                    };
                    cartoon::app::run_wrap(&argv, &opts, &cfg).unwrap_or_else(|e| {
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
        Ok(cartoon::cli::Mode::Doctor) => cartoon::doctor::run().unwrap_or_else(|e| {
            eprintln!("cartoon: {e}");
            2
        }),
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
        Ok(cartoon::cli::Mode::Init) => std::env::current_dir()
            .map_err(anyhow::Error::from)
            .and_then(|cwd| cartoon::init::run(&cwd))
            .unwrap_or_else(|e| {
                eprintln!("cartoon: {e}");
                2
            }),
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
            let mut cfg = cartoon::config::load_for_cwd();
            cfg.max_tokens = std::env::var("CARTOON_MAX_TOKENS")
                .ok()
                .and_then(|v| v.trim().parse().ok())
                .or(cfg.max_tokens);
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
