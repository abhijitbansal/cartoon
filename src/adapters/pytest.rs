use super::{basename, is_python_module, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::Result;

pub struct Pytest;

impl Adapter for Pytest {
    fn name(&self) -> &'static str {
        "pytest"
    }
    fn matches(&self) -> &'static str {
        "pytest | python -m pytest"
    }
    fn detect(&self, argv: &[String]) -> bool {
        argv.first()
            .map(|a| basename(a) == "pytest")
            .unwrap_or(false)
            || is_python_module(argv, "pytest")
    }
    fn prepare(&self, argv: Vec<String>) -> Prepared {
        Prepared {
            argv,
            artifact: None,
        }
    }
    fn parse(&self, _captured: &Captured, _prepared: &Prepared) -> Result<ParseOutcome> {
        anyhow::bail!("not implemented yet")
    }
}
