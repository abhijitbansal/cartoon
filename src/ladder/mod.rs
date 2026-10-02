//! Tiered compression ladder for generic (no-adapter) CLI output.
//! Rules are pure functions applied in a fixed order; `CompressLevel`
//! selects the subset. See docs/superpowers/specs/2026-06-11-*.md.
//!
//! Rule order (fixed):
//! `strip_ansi -> collapse_progress -> collapse_blanks -> collapse_repeats` (safe)
//! `-> filter_levels -> collapse_near_dups -> extract_diagnostics -> window_errors` (aggressive)
//! Phase 2 inserts `drain` after `collapse_near_dups`. Phase 3 appends `model_score` last.

mod diagnostics;
mod errors;
mod levels;
mod near_dups;
mod progress;
mod safe;
mod window;

pub use diagnostics::extract_diagnostics;
pub use errors::is_error_line;
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
            let folded = collapse_near_dups(&filter_levels(&safe));
            // Window the body only: the diagnostics table is appended after
            // so windowing can never cut it into orphan rows.
            match diagnostics::split_diagnostics(&folded) {
                None => window_errors(&folded),
                Some((body, table)) => {
                    let body = window_errors(&body);
                    if body.trim().is_empty() {
                        table
                    } else {
                        format!("{body}{}{table}", safe::line_sep(&folded))
                    }
                }
            }
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
    fn three_same_message_diagnostics_all_reach_the_table() {
        let mut log = String::new();
        for i in 0..90 {
            log.push_str(&format!("compiling unit {i}\n"));
        }
        for l in [10, 20, 30] {
            log.push_str(&format!("src/a.c:{l}:5: error: expected ';'\n"));
        }
        let out = compress(&log, CompressLevel::Aggressive);
        for l in [10, 20, 30] {
            assert!(out.contains(&format!("src/a.c:{l}:5")), "{out}");
        }
    }

    #[test]
    fn prose_survives_both_levels_unchanged() {
        let prose = "Compiling cartoon v0.1.0\nFinished release in 2.41s";
        assert_eq!(compress(prose, CompressLevel::Safe), prose);
        assert_eq!(compress(prose, CompressLevel::Aggressive), prose);
    }

    #[test]
    fn windowing_never_cuts_the_diagnostics_table() {
        // 120 body lines + 30 diagnostics: windowing the combined text used
        // to elide the middle of the table, leaving orphan rows.
        let mut log = String::new();
        for i in 0..120 {
            log.push_str(&format!("step {i}: building target {}\n", i * 7 % 13));
        }
        for l in 0..30 {
            log.push_str(&format!(
                "src/m{l}.c:{l}:1: warning: unused variable 'v{l}'\n"
            ));
        }
        let out = compress(&log, CompressLevel::Aggressive);
        assert!(out.contains("diagnostics[30]{loc,severity,msg}:"), "{out}");
        for l in 0..30 {
            assert!(
                out.contains(&format!("src/m{l}.c:{l}:1")),
                "row {l} lost: {out}"
            );
        }
        let table_at = out.find("diagnostics[30]").unwrap();
        assert!(!out[table_at..].contains("skipped"), "table cut: {out}");
    }

    /// Tiny deterministic PRNG (no dev-dependency needed for the property test).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
        fn pick(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    fn random_line(r: &mut Lcg) -> String {
        let n = r.pick(100);
        match r.pick(16) {
            0 => format!("GPU{} util: {n}%", r.pick(4)),
            1 => format!("Downloading {n}%"),
            2 => format!(
                "[{}{}] {}/10",
                "=".repeat(r.pick(6) + 4),
                " ".repeat(r.pick(4)),
                r.pick(10)
            ),
            3 => format!(
                "train {}{} {n}%",
                "█".repeat(r.pick(5) + 2),
                "░".repeat(r.pick(5))
            ),
            4 => format!("step {n}\rstep {}", n + 1),
            5 => format!("{n}%\r{}%\r", n + 1),
            6 => format!("trailing space {n}   "),
            7 => " \t ".into(),
            8 | 9 => String::new(),
            10 | 11 => "same line".into(),
            12 => format!("\x1b[31mred {n}\x1b[0m and \x1b]8;;http://x/{n}\x07link\x1b]8;;\x07"),
            13 => format!("FATAL code {n}\r"),
            14 => format!("Coverage total: {n}%"),
            _ => format!("ordinary output {n}"),
        }
    }

    #[test]
    fn safe_tier_property_every_line_survives_or_is_disclosed() {
        for seed in 0..400u64 {
            let mut r = Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) + 1);
            let mut text = String::new();
            let mut inputs = Vec::new();
            let count = r.pick(40) + 1;
            for i in 0..count {
                let line = random_line(&mut r);
                inputs.push(line.clone());
                text.push_str(&line);
                // the last line is sometimes unterminated
                if i + 1 < count || r.pick(2) == 0 {
                    text.push_str(["\n", "\r\n", "\r\r\n"][r.pick(3)]);
                }
            }
            let out = compress(&text, CompressLevel::Safe);
            let out_lines: Vec<&str> = out.split('\n').map(|l| l.trim_end_matches('\r')).collect();
            for raw in &inputs {
                let stripped = strip_ansi(raw);
                let line = progress::redrawn_final(&stripped)
                    .unwrap_or(&stripped)
                    .trim_end_matches('\r');
                if line.trim().is_empty() {
                    continue;
                }
                let kept = out_lines.contains(&line);
                let disclosed = progress::is_bar_line(line) && out.contains("progress frames");
                assert!(
                    kept || disclosed,
                    "seed {seed}: {line:?} lost\ninput: {text:?}\noutput: {out:?}"
                );
            }
        }
    }
}
