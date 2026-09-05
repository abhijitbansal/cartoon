pub mod diagnostics;
pub mod eslint;
pub mod jest;
pub mod pytest;
pub mod report;
pub mod ruff;
pub mod swift_build;
pub mod swift_test;
pub mod tsc;
pub mod unittest;
pub mod vitest;
pub mod xcodebuild;
pub mod xcodebuild_build;
pub mod xcodebuild_test;

use crate::runner::Captured;
use anyhow::Result;
use std::path::PathBuf;

/// Owns the temp artifact for cleanup AND exposes the path the adapter reads.
///
/// `File`: pytest/swift_test write into this file (e.g. junit xml). `Dir`:
/// xcodebuild writes a `.xcresult` bundle at `path` — a non-existent child of
/// `_guard` (xcodebuild refuses a pre-existing `-resultBundlePath`); the dir
/// tree is removed on drop. Keep "thing that cleans up" and "path to read" in
/// sync here — `artifact_path()` is the single accessor both variants go through.
pub enum Artifact {
    File(tempfile::NamedTempFile),
    Dir {
        _guard: tempfile::TempDir,
        path: PathBuf,
    },
}

/// Invocation after `prepare`: possibly extended argv plus an artifact the
/// adapter expects the child to write (junit xml, xcresult bundle, ...).
pub struct Prepared {
    pub argv: Vec<String>,
    pub artifact: Option<Artifact>,
}

impl Prepared {
    pub fn artifact_path(&self) -> Option<PathBuf> {
        match &self.artifact {
            Some(Artifact::File(f)) => Some(f.path().to_path_buf()),
            Some(Artifact::Dir { path, .. }) => Some(path.clone()),
            None => None,
        }
    }
}

/// What an adapter produces: a structured test report (rendered with the
/// asymmetric pass/fail layout) or an arbitrary TOON-encodable value
/// (diagnostics tools like linters and typecheckers).
pub enum AdapterReport {
    Tests(report::TestReport),
    Value(serde_json::Value),
}

impl AdapterReport {
    pub fn render(&self, trace_lines: usize, fast_note: Option<&str>) -> String {
        match self {
            AdapterReport::Tests(r) => report::render(r, trace_lines, fast_note),
            AdapterReport::Value(v) => crate::toon::encode(v),
        }
    }
}

/// What the agent should still see besides the TOON report.
/// `None` means the adapter consumed that stream (it WAS the report).
pub struct ParseOutcome {
    pub report: AdapterReport,
    pub passthrough_stdout: Option<String>,
    pub passthrough_stderr: Option<String>,
}

pub trait Adapter {
    fn name(&self) -> &'static str;
    /// Human description of what it matches, for `cartoon adapters`.
    fn matches(&self) -> &'static str;
    fn detect(&self, argv: &[String]) -> bool;
    /// Append machine-output flags. Must never remove or reorder user args.
    fn prepare(&self, argv: Vec<String>) -> Prepared;
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome>;
    /// Extra args that accelerate this runner, appended after prepare()'s
    /// injection when --fast is active. Default: none (silent no-op).
    fn fast_args(&self) -> Vec<String> {
        Vec::new()
    }
}

pub fn registry() -> Vec<Box<dyn Adapter>> {
    vec![
        Box::new(pytest::Pytest),
        Box::new(unittest::Unittest),
        Box::new(jest::Jest),
        Box::new(vitest::Vitest),
        Box::new(swift_test::SwiftTest),
        Box::new(xcodebuild_test::XcodebuildTest),
        Box::new(ruff::Ruff),
        Box::new(eslint::Eslint),
        Box::new(tsc::Tsc),
        Box::new(swift_build::SwiftBuild),
        Box::new(xcodebuild_build::XcodebuildBuild),
    ]
}

pub fn find_adapter(argv: &[String]) -> Option<Box<dyn Adapter>> {
    registry().into_iter().find(|a| a.detect(argv))
}

pub fn basename(arg: &str) -> &str {
    arg.rsplit(['/', '\\']).next().unwrap_or(arg)
}

/// `xcrun <tool> …` is transparent: xcrun only locates the Xcode toolchain
/// binary and forwards every argument, so detection looks past it.
pub fn strip_xcrun(argv: &[String]) -> &[String] {
    match argv.first() {
        Some(f) if basename(f) == "xcrun" && argv.len() > 1 => &argv[1..],
        _ => argv,
    }
}

pub fn is_python_module(argv: &[String], module: &str) -> bool {
    let first = argv.first().map(String::as_str).unwrap_or("");
    basename(first).starts_with("python") && argv.windows(2).any(|w| w[0] == "-m" && w[1] == module)
}

/// uv's own module form: `uv run -m pytest` strips (below) to `-m pytest`,
/// where uv runs the module like `python -m` would. Recognize that leading
/// `-m <module>` / `--module <module>`. Only meaningful on an argv that a uv
/// wrapper was actually stripped from — a bare command never starts with `-m`.
pub fn is_module_run(argv: &[String], module: &str) -> bool {
    matches!(
        argv.first().map(String::as_str),
        Some("-m") | Some("--module")
    ) && argv.get(1).map(String::as_str) == Some(module)
}

/// `uv run` / `uvx` options that take a following value (skipped *with* their
/// value when scanning past the wrapper). Need not be exhaustive: an unknown
/// option makes `skip_uv_opts` bail (fail open), so a missing entry only costs
/// a wrap, never a wrong one. Boolean options must NOT appear here.
const UV_VALUE_OPTS: &[&str] = &[
    "--extra",
    "--no-extra",
    "--group",
    "--no-group",
    "--only-group",
    "--env-file",
    "--with",
    "-w",
    "--with-editable",
    "--with-requirements",
    "--package",
    "--python-platform",
    "--python",
    "-p",
    "--directory",
    "--project",
    "--config-file",
    "--index",
    "--default-index",
    "--find-links",
    "-f",
    "--cache-dir",
    "--refresh-package",
    "--index-strategy",
    "--keyring-provider",
    "--resolution",
    "--prerelease",
    "--exclude-newer",
    "--link-mode",
    "--index-url",
    "--extra-index-url",
    "--config-setting",
    "-C",
];

/// `uv run` / `uvx` boolean options (skipped on their own). MUST stay purely
/// boolean: a value-taking option misfiled here could let its value masquerade
/// as the wrapped command and cause a false match. Unknown options aren't
/// skipped (we bail), so this list only needs the common ones.
const UV_BOOL_OPTS: &[&str] = &[
    "--all-extras",
    "--no-dev",
    "--no-default-groups",
    "--all-groups",
    "--only-dev",
    "--no-editable",
    "--exact",
    "--no-env-file",
    "--isolated",
    "--active",
    "--no-sync",
    "--locked",
    "--frozen",
    "--all-packages",
    "--no-project",
    "--script",
    "-s",
    "--gui-script",
    "--offline",
    "--native-tls",
    "--no-config",
    "--no-progress",
    "--quiet",
    "-q",
    "--verbose",
    "-v",
    "--refresh",
    "--no-cache",
    "--upgrade",
    "-U",
    "--reinstall",
    "--compile-bytecode",
    "--no-binary",
    "--no-build",
    "--preview",
];

/// Walk past uv-level options that sit between `uv run` and the wrapped command
/// (`uv run --no-sync pytest`, `uv run --python 3.12 pytest`). Stops at the
/// first positional (the command), at `-m`/`--module` (kept for the adapter's
/// module-run check), or at `--` (consumed: the command follows). Bails on any
/// unrecognized option so we never skip a value we don't understand and match
/// the wrong token — fail open, exactly like the rest of the pipeline.
fn skip_uv_opts(mut rest: &[String]) -> &[String] {
    while let Some(tok) = rest.first().map(String::as_str) {
        if tok == "--" {
            return rest.get(1..).unwrap_or(&[]);
        }
        if tok == "-m" || tok == "--module" || !tok.starts_with('-') {
            return rest;
        }
        if tok.contains('=') {
            rest = &rest[1..]; // --opt=value: a single token
        } else if UV_VALUE_OPTS.contains(&tok) {
            rest = rest.get(2..).unwrap_or(&[]); // --opt value: two tokens
        } else if UV_BOOL_OPTS.contains(&tok) {
            rest = &rest[1..];
        } else {
            return rest; // unknown option: don't guess, leave it for fail-open
        }
    }
    rest
}

/// Strip a leading `uv run`, `uvx`, or `uv tool run` wrapper (plus any uv-level
/// options before the command), returning the inner command's argv. uv forwards
/// everything after the target command straight through to it, and adapters only
/// *append* their machine-output flags, so the wrapper is transparent to
/// prepare/parse once detection looks past it. A returned slice strictly shorter
/// than `argv` signals a wrapper was present. Returns argv unchanged when
/// there's no uv `run`/`tool run`/`uvx` wrapper (e.g. `uv pip install`).
pub fn strip_uv_run(argv: &[String]) -> &[String] {
    let arg = |i: usize| argv.get(i).map(String::as_str);
    let rest = match basename(argv.first().map(String::as_str).unwrap_or("")) {
        "uvx" => &argv[1..],
        "uv" if arg(1) == Some("run") => &argv[2..],
        "uv" if arg(1) == Some("tool") && arg(2) == Some("run") => &argv[3..],
        _ => return argv,
    };
    skip_uv_opts(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn basename_strips_paths() {
        assert_eq!(basename("/usr/bin/pytest"), "pytest");
        assert_eq!(basename("pytest"), "pytest");
    }

    #[test]
    fn python_module_detection() {
        assert!(is_python_module(
            &argv(&["python3", "-m", "pytest", "-q"]),
            "pytest"
        ));
        assert!(is_python_module(
            &argv(&["python", "-m", "unittest"]),
            "unittest"
        ));
        assert!(!is_python_module(
            &argv(&["python3", "script.py"]),
            "pytest"
        ));
    }

    #[test]
    fn strip_uv_run_unwraps_wrappers() {
        assert_eq!(
            strip_uv_run(&argv(&["uv", "run", "pytest", "-q"])),
            &argv(&["pytest", "-q"])[..]
        );
        assert_eq!(
            strip_uv_run(&argv(&["uvx", "pytest"])),
            &argv(&["pytest"])[..]
        );
        assert_eq!(
            strip_uv_run(&argv(&["uv", "tool", "run", "pytest"])),
            &argv(&["pytest"])[..]
        );
        assert_eq!(
            strip_uv_run(&argv(&["uv", "run", "python", "-m", "pytest"])),
            &argv(&["python", "-m", "pytest"])[..]
        );
    }

    #[test]
    fn strip_uv_run_leaves_non_uv_untouched() {
        assert_eq!(
            strip_uv_run(&argv(&["pytest", "-q"])),
            &argv(&["pytest", "-q"])[..]
        );
        // `uv` without a recognized subcommand is not a runner wrapper.
        assert_eq!(
            strip_uv_run(&argv(&["uv", "pip", "install"])),
            &argv(&["uv", "pip", "install"])[..]
        );
        assert!(strip_uv_run(&[]).is_empty());
    }

    #[test]
    fn strip_uv_run_skips_boolean_uv_options() {
        assert_eq!(
            strip_uv_run(&argv(&["uv", "run", "--no-sync", "pytest", "-q"])),
            &argv(&["pytest", "-q"])[..]
        );
        assert_eq!(
            strip_uv_run(&argv(&["uv", "run", "--frozen", "--isolated", "pytest"])),
            &argv(&["pytest"])[..]
        );
    }

    #[test]
    fn strip_uv_run_skips_value_uv_options() {
        // `--with pkg` / `--python 3.12` consume their value, not the command.
        assert_eq!(
            strip_uv_run(&argv(&["uv", "run", "--with", "pytest-xdist", "pytest"])),
            &argv(&["pytest"])[..]
        );
        assert_eq!(
            strip_uv_run(&argv(&["uv", "run", "--python", "3.12", "pytest", "-q"])),
            &argv(&["pytest", "-q"])[..]
        );
        // `--opt=value` is a single token.
        assert_eq!(
            strip_uv_run(&argv(&["uv", "run", "--python=3.12", "pytest"])),
            &argv(&["pytest"])[..]
        );
    }

    #[test]
    fn strip_uv_run_handles_module_and_separator() {
        // `-m`/`--module` is kept for the module-run check.
        assert_eq!(
            strip_uv_run(&argv(&["uv", "run", "-m", "pytest", "tests"])),
            &argv(&["-m", "pytest", "tests"])[..]
        );
        // `--` ends uv options; the command follows.
        assert_eq!(
            strip_uv_run(&argv(&["uv", "run", "--", "pytest", "-q"])),
            &argv(&["pytest", "-q"])[..]
        );
    }

    #[test]
    fn strip_uv_run_bails_on_unknown_option() {
        // An option we don't model is left in place so detection fails open
        // rather than skipping a value we can't reason about.
        let a = argv(&["uv", "run", "--brand-new-flag", "pytest"]);
        assert_eq!(strip_uv_run(&a), &a[2..]);
        assert_eq!(strip_uv_run(&a)[0], "--brand-new-flag");
    }

    #[test]
    fn is_module_run_matches_uv_dash_m() {
        assert!(is_module_run(&argv(&["-m", "pytest"]), "pytest"));
        assert!(is_module_run(&argv(&["--module", "unittest"]), "unittest"));
        assert!(!is_module_run(&argv(&["-m", "pytest"]), "unittest"));
        assert!(!is_module_run(&argv(&["pytest"]), "pytest"));
    }

    #[test]
    fn find_adapter_matches_robust_uv_forms() {
        for (a, want) in [
            (argv(&["uv", "run", "-m", "pytest", "tests"]), "pytest"),
            (argv(&["uv", "run", "--no-sync", "pytest"]), "pytest"),
            (argv(&["uv", "run", "--with", "x", "pytest"]), "pytest"),
            (argv(&["uv", "run", "--", "pytest"]), "pytest"),
            (argv(&["uv", "run", "-m", "unittest"]), "unittest"),
        ] {
            assert_eq!(find_adapter(&a).map(|x| x.name()), Some(want), "argv {a:?}");
        }
        // Unknown uv option → fail open (no adapter), not a wrong match.
        assert!(find_adapter(&argv(&["uv", "run", "--brand-new-flag", "pytest"])).is_none());
    }

    #[test]
    fn find_adapter_matches_uv_run_pytest() {
        assert_eq!(
            find_adapter(&argv(&["uv", "run", "pytest", "-q"])).map(|a| a.name()),
            Some("pytest")
        );
        assert_eq!(
            find_adapter(&argv(&["uvx", "pytest"])).map(|a| a.name()),
            Some("pytest")
        );
        assert_eq!(
            find_adapter(&argv(&["uv", "run", "python", "-m", "pytest"])).map(|a| a.name()),
            Some("pytest")
        );
        assert_eq!(
            find_adapter(&argv(&["uv", "run", "python", "-m", "unittest"])).map(|a| a.name()),
            Some("unittest")
        );
    }

    #[test]
    fn registry_lists_all_adapters() {
        let names: Vec<&str> = registry().iter().map(|a| a.name()).collect();
        assert_eq!(
            names,
            vec![
                "pytest",
                "unittest",
                "jest",
                "vitest",
                "swift-test",
                "xcodebuild-test",
                "ruff",
                "eslint",
                "tsc",
                "swift-build",
                "xcodebuild-build"
            ]
        );
    }

    #[test]
    fn find_adapter_matches_pytest() {
        assert_eq!(
            find_adapter(&argv(&["pytest", "-q"])).map(|a| a.name()),
            Some("pytest")
        );
        assert!(find_adapter(&argv(&["ls", "-la"])).is_none());
    }

    #[test]
    fn python_versioned_binary_detected() {
        assert!(is_python_module(
            &argv(&["python3.12", "-m", "pytest"]),
            "pytest"
        ));
    }

    #[test]
    fn npx_path_to_jest_detected() {
        assert_eq!(
            find_adapter(&argv(&["npx", "./node_modules/.bin/jest"])).map(|a| a.name()),
            Some("jest")
        );
    }

    #[test]
    fn pytest_fast_args_inject_xdist() {
        let pytest = registry()
            .into_iter()
            .find(|a| a.name() == "pytest")
            .unwrap();
        assert_eq!(
            pytest.fast_args(),
            vec!["-n".to_string(), "auto".to_string()]
        );
    }

    #[test]
    fn other_adapters_have_no_fast_args() {
        let fast: Vec<&str> = registry()
            .iter()
            .filter(|a| !a.fast_args().is_empty())
            .map(|a| a.name())
            .collect();
        assert_eq!(fast, vec!["pytest"]);
    }
}
