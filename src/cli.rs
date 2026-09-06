use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "cartoon",
    version,
    about = "Token-optimized TOON output wrapper for any CLI",
    after_help = "Subcommands:
  stats [--since <7d|24h|30m>]               tokens saved, per adapter
  adapters                                   list built-in adapters
  doctor                                     health report: hook, config,
                                             allowlist gaps, ledger damage
  init                                        scan for wrapper scripts (e.g.
                                             build.sh) and suggest a
                                             .cartoon.toml wrap_scripts pin
  logs [--tag <t>]                           list archived raw runs
  logs (<id> | --last) [--stdout|--stderr]   print a run's full raw output
  logs grep <pattern> [<id>|--last] [-C n]   search a run's raw output
  learn [--since <7d|24h|30m>]               config suggestions from your runs
  hook (install|uninstall|status|rewrite)    agent auto-wrap hook
                                             (Claude Code, Copilot CLI,
                                             VS Code Copilot Chat)
  shim (install|uninstall|status|print)      shell-function wrappers for
                                             agents without a hook
  instructions (install|uninstall|status|print)
                                             write the wrap/never-pipe directive
                                             (CLAUDE.md if present, else AGENTS.md;
                                             --copilot/--claude/--agents force one)
                                             — covers the pipe case the hook can't
  ingest (<file> | -)                        compress an existing log file
                                             (or stdin: some-cmd | cartoon -)

Every wrapped run archives its complete raw stdout/stderr and prints the
location as a `raw_log:` footer — `cartoon logs grep` that instead of
rerunning unwrapped.

Non-adapter output compresses through the safe tier by default (ANSI,
progress, duplicate and blank collapse — non-lossy in practice);
--compress=aggressive adds lossy rules with the raw log as escape hatch.

`stats`, `adapters`, `doctor`, `init`, `logs`, `learn`, `hook`, `shim`, \
`instructions`, and `ingest` are reserved words; to wrap a binary literally named `stats`, \
use: cartoon env stats"
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
    /// and compress through the generic ladder. `<adapter cmd> | head|tail|
    /// grep …` runs the adapter and drops the filter (disclosed).
    #[arg(short = 'c', long = "shell", value_name = "STRING")]
    pub shell: Option<String>,

    /// JUnit XML file (or directory of them) the command writes; rendered
    /// as a test report after the run. Works for any runner (gradle, mvn,
    /// dotnet --logger junit, phpunit --log-junit, …).
    #[arg(long, value_name = "PATH")]
    pub junit: Option<String>,

    /// Hard ceiling on emitted tokens: head + tail kept, middle replaced by
    /// one disclosed marker. Also: CARTOON_MAX_TOKENS env, `max_tokens` config.
    #[arg(long, value_name = "N")]
    pub max_tokens: Option<usize>,

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
        junit: Option<String>,
        max_tokens: Option<usize>,
        /// A pure output filter (`tail -5`) dropped from a `-c` pipeline
        /// because the adapter report already shrinks the output.
        dropped_filter: Option<String>,
    },
    Doctor,
    Stats {
        since: Option<String>,
    },
    Adapters,
    Init,
    Logs(LogsQuery),
    Learn {
        since: Option<String>,
    },
    Hook {
        args: Vec<String>,
    },
    Shim {
        args: Vec<String>,
    },
    Instructions {
        args: Vec<String>,
    },
    /// Run an existing log (file or stdin) through the compression flow.
    Ingest {
        source: String,
        compress: Option<String>,
        tags: Vec<String>,
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

/// True shell syntax that needs `sh -c`: operators, substitution, globbing,
/// brace/tilde expansion, and a leading `NAME=value` env assignment. Quotes
/// and `=` inside an argument are NOT shell syntax — `shell_words` tokenizes
/// them so adapters still see the real argv0 (`xcodebuild test -destination
/// 'platform=iOS Simulator,name=iPhone 17'` must reach the xcodebuild adapter).
fn needs_shell(s: &str) -> bool {
    has_unquoted_operator(s) || leading_env_assignment(s)
}

/// Shell metacharacters count only outside quotes: `pytest -k 'a|b'` is a
/// plain argument, `pytest | tail` is a pipeline.
fn has_unquoted_operator(s: &str) -> bool {
    let (mut in_single, mut in_double, mut escaped) = (false, false, false);
    for c in s.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if !in_single => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '\n' => return true,
            // `$` and backtick expand inside double quotes too.
            '$' | '`' if !in_single => return true,
            '|' | '&' | ';' | '<' | '>' | '(' | ')' | '*' | '?' | '[' | '{' | '~'
                if !in_single && !in_double =>
            {
                return true
            }
            _ => {}
        }
    }
    false
}

fn leading_env_assignment(s: &str) -> bool {
    s.split_whitespace().next().is_some_and(|w| {
        w.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    })
}

fn via_shell(s: &str) -> Vec<String> {
    let sh = if cfg!(windows) { "cmd" } else { "sh" };
    let flag = if cfg!(windows) { "/C" } else { "-c" };
    vec![sh.to_string(), flag.to_string(), s.to_string()]
}

/// Turn a `-c` string into argv: real shell syntax runs via `sh -c`; anything
/// else is word-split (quote-aware) so adapter detection works. Unbalanced
/// quotes fail open to the shell rather than guess.
pub fn shell_argv(s: &str) -> Vec<String> {
    if needs_shell(s) {
        return via_shell(s);
    }
    match shell_words::split(s) {
        Ok(argv) if !argv.is_empty() => argv,
        _ => via_shell(s),
    }
}

/// Output filters whose only job is to shrink text — which the adapter
/// report already does better. `tee`, `xargs`, `sort` and friends change or
/// redirect the data and keep today's `sh -c` behavior.
const PURE_FILTERS: &[&str] = &["head", "tail", "grep", "wc", "cat", "less", "more"];

/// `<adapter cmd> | <pure output filter>` → run the adapter and drop the
/// filter (disclosed in the report as `pipe_filter_dropped`). Anything else
/// goes through `shell_argv` unchanged. Closes the `cartoon -c 'pytest | tail'`
/// gap (issue #12).
pub fn shell_argv_with_filter(s: &str) -> (Vec<String>, Option<String>) {
    if let Ok(tokens) = shell_words::split(s) {
        let bars: Vec<usize> = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| *t == "|")
            .map(|(i, _)| i)
            .collect();
        if let [i] = bars[..] {
            let (lhs, rhs) = (&tokens[..i], &tokens[i + 1..]);
            let lhs_str = shell_words::join(lhs.iter().map(String::as_str));
            let filter_ok = rhs
                .first()
                .is_some_and(|f| PURE_FILTERS.contains(&f.as_str()));
            if !lhs.is_empty()
                && filter_ok
                && !needs_shell(&lhs_str)
                && crate::adapters::find_adapter(lhs).is_some()
            {
                return (
                    lhs.to_vec(),
                    Some(shell_words::join(rhs.iter().map(String::as_str))),
                );
            }
        }
    }
    (shell_argv(s), None)
}

/// For a shell-string argv (`sh -c <string>` / `cmd /C <string>`), the first
/// word of the string that is not an env assignment — the command the user
/// actually meant. Recorded in stats/logs so `learn` can see through `sh`.
pub fn inner_command(argv: &[String]) -> Option<String> {
    let first = argv.first()?;
    let is_shell = matches!(
        crate::adapters::basename(first),
        "sh" | "bash" | "zsh" | "dash" | "cmd"
    );
    let is_c = matches!(argv.get(1)?.as_str(), "-c" | "/C" | "/c");
    if !is_shell || !is_c {
        return None;
    }
    argv.get(2)?
        .split_whitespace()
        .find(|w| {
            !w.split_once('=').is_some_and(|(name, _)| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
        })
        .map(String::from)
}

pub fn parse_mode(cli: Cli) -> anyhow::Result<Mode> {
    if let Some(s) = cli.shell {
        if !cli.command.is_empty() {
            anyhow::bail!("-c/--shell takes the whole command as one string; drop the extra args");
        }
        let (argv, dropped_filter) = shell_argv_with_filter(&s);
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
            junit: cli.junit,
            max_tokens: cli.max_tokens,
            dropped_filter,
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
        "doctor" => Ok(Mode::Doctor),
        "init" => Ok(Mode::Init),
        "logs" => Ok(Mode::Logs(parse_logs(&cli.command[1..])?)),
        "learn" => Ok(Mode::Learn {
            since: parse_since(&cli.command[1..])?,
        }),
        "hook" => Ok(Mode::Hook {
            args: cli.command[1..].to_vec(),
        }),
        "shim" => Ok(Mode::Shim {
            args: cli.command[1..].to_vec(),
        }),
        "instructions" => Ok(Mode::Instructions {
            args: cli.command[1..].to_vec(),
        }),
        "ingest" => match &cli.command[1..] {
            [source] => Ok(Mode::Ingest {
                source: source.clone(),
                compress: cli.compress,
                tags: cli.tags,
            }),
            _ => anyhow::bail!("usage: cartoon ingest (<file> | -)"),
        },
        // `some-cmd | cartoon -` shorthand for stdin ingest
        "-" if cli.command.len() == 1 => Ok(Mode::Ingest {
            source: "-".into(),
            compress: cli.compress,
            tags: cli.tags,
        }),
        _ => Ok(Mode::Wrap {
            argv: cli.command,
            compress: cli.compress,
            heuristic: cli.heuristic,
            raw: cli.raw,
            tags: cli.tags,
            fast: cli.fast,
            junit: cli.junit,
            max_tokens: cli.max_tokens,
            dropped_filter: None,
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

    fn sv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn quoted_args_and_equals_do_not_force_a_shell() {
        assert_eq!(
            shell_argv(
                "xcodebuild test -destination 'platform=iOS Simulator,name=iPhone 17' -scheme App"
            ),
            sv(&[
                "xcodebuild",
                "test",
                "-destination",
                "platform=iOS Simulator,name=iPhone 17",
                "-scheme",
                "App"
            ])
        );
        assert_eq!(
            shell_argv("swift build -Xswiftc -strict-concurrency=complete"),
            sv(&["swift", "build", "-Xswiftc", "-strict-concurrency=complete"])
        );
        assert_eq!(
            shell_argv(r#"pytest -k "a and b" tests/"#),
            sv(&["pytest", "-k", "a and b", "tests/"])
        );
    }

    #[test]
    fn operators_inside_quotes_are_arguments() {
        assert_eq!(shell_argv("pytest -k 'a|b'"), sv(&["pytest", "-k", "a|b"]));
        assert_eq!(
            shell_argv(r#"grep -E "x|y" f"#),
            sv(&["grep", "-E", "x|y", "f"])
        );
        // Expansion still happens inside double quotes: shell it is.
        assert_eq!(&shell_argv(r#"echo "$HOME""#)[..2], &sv(&["sh", "-c"])[..]);
    }

    #[test]
    fn real_shell_syntax_still_forces_sh_c() {
        for s in [
            "pytest | tail -5",
            "FOO=1 pytest",
            "cargo test && echo ok",
            "ls *.py",
            "echo $HOME",
            "pytest > out.txt",
        ] {
            assert_eq!(&shell_argv(s)[..2], &sv(&["sh", "-c"])[..], "{s}");
        }
        // Unbalanced quote: fail open to the shell rather than guess.
        assert_eq!(&shell_argv("pytest -k 'oops")[..2], &sv(&["sh", "-c"])[..]);
    }

    #[test]
    fn inner_command_reads_through_sh_c() {
        assert_eq!(
            inner_command(&sv(&["sh", "-c", "xcodebuild test -scheme A | tail -3"])),
            Some("xcodebuild".into())
        );
        assert_eq!(
            inner_command(&sv(&["sh", "-c", "FOO=1 ./build.sh -d"])),
            Some("./build.sh".into())
        );
        assert_eq!(inner_command(&sv(&["pytest", "-q"])), None);
        assert_eq!(inner_command(&sv(&["sh", "script.sh"])), None);
    }

    #[test]
    fn adapter_command_piped_to_a_pure_filter_drops_the_filter() {
        let (argv, dropped) = shell_argv_with_filter("pytest -v | tail -5");
        assert_eq!(argv, sv(&["pytest", "-v"]));
        assert_eq!(dropped.as_deref(), Some("tail -5"));
        let (argv, dropped) = shell_argv_with_filter("npx jest src/ | grep -i fail");
        assert_eq!(argv, sv(&["npx", "jest", "src/"]));
        assert_eq!(dropped.as_deref(), Some("grep -i fail"));
    }

    #[test]
    fn pipes_that_are_not_pure_filters_or_not_adapters_keep_the_shell() {
        assert_eq!(
            &shell_argv_with_filter("pytest | tee out.txt").0[..2],
            &sv(&["sh", "-c"])[..]
        );
        assert_eq!(
            &shell_argv_with_filter("echo hi | tail -1").0[..2],
            &sv(&["sh", "-c"])[..]
        );
        assert_eq!(
            &shell_argv_with_filter("pytest | head | tail").0[..2],
            &sv(&["sh", "-c"])[..]
        );
        // A pipe inside quotes is an argument, not a pipeline.
        assert_eq!(
            shell_argv_with_filter("pytest -k 'a|b'").0,
            sv(&["pytest", "-k", "a|b"])
        );
    }

    #[test]
    fn doctor_subcommand() {
        assert_eq!(mode(&["cartoon", "doctor"]), Mode::Doctor);
    }

    #[test]
    fn junit_and_max_tokens_flags_parse() {
        let m = mode(&[
            "cartoon",
            "--junit",
            "build/test-results",
            "--max-tokens",
            "1500",
            "gradle",
            "test",
        ]);
        assert!(
            matches!(m, Mode::Wrap { junit: Some(ref j), max_tokens: Some(1500), .. } if j == "build/test-results")
        );
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
                fast: false,
                junit: None,
                max_tokens: None,
                dropped_filter: None
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
    fn init_subcommand() {
        assert_eq!(mode(&["cartoon", "init"]), Mode::Init);
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
                fast: false,
                junit: None,
                max_tokens: None,
                dropped_filter: None
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

    #[test]
    fn instructions_subcommand_collects_args() {
        assert_eq!(
            mode(&["cartoon", "instructions", "install", "--copilot"]),
            Mode::Instructions {
                args: vec!["install".into(), "--copilot".into()]
            }
        );
    }

    #[test]
    fn ingest_file_parses() {
        assert_eq!(
            mode(&["cartoon", "ingest", "build.log"]),
            Mode::Ingest {
                source: "build.log".into(),
                compress: None,
                tags: vec![]
            }
        );
    }

    #[test]
    fn ingest_with_compress_and_tag() {
        let m = mode(&[
            "cartoon",
            "--compress",
            "aggressive",
            "--tag",
            "ci",
            "ingest",
            "x.log",
        ]);
        assert_eq!(
            m,
            Mode::Ingest {
                source: "x.log".into(),
                compress: Some("aggressive".into()),
                tags: vec!["ci".into()]
            }
        );
    }

    #[test]
    fn bare_dash_is_stdin_ingest() {
        assert_eq!(
            mode(&["cartoon", "-"]),
            Mode::Ingest {
                source: "-".into(),
                compress: None,
                tags: vec![]
            }
        );
    }

    #[test]
    fn ingest_without_source_errors() {
        assert!(parse_mode(Cli::parse_from(["cartoon", "ingest"])).is_err());
        assert!(parse_mode(Cli::parse_from(["cartoon", "ingest", "a", "b"])).is_err());
    }
}
