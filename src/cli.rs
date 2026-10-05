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
  last [--cmd <substring>]                   re-show the newest run's report
  diff [<id-a> <id-b>] [--cmd <substring>]   fixed / still failing / new
                                             failures vs the previous run
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
  mcp                                        MCP server on stdio (run, logs,
                                             stats tools) for hookless agents

Every wrapped run archives its complete raw stdout/stderr and prints the
location as a `raw_log:` footer — `cartoon logs grep` that instead of
rerunning unwrapped.

Non-adapter output compresses through the safe tier by default (ANSI,
progress, duplicate and blank collapse — non-lossy in practice);
--compress=aggressive adds lossy rules with the raw log as escape hatch.

`stats`, `adapters`, `doctor`, `init`, `logs`, `learn`, `hook`, `shim`, \
`instructions`, `ingest`, and `mcp` are reserved words (each takes --help); to wrap a \
binary literally named `stats`, use: cartoon env stats. `last` and `diff` are \
reserved only in the forms above (`cartoon diff a.txt b.txt` wraps diff). A \
command whose name starts with `-` goes after `--`: cartoon -- --weird-bin"
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

    /// Compress stdout and stderr together in arrival order (as `2>&1`
    /// shows them) and write the result to stdout only. Also: `[compress]`
    /// or `[command.X]` `merge_streams = true` in config.
    #[arg(long)]
    pub merge_streams: bool,

    /// Command to wrap plus its args (or a reserved subcommand: stats |
    /// adapters | doctor | init | logs | learn | hook | shim | instructions |
    /// ingest | mcp). Unknown `--flags` before the command are an error, not a
    /// command name; use `--` to run a binary whose name starts with `-`.
    #[arg(trailing_var_arg = true)]
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
        merge_streams: bool,
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
        max_tokens: Option<usize>,
    },
    /// `cartoon last [--cmd <s>]`: re-show the newest archived run's report.
    Last {
        cmd: Option<String>,
    },
    /// `cartoon diff [<id-a> <id-b>] [--cmd <s>]`: compare two runs' reports.
    Diff {
        ids: Option<(String, String)>,
        cmd: Option<String>,
    },
    /// Model Context Protocol server on stdin/stdout.
    Mcp {
        args: Vec<String>,
    },
    /// `<subcommand> --help`: print this usage and exit 0 without running.
    Help(&'static str),
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
///
/// Both sides must be free of any other shell syntax: `| tail -5 > f.txt`
/// or `| tail -2 && echo X` would silently lose the redirect / the trailing
/// command if the filter were dropped, so those keep `sh -c`.
pub fn shell_argv_with_filter(s: &str) -> (Vec<String>, Option<String>) {
    if let Some((lhs_str, rhs_str)) = split_single_pipe(s) {
        if !needs_shell(lhs_str) && !has_unquoted_operator(rhs_str) {
            if let (Ok(lhs), Ok(rhs)) = (shell_words::split(lhs_str), shell_words::split(rhs_str)) {
                let filter_ok = rhs
                    .first()
                    .is_some_and(|f| PURE_FILTERS.contains(&f.as_str()));
                if !lhs.is_empty() && filter_ok && crate::adapters::find_adapter(&lhs).is_some() {
                    return (lhs, Some(shell_words::join(rhs.iter().map(String::as_str))));
                }
            }
        }
    }
    (shell_argv(s), None)
}

/// Split `s` at its one unquoted `|` (not `||`, not `|&`). None when there is
/// no such pipe or more than one.
fn split_single_pipe(s: &str) -> Option<(&str, &str)> {
    let (mut in_single, mut in_double, mut escaped) = (false, false, false);
    let mut found: Option<usize> = None;
    let bytes = s.as_bytes();
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if !in_single => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '|' if !in_single && !in_double => {
                let prev = i.checked_sub(1).map(|j| bytes[j]);
                let next = bytes.get(i + 1).copied();
                if prev == Some(b'|') || next == Some(b'|') || next == Some(b'&') || found.is_some()
                {
                    return None;
                }
                found = Some(i);
            }
            _ => {}
        }
    }
    found.map(|i| (&s[..i], &s[i + 1..]))
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
            merge_streams: cli.merge_streams,
        });
    }
    if cli.command.is_empty() {
        anyhow::bail!("no command given. usage: cartoon <cmd> [args...]");
    }
    if let Some(text) = subcommand_help(&cli.command) {
        return Ok(Mode::Help(text));
    }
    if let Some(m) = parse_last_diff(&cli.command) {
        return Ok(m);
    }
    match cli.command[0].as_str() {
        "stats" => Ok(Mode::Stats {
            since: parse_since(&cli.command[1..], STATS_USAGE)?,
        }),
        "adapters" => Ok(Mode::Adapters),
        "doctor" => Ok(Mode::Doctor),
        "init" => Ok(Mode::Init),
        "logs" => Ok(Mode::Logs(parse_logs(&cli.command[1..])?)),
        "learn" => Ok(Mode::Learn {
            since: parse_since(&cli.command[1..], LEARN_USAGE)?,
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
        "mcp" => Ok(Mode::Mcp {
            args: cli.command[1..].to_vec(),
        }),
        "ingest" => match &cli.command[1..] {
            [source] => Ok(Mode::Ingest {
                source: source.clone(),
                compress: cli.compress,
                tags: cli.tags,
                max_tokens: cli.max_tokens,
            }),
            _ => anyhow::bail!(INGEST_USAGE),
        },
        // `some-cmd | cartoon -` shorthand for stdin ingest
        "-" if cli.command.len() == 1 => Ok(Mode::Ingest {
            source: "-".into(),
            compress: cli.compress,
            tags: cli.tags,
            max_tokens: cli.max_tokens,
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
            merge_streams: cli.merge_streams,
        }),
    }
}

fn parse_since(args: &[String], usage: &'static str) -> anyhow::Result<Option<String>> {
    match args {
        [] => Ok(None),
        [flag, value] if flag == "--since" => Ok(Some(value.clone())),
        _ => anyhow::bail!(usage),
    }
}

/// `last [--cmd <s>]`, `diff [--cmd <s>]`, `diff <run-id> <run-id>`; None
/// for any other shape (so `cartoon diff a.txt b.txt` wraps the system diff).
fn parse_last_diff(command: &[String]) -> Option<Mode> {
    let (sub, rest) = command.split_first()?;
    if sub != "last" && sub != "diff" {
        return None;
    }
    let cmd = match rest {
        [] => None,
        [flag, s] if flag == "--cmd" => Some(s.clone()),
        [a, b] if sub == "diff" && [a, b].iter().all(|x| crate::archive::looks_like_run_id(x)) => {
            return Some(Mode::Diff {
                ids: Some((a.clone(), b.clone())),
                cmd: None,
            })
        }
        _ => return None,
    };
    Some(match sub.as_str() {
        "last" => Mode::Last { cmd },
        _ => Mode::Diff { ids: None, cmd },
    })
}

const STATS_USAGE: &str = "usage: cartoon stats [--since <e.g. 7d|24h|30m>]";
const LEARN_USAGE: &str = "usage: cartoon learn [--since <e.g. 7d|24h|30m>]";
const INGEST_USAGE: &str =
    "usage: cartoon [--compress <level>] [--max-tokens <n>] [--tag <t>] ingest (<file> | -)";

/// Per-subcommand `--help` text. Each reserved word answers its own help
/// instead of running (`doctor`, `init`), parsing `--help` as an argument
/// (`ingest` opening a file named `--help`) or printing a sibling's usage.
fn help_text(sub: &str) -> Option<&'static str> {
    Some(match sub {
        "stats" => concat!(
            "usage: cartoon stats [--since <e.g. 7d|24h|30m>]\n\n",
            "Tokens saved by wrapped runs, per adapter, from the local ledger."
        ),
        "learn" => concat!(
            "usage: cartoon learn [--since <e.g. 7d|24h|30m>]\n\n",
            "Config suggestions mined from your recorded runs (token wasters,\n",
            "repeated failures), with a ready-to-paste config snippet."
        ),
        "adapters" => concat!(
            "usage: cartoon adapters\n\n",
            "List the built-in adapters and the commands each one matches."
        ),
        "doctor" => concat!(
            "usage: cartoon doctor\n\n",
            "Health report: hook / plugin install, config validity, wrap_scripts\n",
            "missing on disk, allowlisted tools without an adapter, ledger damage."
        ),
        "init" => concat!(
            "usage: cartoon init\n\n",
            "Scan the current directory for wrapper scripts (e.g. build.sh) that run\n",
            "a noisy dev tool and suggest a .cartoon.toml wrap_scripts pin."
        ),
        "logs" => concat!(
            "usage: cartoon logs [--tag <t>]                          list archived runs\n",
            "       cartoon logs (<id> | --last) [--stdout | --stderr]  print a run's raw output\n",
            "       cartoon logs grep <pattern> [<id> | --last] [-C <lines>]\n",
            "                                                         search a run's raw output"
        ),
        "hook" => concat!(
            "usage: cartoon hook install [--copilot|--vscode] [--project] [--deny] [--instructions]\n",
            "       cartoon hook uninstall [--copilot|--vscode] [--project] [--instructions]\n",
            "       cartoon hook status\n",
            "       cartoon hook rewrite [--deny-mode]   (reads the agent's hook JSON on stdin)\n\n",
            "Agent auto-wrap hook for Claude Code, Copilot CLI and VS Code Copilot Chat."
        ),
        "shim" => concat!(
            "usage: cartoon shim (print | install | uninstall | status | path)\n\n",
            "Shell-function wrappers for agents without a hook. Disable per shell\n",
            "with CARTOON_NO_SHIM=1."
        ),
        "instructions" => concat!(
            "usage: cartoon instructions (install | uninstall) [--agents|--copilot|--claude]\n",
            "       cartoon instructions (status | print)\n\n",
            "Write the wrap/never-pipe directive into CLAUDE.md if present, else\n",
            "AGENTS.md (or the file the flag names)."
        ),
        "last" => concat!(
            "usage: cartoon last [--cmd <substring>]\n\n",
            "Re-show the newest archived run's report (or, for a run no adapter\n",
            "parsed, a short summary and its raw_log path) without re-running it.\n",
            "--cmd picks the newest run whose command contains <substring>."
        ),
        "diff" => concat!(
            "usage: cartoon diff [--cmd <substring>]\n",
            "       cartoon diff <id-a> <id-b>\n\n",
            "Compare the newest adapter run (test/lint/build) with the previous run\n",
            "of the same command in the same directory, or run <id-a> with <id-b>:\n",
            "fixed, still_failing and new_failures. Tests match by id; diagnostics\n",
            "by file + rule + message (line numbers shift as you edit). Exits 0, or\n",
            "1 when there is no comparable pair. Other args wrap the system diff."
        ),
        "ingest" => concat!(
            "usage: cartoon [--compress <level>] [--max-tokens <n>] [--tag <t>] ingest (<file> | -)\n",
            "       some-cmd | cartoon -\n\n",
            "Compress an existing log file (or stdin) through the same flow as a\n",
            "wrapped run, archiving the raw text."
        ),
        "mcp" => concat!(
            "usage: cartoon mcp\n\n",
            "Model Context Protocol server over stdio (JSON-RPC, one message per\n",
            "line) for agents without a PreToolUse hook: tools run, logs_grep,\n",
            "logs_list and stats. Register it in the agent's MCP config, e.g.\n",
            "claude mcp add cartoon -- cartoon mcp"
        ),
        _ => return None,
    })
}

/// `cartoon <reserved> --help|-h` (as the first argument, or `--help`
/// anywhere after it) → that subcommand's help. Only `-h` in the first slot
/// counts, so `cartoon logs grep -h` still searches for `-h`.
fn subcommand_help(command: &[String]) -> Option<&'static str> {
    let (sub, rest) = command.split_first()?;
    let asks = rest.first().is_some_and(|a| a == "-h") || rest.iter().any(|a| a == "--help");
    if asks {
        help_text(sub)
    } else {
        None
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
        // ...and does not hide the real pipe after it.
        let (argv, dropped) = shell_argv_with_filter("pytest -k 'a|b' | tail -3");
        assert_eq!(argv, sv(&["pytest", "-k", "a|b"]));
        assert_eq!(dropped.as_deref(), Some("tail -3"));
    }

    #[test]
    fn filter_with_any_other_shell_syntax_keeps_the_shell() {
        for s in [
            "pytest | tail -5 > f.txt",
            "pytest | tail -5 >> f.txt",
            "pytest | tail -2 && echo X",
            "pytest | tail -2 || echo X",
            "pytest | tail -2; echo X",
            "pytest | tail -2 & echo X",
            "pytest | grep $PAT",
            "pytest | grep `cat pat`",
            "pytest | head < /dev/null",
            "pytest | tail -2&&echo X",
            "pytest || tail -2",
            "pytest |& tail -2",
            "pytest | head | tail",
        ] {
            let (argv, dropped) = shell_argv_with_filter(s);
            assert_eq!(argv, via_shell(s), "{s}");
            assert_eq!(dropped, None, "{s}");
        }
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
    fn merge_streams_flag_parses_for_argv_and_shell_strings() {
        let m = mode(&["cartoon", "--merge-streams", "make"]);
        assert!(matches!(
            m,
            Mode::Wrap {
                merge_streams: true,
                ..
            }
        ));
        let m = mode(&["cartoon", "--merge-streams", "-c", "make && make test"]);
        assert!(matches!(
            m,
            Mode::Wrap {
                merge_streams: true,
                ..
            }
        ));
        let m = mode(&["cartoon", "-c", "make"]);
        assert!(matches!(
            m,
            Mode::Wrap {
                merge_streams: false,
                ..
            }
        ));
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
                dropped_filter: None,
                merge_streams: false
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
                dropped_filter: None,
                merge_streams: false
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
                tags: vec![],
                max_tokens: None
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
                tags: vec!["ci".into()],
                max_tokens: None
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
                tags: vec![],
                max_tokens: None
            }
        );
    }

    #[test]
    fn ingest_without_source_errors() {
        assert!(parse_mode(Cli::parse_from(["cartoon", "ingest"])).is_err());
        assert!(parse_mode(Cli::parse_from(["cartoon", "ingest", "a", "b"])).is_err());
    }

    #[test]
    fn ingest_carries_max_tokens() {
        let m = mode(&["cartoon", "--max-tokens", "40", "ingest", "x.log"]);
        assert!(matches!(
            m,
            Mode::Ingest {
                max_tokens: Some(40),
                ..
            }
        ));
        let m = mode(&["cartoon", "--max-tokens", "40", "-"]);
        assert!(matches!(
            m,
            Mode::Ingest {
                max_tokens: Some(40),
                ..
            }
        ));
    }

    #[test]
    fn unknown_option_before_the_command_is_a_usage_error() {
        for args in [
            &["cartoon", "--rwa", "tool"][..],
            &["cartoon", "-x", "tool"][..],
        ] {
            let e = Cli::try_parse_from(args).unwrap_err();
            assert_eq!(e.kind(), clap::error::ErrorKind::UnknownArgument);
            assert_eq!(e.exit_code(), 2);
        }
    }

    #[test]
    fn double_dash_runs_a_hyphenated_binary_and_args_stay_verbatim() {
        let m = mode(&["cartoon", "--", "--weird-bin", "-q"]);
        assert!(matches!(m, Mode::Wrap { ref argv, .. } if *argv == sv(&["--weird-bin", "-q"])));
        // Flags after the command belong to the child, even cartoon's own.
        let m = mode(&["cartoon", "pytest", "--raw", "-c", "x", "--", "-k"]);
        assert!(
            matches!(m, Mode::Wrap { ref argv, raw: false, .. } if *argv == sv(&["pytest", "--raw", "-c", "x", "--", "-k"]))
        );
    }

    #[test]
    fn every_reserved_subcommand_answers_its_own_help() {
        for (sub, needle) in [
            ("stats", "cartoon stats"),
            ("learn", "cartoon learn"),
            ("adapters", "cartoon adapters"),
            ("doctor", "cartoon doctor"),
            ("init", "cartoon init"),
            ("logs", "cartoon logs"),
            ("hook", "cartoon hook"),
            ("shim", "cartoon shim"),
            ("instructions", "cartoon instructions"),
            ("ingest", "ingest (<file> | -)"),
            ("mcp", "cartoon mcp"),
        ] {
            for flag in ["--help", "-h"] {
                match mode(&["cartoon", sub, flag]) {
                    Mode::Help(text) => assert!(text.contains(needle), "{sub} {flag}: {text}"),
                    other => panic!("{sub} {flag}: {other:?}"),
                }
            }
        }
        // `--help` later in the args too.
        assert!(matches!(
            mode(&["cartoon", "hook", "install", "--help"]),
            Mode::Help(t) if t.contains("cartoon hook")
        ));
        // Not a reserved word: `--help` belongs to the wrapped command.
        assert!(matches!(
            mode(&["cartoon", "pytest", "--help"]),
            Mode::Wrap { .. }
        ));
        // `-h` past the first slot is an argument (a grep pattern here).
        assert!(matches!(
            mode(&["cartoon", "logs", "grep", "-h"]),
            Mode::Logs(LogsQuery::Grep { ref pattern, .. }) if pattern == "-h"
        ));
    }

    #[test]
    fn last_and_diff_parse_only_their_query_forms() {
        assert_eq!(mode(&["cartoon", "last"]), Mode::Last { cmd: None });
        assert_eq!(
            mode(&["cartoon", "last", "--cmd", "pytest"]),
            Mode::Last {
                cmd: Some("pytest".into())
            }
        );
        assert_eq!(
            mode(&["cartoon", "diff"]),
            Mode::Diff {
                ids: None,
                cmd: None
            }
        );
        assert_eq!(
            mode(&["cartoon", "diff", "--cmd", "ruff"]),
            Mode::Diff {
                ids: None,
                cmd: Some("ruff".into())
            }
        );
        assert_eq!(
            mode(&[
                "cartoon",
                "diff",
                "20261005-100000-0001",
                "20261005-100003-00ab"
            ]),
            Mode::Diff {
                ids: Some(("20261005-100000-0001".into(), "20261005-100003-00ab".into())),
                cmd: None
            }
        );
        // Anything else is the system tool, wrapped as before.
        assert!(matches!(
            mode(&["cartoon", "diff", "a.txt", "b.txt"]),
            Mode::Wrap { ref argv, .. } if *argv == sv(&["diff", "a.txt", "b.txt"])
        ));
        assert!(matches!(
            mode(&["cartoon", "last", "-n", "5"]),
            Mode::Wrap { .. }
        ));
        for sub in ["last", "diff"] {
            assert!(matches!(
                mode(&["cartoon", sub, "--help"]),
                Mode::Help(t) if t.contains(&format!("cartoon {sub}"))
            ));
        }
    }

    #[test]
    fn mcp_subcommand_collects_args() {
        assert_eq!(mode(&["cartoon", "mcp"]), Mode::Mcp { args: vec![] });
    }

    #[test]
    fn learn_usage_error_names_learn() {
        let e = parse_mode(Cli::parse_from(["cartoon", "learn", "--bogus"])).unwrap_err();
        assert!(e.to_string().contains("cartoon learn"), "{e}");
    }

    #[test]
    fn top_level_help_lists_every_reserved_subcommand() {
        use clap::CommandFactory;
        let help = Cli::command().render_long_help().to_string();
        for sub in [
            "stats",
            "adapters",
            "doctor",
            "init",
            "logs",
            "learn",
            "hook",
            "shim",
            "instructions",
            "ingest",
            "mcp",
        ] {
            assert!(
                help.contains(&format!("`{sub}`")),
                "{sub} not reserved in:\n{help}"
            );
        }
        assert!(!help.contains("or: stats | adapters)"), "{help}");
    }
}
