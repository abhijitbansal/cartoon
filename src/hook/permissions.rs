//! The user's own Claude Code permission rules, as far as the hook needs
//! them. Claude Code evaluates `ask`/`deny` rules against the command a
//! PreToolUse hook *returns*, so a rule such as `Bash(make deploy:*)` never
//! sees `cartoon --merge-streams -c 'make deploy'` and the hook's `allow`
//! would run it unprompted. The hook therefore loads every Bash `ask` and
//! `deny` rule itself and emits nothing (normal permission flow) when one
//! could match the original command.
//!
//! Matching is deliberately conservative: a settings file that exists but
//! can't be read or has an unexpected shape, a bare `Bash` rule, or any rule
//! syntax not understood here guards every command.
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Raw `ask` + `deny` rule strings whose tool is Bash (`"Bash"`,
/// `"Bash(<pattern>)"`), in load order, deduplicated. `unreadable` is set
/// when any settings file existed but could not be read or parsed.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct UserRules {
    pub guards: Vec<String>,
    pub unreadable: bool,
}

impl UserRules {
    /// Both rule sets together: guards unioned (first seen order kept),
    /// unreadable if either was.
    pub fn union(self, other: UserRules) -> UserRules {
        let mut guards = self.guards;
        for rule in other.guards {
            if !guards.contains(&rule) {
                guards.push(rule);
            }
        }
        UserRules {
            guards,
            unreadable: self.unreadable || other.unreadable,
        }
    }
}

/// Where Claude Code reads enterprise-managed settings on this platform.
pub fn managed_settings_path() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(PathBuf::from(
            "/Library/Application Support/ClaudeCode/managed-settings.json",
        ))
    } else if cfg!(target_os = "linux") {
        Some(PathBuf::from("/etc/claude-code/managed-settings.json"))
    } else {
        None
    }
}

/// Load Bash `ask`/`deny` rules from, in order: the user settings
/// (`<config_dir>/settings.json` when `$CLAUDE_CONFIG_DIR` is set, else
/// `<home>/.claude/settings.json`); `.claude/settings.json` and
/// `.claude/settings.local.json` in `cwd` and every ancestor; the managed
/// settings file. Environment lookups happen at the call site so this stays
/// deterministic for tests.
pub fn load_user_rules(
    cwd: &Path,
    home: Option<&Path>,
    config_dir: Option<&Path>,
    managed: Option<&Path>,
) -> UserRules {
    let user_dir = config_dir
        .map(Path::to_path_buf)
        .or_else(|| home.map(|h| h.join(".claude")));
    let mut files: Vec<PathBuf> = user_dir
        .map(|d| d.join("settings.json"))
        .into_iter()
        .collect();
    for dir in cwd.ancestors() {
        files.push(dir.join(".claude/settings.json"));
        files.push(dir.join(".claude/settings.local.json"));
    }
    files.extend(managed.map(Path::to_path_buf));

    let mut rules = UserRules::default();
    for file in &files {
        if collect_file(file, &mut rules).is_err() {
            rules.unreadable = true;
        }
    }
    rules
}

/// A file that does not exist contributes nothing. Any other read error, a
/// parse error or an unexpected shape is `Err` (the caller guards everything).
fn collect_file(path: &Path, rules: &mut UserRules) -> Result<(), ()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(()),
    };
    let value: Value = serde_json::from_str(&text).map_err(|_| ())?;
    let Value::Object(top) = value else {
        return Err(());
    };
    let permissions = match top.get("permissions") {
        None => return Ok(()),
        Some(Value::Object(p)) => p,
        Some(_) => return Err(()),
    };
    for key in ["ask", "deny"] {
        let entries = match permissions.get(key) {
            None => continue,
            Some(Value::Array(a)) => a,
            Some(_) => return Err(()),
        };
        for entry in entries {
            let rule = entry.as_str().ok_or(())?.trim();
            if is_bash_rule(rule) && !rules.guards.iter().any(|g| g == rule) {
                rules.guards.push(rule.to_string());
            }
        }
    }
    Ok(())
}

fn is_bash_rule(rule: &str) -> bool {
    rule == "Bash" || rule.starts_with("Bash(") || rule.starts_with("Bash ")
}

/// True when any loaded rule could match `cmd` (or rules are unreadable).
/// Candidates: the whole command, each segment between `&&`, `||`, `;` or
/// `|`, and each segment with leading `NAME=value` words removed.
pub fn is_guarded(cmd: &str, rules: &UserRules) -> bool {
    if rules.unreadable {
        return true;
    }
    if rules.guards.is_empty() {
        return false;
    }
    let candidates = candidates(cmd);
    rules.guards.iter().any(|rule| match parse_rule(rule) {
        Pattern::All => true,
        Pattern::Exact(p) => candidates.contains(&p),
        Pattern::Glob(p) => candidates.iter().any(|c| glob_matches(&p, c)),
    })
}

enum Pattern {
    All,
    Exact(String),
    Glob(String),
}

fn parse_rule(rule: &str) -> Pattern {
    let Some(inner) = rule
        .trim()
        .strip_prefix("Bash(")
        .and_then(|s| s.strip_suffix(')'))
    else {
        // `Bash` itself, or a shape we don't understand: guard everything.
        return Pattern::All;
    };
    let inner = normalise(inner);
    if inner.is_empty() || inner == "*" {
        return Pattern::All;
    }
    if let Some(prefix) = inner.strip_suffix(":*") {
        return Pattern::Glob(format!("{prefix}*"));
    }
    if inner.contains('*') {
        Pattern::Glob(inner)
    } else {
        Pattern::Exact(inner)
    }
}

/// Anchored glob where `*` is any run of characters (possibly empty). A
/// trailing ` *` also matches the bare command (`git status *` guards
/// `git status`), erring towards guarding.
fn glob_matches(pattern: &str, text: &str) -> bool {
    wildcard(pattern.as_bytes(), text.as_bytes())
        || pattern
            .strip_suffix(" *")
            .is_some_and(|base| wildcard(base.as_bytes(), text.as_bytes()))
}

/// Iterative `*`-only wildcard match with single-star backtracking.
fn wildcard(p: &[u8], t: &[u8]) -> bool {
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && p[pi] == b'*' {
            star = Some((pi, ti));
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&b| b == b'*')
}

fn candidates(cmd: &str) -> Vec<String> {
    let mut out = vec![normalise(cmd)];
    let segments = cmd
        .split("&&")
        .flat_map(|s| s.split("||"))
        .flat_map(|s| s.split([';', '|']));
    for segment in segments {
        let segment = normalise(segment);
        if segment.is_empty() {
            continue;
        }
        let stripped = strip_env_prefix(&segment);
        if stripped != segment && !stripped.is_empty() {
            out.push(stripped);
        }
        out.push(segment);
    }
    out
}

fn strip_env_prefix(segment: &str) -> String {
    segment
        .split(' ')
        .skip_while(|w| is_env_assignment(w))
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_env_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn normalise(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn rules(guards: &[&str]) -> UserRules {
        UserRules {
            guards: guards.iter().map(|s| s.to_string()).collect(),
            unreadable: false,
        }
    }

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn guards_prefix_rule() {
        let r = rules(&["Bash(make deploy:*)"]);
        assert!(is_guarded("make deploy", &r));
        assert!(is_guarded("make deploy --dry-run", &r));
        assert!(!is_guarded("make test", &r));
    }

    #[test]
    fn guards_exact_rule() {
        let r = rules(&["Bash(npm test)"]);
        assert!(is_guarded("npm test", &r));
        assert!(!is_guarded("npm test -- -u", &r));
    }

    #[test]
    fn guards_wildcard_forms() {
        let r = rules(&["Bash(*deploy*)"]);
        assert!(is_guarded("make deploy", &r));
        let all = rules(&["Bash"]);
        assert!(is_guarded("pytest", &all));
        let star = rules(&["Bash(*)"]);
        assert!(is_guarded("pytest", &star));
    }

    #[test]
    fn guards_segment_match() {
        let r = rules(&["Bash(make deploy:*)"]);
        assert!(is_guarded("pytest tests && make deploy", &r));
        assert!(is_guarded("CI=1 make deploy", &r));
    }

    #[test]
    fn unreadable_settings_guard_everything() {
        let r = UserRules {
            guards: vec![],
            unreadable: true,
        };
        assert!(is_guarded("pytest", &r));
    }

    #[test]
    fn space_star_rule_guards_bare_command_too() {
        let r = rules(&["Bash(git status *)"]);
        assert!(is_guarded("git status", &r));
        assert!(is_guarded("git status --short", &r));
        assert!(!is_guarded("git statusx", &r));
        assert!(!is_guarded("git log", &r));
    }

    #[test]
    fn whitespace_runs_are_normalised() {
        let r = rules(&["Bash(npm   test)"]);
        assert!(is_guarded("  npm  test ", &r));
    }

    #[test]
    fn unknown_bash_rule_shape_guards_everything() {
        for rule in ["Bash(", "Bash()", "Bash[pytest]"] {
            assert!(is_guarded("pytest", &rules(&[rule])), "{rule}");
        }
    }

    #[test]
    fn no_rules_guard_nothing() {
        assert!(!is_guarded("pytest", &UserRules::default()));
    }

    #[test]
    fn union_merges_guards_and_unreadable() {
        let a = rules(&["Bash(a)", "Bash(b)"]);
        let b = UserRules {
            guards: vec!["Bash(b)".into(), "Bash(c)".into()],
            unreadable: true,
        };
        assert_eq!(
            a.union(b),
            UserRules {
                guards: vec!["Bash(a)".into(), "Bash(b)".into(), "Bash(c)".into()],
                unreadable: true,
            }
        );
    }

    #[test]
    fn load_reads_ask_and_deny_from_project_and_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("proj");
        write(
            &home.join(".claude/settings.json"),
            r#"{"permissions":{"deny":["Bash(make deploy:*)","Read(./.env)"],"allow":["Bash(pytest:*)"]}}"#,
        );
        write(
            &proj.join(".claude/settings.local.json"),
            r#"{"permissions":{"ask":["Bash(npm ci)","WebFetch"],"allow":["Bash"]}}"#,
        );
        let got = load_user_rules(&proj, Some(&home), None, None);
        assert_eq!(
            got,
            UserRules {
                guards: vec!["Bash(make deploy:*)".into(), "Bash(npm ci)".into()],
                unreadable: false,
            }
        );
    }

    #[test]
    fn load_reads_ancestor_config_dir_and_managed_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let config = tmp.path().join("config");
        let proj = tmp.path().join("repo");
        let sub = proj.join("pkg/inner");
        fs::create_dir_all(&sub).unwrap();
        let managed = tmp.path().join("managed-settings.json");
        // CLAUDE_CONFIG_DIR wins over <home>/.claude.
        write(
            &home.join(".claude/settings.json"),
            r#"{"permissions":{"deny":["Bash(ignored)"]}}"#,
        );
        write(
            &config.join("settings.json"),
            r#"{"permissions":{"deny":["Bash(rm:*)"]}}"#,
        );
        write(
            &proj.join(".claude/settings.json"),
            r#"{"permissions":{"ask":["Bash(git push:*)"]}}"#,
        );
        write(&managed, r#"{"permissions":{"deny":["Bash(curl:*)"]}}"#);
        let got = load_user_rules(&sub, Some(&home), Some(&config), Some(&managed));
        assert_eq!(
            got.guards,
            vec!["Bash(rm:*)", "Bash(git push:*)", "Bash(curl:*)"]
        );
        assert!(!got.unreadable);
    }

    #[test]
    fn load_marks_malformed_settings_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        write(&proj.join(".claude/settings.json"), "{not json");
        assert!(load_user_rules(&proj, None, None, None).unreadable);
    }

    #[test]
    fn load_marks_wrong_shapes_unreadable() {
        for body in [
            r#"[]"#,
            r#"{"permissions":[]}"#,
            r#"{"permissions":{"deny":"Bash"}}"#,
            r#"{"permissions":{"ask":[42]}}"#,
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let proj = tmp.path().join("proj");
            write(&proj.join(".claude/settings.json"), body);
            assert!(
                load_user_rules(&proj, None, None, None).unreadable,
                "{body}"
            );
        }
    }

    #[test]
    fn load_marks_unreadable_file_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        // A directory where the settings file should be: exists, can't be read.
        fs::create_dir_all(proj.join(".claude/settings.json")).unwrap();
        assert!(load_user_rules(&proj, None, None, None).unreadable);
    }
}
