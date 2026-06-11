//! Tiered compression ladder for generic (no-adapter) CLI output.
//! Rules are pure functions applied in a fixed order; `CompressLevel`
//! selects the subset. See docs/superpowers/specs/2026-06-11-*.md.
//!
//! Rule order (fixed):
//! `strip_ansi -> collapse_progress -> collapse_blanks -> collapse_repeats` (safe)
//! `-> filter_levels -> collapse_near_dups -> extract_diagnostics -> window_errors` (aggressive)
//! Phase 2 inserts `drain` after `collapse_near_dups`. Phase 3 appends `model_score` last.

mod diagnostics;
mod levels;
mod near_dups;
mod progress;
mod safe;
mod window;

pub use diagnostics::extract_diagnostics;
pub use levels::filter_levels;
pub use near_dups::collapse_near_dups;
pub use progress::collapse_progress;
pub use safe::{collapse_blanks, collapse_repeats, strip_ansi};
pub use window::window_errors;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressLevel {
    Safe,
    Aggressive,
}

impl CompressLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompressLevel::Safe => "safe",
            CompressLevel::Aggressive => "aggressive",
        }
    }
}

impl std::str::FromStr for CompressLevel {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "safe" => Ok(CompressLevel::Safe),
            "aggressive" => Ok(CompressLevel::Aggressive),
            other => Err(format!(
                "invalid compress level '{other}' (expected: safe | aggressive)"
            )),
        }
    }
}

/// Apply the ladder at the given level. Fixed rule order; each rule is a
/// pure fn(&str) -> String that no-ops when its pattern is absent.
pub fn compress(text: &str, level: CompressLevel) -> String {
    let safe = collapse_repeats(&collapse_blanks(&collapse_progress(&strip_ansi(text))));
    match level {
        CompressLevel::Safe => safe,
        CompressLevel::Aggressive => {
            window_errors(&extract_diagnostics(&collapse_near_dups(&filter_levels(
                &safe,
            ))))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_parses_known_values() {
        assert_eq!(
            "safe".parse::<CompressLevel>().unwrap(),
            CompressLevel::Safe
        );
        assert_eq!(
            "aggressive".parse::<CompressLevel>().unwrap(),
            CompressLevel::Aggressive
        );
    }

    #[test]
    fn level_rejects_unknown_value() {
        assert!("turbo".parse::<CompressLevel>().is_err());
    }

    #[test]
    fn level_round_trips_as_str() {
        assert_eq!(CompressLevel::Safe.as_str(), "safe");
        assert_eq!(CompressLevel::Aggressive.as_str(), "aggressive");
    }

    #[test]
    fn safe_level_skips_aggressive_rules() {
        // leveled log: aggressive would filter INFO lines, safe must not
        let mut log = String::new();
        for i in 0..15 {
            log.push_str(&format!("2026-06-11 INFO item {i}\n"));
        }
        let safe = compress(&log, CompressLevel::Safe);
        assert!(safe.contains("INFO item 3"));
        let aggressive = compress(&log, CompressLevel::Aggressive);
        assert!(!aggressive.contains("INFO item 3"));
    }

    #[test]
    fn prose_survives_both_levels_unchanged() {
        let prose = "Compiling cartoon v0.1.0\nFinished release in 2.41s";
        assert_eq!(compress(prose, CompressLevel::Safe), prose);
        assert_eq!(compress(prose, CompressLevel::Aggressive), prose);
    }
}
