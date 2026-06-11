use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "cartoon",
    version,
    about = "Token-optimized TOON output wrapper for any CLI",
    after_help = "Subcommands:
  stats [--since <7d|24h|30m>]               tokens saved, per adapter
  adapters                                   list built-in adapters
  logs [--tag <t>]                           list archived raw runs
  logs (<id> | --last) [--stdout|--stderr]   print a run's full raw output

Every wrapped run archives its complete raw stdout/stderr and prints the
location as a `raw_log:` footer — read that instead of rerunning unwrapped.

`stats`, `adapters`, and `logs` are reserved words; to wrap a binary \
literally named `stats`, use: cartoon env stats"
)]
pub struct Cli {
    /// Compression level for non-adapter output: safe (default) | aggressive
    #[arg(long, value_name = "LEVEL")]
    pub compress: Option<String>,

    /// Deprecated alias for --compress=aggressive
    #[arg(long)]
    pub heuristic: bool,

    /// Bypass cartoon entirely; run the command untouched.
    /// (v1 limitation: output is still UTF-8-lossy converted, so non-UTF-8
    /// bytes become U+FFFD even in raw mode.)
    #[arg(long)]
    pub raw: bool,

    /// Tag this run in the raw-log archive (repeatable)
    #[arg(long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,

    /// Opt-in acceleration: inject parallelization args for runners that
    /// support it (pytest: -n auto via pytest-xdist). Disclosed in output.
    #[arg(long)]
    pub fast: bool,

    /// Wrap a shell command string (like sh -c). Simple commands are
    /// adapter-detected; strings with shell operators run via the shell
    /// and compress through the generic ladder.
    #[arg(short = 'c', long = "shell", value_name = "STRING")]
    pub shell: Option<String>,

    /// Command to wrap plus its args (or: stats | adapters)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum Mode {
    Wrap {
        argv: Vec<String>,
        compress: Option<String>,
        heuristic: bool,
        raw: bool,
        tags: Vec<String>,
        fast: bool,
    },
    Stats {
        since: Option<String>,
    },
    Adapters,
    Logs(LogsQuery),
    Learn {
        since: Option<String>,
    },
}

#[derive(Debug, PartialEq)]
pub enum LogsQuery {
    List {
        tag: Option<String>,
    },
    Show {
        sel: RunSel,
        stream: StreamSel,
    },
    /// Search a run's raw output instead of re-reading all of it.
    Grep {
        sel: RunSel,
        pattern: String,
        context: usize,
    },
}

#[derive(Debug, PartialEq)]
pub enum RunSel {
    Id(String),
    Last,
}

#[derive(Debug, PartialEq)]
pub enum StreamSel {
    Both,
    Stdout,
    Stderr,
}

/// Shell metacharacters that force `sh -c` execution; a string without
/// any of these is split into argv so adapters can detect the command.
fn needs_shell(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(
            c,
            '|' | '&'
                | ';'
                | '<'
                | '>'
                | '('
                | ')'
                | '$'
                | '`'
                | '\\'
                | '"'
                | '\''
                | '*'
                | '?'
                | '['
                | ']'
                | '{'
                | '}'
                | '~'
                | '\n'
                | '='
        )
    })
}

pub fn shell_argv(s: &str) -> Vec<String> {
    if needs_shell(s) {
        let sh = if cfg!(windows) { "cmd" } else { "sh" };
        let flag = if cfg!(windows) { "/C" } else { "-c" };
        vec![sh.to_string(), flag.to_string(), s.to_string()]
    } else {
        s.split_whitespace().map(String::from).collect()
    }
}

pub fn parse_mode(cli: Cli) -> anyhow::Result<Mode> {
    if let Some(s) = cli.shell {
        if !cli.command.is_empty() {
            anyhow::bail!("-c/--shell takes the whole command as one string; drop the extra args");
        }
        let argv = shell_argv(&s);
        if argv.is_empty() {
            anyhow::bail!("-c/--shell got an empty command string");
        }
        return Ok(Mode::Wrap {
            argv,
            compress: cli.compress,
            heuristic: cli.heuristic,
            raw: cli.raw,
            tags: cli.tags,
            fast: cli.fast,
        });
    }
    if cli.command.is_empty() {
        anyhow::bail!("no command given. usage: cartoon <cmd> [args...]");
    }
    match cli.command[0].as_str() {
        "stats" => Ok(Mode::Stats {
            since: parse_since(&cli.command[1..])?,
        }),
        "adapters" => Ok(Mode::Adapters),
        "logs" => Ok(Mode::Logs(parse_logs(&cli.command[1..])?)),
        "learn" => Ok(Mode::Learn {
            since: parse_since(&cli.command[1..])?,
        }),
        _ => Ok(Mode::Wrap {
            argv: cli.command,
            compress: cli.compress,
            heuristic: cli.heuristic,
            raw: cli.raw,
            tags: cli.tags,
            fast: cli.fast,
        }),
    }
}

fn parse_since(args: &[String]) -> anyhow::Result<Option<String>> {
    match args {
        [] => Ok(None),
        [flag, value] if flag == "--since" => Ok(Some(value.clone())),
        _ => anyhow::bail!("usage: cartoon stats [--since <e.g. 7d|24h|30m>]"),
    }
}

fn parse_logs(args: &[String]) -> anyhow::Result<LogsQuery> {
    const USAGE: &str = "usage: cartoon logs [--tag <t>] | cartoon logs (<id> | --last) [--stdout | --stderr] | cartoon logs grep <pattern> [<id> | --last] [-C <lines>]";
    if args.first().map(String::as_str) == Some("grep") {
        return parse_logs_grep(&args[1..], USAGE);
    }
    let mut sel: Option<RunSel> = None;
    let mut stream = StreamSel::Both;
    let mut tag: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--last" if sel.is_none() => sel = Some(RunSel::Last),
            "--stdout" if stream == StreamSel::Both => stream = StreamSel::Stdout,
            "--stderr" if stream == StreamSel::Both => stream = StreamSel::Stderr,
            "--tag" => {
                let v = it.next().ok_or_else(|| anyhow::anyhow!(USAGE))?;
                tag = Some(v.clone());
            }
            s if !s.starts_with('-') && sel.is_none() => sel = Some(RunSel::Id(s.to_string())),
            _ => anyhow::bail!(USAGE),
        }
    }
    match (sel, tag) {
        (None, t) if stream == StreamSel::Both => Ok(LogsQuery::List { tag: t }),
        (Some(sel), None) => Ok(LogsQuery::Show { sel, stream }),
        _ => anyhow::bail!(USAGE),
    }
}

fn parse_logs_grep(args: &[String], usage: &str) -> anyhow::Result<LogsQuery> {
    let mut pattern: Option<String> = None;
    let mut sel: Option<RunSel> = None;
    let mut context = 2usize;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--last" if sel.is_none() => sel = Some(RunSel::Last),
            "-C" => {
                let v = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!(usage.to_string()))?;
                context = v.parse().map_err(|_| anyhow::anyhow!(usage.to_string()))?;
            }
            s if pattern.is_none() => pattern = Some(s.to_string()),
            s if sel.is_none() && !s.starts_with('-') => sel = Some(RunSel::Id(s.to_string())),
            _ => anyhow::bail!(usage.to_string()),
        }
    }
    let pattern = pattern.ok_or_else(|| anyhow::anyhow!(usage.to_string()))?;
    Ok(LogsQuery::Grep {
        sel: sel.unwrap_or(RunSel::Last),
        pattern,
        context,
    })
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
                compress: None,
                heuristic: false,
                raw: false,
                tags: vec![],
                fast: false
            }
        );
    }

    #[test]
    fn heuristic_flag_before_command() {
        let m = mode(&["cartoon", "--heuristic", "ls", "-la"]);
        assert!(matches!(
            m,
            Mode::Wrap {
                heuristic: true,
                ..
            }
        ));
    }

    #[test]
    fn stats_subcommand_with_since() {
        let m = mode(&["cartoon", "stats", "--since", "7d"]);
        assert_eq!(
            m,
            Mode::Stats {
                since: Some("7d".into())
            }
        );
    }

    #[test]
    fn adapters_subcommand() {
        assert_eq!(mode(&["cartoon", "adapters"]), Mode::Adapters);
    }

    #[test]
    fn no_command_is_error() {
        assert!(parse_mode(Cli::parse_from(["cartoon"])).is_err());
    }

    #[test]
    fn raw_flag_before_command() {
        let m = mode(&["cartoon", "--raw", "pytest"]);
        assert!(matches!(m, Mode::Wrap { raw: true, .. }));
    }

    #[test]
    fn stats_bare_gives_none() {
        assert_eq!(mode(&["cartoon", "stats"]), Mode::Stats { since: None });
    }

    #[test]
    fn tag_flags_collect_into_wrap_mode() {
        let m = mode(&["cartoon", "--tag", "api", "--tag", "ci", "pytest"]);
        assert_eq!(
            m,
            Mode::Wrap {
                argv: vec!["pytest".into()],
                compress: None,
                heuristic: false,
                raw: false,
                tags: vec!["api".into(), "ci".into()],
                fast: false
            }
        );
    }

    #[test]
    fn logs_bare_lists() {
        assert_eq!(
            mode(&["cartoon", "logs"]),
            Mode::Logs(LogsQuery::List { tag: None })
        );
    }

    #[test]
    fn logs_tag_filter() {
        assert_eq!(
            mode(&["cartoon", "logs", "--tag", "api"]),
            Mode::Logs(LogsQuery::List {
                tag: Some("api".into())
            })
        );
    }

    #[test]
    fn logs_by_id_with_stream() {
        assert_eq!(
            mode(&["cartoon", "logs", "20260610-051203-ab12", "--stdout"]),
            Mode::Logs(LogsQuery::Show {
                sel: RunSel::Id("20260610-051203-ab12".into()),
                stream: StreamSel::Stdout
            })
        );
    }

    #[test]
    fn logs_last_both_streams() {
        assert_eq!(
            mode(&["cartoon", "logs", "--last"]),
            Mode::Logs(LogsQuery::Show {
                sel: RunSel::Last,
                stream: StreamSel::Both
            })
        );
    }

    #[test]
    fn logs_bad_args_error() {
        assert!(parse_mode(Cli::parse_from(["cartoon", "logs", "--nope"])).is_err());
        assert!(parse_mode(Cli::parse_from(["cartoon", "logs", "id1", "id2"])).is_err());
    }

    #[test]
    fn fast_flag_before_command() {
        let m = mode(&["cartoon", "--fast", "pytest", "-q"]);
        assert!(matches!(m, Mode::Wrap { fast: true, .. }));
    }

    #[test]
    fn fast_composes_with_tag_and_heuristic() {
        let m = mode(&["cartoon", "--fast", "--tag", "ci", "--heuristic", "make"]);
        assert!(matches!(
            m,
            Mode::Wrap {
                fast: true,
                heuristic: true,
                ..
            }
        ));
    }

    #[test]
    fn fast_defaults_off() {
        let m = mode(&["cartoon", "pytest"]);
        assert!(matches!(m, Mode::Wrap { fast: false, .. }));
    }

    #[test]
    fn compress_flag_parses() {
        let cli = Cli::parse_from(["cartoon", "--compress", "aggressive", "make"]);
        assert_eq!(cli.compress.as_deref(), Some("aggressive"));
    }
}
