//! `cartoon hook install|uninstall|status` — where each agent surface keeps
//! its hook entry, and how we write, refresh and remove ours without
//! touching anything else in the file. The rewrite policy lives in the
//! parent module; this file only manages configuration.
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// What `hook install|uninstall` was asked to do.
#[derive(Clone, Copy)]
pub(super) struct Target {
    /// `uninstall --copilot` only: legacy cleanup of the Copilot CLI hook
    /// file written before 0.8.0 (the Copilot surface itself is gone).
    copilot: bool,
    /// Project scope vs user/home scope.
    project: bool,
    /// Also write the matching `cartoon instructions` directive (the
    /// piped-command case the hook can't catch). Opt-in on install.
    instructions: bool,
}

/// Flags removed in 0.8.0, named so the error can say why they fail.
const REMOVED_FLAGS: &[&str] = &["--vscode", "--deny"];

fn removed_flag_error(flag: &str, accepted: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{flag} was removed in 0.8.0 (the Copilot CLI / VS Code surfaces and deny mode are gone); accepted flags: {accepted}"
    )
}

/// Parse `install` flags: `--project`, `--instructions`.
pub(super) fn install_target(args: &[String]) -> Result<Target> {
    const ACCEPTED: &str = "--project, --instructions";
    let mut t = Target {
        copilot: false,
        project: false,
        instructions: false,
    };
    for a in &args[1..] {
        match a.as_str() {
            "--project" => t.project = true,
            "--instructions" => t.instructions = true,
            f if f == "--copilot" || REMOVED_FLAGS.contains(&f) => {
                return Err(removed_flag_error(f, ACCEPTED))
            }
            other => bail!("unknown flag {other} (accepted flags: {ACCEPTED})"),
        }
    }
    Ok(t)
}

/// Parse `uninstall` flags: `--project`, `--instructions`, and `--copilot`
/// (legacy cleanup only).
pub(super) fn uninstall_target(args: &[String]) -> Result<Target> {
    const ACCEPTED: &str = "--project, --copilot (legacy cleanup), --instructions";
    let mut t = Target {
        copilot: false,
        project: false,
        instructions: false,
    };
    for a in &args[1..] {
        match a.as_str() {
            "--project" => t.project = true,
            "--copilot" => t.copilot = true,
            "--instructions" => t.instructions = true,
            f if REMOVED_FLAGS.contains(&f) => return Err(removed_flag_error(f, ACCEPTED)),
            other => bail!("unknown flag {other} (accepted flags: {ACCEPTED})"),
        }
    }
    Ok(t)
}

// ---------- install / uninstall / status ----------

/// Substring every installed hook entry's command contains — the single
/// source of truth for both building the command and recognizing our own
/// entries on uninstall/status (so a rename can't desync them).
const HOOK_MARKER: &str = "cartoon hook rewrite";

/// The shell command an installed hook entry runs. Fail-open if cartoon is
/// not on PATH.
fn hook_command() -> String {
    format!("command -v cartoon >/dev/null 2>&1 && {HOOK_MARKER} || exit 0")
}

pub(super) fn install(t: Target) -> Result<i32> {
    let code = install_claude(t)?;
    // The hook can't rewrite piped commands; the matching instruction closes
    // that gap. Offer it (or write it with --instructions) after the hook
    // itself is in place.
    offer_instructions(t)?;
    Ok(code)
}

/// Install (or refresh) the Claude Code hook entry and nothing else: no
/// instruction-file offer, no prompt, safe to call unattended. Used by
/// `cartoon setup`.
pub fn install_claude_hook(project: bool) -> Result<i32> {
    install_claude(Target {
        copilot: false,
        project,
        instructions: false,
    })
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
        let path = crate::instructions::doc_path(uninstall_doc(t));
        if crate::instructions::uninstall_doc(&path)? {
            println!("also removed the cartoon directive from {}", path.display());
        }
    }
    Ok(code)
}

/// Which instruction file `uninstall --instructions` cleans: the legacy
/// Copilot cleanup (`--copilot`) targets the GitHub file an old install may
/// have written; otherwise `CLAUDE.md` (preferred when present) or
/// `AGENTS.md`, via `instructions::default_agent_doc`.
fn uninstall_doc(t: Target) -> crate::instructions::Doc {
    if t.copilot {
        crate::instructions::Doc::Copilot
    } else {
        crate::instructions::default_agent_doc()
    }
}

/// After a hook install, surface the piped-command gap and the matching
/// directive. With `--instructions`, write it outright; otherwise print the
/// hint and — only on an interactive terminal — offer to write it now.
fn offer_instructions(t: Target) -> Result<()> {
    let doc = crate::instructions::default_agent_doc();
    let path = crate::instructions::doc_path(doc);
    let present = crate::instructions::is_present(&path);

    if t.instructions {
        // Always (re)write: install_doc is idempotent and refreshes a stale
        // body in place, matching standalone `cartoon instructions install`.
        let outcome = crate::instructions::install_doc(&path)?;
        println!("\n{}", crate::instructions::describe(&path, outcome));
        return Ok(());
    }

    // Suggest the instruction-file flag so the directive lands in the file
    // this hint names.
    let instr_flag = crate::instructions::doc_flag(doc);
    println!(
        "\nHeads-up: the hook can't rewrite *piped* commands — `pytest | tail`\n\
         slips past it. A matching instruction closes that gap by telling the\n\
         agent to wrap and never pipe noisy commands:\n    \
         cartoon instructions install{instr_flag}        # writes the directive to {p}\n    \
         (or fold it into install next time: cartoon hook install --instructions)",
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

// --- Claude Code (settings.json) ---

fn claude_settings_path(project: bool) -> Result<PathBuf> {
    if project {
        Ok(PathBuf::from(".claude/settings.json"))
    } else {
        user_settings_path().ok_or_else(|| anyhow::anyhow!("cannot resolve home directory"))
    }
}

/// The user-scope Claude Code settings file: `$CLAUDE_CONFIG_DIR/settings.json`
/// when that is set, else `~/.claude/settings.json`, the same file the
/// hook's rule reader loads. Shared by install, uninstall, status, doctor.
pub fn user_settings_path() -> Option<PathBuf> {
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    user_settings_path_from(dirs::home_dir().as_deref(), config_dir.as_deref())
}

fn user_settings_path_from(home: Option<&Path>, config_dir: Option<&Path>) -> Option<PathBuf> {
    config_dir
        .map(Path::to_path_buf)
        .or_else(|| home.map(|h| h.join(".claude")))
        .map(|d| d.join("settings.json"))
}

/// The `PreToolUse` entry: Claude Code's `Bash` tool only.
fn claude_entry() -> Value {
    json!({
        "matcher": "Bash",
        "hooks": [{ "type": "command", "command": hook_command() }]
    })
}

/// A hook object whose command carries `HOOK_MARKER`.
fn is_our_hook(hook: &Value) -> bool {
    hook["command"]
        .as_str()
        .is_some_and(|c| c.contains(HOOK_MARKER))
}

fn is_our_entry(entry: &Value) -> bool {
    entry["hooks"]
        .as_array()
        .is_some_and(|hs| hs.iter().any(is_our_hook))
}

/// How many of our hook objects the whole `PreToolUse` array holds.
fn our_hook_count(arr: &[Value]) -> usize {
    arr.iter()
        .filter_map(|e| e["hooks"].as_array())
        .flatten()
        .filter(|h| is_our_hook(h))
        .count()
}

/// Is our entry exactly what `claude_entry()` writes today (matcher and
/// command)? Other hooks in the same entry don't matter.
fn entry_is_current(entry: &Value) -> bool {
    let want = claude_entry();
    entry["matcher"] == want["matcher"]
        && entry["hooks"].as_array().is_some_and(|hs| {
            hs.iter()
                .any(|h| h["command"] == want["hooks"][0]["command"])
        })
}

/// Does this entry of ours still wrap under 0.8? A pre-0.8 allow-mode entry
/// (`Bash|run_in_terminal` matcher, plain command) does; a `--deny-mode`
/// command (refused by `hook rewrite`) or a matcher that misses `Bash`
/// does not.
fn entry_still_wraps(entry: &Value) -> bool {
    let matcher = entry["matcher"].as_str().unwrap_or("");
    let matches_bash =
        matches!(matcher, "" | "*" | ".*") || matcher.split('|').any(|m| m.trim() == "Bash");
    matches_bash
        && entry["hooks"].as_array().is_some_and(|hs| {
            hs.iter().any(|h| {
                is_our_hook(h)
                    && h["command"]
                        .as_str()
                        .is_some_and(|c| !c.contains("--deny-mode"))
            })
        })
}

/// Outcome of inserting our entry into a settings `PreToolUse` array.
#[derive(Debug, PartialEq, Eq)]
enum Upsert {
    Added,
    Unchanged,
    /// Our hook was present in an older shape (pre-0.8 matcher,
    /// `--deny-mode`) or more than once, and has been refreshed.
    Refreshed,
}

/// Insert or refresh our hook, touching nothing else. Only hook objects
/// whose command carries `HOOK_MARKER` are ours: other hooks in the same
/// entry keep their entry and matcher. Every entry of ours is refreshed or
/// dropped, so exactly one of our hooks remains. An entry holding only our
/// hook is rewritten in place (matcher `Bash`, current command, other keys
/// kept); otherwise our hook moves into an entry of its own.
fn upsert_claude_entry(arr: &mut Vec<Value>) -> Upsert {
    let ours: Vec<&Value> = arr.iter().filter(|e| is_our_entry(e)).collect();
    if ours.is_empty() {
        arr.push(claude_entry());
        return Upsert::Added;
    }
    if ours.len() == 1 && entry_is_current(ours[0]) && our_hook_count(arr) == 1 {
        return Upsert::Unchanged;
    }
    let mut placed = false;
    let mut out = Vec::with_capacity(arr.len() + 1);
    for entry in arr.drain(..) {
        if !is_our_entry(&entry) {
            out.push(entry);
            continue;
        }
        let hooks = entry["hooks"].as_array().cloned().unwrap_or_default();
        let (mine, others): (Vec<Value>, Vec<Value>) = hooks.into_iter().partition(is_our_hook);
        if !others.is_empty() {
            let mut kept = entry;
            kept["hooks"] = Value::Array(others);
            out.push(kept);
        } else if !placed {
            let mut hook = mine.into_iter().next().unwrap_or_else(|| json!({}));
            hook["type"] = json!("command");
            hook["command"] = json!(hook_command());
            let mut refreshed = entry;
            refreshed["matcher"] = json!("Bash");
            refreshed["hooks"] = json!([hook]);
            out.push(refreshed);
            placed = true;
        }
    }
    if !placed {
        out.push(claude_entry());
    }
    *arr = out;
    Upsert::Refreshed
}

/// Remove every hook object of ours, and an entry left with no hooks.
/// Other hooks keep their entry. True when anything was removed.
fn remove_claude_hooks(arr: &mut Vec<Value>) -> bool {
    let before = our_hook_count(arr);
    for entry in arr.iter_mut() {
        if let Some(hooks) = entry["hooks"].as_array_mut() {
            hooks.retain(|h| !is_our_hook(h));
        }
    }
    arr.retain(|e| e["hooks"].as_array().is_none_or(|hs| !hs.is_empty()));
    before > 0
}

fn install_claude(t: Target) -> Result<i32> {
    let path = claude_settings_path(t.project)?;
    let mut root: Value = match read_settings(&path)? {
        Some(s) => serde_json::from_str(&s).map_err(|e| {
            anyhow::anyhow!("{} is not valid JSON ({e}); fix it first", path.display())
        })?,
        None => json!({}),
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
    let outcome = upsert_claude_entry(arr);
    if outcome == Upsert::Unchanged {
        println!("cartoon hook already installed in {}", path.display());
        return Ok(0);
    }
    write_atomic(&path, &serde_json::to_string_pretty(&root)?)?;
    if outcome == Upsert::Refreshed {
        println!(
            "cartoon hook entry refreshed in {} (now the Claude Code `Bash` matcher, transparent rewrite). Restart the agent (or run /hooks) to load it.",
            path.display()
        );
        return Ok(0);
    }
    println!(
        "cartoon hook installed in {}\n\
         Noisy dev commands (test/lint/build) now auto-wrap; matching calls\n\
         are auto-approved, so the allowlist stays conservative by design.\n\
         Restart the agent (or run /hooks) to load it.\n\
         Disable a session: export CARTOON_NO_WRAP=1 · remove: cartoon hook uninstall",
        path.display()
    );
    Ok(0)
}

fn uninstall_claude(t: Target) -> Result<i32> {
    let path = claude_settings_path(t.project)?;
    let Some(s) = read_settings(&path)? else {
        println!("nothing to remove: {} not found", path.display());
        return Ok(0);
    };
    let mut root: Value = serde_json::from_str(&s)
        .map_err(|e| anyhow::anyhow!("{} is not valid JSON ({e}); fix it first", path.display()))?;
    let mut removed = false;
    if let Some(arr) = root
        .get_mut("hooks")
        .and_then(|h| h.get_mut("PreToolUse"))
        .and_then(Value::as_array_mut)
    {
        removed = remove_claude_hooks(arr);
    }
    if removed {
        write_atomic(&path, &serde_json::to_string_pretty(&root)?)?;
        println!("cartoon hook removed from {}", path.display());
    } else {
        println!("cartoon hook not present in {}", path.display());
    }
    Ok(0)
}

// --- Legacy Copilot CLI hook file (cleanup only) ---

/// Where the Copilot CLI hook lived before 0.8.0: `~/.copilot/hooks` or
/// `.github/hooks`. We own the dedicated `cartoon.json` name, and only ever
/// remove it (`hook uninstall --copilot`).
fn copilot_path(project: bool) -> Result<PathBuf> {
    if project {
        Ok(PathBuf::from(".github/hooks/cartoon.json"))
    } else {
        dirs::home_dir()
            .map(|h| h.join(".copilot/hooks/cartoon.json"))
            .ok_or_else(|| anyhow::anyhow!("cannot resolve home directory"))
    }
}

fn uninstall_copilot(t: Target) -> Result<i32> {
    let path = copilot_path(t.project)?;
    match std::fs::read(&path) {
        Ok(bytes) if String::from_utf8_lossy(&bytes).contains(HOOK_MARKER) => {
            std::fs::remove_file(&path)?;
            println!(
                "legacy cartoon Copilot hook removed from {}",
                path.display()
            );
        }
        Ok(_) => println!(
            "{} exists but is not a cartoon hook; left untouched",
            path.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("nothing to remove: {} not found", path.display());
        }
        Err(e) => bail!("cannot read {}: {e}", path.display()),
    }
    Ok(0)
}

/// Read a config file we may rewrite. `Ok(None)` only when it does not
/// exist; any other failure (permissions, a directory, invalid UTF-8) is an
/// error, so the caller never mistakes an unreadable file for an absent one
/// and overwrites it.
fn read_settings(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => bail!(
            "{} is not valid UTF-8; fix it first (left untouched)",
            path.display()
        ),
        Err(e) => bail!("cannot read {}: {e} (left untouched)", path.display()),
    }
}

/// Replace `path` atomically: write a temp file in the same directory, then
/// rename it over the target, so a crash or a full disk never leaves a
/// truncated settings file. An existing file keeps its permissions, and a
/// symlinked settings file (dotfile managers) is updated at its target
/// instead of being replaced by a regular file.
fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    crate::paths::write_atomic(path, contents.as_bytes())?;
    Ok(())
}

fn is_our_copilot_file(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|s| s.contains(HOOK_MARKER))
}

/// State of our hook entry in a Claude-format settings.json.
fn claude_entry_state(path: &Path) -> HookState {
    let entries = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["hooks"]["PreToolUse"].as_array().cloned())
        .unwrap_or_default();
    let ours: Vec<&Value> = entries.iter().filter(|e| is_our_entry(e)).collect();
    if ours.is_empty() {
        HookState::Missing
    } else if ours.len() == 1 && entry_is_current(ours[0]) && our_hook_count(&entries) == 1 {
        HookState::Installed
    } else if ours.iter().any(|e| entry_still_wraps(e)) {
        HookState::Outdated
    } else {
        HookState::Stale
    }
}

// --- status across every surface ---

/// What `hook status` and `cartoon doctor` know about one hook location.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookState {
    /// Our entry is present and current.
    Installed,
    /// Our hook is present in an older shape (the pre-0.8 matcher) or more
    /// than once, and still wraps; `hook install` refreshes it.
    Outdated,
    /// Our hook is present only in a pre-0.8 `--deny-mode` shape (or under a
    /// matcher that misses `Bash`), so it no longer wraps.
    Stale,
    Missing,
    /// A Copilot CLI hook file from before 0.8.0: dead weight, never a
    /// working hook.
    Legacy,
}

impl HookState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Outdated => {
                "installed, older entry (still wraps): re-run `cartoon hook install` to refresh it"
            }
            Self::Stale => {
                "stale (pre-0.8 `--deny-mode` entry, no longer wraps): re-run `cartoon hook install`"
            }
            Self::Missing => "not installed",
            Self::Legacy => {
                "legacy, unused since 0.8.0: remove with `cartoon hook uninstall --copilot`"
            }
        }
    }
}

pub struct HookRow {
    pub path: String,
    pub surface: &'static str,
    pub state: HookState,
}

/// One row per hook location, user and project scope. Shared by
/// `hook status` and `cartoon doctor`.
pub fn status_rows() -> Vec<HookRow> {
    let mut rows = Vec::new();
    for project in [false, true] {
        if let Ok(path) = claude_settings_path(project) {
            rows.push(HookRow {
                state: claude_entry_state(&path),
                path: path.display().to_string(),
                surface: "Claude Code",
            });
        }
    }
    // A Copilot CLI hook written before 0.8.0: listed only when present.
    for project in [false, true] {
        if let Ok(path) = copilot_path(project) {
            if is_our_copilot_file(&path) {
                rows.push(HookRow {
                    path: path.display().to_string(),
                    surface: "Copilot CLI",
                    state: HookState::Legacy,
                });
            }
        }
    }
    rows
}

pub(super) fn status() -> Result<i32> {
    for row in status_rows() {
        println!("{} ({}): {}", row.path, row.surface, row.state.label());
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_refreshes_an_older_entry_in_place() {
        let old = json!({
            "matcher": "Bash|run_in_terminal",
            "hooks": [{ "type": "command", "command": "command -v cartoon >/dev/null 2>&1 && cartoon hook rewrite --deny-mode || exit 0" }]
        });
        let mut arr = vec![old];
        assert_eq!(upsert_claude_entry(&mut arr), Upsert::Refreshed);
        assert_eq!(arr, vec![claude_entry()]);
        assert_eq!(upsert_claude_entry(&mut arr), Upsert::Unchanged);
        assert_eq!(arr.len(), 1);
        let mut empty = Vec::new();
        assert_eq!(upsert_claude_entry(&mut empty), Upsert::Added);
    }

    fn guard_hook() -> Value {
        json!({ "type": "command", "command": "~/bin/my-guard.sh" })
    }

    fn old_cartoon_hook(cmd: &str) -> Value {
        json!({ "type": "command", "command": format!("command -v cartoon >/dev/null 2>&1 && {cmd} || exit 0") })
    }

    #[test]
    fn refresh_keeps_other_hooks_that_share_our_entry() {
        // Final review I8: refreshing used to replace the whole entry and
        // delete the user's guard hook.
        let shared = json!({
            "matcher": "Bash|run_in_terminal",
            "hooks": [guard_hook(), old_cartoon_hook("cartoon hook rewrite")]
        });
        let mut arr = vec![shared];
        assert_eq!(upsert_claude_entry(&mut arr), Upsert::Refreshed);
        assert_eq!(
            arr,
            vec![
                json!({ "matcher": "Bash|run_in_terminal", "hooks": [guard_hook()] }),
                claude_entry(),
            ]
        );
        assert_eq!(upsert_claude_entry(&mut arr), Upsert::Unchanged);
    }

    #[test]
    fn refresh_changes_the_matcher_only_when_ours_is_the_sole_hook() {
        let mut hook = old_cartoon_hook("cartoon hook rewrite --deny-mode");
        hook["timeout"] = json!(5);
        let mut arr = vec![
            json!({ "matcher": "Edit", "hooks": [guard_hook()] }),
            json!({ "matcher": "Bash|run_in_terminal", "hooks": [hook] }),
        ];
        assert_eq!(upsert_claude_entry(&mut arr), Upsert::Refreshed);
        assert_eq!(arr.len(), 2);
        assert_eq!(
            arr[0],
            json!({ "matcher": "Edit", "hooks": [guard_hook()] })
        );
        assert_eq!(arr[1]["matcher"], "Bash");
        assert_eq!(arr[1]["hooks"][0]["command"], hook_command());
        assert_eq!(arr[1]["hooks"][0]["timeout"], 5, "extra keys kept");
    }

    #[test]
    fn refresh_leaves_exactly_one_cartoon_hook() {
        // Final review M3: a second (`--deny-mode`) entry survived.
        let mut arr = vec![
            claude_entry(),
            json!({ "matcher": "Bash", "hooks": [old_cartoon_hook("cartoon hook rewrite --deny-mode")] }),
            json!({ "matcher": "Bash", "hooks": [guard_hook(), old_cartoon_hook("cartoon hook rewrite")] }),
        ];
        assert_eq!(upsert_claude_entry(&mut arr), Upsert::Refreshed);
        let doc = Value::Array(arr.clone()).to_string();
        assert_eq!(doc.matches(HOOK_MARKER).count(), 1, "{doc}");
        assert_eq!(arr[0], claude_entry());
        assert_eq!(
            arr[1],
            json!({ "matcher": "Bash", "hooks": [guard_hook()] })
        );
    }

    #[test]
    fn uninstall_removes_only_our_hook_objects() {
        let mut arr = vec![
            claude_entry(),
            json!({ "matcher": "Bash", "hooks": [guard_hook(), old_cartoon_hook("cartoon hook rewrite")] }),
            json!({ "matcher": "Edit", "hooks": [guard_hook()] }),
        ];
        assert!(remove_claude_hooks(&mut arr));
        assert_eq!(
            arr,
            vec![
                json!({ "matcher": "Bash", "hooks": [guard_hook()] }),
                json!({ "matcher": "Edit", "hooks": [guard_hook()] }),
            ]
        );
        assert!(!remove_claude_hooks(&mut arr));
    }

    #[test]
    fn user_settings_path_honours_claude_config_dir() {
        // Final review M12: the rule reader honours it; install did not.
        let home = Path::new("/h");
        assert_eq!(
            user_settings_path_from(Some(home), None),
            Some(PathBuf::from("/h/.claude/settings.json"))
        );
        assert_eq!(
            user_settings_path_from(Some(home), Some(Path::new("/c"))),
            Some(PathBuf::from("/c/settings.json"))
        );
        assert_eq!(user_settings_path_from(None, None), None);
    }

    #[test]
    fn our_entry_detection() {
        assert!(is_our_entry(&claude_entry()));
        assert!(!is_our_entry(
            &json!({"matcher":"Bash","hooks":[{"type":"command","command":"other"}]})
        ));
    }

    #[test]
    fn claude_entry_matches_bash_only_with_no_flags() {
        let e = claude_entry();
        assert_eq!(e["matcher"], "Bash");
        let cmd = e["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.ends_with("cartoon hook rewrite || exit 0"), "{cmd}");
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn install_target_accepts_project_and_instructions_only() {
        let t = install_target(&args(&["install", "--project", "--instructions"])).unwrap();
        assert!(t.project && t.instructions && !t.copilot);
        assert!(!install_target(&args(&["install"])).unwrap().instructions);
        for removed in ["--copilot", "--vscode", "--deny"] {
            let e = install_target(&args(&["install", removed])).err().unwrap();
            let msg = e.to_string();
            assert!(msg.contains("removed in 0.8.0"), "{removed}: {msg}");
            assert!(
                msg.contains("--project, --instructions"),
                "{removed}: {msg}"
            );
        }
        let e = install_target(&args(&["install", "--nope"])).err().unwrap();
        assert!(e.to_string().contains("accepted flags"), "{e}");
    }

    #[test]
    fn uninstall_target_keeps_copilot_as_legacy_cleanup() {
        let t = uninstall_target(&args(&["uninstall", "--copilot", "--project"])).unwrap();
        assert!(t.copilot && t.project);
        for removed in ["--vscode", "--deny"] {
            assert!(uninstall_target(&args(&["uninstall", removed])).is_err());
        }
    }

    #[test]
    fn uninstall_instructions_doc_follows_the_legacy_copilot_flag() {
        let t = uninstall_target(&args(&["uninstall", "--copilot"])).unwrap();
        assert_eq!(uninstall_doc(t), crate::instructions::Doc::Copilot);
        // The default auto-detects CLAUDE.md vs AGENTS.md from the
        // filesystem; that resolution is unit-tested in
        // `instructions::resolve_agent_doc`.
    }

    #[test]
    fn read_settings_only_treats_not_found_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_settings(&dir.path().join("missing.json"))
            .unwrap()
            .is_none());
        // A directory where the file should be: an error, never "absent".
        assert!(read_settings(dir.path()).is_err());
        // Invalid UTF-8: an error, so install can't overwrite the file.
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, b"{\"a\":\"\xff\"}").unwrap();
        let err = read_settings(&bad).unwrap_err().to_string();
        assert!(err.contains("UTF-8"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_preserves_mode_and_symlink() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.json");
        std::fs::write(&real, "{}").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, "{\"x\":1}").unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "{\"x\":1}");
        let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // No temp files left behind.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        // A fresh file in a not-yet-existing directory.
        let fresh = dir.path().join("sub/new.json");
        write_atomic(&fresh, "{}").unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "{}");
    }

    #[test]
    fn entry_state_distinguishes_current_stale_and_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(claude_entry_state(&path), HookState::Missing);
        let doc = |e: Value| json!({"hooks": {"PreToolUse": [e]}}).to_string();
        std::fs::write(&path, doc(claude_entry())).unwrap();
        assert_eq!(claude_entry_state(&path), HookState::Installed);
        let old_cmd = json!({
            "matcher": "Bash",
            "hooks": [{ "type": "command", "command": "command -v cartoon >/dev/null 2>&1 && cartoon hook rewrite --deny-mode || exit 0" }]
        });
        std::fs::write(&path, doc(old_cmd)).unwrap();
        assert_eq!(claude_entry_state(&path), HookState::Stale);
        // Final review M2: a 0.7 allow-mode entry still wraps under 0.8.
        let old_matcher = json!({
            "matcher": "Bash|run_in_terminal",
            "hooks": [{ "type": "command", "command": hook_command() }]
        });
        std::fs::write(&path, doc(old_matcher)).unwrap();
        assert_eq!(claude_entry_state(&path), HookState::Outdated);
        assert!(HookState::Outdated.label().contains("still wraps"));
        assert!(HookState::Stale.label().contains("no longer wraps"));
        let other = json!({"matcher":"Bash","hooks":[{"type":"command","command":"other"}]});
        std::fs::write(&path, doc(other)).unwrap();
        assert_eq!(claude_entry_state(&path), HookState::Missing);
    }

    #[test]
    fn extra_keys_on_a_current_entry_are_not_stale() {
        let mut e = claude_entry();
        e["hooks"][0]["timeout"] = json!(5);
        assert!(entry_is_current(&e));
        let mut arr = vec![e];
        assert_eq!(upsert_claude_entry(&mut arr), Upsert::Unchanged);
    }
}
