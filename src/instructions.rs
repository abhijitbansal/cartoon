//! `cartoon instructions` — write the "wrap noisy commands, never pipe them"
//! directive into an agent's instruction file (`AGENTS.md`,
//! `.github/copilot-instructions.md`, or `CLAUDE.md`).
//!
//! This is the instruction-layer companion to `cartoon hook`. The hook is
//! deterministic but has two blind spots the model has to cover itself. First,
//! it cannot rewrite a *piped* command: `pytest | tail` is split on `|` and the
//! non-allowlisted `tail` segment makes the whole compound fail the auto-approve
//! check, so it runs raw (by design — a rewrite auto-approves, and nothing may
//! ride along an allowlisted segment). Second, VS Code Copilot Chat can only
//! deny, not rewrite. In both cases the only thing that keeps cartoon in play is
//! the agent *choosing* to wrap and not pipe, which is exactly what this
//! directive tells it to do. Hook plus instructions cover far more together than
//! either does alone.
//!
//! The directive is wrapped in stable marker comments so install is
//! idempotent (re-running updates in place) and uninstall removes exactly our
//! block, never the surrounding file the user owns.
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// Begin/end markers. Fixed strings (no variable parts) so detection and
/// region replacement are exact. HTML comments render invisibly in Markdown.
const MARKER_BEGIN: &str =
    "<!-- BEGIN cartoon instructions (managed by `cartoon instructions`; edit outside this block) -->";
const MARKER_END: &str = "<!-- END cartoon instructions -->";

/// The directive itself. Front-loads the hard rule and the pipe ban, because
/// the pipe case is the one the hook can't catch. Kept tight — it lives in the
/// agent's always-on context.
const BODY: &str = r#"## CLI output: wrap noisy commands in `cartoon` — never pipe them

Hard rule, not a suggestion. Before running any test, build, lint, typecheck,
or JSON-emitting CLI, prefix it with `cartoon`. Wrapping is always safe: exit
codes are mirrored exactly, parse failures pass the original through, args are
never reordered, and a net-savings guard means the output never gets larger.

ALWAYS prefix these (running one bare is a mistake): `pytest`,
`python -m pytest`, `unittest`, `jest`, `vitest`, `tsc`, `eslint`, `ruff`,
`mypy`, `make`, `cargo build|test|check|clippy`, `go test|build|vet`,
`npm test`, `swift test|build`, `xcodebuild test|build`, the same tools run
through uv (`uv run pytest`, `uvx ruff check`), and any `… --output json` CLI
(`aws`, `gh`, `kubectl`). Examples: `cartoon pytest -q`,
`cartoon uv run pytest`, `cartoon npx jest src/`, `cartoon make`.

NEVER pipe a noisy command into `head`/`tail`/`grep` to shrink it. A pipe is
doubly wrong: it keeps an arbitrary slice (a build's real error sits mid-log
while `tail` shows only `BUILD FAILED`), AND the auto-wrap hook cannot rewrite
a piped command, so you lose cartoon entirely. Wrap the command bare instead,
then search the archived raw log: `cartoon logs grep <pattern> --last`.

    pytest | tail -20      # WRONG: lossy cut, and the hook can't wrap it
    cartoon pytest         # RIGHT: signal kept, ~70% fewer tokens

Don't wrap interactive/TTY commands (REPLs, watch modes). If `cartoon` is
missing: `uv tool install cartoon` (or `pipx install cartoon` /
`npm i -g cartoon-wrap` / `cargo install cartoon`)."#;

/// Which instruction file to manage. All are project-scoped: AGENTS.md,
/// copilot-instructions.md and CLAUDE.md all live in the repo, so unlike the
/// hook (which defaults to user `~/.claude`) there is no user/project split.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Doc {
    /// `AGENTS.md` — the cross-agent standard (Codex, Cursor, Copilot, …).
    Agents,
    /// `.github/copilot-instructions.md` — GitHub Copilot (Chat + coding agent).
    Copilot,
    /// `CLAUDE.md` — Claude Code.
    Claude,
}

/// What an install did, for the human-facing message.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Outcome {
    /// File did not exist (or was empty) — created with just the directive.
    Created,
    /// File existed without our block — directive appended.
    Added,
    /// Our block was already present — replaced in place.
    Updated,
}

pub fn run(args: &[String]) -> Result<i32> {
    match args.first().map(String::as_str) {
        Some("install") => install(parse_doc(&args[1..])?),
        Some("uninstall") => uninstall(parse_doc(&args[1..])?),
        Some("status") => status(),
        Some("print") => {
            println!("{}", block());
            Ok(0)
        }
        _ => bail!(
            "usage: cartoon instructions (install [--copilot|--claude] | uninstall [--copilot|--claude] | status | print)"
        ),
    }
}

/// Pick the target file from flags. Default is `AGENTS.md`.
fn parse_doc(args: &[String]) -> Result<Doc> {
    let mut doc: Option<Doc> = None;
    for a in args {
        let next = match a.as_str() {
            "--agents" => Doc::Agents,
            "--copilot" => Doc::Copilot,
            "--claude" => Doc::Claude,
            other => bail!("unknown flag {other} (expected --copilot, --claude, or --agents)"),
        };
        if doc.is_some_and(|d| d != next) {
            bail!("pick one target file: --agents, --copilot, or --claude");
        }
        doc = Some(next);
    }
    Ok(doc.unwrap_or(Doc::Agents))
}

/// The cwd-relative path for a target file.
pub fn doc_path(doc: Doc) -> PathBuf {
    match doc {
        Doc::Agents => PathBuf::from("AGENTS.md"),
        Doc::Copilot => PathBuf::from(".github/copilot-instructions.md"),
        Doc::Claude => PathBuf::from("CLAUDE.md"),
    }
}

/// The full managed block (markers + directive, no trailing newline).
pub fn block() -> String {
    format!("{MARKER_BEGIN}\n{BODY}\n{MARKER_END}")
}

/// Is our directive present in this file?
pub fn is_present(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|s| s.contains(MARKER_BEGIN))
}

/// Write/update the directive in `path`, creating parent dirs as needed.
/// Idempotent: an existing block is replaced, not duplicated.
pub fn install_doc(path: &Path) -> Result<Outcome> {
    let existing = std::fs::read_to_string(path).ok();
    let (out, outcome) = apply(existing.as_deref())?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, out)?;
    Ok(outcome)
}

/// Remove our directive from `path`. Returns whether anything was removed.
/// If the file is left empty, it is deleted.
pub fn uninstall_doc(path: &Path) -> Result<bool> {
    let Ok(existing) = std::fs::read_to_string(path) else {
        return Ok(false);
    };
    match apply_remove(&existing)? {
        None => Ok(false),
        Some(rest) => {
            if rest.trim().is_empty() {
                std::fs::remove_file(path)?;
            } else {
                std::fs::write(path, rest)?;
            }
            Ok(true)
        }
    }
}

/// One-line human summary of an install outcome.
pub fn describe(path: &Path, outcome: Outcome) -> String {
    let p = path.display();
    match outcome {
        Outcome::Created => format!("created {p} with the cartoon directive"),
        Outcome::Added => format!("added the cartoon directive to {p}"),
        Outcome::Updated => format!("updated the cartoon directive in {p}"),
    }
}

// ---------- pure transforms (unit-tested without touching the filesystem) ----------

/// Compute the new file contents for an install. `existing` is the current
/// file (None if absent). Replaces our block if present, else appends it.
fn apply(existing: Option<&str>) -> Result<(String, Outcome)> {
    let blk = block();
    let Some(s) = existing.filter(|s| !s.trim().is_empty()) else {
        return Ok((format!("{blk}\n"), Outcome::Created));
    };
    match (s.find(MARKER_BEGIN), s.find(MARKER_END)) {
        (Some(b), Some(e)) if e > b => {
            let end = e + MARKER_END.len();
            Ok((format!("{}{}{}", &s[..b], blk, &s[end..]), Outcome::Updated))
        }
        (None, None) => {
            let head = s.trim_end_matches('\n');
            Ok((format!("{head}\n\n{blk}\n"), Outcome::Added))
        }
        _ => bail!(
            "cartoon instruction markers are malformed (only one of the begin/end \
             markers was found); fix the file by hand and retry"
        ),
    }
}

/// Compute the file contents after removing our block. `Ok(None)` means no
/// block was present (nothing to remove).
fn apply_remove(s: &str) -> Result<Option<String>> {
    match (s.find(MARKER_BEGIN), s.find(MARKER_END)) {
        (Some(b), Some(e)) if e > b => {
            let end = e + MARKER_END.len();
            let head = s[..b].trim_end_matches('\n');
            let tail = s[end..].trim_start_matches('\n');
            let joined = match (head.is_empty(), tail.is_empty()) {
                (true, true) => String::new(),
                (false, true) => format!("{head}\n"),
                (true, false) => format!("{tail}\n"),
                (false, false) => format!("{head}\n\n{tail}\n"),
            };
            Ok(Some(joined))
        }
        (None, None) => Ok(None),
        _ => bail!(
            "cartoon instruction markers are malformed (only one of the begin/end \
             markers was found); fix the file by hand and retry"
        ),
    }
}

// ---------- CLI command bodies ----------

fn install(doc: Doc) -> Result<i32> {
    let path = doc_path(doc);
    let outcome = install_doc(&path)?;
    println!(
        "{}\n\
         The agent is now told to wrap noisy commands (test/build/lint/JSON CLIs)\n\
         and never pipe them — the case the auto-wrap hook can't catch.\n\
         For deterministic wrapping too, pair it with: cartoon hook install\n\
         Remove with: cartoon instructions uninstall{}",
        describe(&path, outcome),
        doc_flag(doc),
    );
    Ok(0)
}

fn uninstall(doc: Doc) -> Result<i32> {
    let path = doc_path(doc);
    if uninstall_doc(&path)? {
        println!("removed the cartoon directive from {}", path.display());
    } else {
        println!("no cartoon directive found in {}", path.display());
    }
    Ok(0)
}

fn status() -> Result<i32> {
    for doc in [Doc::Agents, Doc::Copilot, Doc::Claude] {
        let path = doc_path(doc);
        println!(
            "{}: {}",
            path.display(),
            if is_present(&path) {
                "installed"
            } else {
                "not installed"
            }
        );
    }
    Ok(0)
}

/// The flag that re-selects a non-default doc (for "remove with:" hints).
fn doc_flag(doc: Doc) -> &'static str {
    match doc {
        Doc::Agents => "",
        Doc::Copilot => " --copilot",
        Doc::Claude => " --claude",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_has_markers_and_the_pipe_ban() {
        let b = block();
        assert!(b.starts_with(MARKER_BEGIN));
        assert!(b.ends_with(MARKER_END));
        // The directive's whole reason for existing: the pipe case.
        assert!(b.contains("NEVER pipe"));
        assert!(b.contains("cartoon logs grep <pattern> --last"));
        assert!(b.contains("cartoon pytest"));
    }

    #[test]
    fn install_into_absent_or_empty_creates() {
        let (out, o) = apply(None).unwrap();
        assert_eq!(o, Outcome::Created);
        assert!(out.contains(MARKER_BEGIN) && out.ends_with("-->\n"));
        // Whitespace-only existing file counts as "create".
        let (_, o2) = apply(Some("   \n\n")).unwrap();
        assert_eq!(o2, Outcome::Created);
    }

    #[test]
    fn install_appends_preserving_existing_content() {
        let existing = "# My project\n\nSome rules here.\n";
        let (out, o) = apply(Some(existing)).unwrap();
        assert_eq!(o, Outcome::Added);
        assert!(out.starts_with("# My project\n\nSome rules here.\n\n"));
        assert!(out.contains(MARKER_BEGIN));
        // Exactly one block.
        assert_eq!(out.matches(MARKER_BEGIN).count(), 1);
    }

    #[test]
    fn reinstall_replaces_in_place_no_duplication() {
        let first = apply(Some("# Doc\n")).unwrap().0;
        let (second, o) = apply(Some(&first)).unwrap();
        assert_eq!(o, Outcome::Updated);
        assert_eq!(second.matches(MARKER_BEGIN).count(), 1);
        assert_eq!(second, first, "idempotent: same input → same output");
    }

    #[test]
    fn reinstall_keeps_surrounding_text_when_block_is_in_the_middle() {
        let middle = format!("# Top\n\n{}\n\n## Tail section\nkeep me\n", block());
        let (out, o) = apply(Some(&middle)).unwrap();
        assert_eq!(o, Outcome::Updated);
        assert!(out.starts_with("# Top\n"));
        assert!(out.contains("## Tail section\nkeep me\n"));
        assert_eq!(out.matches(MARKER_BEGIN).count(), 1);
    }

    #[test]
    fn uninstall_removes_block_and_keeps_the_rest() {
        let existing = format!("# Doc\n\nrule one\n\n{}\n\n## After\ntail\n", block());
        let rest = apply_remove(&existing).unwrap().unwrap();
        assert!(!rest.contains(MARKER_BEGIN));
        assert!(rest.contains("# Doc"));
        assert!(rest.contains("rule one"));
        assert!(rest.contains("## After\ntail"));
    }

    #[test]
    fn uninstall_only_our_block_leaves_empty() {
        let only = format!("{}\n", block());
        let rest = apply_remove(&only).unwrap().unwrap();
        assert!(rest.trim().is_empty());
    }

    #[test]
    fn uninstall_reports_absent_when_no_block() {
        assert!(apply_remove("# nothing of ours here\n").unwrap().is_none());
    }

    #[test]
    fn malformed_markers_error_not_clobber() {
        let half = format!("# Doc\n{MARKER_BEGIN}\nbody but no end\n");
        assert!(apply(Some(&half)).is_err());
        assert!(apply_remove(&half).is_err());
    }

    #[test]
    fn doc_paths_and_flags() {
        assert_eq!(doc_path(Doc::Agents), PathBuf::from("AGENTS.md"));
        assert_eq!(
            doc_path(Doc::Copilot),
            PathBuf::from(".github/copilot-instructions.md")
        );
        assert_eq!(doc_path(Doc::Claude), PathBuf::from("CLAUDE.md"));
        assert_eq!(doc_flag(Doc::Copilot), " --copilot");
    }

    #[test]
    fn parse_doc_defaults_and_validates() {
        assert_eq!(parse_doc(&[]).unwrap(), Doc::Agents);
        assert_eq!(parse_doc(&["--copilot".into()]).unwrap(), Doc::Copilot);
        assert_eq!(parse_doc(&["--claude".into()]).unwrap(), Doc::Claude);
        // repeated same flag is fine; conflicting flags and unknown flags error
        assert_eq!(
            parse_doc(&["--copilot".into(), "--copilot".into()]).unwrap(),
            Doc::Copilot
        );
        assert!(parse_doc(&["--copilot".into(), "--claude".into()]).is_err());
        assert!(parse_doc(&["--nope".into()]).is_err());
    }
}
