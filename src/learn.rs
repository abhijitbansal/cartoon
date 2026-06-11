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
/// Modes that mean "output reached the agent (mostly) uncompressed".
const UNCOMPRESSED_MODES: &[&str] = &["passthrough", "safe", "heuristic"];
/// Same command failing this many times in a row is a loop worth breaking.
const REPEAT_FAIL_RUN: usize = 3;

pub fn run(since: Option<&str>) -> Result<i32> {
    let recs = crate::stats::read_records(since)?;
    println!("{}", render(&recs, since));
    Ok(0)
}

struct CmdAgg {
    cmd: String,
    calls: usize,
    tokens_in: usize,
}

pub fn render(recs: &[StatRecord], since: Option<&str>) -> String {
    if recs.is_empty() {
        return "learn: no wrapped runs recorded yet — wrap some commands first".into();
    }
    let mut suggestions: Vec<Value> = Vec::new();
    let mut config_lines: Vec<String> = Vec::new();

    // 1. Token wasters: frequent commands stuck in uncompressed modes.
    let mut aggs: Vec<CmdAgg> = Vec::new();
    for r in recs
        .iter()
        .filter(|r| UNCOMPRESSED_MODES.contains(&r.adapter.as_str()))
    {
        match aggs.iter_mut().find(|a| a.cmd == r.cmd) {
            Some(a) => {
                a.calls += 1;
                a.tokens_in += r.tokens_in;
            }
            None => aggs.push(CmdAgg {
                cmd: r.cmd.clone(),
                calls: 1,
                tokens_in: r.tokens_in,
            }),
        }
    }
    aggs.retain(|a| a.calls >= MIN_CALLS && a.tokens_in / a.calls >= MIN_AVG_TOKENS_IN);
    aggs.sort_by_key(|a| std::cmp::Reverse(a.tokens_in));
    for a in &aggs {
        let avg = a.tokens_in / a.calls;
        suggestions.push(json!({
            "kind": "token_waster",
            "cmd": a.cmd,
            "calls": a.calls,
            "avg_tokens_in": avg,
            "action": format!("pin [command.{}] level=\"aggressive\" (output mostly uncompressed today)", a.cmd),
        }));
        config_lines.push(format!("[command.{}]\nlevel = \"aggressive\"", a.cmd));
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
        out.push_str("\n\n# paste into cartoon.toml:\n");
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
        }
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
