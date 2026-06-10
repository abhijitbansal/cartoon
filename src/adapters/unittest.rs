use super::{is_python_module, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::Result;

pub struct Unittest;

impl Adapter for Unittest {
    fn name(&self) -> &'static str {
        "unittest"
    }
    fn matches(&self) -> &'static str {
        "python -m unittest"
    }
    fn detect(&self, argv: &[String]) -> bool {
        is_python_module(argv, "unittest")
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
