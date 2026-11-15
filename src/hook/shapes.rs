//! Pre-lex normalisation of the command shapes agents actually type around a
//! noisy tool. `lex` refuses `|`, `>` and `cd` outright; this module peels
//! three narrow, inert wrappers off first so the tool itself can be judged:
//!
//! - a trailing `2>&1` on the last segment (cartoon `--merge-streams`
//!   already merges stderr into stdout), stripped;
//! - one trailing display pipe (`| tail -40`, `| xcbeautify`), kept inside
//!   the `-c` string so `cartoon -c` drops it and discloses that it did;
//! - a leading `cd <dir> &&` into a directory inside the project, kept
//!   outside the wrapped string so the Bash tool's cwd tracking still sees
//!   it.
//!
//! Anything else that looks like one of these shapes but is not exactly it
//! returns None: no rewrite.

use std::path::Path;

/// A command split into its parts. `body` still goes through `lex` and the
/// per-segment policy; `cd` and `filter` are already validated.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Shape {
    pub cd: Option<String>,
    pub body: String,
    pub filter: Option<String>,
}

impl Shape {
    /// The string `cartoon -c` runs: the body plus the display filter it
    /// drops.
    pub fn inner(&self) -> String {
        match &self.filter {
            Some(f) => format!("{} | {f}", self.body),
            None => self.body.clone(),
        }
    }
}

/// Display filters the hook lets ride along. A subset of
/// `cli::DISPLAY_FILTERS`: `cat`, `less` and `more` have no use after a
/// dev-loop tool.
const HOOK_FILTERS: &[&str] = &["head", "tail", "xcbeautify", "xcpretty"];

/// The only arguments `xcbeautify` / `xcpretty` may carry: output-style
/// switches. Report writers (`--report junit`, `-r html`) write files.
const BEAUTIFY_FLAGS: &[&str] = &[
    "--quiet",
    "-q",
    "--is-ci",
    "--disable-colored-output",
    "-s",
    "--simple",
    "--no-color",
];

/// The stderr merge the hook strips. It must be the last whitespace-
/// delimited word of the body.
const MERGE_STDERR: &str = "2>&1";

/// Split `cmd` into a `Shape`, or None when it carries a `cd`, pipe or
/// trailing filter that is not exactly an accepted form. A command with no
/// such wrapper comes back as its trimmed self in `body`.
pub(super) fn normalise(cmd: &str, root: Option<&Path>) -> Option<Shape> {
    let trimmed = cmd.trim();
    let (cd, rest) = peel_cd(trimmed, root)?;
    let (body, filter) = match crate::cli::split_single_pipe(rest) {
        Some((lhs, rhs)) => (strip_merge(lhs.trim()), Some(display_filter(rhs)?)),
        // No pipe, or several: `lex` refuses any `|` left in the body.
        None => (strip_merge(rest), None),
    };
    if body.is_empty() {
        return None;
    }
    let shape = Shape {
        cd,
        body: body.to_string(),
        filter,
    };
    // Keep only pipes `cartoon -c` will drop again (adapter on the left, no
    // leading `NAME=value`, nothing else shell-like): otherwise the filter
    // would really run and the hook would have approved a pipeline.
    let cli_drops = |s: &Shape| crate::cli::shell_argv_with_filter(&s.inner()).1.is_some();
    if shape.filter.is_some() && !cli_drops(&shape) {
        return None;
    }
    Some(shape)
}

/// Peel a leading `cd <dir> &&`. Not a cd at all → `(None, cmd)`. A cd that
/// is not exactly one unquoted, relative, in-project directory → None.
fn peel_cd<'a>(cmd: &'a str, root: Option<&Path>) -> Option<(Option<String>, &'a str)> {
    let Some(after) = cmd.strip_prefix("cd ") else {
        return Some((None, cmd));
    };
    let (dir, rest) = after.split_once("&&")?;
    let (dir, rest) = (dir.trim(), rest.trim());
    (!rest.is_empty() && dir_in_project(dir, root?)).then(|| (Some(dir.to_string()), rest))
}

/// `dir` is one plain word (no quote, space, `$`, `~`, glob, ...), relative,
/// not an option, free of `..`, and resolves (symlinks followed) to an
/// existing directory under `root`. `CDPATH` can still redirect a bare name
/// in the user's shell; that residual is accepted (see `mod.rs`).
fn dir_in_project(dir: &str, root: &Path) -> bool {
    let plain = !dir.is_empty() && dir.chars().all(|c| c != ' ' && super::is_plain(c));
    if !plain || dir.starts_with(['-', '/']) || dir.split('/').any(|p| p == "..") {
        return false;
    }
    let (Ok(root), Ok(target)) = (root.canonicalize(), root.join(dir).canonicalize()) else {
        return false;
    };
    target.is_dir() && target.starts_with(&root)
}

/// Drop one trailing ` 2>&1`. Anything else, including a second one, stays
/// for `lex` to refuse.
fn strip_merge(s: &str) -> &str {
    match s.strip_suffix(MERGE_STDERR) {
        Some(head) if head.ends_with(' ') => head.trim_end(),
        _ => s,
    }
}

/// The filter after the pipe, normalised to single spaces, when it is an
/// accepted display filter with accepted arguments only.
fn display_filter(rhs: &str) -> Option<String> {
    if !rhs.chars().all(super::is_plain) {
        return None;
    }
    let words: Vec<&str> = rhs.split_whitespace().collect();
    let (name, args) = words.split_first()?;
    if !HOOK_FILTERS.contains(name) || !crate::cli::DISPLAY_FILTERS.contains(name) {
        return None;
    }
    let ok = match *name {
        "head" | "tail" => line_count_ok(args),
        _ => args.iter().all(|a| BEAUTIFY_FLAGS.contains(a)),
    };
    ok.then(|| words.join(" "))
}

/// Exactly one line count: `-n N`, `-nN`, `-N` or `--lines=N`.
fn line_count_ok(args: &[&str]) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match args {
        ["-n", n] => digits(n),
        [a] => a
            .strip_prefix("--lines=")
            .or_else(|| a.strip_prefix("-n"))
            .or_else(|| a.strip_prefix('-'))
            .is_some_and(digits),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(cd: Option<&str>, body: &str, filter: Option<&str>) -> Shape {
        Shape {
            cd: cd.map(String::from),
            body: body.into(),
            filter: filter.map(String::from),
        }
    }

    #[test]
    fn plain_command_is_its_own_body() {
        assert_eq!(
            normalise("  pytest -q  ", None),
            Some(shape(None, "pytest -q", None))
        );
    }

    #[test]
    fn strips_one_trailing_merge_only() {
        assert_eq!(strip_merge("swift build 2>&1"), "swift build");
        assert_eq!(strip_merge("swift build 2>&1 2>&1"), "swift build 2>&1");
        assert_eq!(strip_merge("swift build x2>&1"), "swift build x2>&1");
        assert_eq!(strip_merge("2>&1"), "2>&1");
    }

    #[test]
    fn splits_display_pipe() {
        assert_eq!(
            normalise("xcodebuild test -scheme App 2>&1 |  tail   -40", None),
            Some(shape(None, "xcodebuild test -scheme App", Some("tail -40")))
        );
    }

    #[test]
    fn line_count_forms() {
        for ok in [&["-n", "5"][..], &["-n5"], &["-5"], &["--lines=5"]] {
            assert!(line_count_ok(ok), "{ok:?}");
        }
        for bad in [
            &[][..],
            &["-n"],
            &["-n", "+5"],
            &["-f"],
            &["--5"],
            &["-5", "file"],
            &["--lines="],
            &["-c5"],
        ] {
            assert!(!line_count_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn beautifier_flags_only() {
        assert_eq!(display_filter(" xcbeautify "), Some("xcbeautify".into()));
        assert_eq!(
            display_filter("xcpretty -s --no-color"),
            Some("xcpretty -s --no-color".into())
        );
        for bad in [
            "xcbeautify --report junit",
            "xcpretty -r html",
            "cat",
            "less",
            "tail -5 > f",
            "tail '-5'",
            "",
        ] {
            assert_eq!(display_filter(bad), None, "{bad}");
        }
    }

    #[test]
    fn hook_filters_are_display_filters() {
        for f in HOOK_FILTERS {
            assert!(crate::cli::DISPLAY_FILTERS.contains(f), "{f}");
        }
    }

    #[test]
    fn cd_needs_a_root_and_a_real_directory_inside_it() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("App/Sub")).unwrap();
        assert_eq!(
            normalise("cd App/Sub && pytest", Some(root.path())),
            Some(shape(Some("App/Sub"), "pytest", None))
        );
        assert_eq!(normalise("cd App && pytest", None), None);
        for bad in [
            "cd App",
            "cd App &&  ",
            "cd App Sub && pytest",
            "cd ./../x && pytest",
        ] {
            assert_eq!(normalise(bad, Some(root.path())), None, "{bad}");
        }
    }
}
