//! `cartoon hook` — Claude Code PreToolUse integration. `rewrite` reads
//! the hook JSON on stdin and, when the Bash command is a known noisy
//! dev-loop tool, rewrites it to run under `cartoon -c`. Fail-open
//! everywhere: any parse problem or non-match emits nothing and Claude
//! proceeds unchanged. `install`/`uninstall`/`status` manage the
//! settings.json entry.
//!
//! SECURITY: emitting `updatedInput` requires `permissionDecision:
//! "allow"`, which bypasses the normal permission prompt for that call.
//! The allowlist below is therefore restricted to read-mostly dev-loop
//! commands (test runners, linters, typecheckers, builds) and is
//! subcommand-aware for tools whose other subcommands mutate state.
//! Infra CLIs (docker, kubectl, terraform, gh, aws) are deliberately
//! excluded even though they are noisy.
use anyhow::Result;
use serde_json::{json, Value};
use std::io::Read;

/// Wrap regardless of arguments.
const ALWAYS: &[&str] = &[
    "pytest",
    "jest",
    "vitest",
    "tsc",
    "eslint",
    "ruff",
    "mypy",
    "make",
    "phpunit",
    "rspec",
    "pre-commit",
];

/// Wrap only when the first subcommand is in the listed set.
const SUBCOMMAND: &[(&str, &[&str])] = &[
    (
        "cargo",
        &["build", "test", "check", "clippy", "doc", "nextest"],
    ),
    ("go", &["test", "build", "vet"]),
    ("npm", &["test", "ci"]),
    ("pnpm", &["test"]),
    ("yarn", &["test"]),
    ("bun", &["test"]),
    ("dotnet", &["test", "build"]),
    ("gradle", &["test", "build", "check"]),
    ("gradlew", &["test", "build", "check"]),
    ("mvn", &["test", "verify", "package"]),
];

/// Runner prefixes: wrap when the NEXT word is itself an ALWAYS tool.
const RUNNERS: &[&str] = &["npx", "bunx", "pnpx"];

/// Shell builtins that mutate the calling shell's state. Claude Code's
/// Bash tool tracks cwd/env across calls; running these inside cartoon's
/// subshell would silently break that, so such commands pass through.
const STATE_BUILTINS: &[&str] = &[
    "cd", "export", "source", ".", "unset", "alias", "eval", "set", "ulimit", "umask",
];

pub fn run(args: &[String]) -> Result<i32> {
    match args.first().map(String::as_str) {
        Some("rewrite") => rewrite_from_stdin(),
        Some("install") => install(scope(args)?),
        Some("uninstall") => uninstall(scope(args)?),
        Some("status") => status(),
        _ => anyhow::bail!(
            "usage: cartoon hook (rewrite | install [--project] | uninstall [--project] | status)"
        ),
    }
}

fn scope(args: &[String]) -> Result<Scope> {
    match args.get(1).map(String::as_str) {
        None => Ok(Scope::User),
        Some("--project") => Ok(Scope::Project),
        Some(other) => anyhow::bail!("unknown flag {other} (expected --project)"),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Scope {
    User,
    Project,
}

// ---------- rewrite ----------

fn rewrite_from_stdin() -> Result<i32> {
    let mut input = String::new();
    // Fail-open: unreadable stdin means no rewrite, never an error.
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return Ok(0);
    }
    if let Some(out) = rewrite_decision(&input) {
        println!("{out}");
    }
    Ok(0)
}

/// Pure decision: full hook stdin JSON -> hook stdout JSON (or None).
pub fn rewrite_decision(input: &str) -> Option<String> {
    let v: Value = serde_json::from_str(input).ok()?;
    if v.get("tool_name")?.as_str()? != "Bash" {
        return None;
    }
    let cmd = v.get("tool_input")?.get("command")?.as_str()?;
    let wrapped = wrap_command(cmd)?;
    // Preserve every other tool_input field (timeout, description, ...).
    let mut tool_input = v.get("tool_input")?.clone();
    tool_input["command"] = json!(wrapped);
    Some(
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": "cartoon auto-wrap (net-savings guard: output only shrinks; raw log archived)",
                "updatedInput": tool_input,
            }
        })
        .to_string(),
    )
}

/// The wrapping rule. None = leave the command alone.
pub fn wrap_command(cmd: &str) -> Option<String> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() || trimmed.contains("<<") || trimmed.contains("cartoon") {
        return None;
    }
    if trimmed.ends_with('&') {
        return None; // background jobs must not be captured
    }
    let mut any_noisy = false;
    for segment in split_segments(trimmed) {
        let mut words = segment.split_whitespace().peekable();
        // skip leading VAR=value assignments
        while words
            .peek()
            .is_some_and(|w| w.contains('=') && !w.starts_with('='))
        {
            words.next();
        }
        let Some(first) = words.next() else { continue };
        let base = first.rsplit('/').next().unwrap_or(first);
        if STATE_BUILTINS.contains(&first) || STATE_BUILTINS.contains(&base) {
            return None;
        }
        let next = words.next();
        if is_noisy(base, next) {
            any_noisy = true;
        }
    }
    if !any_noisy {
        return None;
    }
    let escaped = trimmed.replace('\'', r"'\''");
    Some(format!("cartoon -c '{escaped}'"))
}

fn is_noisy(base: &str, next: Option<&str>) -> bool {
    if ALWAYS.contains(&base) {
        return true;
    }
    if RUNNERS.contains(&base) {
        return next
            .map(|n| n.rsplit('/').next().unwrap_or(n))
            .is_some_and(|n| ALWAYS.contains(&n) || n == "vitest" || n == "jest");
    }
    if let Some((_, subs)) = SUBCOMMAND.iter().find(|(c, _)| *c == base) {
        return next.is_some_and(|n| subs.contains(&n));
    }
    // python -m pytest / unittest
    if base.starts_with("python") {
        return next == Some("-m");
    }
    false
}

/// Split on top-level shell connectors. Coarse (quotes not honored), but
/// errs toward NOT wrapping: a connector inside quotes only adds segments
/// whose first words are unlikely to hit the allowlist.
fn split_segments(cmd: &str) -> Vec<&str> {
    cmd.split("&&")
        .flat_map(|s| s.split("||"))
        .flat_map(|s| s.split(';'))
        .flat_map(|s| s.split('|'))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

// ---------- install / uninstall / status ----------

const HOOK_COMMAND: &str = "command -v cartoon >/dev/null 2>&1 && cartoon hook rewrite || exit 0";

fn settings_path(scope: Scope) -> Result<std::path::PathBuf> {
    match scope {
        Scope::User => dirs::home_dir()
            .map(|h| h.join(".claude/settings.json"))
            .ok_or_else(|| anyhow::anyhow!("cannot resolve home directory")),
        Scope::Project => Ok(std::path::PathBuf::from(".claude/settings.json")),
    }
}

fn hook_entry() -> Value {
    json!({
        "matcher": "Bash",
        "hooks": [{ "type": "command", "command": HOOK_COMMAND }]
    })
}

fn is_our_entry(entry: &Value) -> bool {
    entry["hooks"]
        .as_array()
        .map(|hs| {
            hs.iter().any(|h| {
                h["command"]
                    .as_str()
                    .is_some_and(|c| c.contains("cartoon hook rewrite"))
            })
        })
        .unwrap_or(false)
}

fn install(scope: Scope) -> Result<i32> {
    let path = settings_path(scope)?;
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
    if arr.iter().any(is_our_entry) {
        println!("cartoon hook already installed in {}", path.display());
        return Ok(0);
    }
    arr.push(hook_entry());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&root)?)?;
    println!(
        "cartoon hook installed in {}\n\
         Noisy dev commands (test/lint/build) now auto-wrap; matching calls\n\
         are auto-approved, so the allowlist stays conservative by design.\n\
         Restart Claude Code (or run /hooks) to load it.\n\
         Remove with: cartoon hook uninstall",
        path.display()
    );
    Ok(0)
}

fn uninstall(scope: Scope) -> Result<i32> {
    let path = settings_path(scope)?;
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

fn status() -> Result<i32> {
    for scope in [Scope::User, Scope::Project] {
        let path = settings_path(scope)?;
        let installed = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v["hooks"]["PreToolUse"].as_array().cloned())
            .map(|arr| arr.iter().any(is_our_entry))
            .unwrap_or(false);
        println!(
            "{}: {}",
            path.display(),
            if installed {
                "installed"
            } else {
                "not installed"
            }
        );
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_noisy_simple_command() {
        assert_eq!(
            wrap_command("pytest -q tests/").as_deref(),
            Some("cartoon -c 'pytest -q tests/'")
        );
    }

    #[test]
    fn wraps_compound_with_noisy_segment() {
        assert_eq!(
            wrap_command("mkdir -p out && cargo build --release").as_deref(),
            Some("cartoon -c 'mkdir -p out && cargo build --release'")
        );
    }

    #[test]
    fn skips_quiet_commands() {
        assert!(wrap_command("ls -la").is_none());
        assert!(wrap_command("echo hi").is_none());
    }

    #[test]
    fn skips_state_mutating_builtins() {
        assert!(wrap_command("cd /app && pytest").is_none());
        assert!(wrap_command("export FOO=1 && cargo test").is_none());
        assert!(wrap_command("source .env; pytest").is_none());
    }

    #[test]
    fn skips_already_wrapped_heredoc_background() {
        assert!(wrap_command("cartoon pytest").is_none());
        assert!(wrap_command("pytest <<EOF\nx\nEOF").is_none());
        assert!(wrap_command("cargo build &").is_none());
    }

    #[test]
    fn subcommand_gating_blocks_mutating_subcommands() {
        assert!(wrap_command("cargo test").is_some());
        assert!(wrap_command("cargo publish").is_none());
        assert!(wrap_command("npm test").is_some());
        assert!(wrap_command("npm install left-pad").is_none());
        assert!(wrap_command("go test ./...").is_some());
        assert!(wrap_command("go run main.go").is_none());
    }

    #[test]
    fn infra_clis_never_wrapped() {
        assert!(wrap_command("kubectl get pods -A").is_none());
        assert!(wrap_command("terraform plan").is_none());
        assert!(wrap_command("docker build .").is_none());
        assert!(wrap_command("gh run view 123 --log").is_none());
    }

    #[test]
    fn python_module_runners_wrapped() {
        assert!(wrap_command("python -m pytest -q").is_some());
        assert!(wrap_command("python3 -m unittest").is_some());
        assert!(wrap_command("python script.py").is_none());
    }

    #[test]
    fn runner_prefix_requires_noisy_target() {
        assert!(wrap_command("npx jest src/").is_some());
        assert!(wrap_command("npx vitest run").is_some());
        assert!(wrap_command("npx cowsay moo").is_none());
    }

    #[test]
    fn env_prefix_does_not_hide_noisy_command() {
        assert_eq!(
            wrap_command("CI=1 pytest -x").as_deref(),
            Some("cartoon -c 'CI=1 pytest -x'")
        );
    }

    #[test]
    fn single_quotes_escaped() {
        let w = wrap_command("pytest -k 'not slow'").unwrap();
        assert_eq!(w, r#"cartoon -c 'pytest -k '\''not slow'\'''"#);
    }

    #[test]
    fn path_prefixed_binary_detected() {
        assert!(wrap_command("./node_modules/.bin/jest src/").is_some());
    }

    #[test]
    fn decision_ignores_non_bash_tools() {
        let input = r#"{"tool_name":"Read","tool_input":{"file_path":"/x"}}"#;
        assert!(rewrite_decision(input).is_none());
    }

    #[test]
    fn decision_rewrites_bash_command_preserving_other_fields() {
        let input = r#"{"tool_name":"Bash","tool_input":{"command":"pytest -q","timeout":5000}}"#;
        let out = rewrite_decision(input).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let hso = &v["hookSpecificOutput"];
        assert_eq!(hso["hookEventName"], "PreToolUse");
        assert_eq!(hso["permissionDecision"], "allow");
        assert_eq!(hso["updatedInput"]["command"], "cartoon -c 'pytest -q'");
        assert_eq!(hso["updatedInput"]["timeout"], 5000);
    }

    #[test]
    fn decision_passes_through_quiet_command() {
        let input = r#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#;
        assert!(rewrite_decision(input).is_none());
    }

    #[test]
    fn decision_fail_open_on_garbage() {
        assert!(rewrite_decision("not json").is_none());
        assert!(rewrite_decision("{}").is_none());
    }

    #[test]
    fn our_entry_detection() {
        assert!(is_our_entry(&hook_entry()));
        assert!(!is_our_entry(
            &json!({"matcher":"Bash","hooks":[{"type":"command","command":"other"}]})
        ));
    }
}
