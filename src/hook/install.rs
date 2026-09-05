//! `cartoon hook install|uninstall|status` — where each agent surface keeps
//! its hook entry, and how we write, refresh and remove ours without
//! touching anything else in the file. The rewrite policy lives in the
//! parent module; this file only manages configuration.
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Which agent's config to install into, and in what mode.
#[derive(Clone, Copy)]
pub(super) struct Target {
    /// Copilot CLI (`~/.copilot/hooks` or `.github/hooks`).
    copilot: bool,
    /// VS Code Copilot Chat. Shares Claude Code's `~/.claude/settings.json`
    /// (the location VS Code documents for hooks); the flag only tailors the
    /// confirmation message, since one entry already covers both.
    vscode: bool,
    /// Project scope vs user/home scope.
    project: bool,
    /// Bake `--deny-mode` into the installed command (block raw commands and
    /// suggest the wrapped form instead of rewriting transparently).
    deny: bool,
    /// Also write the matching `cartoon instructions` directive (the
    /// piped-command case the hook can't catch). Opt-in on install.
    instructions: bool,
}

pub(super) fn target(args: &[String]) -> Result<Target> {
    let mut t = Target {
        copilot: false,
        vscode: false,
        project: false,
        deny: false,
        instructions: false,
    };
    for a in &args[1..] {
        match a.as_str() {
            "--copilot" => t.copilot = true,
            "--vscode" => t.vscode = true,
            "--project" => t.project = true,
            "--deny" => t.deny = true,
            "--instructions" => t.instructions = true,
            other => bail!(
                "unknown flag {other} (expected --copilot, --vscode, --project, --deny, or --instructions)"
            ),
        }
    }
    if t.copilot && t.vscode {
        bail!("--copilot and --vscode are different config files; pick one");
    }
    Ok(t)
}

// ---------- install / uninstall / status ----------

/// Substring every installed hook entry's command contains — the single
/// source of truth for both building the command and recognizing our own
/// entries on uninstall/status (so a rename can't desync them).
const HOOK_MARKER: &str = "cartoon hook rewrite";

/// The shell command an installed hook entry runs. Fail-open if cartoon is
/// not on PATH. `--deny-mode` switches transparent rewrite to deny.
fn hook_command(deny: bool) -> String {
    let mode = if deny { " --deny-mode" } else { "" };
    format!("command -v cartoon >/dev/null 2>&1 && {HOOK_MARKER}{mode} || exit 0")
}

pub(super) fn install(t: Target) -> Result<i32> {
    let code = if t.copilot {
        install_copilot(t)?
    } else {
        install_claude(t)?
    };
    // The hook can't rewrite piped commands and VS Code Chat can only deny;
    // the matching instruction closes that gap. Offer it (or write it with
    // --instructions) after the hook itself is in place.
    offer_instructions(t)?;
    Ok(code)
}

pub(super) fn uninstall(t: Target) -> Result<i32> {
    let code = if t.copilot {
        uninstall_copilot(t)?
    } else {
        uninstall_claude(t)?
    };
    // Symmetry: `--instructions` on uninstall also removes the directive we
    // would have written, so the pair leaves nothing behind.
    if t.instructions {
        let path = crate::instructions::doc_path(instructions_doc(t));
        if crate::instructions::uninstall_doc(&path)? {
            println!("also removed the cartoon directive from {}", path.display());
        }
    }
    Ok(code)
}

/// Which instruction file matches a hook target: the Copilot surfaces read
/// `.github/copilot-instructions.md`; everything else auto-detects between
/// `CLAUDE.md` (preferred when present) and `AGENTS.md` (the cross-agent
/// default that Claude Code also reads), via `instructions::default_agent_doc`.
fn instructions_doc(t: Target) -> crate::instructions::Doc {
    if t.copilot || t.vscode {
        crate::instructions::Doc::Copilot
    } else {
        crate::instructions::default_agent_doc()
    }
}

/// The `hook install` surface flag that reproduces this target, for the "fold
/// it in next time" hint — so a Copilot/VS Code user isn't told to run the
/// bare command (which installs the Claude hook). `--project` is orthogonal
/// (it only moves the hook between user/project settings, not the instruction
/// file the hint is about) and is deliberately omitted.
fn surface_flag(t: Target) -> &'static str {
    if t.copilot {
        " --copilot"
    } else if t.vscode {
        " --vscode"
    } else {
        ""
    }
}

/// After a hook install, surface the piped-command gap and the matching
/// directive. With `--instructions`, write it outright; otherwise print the
/// hint and — only on an interactive terminal — offer to write it now.
fn offer_instructions(t: Target) -> Result<()> {
    let doc = instructions_doc(t);
    let path = crate::instructions::doc_path(doc);
    let present = crate::instructions::is_present(&path);

    if t.instructions {
        // Always (re)write: install_doc is idempotent and refreshes a stale
        // body in place, matching standalone `cartoon instructions install`.
        let outcome = crate::instructions::install_doc(&path)?;
        println!("\n{}", crate::instructions::describe(&path, outcome));
        return Ok(());
    }

    // Suggest surface-correct commands: the instruction-file flag so the
    // directive lands in the file this hint names (bare resolves to AGENTS.md,
    // never the Copilot file), and the hook surface flag so "fold it in next
    // time" reinstalls this same hook.
    let instr_flag = crate::instructions::doc_flag(doc);
    let hook_flag = surface_flag(t);
    println!(
        "\nHeads-up: the hook can't rewrite *piped* commands — `pytest | tail`\n\
         slips past it (and VS Code Copilot Chat can only deny, not rewrite). A\n\
         matching instruction closes that gap by telling the agent to wrap and\n\
         never pipe noisy commands:\n    \
         cartoon instructions install{instr_flag}        # writes the directive to {p}\n    \
         (or fold it into install next time: cartoon hook install{hook_flag} --instructions)",
        p = path.display()
    );
    if present {
        println!("It's already present in {}.", path.display());
        return Ok(());
    }
    if prompt_yes(&format!("Write that directive to {} now?", path.display())) {
        let outcome = crate::instructions::install_doc(&path)?;
        println!("{}", crate::instructions::describe(&path, outcome));
    }
    Ok(())
}

/// Ask a yes/no question, but only when attached to an interactive terminal.
/// Non-interactive callers (agents, scripts, CI) never block and get `false`,
/// so `cartoon hook install` stays safe to run unattended.
fn prompt_yes(question: &str) -> bool {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return false;
    }
    print!("{question} [y/N] ");
    if std::io::stdout().flush().is_err() {
        return false;
    }
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

// --- Claude Code / VS Code Copilot Chat (settings.json) ---

fn claude_settings_path(project: bool) -> Result<PathBuf> {
    if project {
        Ok(PathBuf::from(".claude/settings.json"))
    } else {
        dirs::home_dir()
            .map(|h| h.join(".claude/settings.json"))
            .ok_or_else(|| anyhow::anyhow!("cannot resolve home directory"))
    }
}

/// One entry serves both Claude Code (`Bash`) and VS Code Copilot Chat
/// (`run_in_terminal`), which read the same settings file; the surface is
/// auto-detected at rewrite time.
fn claude_entry(deny: bool) -> Value {
    json!({
        "matcher": "Bash|run_in_terminal",
        "hooks": [{ "type": "command", "command": hook_command(deny) }]
    })
}

fn is_our_entry(entry: &Value) -> bool {
    entry["hooks"]
        .as_array()
        .map(|hs| {
            hs.iter().any(|h| {
                h["command"]
                    .as_str()
                    .is_some_and(|c| c.contains(HOOK_MARKER))
            })
        })
        .unwrap_or(false)
}

/// Outcome of inserting our entry into a settings `PreToolUse` array.
#[derive(Debug, PartialEq, Eq)]
enum Upsert {
    Added,
    Unchanged,
    /// Our entry was present with a different command (other mode, or an
    /// older shape) and has been replaced in place.
    ModeUpdated,
}

fn upsert_claude_entry(arr: &mut Vec<Value>, deny: bool) -> Upsert {
    let want = claude_entry(deny);
    match arr.iter_mut().find(|e| is_our_entry(e)) {
        None => {
            arr.push(want);
            Upsert::Added
        }
        Some(existing) if *existing == want => Upsert::Unchanged,
        Some(existing) => {
            *existing = want;
            Upsert::ModeUpdated
        }
    }
}

fn mode_label(deny: bool) -> &'static str {
    if deny {
        "deny-with-suggestion mode"
    } else {
        "transparent rewrite mode"
    }
}

fn install_claude(t: Target) -> Result<i32> {
    let path = claude_settings_path(t.project)?;
    let mut root: Value = match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| {
            anyhow::anyhow!("{} is not valid JSON ({e}); fix it first", path.display())
        })?,
        Err(_) => json!({}),
    };
    let pre = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("settings root is not an object"))?
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("settings `hooks` is not an object"))?
        .entry("PreToolUse")
        .or_insert_with(|| json!([]));
    let arr = pre
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("settings `hooks.PreToolUse` is not an array"))?;
    let outcome = upsert_claude_entry(arr, t.deny);
    if outcome == Upsert::Unchanged {
        println!("cartoon hook already installed in {}", path.display());
        return Ok(0);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&root)?)?;
    if outcome == Upsert::ModeUpdated {
        println!(
            "cartoon hook entry updated in {} — now {}. Restart the agent (or run /hooks) to load it.",
            path.display(),
            mode_label(t.deny)
        );
        return Ok(0);
    }
    if t.vscode {
        println!(
            "cartoon hook installed for VS Code Copilot Chat in {}\n\
             VS Code reads this Claude-format file (hooks are Preview); the\n\
             same entry also covers Claude Code. run_in_terminal calls are\n\
             blocked and re-suggested wrapped (Chat has no rewrite field).\n\
             Reload the window to load it. Remove with:\n\
             cartoon hook uninstall --vscode{}",
            path.display(),
            if t.project { " --project" } else { "" }
        );
    } else {
        println!(
            "cartoon hook installed in {}\n\
             Covers Claude Code and VS Code Copilot Chat (both read this file).\n\
             Noisy dev commands (test/lint/build) now auto-wrap; matching calls\n\
             are auto-approved, so the allowlist stays conservative by design.\n\
             Restart the agent (or run /hooks) to load it.\n\
             Disable a session: export CARTOON_NO_WRAP=1 · remove: cartoon hook uninstall",
            path.display()
        );
    }
    Ok(0)
}

fn uninstall_claude(t: Target) -> Result<i32> {
    let path = claude_settings_path(t.project)?;
    let Ok(s) = std::fs::read_to_string(&path) else {
        println!("nothing to remove: {} not found", path.display());
        return Ok(0);
    };
    let mut root: Value = serde_json::from_str(&s)?;
    let mut removed = false;
    if let Some(arr) = root
        .get_mut("hooks")
        .and_then(|h| h.get_mut("PreToolUse"))
        .and_then(Value::as_array_mut)
    {
        let before = arr.len();
        arr.retain(|e| !is_our_entry(e));
        removed = arr.len() != before;
    }
    if removed {
        std::fs::write(&path, serde_json::to_string_pretty(&root)?)?;
        println!("cartoon hook removed from {}", path.display());
    } else {
        println!("cartoon hook not present in {}", path.display());
    }
    Ok(0)
}

// --- Copilot CLI (.copilot/hooks or .github/hooks) ---

/// Copilot CLI reads `~/.copilot/hooks/*.json` (personal) and
/// `.github/hooks/*.json` (repo-shared; also picked up by the Copilot coding
/// agent). We own a dedicated `cartoon.json` so install can write it whole.
fn copilot_path(project: bool) -> Result<PathBuf> {
    if project {
        Ok(PathBuf::from(".github/hooks/cartoon.json"))
    } else {
        dirs::home_dir()
            .map(|h| h.join(".copilot/hooks/cartoon.json"))
            .ok_or_else(|| anyhow::anyhow!("cannot resolve home directory"))
    }
}

fn copilot_config(deny: bool) -> Value {
    json!({
        "version": 1,
        "hooks": {
            // Copilot uses camelCase event names and a regex `matcher` on the
            // tool name (honored v1.0.36+; harmless before).
            "preToolUse": [{
                "type": "command",
                "command": hook_command(deny),
                "matcher": "bash|shell",
                "timeout": 10
            }]
        }
    })
}

fn install_copilot(t: Target) -> Result<i32> {
    let path = copilot_path(t.project)?;
    // We own the dedicated `cartoon.json` name; refuse to clobber a file at
    // that path we didn't write (uninstall is already this careful).
    if path.exists() && !is_our_copilot_file(&path) {
        bail!(
            "{} exists but is not a cartoon hook; move it aside first",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&copilot_config(t.deny))?,
    )?;
    let mode = if t.deny {
        "deny-with-suggestion mode (raw command blocked; agent re-runs wrapped)"
    } else {
        "transparent rewrite mode (requires Copilot CLI v1.0.24+)"
    };
    println!(
        "cartoon Copilot hook installed in {}\n\
         Using {mode}.\n\
         If Copilot prompts for confirmation on every wrapped command\n\
         (a known v1.0.24 bug), reinstall with: cartoon hook install --copilot --deny\n\
         Disable a session: export CARTOON_NO_WRAP=1\n\
         Remove with: cartoon hook uninstall --copilot{}",
        path.display(),
        if t.project { " --project" } else { "" }
    );
    Ok(0)
}

fn uninstall_copilot(t: Target) -> Result<i32> {
    let path = copilot_path(t.project)?;
    if is_our_copilot_file(&path) {
        std::fs::remove_file(&path)?;
        println!("cartoon Copilot hook removed from {}", path.display());
    } else if path.exists() {
        println!(
            "{} exists but is not a cartoon hook; left untouched",
            path.display()
        );
    } else {
        println!("nothing to remove: {} not found", path.display());
    }
    Ok(0)
}

fn is_our_copilot_file(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|s| s.contains(HOOK_MARKER))
}

/// Is our hook entry present in a Claude-format settings.json?
fn claude_entry_present(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["hooks"]["PreToolUse"].as_array().cloned())
        .is_some_and(|arr| arr.iter().any(is_our_entry))
}

// --- status across every surface ---

/// (path, surface, installed) for every hook location, user and project
/// scope. Shared by `hook status` and `cartoon doctor`.
pub fn status_rows() -> Vec<(String, &'static str, bool)> {
    let mut rows = Vec::new();
    for project in [false, true] {
        if let Ok(path) = claude_settings_path(project) {
            rows.push((
                path.display().to_string(),
                "Claude Code / VS Code Copilot Chat",
                claude_entry_present(&path),
            ));
        }
    }
    for project in [false, true] {
        if let Ok(path) = copilot_path(project) {
            rows.push((
                path.display().to_string(),
                "Copilot CLI",
                is_our_copilot_file(&path),
            ));
        }
    }
    rows
}

pub(super) fn status() -> Result<i32> {
    for (path, surface, installed) in status_rows() {
        println!("{path} ({surface}): {}", yes_no(installed));
    }
    Ok(0)
}

fn yes_no(installed: bool) -> &'static str {
    if installed {
        "installed"
    } else {
        "not installed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_switches_mode_when_entry_exists_with_other_mode() {
        let mut arr = vec![claude_entry(false)];
        assert_eq!(upsert_claude_entry(&mut arr, true), Upsert::ModeUpdated);
        assert!(arr[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("--deny-mode"));
        assert_eq!(upsert_claude_entry(&mut arr, true), Upsert::Unchanged);
        assert_eq!(arr.len(), 1);
        let mut empty = Vec::new();
        assert_eq!(upsert_claude_entry(&mut empty, false), Upsert::Added);
    }

    #[test]
    fn our_entry_detection() {
        assert!(is_our_entry(&claude_entry(false)));
        assert!(is_our_entry(&claude_entry(true)));
        assert!(!is_our_entry(
            &json!({"matcher":"Bash","hooks":[{"type":"command","command":"other"}]})
        ));
    }

    #[test]
    fn claude_entry_covers_both_shell_tools() {
        assert_eq!(claude_entry(false)["matcher"], "Bash|run_in_terminal");
    }

    #[test]
    fn deny_flag_bakes_into_command() {
        let c = claude_entry(true)["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(c.contains("cartoon hook rewrite --deny-mode"));
    }

    #[test]
    fn target_parses_instructions_flag() {
        let t = target(&["install".into(), "--instructions".into()]).unwrap();
        assert!(t.instructions);
        // default off, and it composes with surface flags
        assert!(!target(&["install".into()]).unwrap().instructions);
        let c = target(&[
            "install".into(),
            "--copilot".into(),
            "--instructions".into(),
        ])
        .unwrap();
        assert!(c.copilot && c.instructions);
    }

    #[test]
    fn instructions_doc_maps_copilot_surfaces_to_copilot_file() {
        // Copilot + VS Code Copilot Chat → the GitHub file, deterministically.
        let cop = target(&["install".into(), "--copilot".into()]).unwrap();
        assert_eq!(instructions_doc(cop), crate::instructions::Doc::Copilot);
        let vsc = target(&["install".into(), "--vscode".into()]).unwrap();
        assert_eq!(instructions_doc(vsc), crate::instructions::Doc::Copilot);
        // The non-Copilot default auto-detects CLAUDE.md vs AGENTS.md from the
        // filesystem; that resolution is unit-tested in
        // `instructions::resolve_agent_doc` and exercised end-to-end in
        // tests/e2e_instructions.rs.
    }

    #[test]
    fn target_parses_vscode_and_rejects_combo() {
        let v = target(&["install".into(), "--vscode".into()]).unwrap();
        assert!(v.vscode && !v.copilot);
        // --vscode and --copilot point at different files: reject the combo.
        assert!(target(&["install".into(), "--vscode".into(), "--copilot".into()]).is_err());
        // unknown flag still errors.
        assert!(target(&["install".into(), "--nope".into()]).is_err());
    }

    #[test]
    fn copilot_config_uses_camelcase_event_and_version() {
        let cfg = copilot_config(false);
        assert_eq!(cfg["version"], 1);
        let cmd = cfg["hooks"]["preToolUse"][0]["command"].as_str().unwrap();
        assert!(cmd.contains("cartoon hook rewrite"));
        assert_eq!(cfg["hooks"]["preToolUse"][0]["matcher"], "bash|shell");
    }
}
