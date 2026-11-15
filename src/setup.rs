//! `cartoon setup [--project] [--no-instructions]` — one idempotent,
//! non-interactive command that does what a new user needs: install the
//! Claude Code hook, write the instructions block, then print the doctor
//! report so the result is visible. Never prompts; safe to run unattended
//! and to re-run.
use anyhow::Result;

/// Accepted flags, named in the error for an unknown one.
pub const ACCEPTED_FLAGS: &str = "--project, --no-instructions";

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Options {
    /// Install the hook into `.claude/settings.json` of the project
    /// instead of `~/.claude/settings.json`.
    pub project: bool,
    /// Write the instructions block into the project's agent doc.
    pub instructions: bool,
}

pub fn parse_args(args: &[String]) -> Result<Options> {
    let mut opts = Options {
        project: false,
        instructions: true,
    };
    for a in args {
        match a.as_str() {
            "--project" => opts.project = true,
            "--no-instructions" => opts.instructions = false,
            other => anyhow::bail!(
                "unknown argument {other} for `cartoon setup` (accepted flags: {ACCEPTED_FLAGS})"
            ),
        }
    }
    Ok(opts)
}

pub fn run(opts: Options) -> Result<i32> {
    crate::hook::install_claude_hook(opts.project)?;
    if opts.instructions {
        let path = crate::instructions::doc_path(crate::instructions::default_agent_doc());
        let outcome = crate::instructions::install_doc(&path)?;
        println!("{}", crate::instructions::describe(&path, outcome));
    }
    println!("\n{}", crate::doctor::report());
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_to_user_scope_with_instructions() {
        assert_eq!(
            parse_args(&[]).unwrap(),
            Options {
                project: false,
                instructions: true
            }
        );
    }

    #[test]
    fn flags_flip_their_options() {
        assert_eq!(
            parse_args(&args(&["--project", "--no-instructions"])).unwrap(),
            Options {
                project: true,
                instructions: false
            }
        );
    }

    #[test]
    fn unknown_flag_names_the_accepted_ones() {
        let e = parse_args(&args(&["--instructions"]))
            .unwrap_err()
            .to_string();
        assert!(e.contains("--instructions"), "{e}");
        assert!(e.contains(ACCEPTED_FLAGS), "{e}");
    }
}
