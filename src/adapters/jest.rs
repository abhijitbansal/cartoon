use super::{basename, Adapter, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::Result;

pub struct Jest;

impl Adapter for Jest {
    fn name(&self) -> &'static str {
        "jest"
    }
    fn matches(&self) -> &'static str {
        "jest | npx jest"
    }
    fn detect(&self, argv: &[String]) -> bool {
        match argv {
            [first, ..] if basename(first) == "jest" => true,
            [first, second, ..]
                if matches!(basename(first), "npx" | "bunx") && second == "jest" =>
            {
                true
            }
            _ => false,
        }
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
