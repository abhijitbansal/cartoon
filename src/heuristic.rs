use regex::Regex;
use std::sync::OnceLock;

/// Lossy compression for unstructured output: strip ANSI, trim trailing
/// whitespace, collapse blank runs, collapse repeated identical lines.
pub fn compress(text: &str) -> String {
    static ANSI: OnceLock<Regex> = OnceLock::new();
    let ansi = ANSI.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap());
    let stripped = ansi.replace_all(text, "");

    let mut out: Vec<String> = Vec::new();
    let mut prev: Option<String> = None;
    let mut repeat = 0usize;
    let mut blank_run = 0usize;
    for raw in stripped.lines() {
        let line = raw.trim_end().to_string();
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        if prev.as_deref() == Some(line.as_str()) {
            repeat += 1;
            continue;
        }
        if repeat > 0 {
            out.push(format!("  (x{})", repeat + 1));
            repeat = 0;
        }
        out.push(line.clone());
        prev = Some(line);
    }
    if repeat > 0 {
        out.push(format!("  (x{})", repeat + 1));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_codes() {
        assert_eq!(compress("\x1b[32mPASS\x1b[0m ok"), "PASS ok");
    }

    #[test]
    fn collapses_blank_runs_to_one() {
        assert_eq!(compress("a\n\n\n\nb"), "a\n\nb");
    }

    #[test]
    fn dedupes_identical_consecutive_lines() {
        assert_eq!(compress("same\nsame\nsame\nend"), "same\n  (x3)\nend");
    }

    #[test]
    fn trims_trailing_whitespace() {
        assert_eq!(compress("line   \nnext"), "line\nnext");
    }
}
