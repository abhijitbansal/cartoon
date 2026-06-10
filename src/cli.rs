use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "cartoon",
    version,
    about = "Token-optimized TOON output wrapper for any CLI",
    after_help = "Subcommands `stats` and `adapters` are reserved words; \
to wrap a binary literally named `stats`, use: cartoon env stats"
)]
pub struct Cli {
    /// Enable the lossy heuristic fallback for this call
    #[arg(long)]
    pub heuristic: bool,

    /// Bypass cartoon entirely; run the command untouched
    #[arg(long)]
    pub raw: bool,

    /// Command to wrap plus its args (or: stats | adapters)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum Mode {
    Wrap { argv: Vec<String>, heuristic: bool, raw: bool },
    Stats { since: Option<String> },
    Adapters,
}

pub fn parse_mode(cli: Cli) -> anyhow::Result<Mode> {
    if cli.command.is_empty() {
        anyhow::bail!("no command given. usage: cartoon <cmd> [args...]");
    }
    match cli.command[0].as_str() {
        "stats" => Ok(Mode::Stats { since: parse_since(&cli.command[1..])? }),
        "adapters" => Ok(Mode::Adapters),
        _ => Ok(Mode::Wrap { argv: cli.command, heuristic: cli.heuristic, raw: cli.raw }),
    }
}

fn parse_since(args: &[String]) -> anyhow::Result<Option<String>> {
    match args {
        [] => Ok(None),
        [flag, value] if flag == "--since" => Ok(Some(value.clone())),
        _ => anyhow::bail!("usage: cartoon stats [--since <e.g. 7d|24h|30m>]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn mode(args: &[&str]) -> Mode {
        parse_mode(Cli::parse_from(args)).unwrap()
    }

    #[test]
    fn wrap_mode_passes_args_verbatim() {
        let m = mode(&["cartoon", "pytest", "-q", "--maxfail=1"]);
        assert_eq!(
            m,
            Mode::Wrap {
                argv: vec!["pytest".into(), "-q".into(), "--maxfail=1".into()],
                heuristic: false,
                raw: false
            }
        );
    }

    #[test]
    fn heuristic_flag_before_command() {
        let m = mode(&["cartoon", "--heuristic", "ls", "-la"]);
        assert!(matches!(m, Mode::Wrap { heuristic: true, .. }));
    }

    #[test]
    fn stats_subcommand_with_since() {
        let m = mode(&["cartoon", "stats", "--since", "7d"]);
        assert_eq!(m, Mode::Stats { since: Some("7d".into()) });
    }

    #[test]
    fn adapters_subcommand() {
        assert_eq!(mode(&["cartoon", "adapters"]), Mode::Adapters);
    }

    #[test]
    fn no_command_is_error() {
        assert!(parse_mode(Cli::parse_from(["cartoon"])).is_err());
    }
}
