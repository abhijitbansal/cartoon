//! `cartoon doctor` — one static health report for the integrations that
//! quietly stop saving tokens: hook not installed, config that does not
//! parse, project scripts declared but missing, allowlisted tools with no
//! adapter (ladder-only), and ledger damage. Output is TOON; paste it into
//! a bug report.
use anyhow::Result;
use serde_json::{json, Value};

pub fn run() -> Result<i32> {
    println!("{}", report());
    Ok(0)
}

/// Hook allowlist entries that have no adapter: they are wrapped, but only
/// the compression ladder touches their output.
pub fn ladder_only_allowlist() -> Vec<String> {
    let probe = |argv: &[&str]| {
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        crate::adapters::find_adapter(&argv).is_none()
    };
    let mut out: Vec<String> = crate::hook::ALWAYS
        .iter()
        .filter(|t| probe(&[t]))
        .map(|t| t.to_string())
        .collect();
    for (tool, subs) in crate::hook::SUBCOMMAND {
        for s in subs.iter() {
            if probe(&[tool, s]) {
                out.push(format!("{tool} {s}"));
            }
        }
    }
    out
}

fn config_row(path: Option<std::path::PathBuf>) -> Value {
    match path {
        None => json!({ "path": "(none)", "status": "absent" }),
        Some(p) => {
            let status = match std::fs::read_to_string(&p) {
                Err(_) => "absent".to_string(),
                Ok(s) => match crate::config::check(&s) {
                    Ok(()) => "ok".to_string(),
                    Err(e) => format!("invalid: {e}"),
                },
            };
            json!({ "path": p.display().to_string(), "status": status })
        }
    }
}

pub fn report() -> String {
    let cwd = std::env::current_dir().ok();
    let hook_rows: Vec<Value> = crate::hook::status_rows()
        .into_iter()
        .map(|(path, surface, installed)| {
            json!({ "path": path, "surface": surface, "installed": installed })
        })
        .collect();

    let project_path = cwd.as_deref().and_then(crate::paths::project_config_file);
    let merged = cwd
        .as_deref()
        .map(crate::config::load_merged)
        .unwrap_or_else(crate::config::load);
    let missing_scripts: Vec<String> = merged
        .wrap_scripts
        .iter()
        .filter(|s| {
            let rel = s.trim_start_matches("./");
            !cwd.as_ref().is_some_and(|c| c.join(rel).exists())
        })
        .cloned()
        .collect();

    let (recs, malformed) = crate::stats::read_ledger();
    let negative = recs.iter().filter(|r| r.saved < 0).count();
    let mut heads: Vec<(String, usize, usize)> = Vec::new();
    for r in recs.iter().filter(|r| r.saved <= 0 && r.tokens_in >= 500) {
        let key = match (&r.inner_cmd, r.cmd.as_str()) {
            (Some(inner), "sh" | "bash" | "zsh" | "cmd") => format!("sh -c {inner}"),
            _ => r.cmd.clone(),
        };
        match heads.iter_mut().find(|(k, _, _)| *k == key) {
            Some(e) => {
                e.1 += 1;
                e.2 += r.tokens_in;
            }
            None => heads.push((key, 1, r.tokens_in)),
        }
    }
    heads.sort_by_key(|(_, _, t)| std::cmp::Reverse(*t));
    let top: Vec<Value> = heads
        .into_iter()
        .take(5)
        .map(
            |(cmd, calls, tokens_in)| json!({ "cmd": cmd, "calls": calls, "tokens_in": tokens_in }),
        )
        .collect();

    let mut root = serde_json::Map::new();
    root.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
    root.insert("hook".into(), Value::Array(hook_rows));
    root.insert(
        "config".into(),
        json!({
            "global": config_row(crate::paths::config_file()),
            "project": config_row(project_path),
            "wrap_scripts_missing_on_disk": missing_scripts,
        }),
    );
    root.insert(
        "allowlist_without_adapter".into(),
        json!(ladder_only_allowlist()),
    );
    root.insert(
        "ledger".into(),
        json!({
            "records": recs.len(),
            "malformed_lines": malformed,
            "negative_saved": negative,
            "top_uncompressed": top,
        }),
    );
    let mut out = crate::toon::encode(&Value::Object(root));
    out.push_str(
        "\n\n# hook not installed → cartoon hook install · invalid config → fix the file · \
         allowlist_without_adapter → ladder compression only (contribute an adapter) · \
         negative_saved > 0 → upgrade (the guard now covers adapter runs)",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_only_lists_allowlisted_tools_without_an_adapter() {
        let only = ladder_only_allowlist();
        assert!(only.iter().any(|t| t == "make"), "{only:?}");
        assert!(!only.iter().any(|t| t == "pytest"), "{only:?}");
        assert!(!only.iter().any(|t| t == "ruff check"), "{only:?}");
    }

    #[test]
    fn report_has_every_section() {
        let r = report();
        for k in [
            "version:",
            "hook",
            "config:",
            "allowlist_without_adapter",
            "ledger:",
        ] {
            assert!(r.contains(k), "missing {k} in:\n{r}");
        }
    }
}
