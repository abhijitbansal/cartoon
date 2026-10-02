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
/// the compression ladder touches their output. Each entry is probed as
/// typed and with a trailing `run` (`vitest run`, `cargo nextest run`): some
/// adapters only match the representative argv, not the bare allowlist word.
pub fn ladder_only_allowlist() -> Vec<String> {
    let probe = |argv: &[&str]| {
        let mut argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        if crate::adapters::find_adapter(&argv).is_some() {
            return false;
        }
        argv.push("run".into());
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

/// The Claude Code plugin's marketplace id. Enabling it installs the same
/// PreToolUse hook `cartoon hook install` would write.
const PLUGIN_ID: &str = "cartoon@cartoon";

/// Settings files where Claude Code records enabled plugins: user, project,
/// and project-local.
fn plugin_settings_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".claude/settings.json"));
    }
    paths.push(".claude/settings.json".into());
    paths.push(".claude/settings.local.json".into());
    paths
}

/// Does this settings JSON enable the cartoon plugin?
pub fn plugin_enabled_in(settings: &str) -> bool {
    serde_json::from_str::<Value>(settings).is_ok_and(|v| {
        v.get("enabledPlugins")
            .and_then(|p| p.get(PLUGIN_ID))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    })
}

/// Settings files that enable the plugin hook.
fn plugin_rows() -> Vec<String> {
    plugin_settings_paths()
        .into_iter()
        .filter(|p| std::fs::read_to_string(p).is_ok_and(|s| plugin_enabled_in(&s)))
        .map(|p| p.display().to_string())
        .collect()
}

/// The trailing advice line, which depends on how the hook is installed:
/// a plugin user must not be told to `hook install` (that adds a duplicate).
fn advice(plugin: bool, hook_installed: bool) -> String {
    let hook = if plugin {
        "hook installed via the Claude Code plugin (cartoon@cartoon) — do not also run \
         cartoon hook install, that would add a duplicate hook"
    } else if hook_installed {
        "hook installed"
    } else {
        "hook not installed → cartoon hook install (or enable the cartoon Claude Code plugin)"
    };
    format!(
        "# {hook} · invalid config → fix the file · \
         allowlist_without_adapter → ladder compression only (contribute an adapter) · \
         negative_saved > 0 → upgrade (the guard now covers adapter runs)"
    )
}

pub fn report() -> String {
    let cwd = std::env::current_dir().ok();
    let plugin = plugin_rows();
    let status = crate::hook::status_rows();
    let hook_installed = status.iter().any(|(_, _, installed)| *installed);
    let hook_rows: Vec<Value> = status
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
        "plugin_hook".into(),
        json!({ "installed": !plugin.is_empty(), "enabled_in": plugin }),
    );
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
    out.push_str("\n\n");
    out.push_str(&advice(!plugin.is_empty(), hook_installed));
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
        // Adapters that need the representative `run` subcommand.
        assert!(!only.iter().any(|t| t == "vitest"), "{only:?}");
        assert!(!only.iter().any(|t| t == "cargo nextest"), "{only:?}");
    }

    #[test]
    fn plugin_detection_reads_enabled_plugins() {
        assert!(plugin_enabled_in(
            r#"{"enabledPlugins": {"cartoon@cartoon": true, "x@y": false}}"#
        ));
        assert!(!plugin_enabled_in(
            r#"{"enabledPlugins": {"cartoon@cartoon": false}}"#
        ));
        assert!(!plugin_enabled_in(
            r#"{"enabledPlugins": {"other@cartoon": true}}"#
        ));
        assert!(!plugin_enabled_in(r#"{"hooks": {}}"#));
        assert!(!plugin_enabled_in("not json"));
    }

    #[test]
    fn plugin_users_are_not_told_to_hook_install() {
        let a = advice(true, false);
        assert!(a.contains("plugin"), "{a}");
        assert!(!a.contains("→ cartoon hook install"), "{a}");
        assert!(advice(false, false).contains("→ cartoon hook install"));
        assert!(!advice(false, true).contains("not installed"));
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
