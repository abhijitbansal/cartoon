//! `cartoon learn` — mine the local stats ledger for actionable
//! suggestions: commands wasting tokens in passthrough/safe mode, repeated
//! identical failures, and ready-to-paste config pins. All local, no
//! telemetry; output is TOON (dogfooding).
use crate::stats::StatRecord;
use anyhow::Result;
use serde_json::{json, Value};

/// A command must waste this much per call, this often, to earn a pin.
const MIN_CALLS: usize = 3;
const MIN_AVG_TOKENS_IN: usize = 500;
/// Modes that mean "output reached the agent (mostly) uncompressed":
/// passthrough always; a ladder tier only when it saved little (see
/// `is_uncompressed`).
const UNCOMPRESSED_MODES: &[&str] = &["passthrough", "safe", "heuristic"];
/// A ladder run that saved at least this share of tokens was compressed;
/// calling it "uncompressed" would be misleading.
const MEANINGFUL_SAVINGS_PCT: usize = 10;
/// Plain file viewers: the right fix is reading less of the file, not a
/// lossy tier over its contents, so they never get an aggressive pin.
const FILE_VIEWERS: &[&str] = &["cat", "less", "more", "head", "tail", "bat", "batcat"];
/// Same command failing this many times in a row is a loop worth breaking.
const REPEAT_FAIL_RUN: usize = 3;
/// argv0 values that mean "a shell ran a command string" — the interesting
/// command is `inner_cmd`, and pinning the shell itself would be wrong.
const SHELL_HEADS: &[&str] = &["sh", "bash", "zsh", "dash", "cmd"];

/// Did this run reach the agent (mostly) uncompressed?
fn is_uncompressed(r: &StatRecord) -> bool {
    match r.adapter.as_str() {
        "passthrough" => true,
        m if UNCOMPRESSED_MODES.contains(&m) => {
            r.tokens_in.saturating_sub(r.tokens_out) * 100 < r.tokens_in * MEANINGFUL_SAVINGS_PCT
        }
        _ => false,
    }
}

/// Is there an adapter for this command head? `inner_cmd` keeps only the
/// first word, so probe the common subcommands too (`cargo test`,
/// `vitest run`, `go test`).
fn has_adapter(head: &str) -> bool {
    const SUBS: &[&[&str]] = &[
        &[],
        &["run"],
        &["test"],
        &["build"],
        &["check"],
        &["lint"],
        &["nextest", "run"],
    ];
    SUBS.iter().any(|sub| {
        let argv: Vec<String> = std::iter::once(head)
            .chain(sub.iter().copied())
            .map(String::from)
            .collect();
        crate::adapters::find_adapter(&argv).is_some()
    })
}

pub fn run(since: Option<&str>) -> Result<i32> {
    let recs = crate::stats::read_records(since)?;
    println!("{}", render(&recs, since));
    Ok(0)
}

/// One `shell_string` suggestion per distinct inner command that reached the
/// agent uncompressed through `sh -c`: the adapter never saw it because a
/// pipe or shell operator forced the shell.
fn shell_string_suggestions(recs: &[StatRecord], shell: &str, out: &mut Vec<Value>) {
    let mut by_inner: Vec<(String, usize, usize)> = Vec::new();
    for r in recs.iter().filter(|r| r.cmd == shell && is_uncompressed(r)) {
        let inner = r.inner_cmd.clone().unwrap_or_else(|| "(unknown)".into());
        match by_inner.iter_mut().find(|(i, _, _)| *i == inner) {
            Some(e) => {
                e.1 += 1;
                e.2 += r.tokens_in;
            }
            None => by_inner.push((inner, 1, r.tokens_in)),
        }
    }
    by_inner.sort_by_key(|(_, _, t)| std::cmp::Reverse(*t));
    for (inner, calls, tokens_in) in by_inner {
        let action = if has_adapter(&inner) {
            format!(
                "`{inner}` ran through `{shell} -c` (a pipe or shell operator in the command string), so its adapter never fired. Run it without the pipe — cartoon already shrinks the output — or drop the operator."
            )
        } else {
            format!(
                "`{inner}` ran through `{shell} -c` (a pipe or shell operator in the command string), so a {} pin cannot apply to it. Run `cartoon {inner} …` directly, without the pipe, and pin it if it stays noisy.",
                crate::config::command_section(&inner)
            )
        };
        out.push(json!({
            "kind": "shell_string",
            "inner_cmd": inner,
            "calls": calls,
            "avg_tokens_in": tokens_in / calls.max(1),
            "action": action,
        }));
    }
}

struct CmdAgg {
    cmd: String,
    calls: usize,
    tokens_in: usize,
    tokens_out: usize,
    passthrough: usize,
}

pub fn render(recs: &[StatRecord], since: Option<&str>) -> String {
    if recs.is_empty() {
        return "learn: no wrapped runs recorded yet — wrap some commands first".into();
    }
    let mut suggestions: Vec<Value> = Vec::new();
    let mut config_lines: Vec<String> = Vec::new();

    // 1. Token wasters: frequent commands stuck in uncompressed modes.
    let mut aggs: Vec<CmdAgg> = Vec::new();
    for r in recs.iter().filter(|r| is_uncompressed(r)) {
        let pass = usize::from(r.adapter == "passthrough");
        match aggs.iter_mut().find(|a| a.cmd == r.cmd) {
            Some(a) => {
                a.calls += 1;
                a.tokens_in += r.tokens_in;
                a.tokens_out += r.tokens_out;
                a.passthrough += pass;
            }
            None => aggs.push(CmdAgg {
                cmd: r.cmd.clone(),
                calls: 1,
                tokens_in: r.tokens_in,
                tokens_out: r.tokens_out,
                passthrough: pass,
            }),
        }
    }
    aggs.retain(|a| a.calls >= MIN_CALLS && a.tokens_in / a.calls >= MIN_AVG_TOKENS_IN);
    aggs.sort_by_key(|a| std::cmp::Reverse(a.tokens_in));
    for a in &aggs {
        let avg = a.tokens_in / a.calls;
        if SHELL_HEADS.contains(&a.cmd.as_str()) {
            // A `[command.sh]` pin would apply to EVERY future shell-string
            // run regardless of what is inside; explain the real cause instead.
            shell_string_suggestions(recs, &a.cmd, &mut suggestions);
            continue;
        }
        if FILE_VIEWERS.contains(&crate::adapters::basename(&a.cmd)) {
            // A lossy tier over a file's contents is the wrong fix.
            continue;
        }
        let section = crate::config::command_section(&a.cmd);
        let today = if a.passthrough == a.calls {
            "output passed through uncompressed today".to_string()
        } else {
            let saved_pct = a.tokens_in.saturating_sub(a.tokens_out) * 100 / a.tokens_in.max(1);
            format!("the safe tier saved only {saved_pct}% today")
        };
        suggestions.push(json!({
            "kind": "token_waster",
            "cmd": a.cmd,
            "calls": a.calls,
            "avg_tokens_in": avg,
            "action": format!("pin {section} level=\"aggressive\" ({today})"),
        }));
        config_lines.push(format!("{section}\nlevel = \"aggressive\""));
    }

    // 2. Repeated failures: same command failing N+ times consecutively.
    let mut run_cmd = String::new();
    let mut run_len = 0usize;
    let mut flagged: Vec<String> = Vec::new();
    for r in recs {
        if r.exit != 0 && r.cmd == run_cmd {
            run_len += 1;
        } else if r.exit != 0 {
            run_cmd = r.cmd.clone();
            run_len = 1;
        } else {
            run_cmd.clear();
            run_len = 0;
        }
        if run_len == REPEAT_FAIL_RUN && !flagged.contains(&r.cmd) {
            flagged.push(r.cmd.clone());
            suggestions.push(json!({
                "kind": "repeat_failure",
                "cmd": r.cmd,
                "calls": REPEAT_FAIL_RUN,
                "action": format!("`{}` failed {REPEAT_FAIL_RUN}x in a row — read the archived log (cartoon logs --last / logs grep) instead of re-running", r.cmd),
            }));
        }
    }

    let mut root = serde_json::Map::new();
    root.insert("analyzed_calls".into(), json!(recs.len()));
    if let Some(s) = since {
        root.insert("window".into(), json!(s));
    }
    let total_saved: i64 = recs.iter().map(|r| r.saved).sum();
    root.insert("tokens_saved".into(), json!(total_saved));
    if suggestions.is_empty() {
        root.insert(
            "verdict".into(),
            json!("no waste detected — adapters and ladder are covering your commands"),
        );
    } else {
        root.insert("suggestions".into(), Value::Array(suggestions));
    }
    let mut out = crate::toon::encode(&Value::Object(root));
    if !config_lines.is_empty() {
        out.push_str(
            "\n\n# paste into .cartoon.toml (repo root) or ~/.config/cartoon/config.toml \
             ($XDG_CONFIG_HOME/cartoon/config.toml):\n",
        );
        out.push_str(&config_lines.join("\n\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(cmd: &str, adapter: &str, tokens_in: usize, exit: i32) -> StatRecord {
        StatRecord {
            ts: "2026-06-11T07:00:00Z".into(),
            cmd: cmd.into(),
            adapter: adapter.into(),
            tokens_in,
            tokens_out: tokens_in,
            saved: 0,
            exit,
            run_id: None,
            inner_cmd: None,
        }
    }

    #[test]
    fn shell_string_runs_get_an_explanation_not_a_sh_pin() {
        let mut recs: Vec<StatRecord> = (0..4)
            .map(|_| rec("sh", "passthrough", 40_000, 0))
            .collect();
        for r in &mut recs {
            r.inner_cmd = Some("xcodebuild".into());
        }
        let out = render(&recs, None);
        assert!(!out.contains("[command.sh]"), "got:\n{out}");
        assert!(out.contains("shell_string"), "got:\n{out}");
        assert!(out.contains("xcodebuild"), "got:\n{out}");
    }

    fn rec_out(cmd: &str, adapter: &str, tokens_in: usize, tokens_out: usize) -> StatRecord {
        StatRecord {
            tokens_out,
            ..rec(cmd, adapter, tokens_in, 0)
        }
    }

    #[test]
    fn path_commands_get_quoted_toml_keys_that_parse() {
        let recs: Vec<StatRecord> = (0..3)
            .map(|_| rec("./b.sh", "passthrough", 5000, 0))
            .collect();
        let out = render(&recs, None);
        assert!(out.contains("[command.\"./b.sh\"]"), "got:\n{out}");
        assert!(!out.contains("[command../b.sh]"), "got:\n{out}");
        let snippet = out.split_once("config.toml):\n").unwrap().1;
        let cfg: crate::config::Config = toml::from_str(snippet).unwrap();
        assert_eq!(cfg.command["./b.sh"].level.as_deref(), Some("aggressive"));
    }

    #[test]
    fn points_at_the_config_files_cartoon_actually_reads() {
        let recs: Vec<StatRecord> = (0..3)
            .map(|_| rec("docker", "passthrough", 5000, 0))
            .collect();
        let out = render(&recs, None);
        assert!(out.contains(".cartoon.toml"), "got:\n{out}");
        assert!(out.contains("~/.config/cartoon/config.toml"), "got:\n{out}");
        assert!(!out.contains("paste into cartoon.toml"), "got:\n{out}");
    }

    #[test]
    fn file_viewers_are_never_pinned_aggressive() {
        for viewer in ["cat", "less", "head", "tail", "bat", "/bin/cat"] {
            let recs: Vec<StatRecord> = (0..4)
                .map(|_| rec(viewer, "passthrough", 9000, 0))
                .collect();
            let out = render(&recs, None);
            assert!(!out.contains("token_waster"), "{viewer}:\n{out}");
            assert!(!out.contains("aggressive"), "{viewer}:\n{out}");
        }
    }

    #[test]
    fn safe_runs_that_saved_tokens_are_not_called_uncompressed() {
        // 40% saved by the safe tier: compressed, not waste.
        let recs: Vec<StatRecord> = (0..4)
            .map(|_| rec_out("docker", "safe", 5000, 3000))
            .collect();
        let out = render(&recs, None);
        assert!(!out.contains("token_waster"), "got:\n{out}");
        // 2% saved: worth a pin, and the action says what happened.
        let recs: Vec<StatRecord> = (0..4)
            .map(|_| rec_out("docker", "safe", 5000, 4900))
            .collect();
        let out = render(&recs, None);
        assert!(out.contains("token_waster"), "got:\n{out}");
        assert!(out.contains("safe tier saved only 2%"), "got:\n{out}");
        assert!(!out.contains("uncompressed today"), "got:\n{out}");
    }

    #[test]
    fn shell_strings_without_an_adapter_do_not_blame_one() {
        let mut recs: Vec<StatRecord> = (0..4)
            .map(|_| rec("sh", "passthrough", 40_000, 0))
            .collect();
        for r in &mut recs {
            r.inner_cmd = Some("./b.sh".into());
        }
        let out = render(&recs, None);
        assert!(out.contains("shell_string"), "got:\n{out}");
        assert!(!out.contains("adapter never fired"), "got:\n{out}");
        // ...while one that has an adapter still says so.
        for r in &mut recs {
            r.inner_cmd = Some("cargo".into());
        }
        let out = render(&recs, None);
        assert!(out.contains("adapter never fired"), "got:\n{out}");
    }

    #[test]
    fn empty_stats_says_so() {
        assert!(render(&[], None).contains("no wrapped runs"));
    }

    #[test]
    fn flags_frequent_passthrough_waster() {
        let recs = vec![
            rec("docker", "passthrough", 4000, 0),
            rec("docker", "passthrough", 5000, 0),
            rec("docker", "safe", 6000, 0),
        ];
        let out = render(&recs, None);
        assert!(out.contains("token_waster"), "got:\n{out}");
        assert!(out.contains("[command.docker]"));
        assert!(out.contains("level = \"aggressive\""));
    }

    #[test]
    fn small_or_rare_commands_not_flagged() {
        let recs = vec![
            rec("ls", "passthrough", 20, 0),
            rec("ls", "passthrough", 20, 0),
            rec("ls", "passthrough", 20, 0),
            rec("make", "passthrough", 9000, 0), // only 1 call
        ];
        let out = render(&recs, None);
        assert!(!out.contains("token_waster"), "got:\n{out}");
        assert!(out.contains("verdict"));
    }

    #[test]
    fn adapter_covered_commands_not_flagged() {
        let recs = vec![
            rec("pytest", "pytest", 9000, 0),
            rec("pytest", "pytest", 9000, 0),
            rec("pytest", "pytest", 9000, 0),
        ];
        let out = render(&recs, None);
        assert!(!out.contains("token_waster"));
    }

    #[test]
    fn repeated_failures_flagged_once() {
        let recs = vec![
            rec("pytest", "pytest", 900, 1),
            rec("pytest", "pytest", 900, 1),
            rec("pytest", "pytest", 900, 1),
            rec("pytest", "pytest", 900, 1),
        ];
        let out = render(&recs, None);
        assert_eq!(out.matches("repeat_failure").count(), 1, "got:\n{out}");
        assert!(out.contains("logs grep"));
    }

    #[test]
    fn passing_runs_reset_failure_streak() {
        let recs = vec![
            rec("pytest", "pytest", 900, 1),
            rec("pytest", "pytest", 900, 0),
            rec("pytest", "pytest", 900, 1),
            rec("pytest", "pytest", 900, 1),
        ];
        let out = render(&recs, None);
        assert!(!out.contains("repeat_failure"), "got:\n{out}");
    }
}
