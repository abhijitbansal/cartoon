pub mod jest;
pub mod pytest;
pub mod report;
pub mod unittest;

use crate::runner::Captured;
use anyhow::Result;
use std::path::PathBuf;

/// Invocation after `prepare`: possibly extended argv plus an artifact file
/// the adapter expects the child to write (e.g. junit xml).
pub struct Prepared {
    pub argv: Vec<String>,
    pub artifact: Option<tempfile::NamedTempFile>,
}

impl Prepared {
    pub fn artifact_path(&self) -> Option<PathBuf> {
        self.artifact.as_ref().map(|f| f.path().to_path_buf())
    }
}

/// What the agent should still see besides the TOON report.
/// `None` means the adapter consumed that stream (it WAS the report).
pub struct ParseOutcome {
    pub report: report::TestReport,
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
    ]
}

pub fn find_adapter(argv: &[String]) -> Option<Box<dyn Adapter>> {
    registry().into_iter().find(|a| a.detect(argv))
}

pub fn basename(arg: &str) -> &str {
    arg.rsplit(['/', '\\']).next().unwrap_or(arg)
}

pub fn is_python_module(argv: &[String], module: &str) -> bool {
    let first = argv.first().map(String::as_str).unwrap_or("");
    basename(first).starts_with("python") && argv.windows(2).any(|w| w[0] == "-m" && w[1] == module)
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
    fn registry_has_three_adapters() {
        let names: Vec<&str> = registry().iter().map(|a| a.name()).collect();
        assert_eq!(names, vec!["pytest", "unittest", "jest"]);
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
