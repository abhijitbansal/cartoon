use regex::Regex;
use std::sync::OnceLock;

/// Strip ANSI escape sequences. Pure; returns a new String.
pub fn strip_ansi(text: &str) -> String {
    static ANSI: OnceLock<Regex> = OnceLock::new();
    let ansi = ANSI.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap());
    ansi.replace_all(text, "").into_owned()
}

/// The line terminator to re-join with: CRLF input stays CRLF (Windows tools,
/// some CI logs) so the safe tier never silently rewrites line endings.
pub(crate) fn line_sep(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Collapse runs of blank lines (after trailing space/tab trim) to one.
pub fn collapse_blanks(text: &str) -> String {
    let sep = line_sep(text);
    let mut out: Vec<&str> = Vec::new();
    let mut blank_run = 0usize;
    for raw in text.lines() {
        let line = raw.trim_end_matches([' ', '\t']);
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push(line);
    }
    out.join(sep)
}

/// Collapse exact consecutive duplicate lines to `line` + `  (xN)`.
/// Blank lines participate in tracking, so duplicates separated by a
/// blank are intentionally NOT collapsed across the gap.
pub fn collapse_repeats(text: &str) -> String {
    let sep = line_sep(text);
    let mut out: Vec<String> = Vec::new();
    let mut prev: Option<&str> = None;
    let mut repeat = 0usize;
    for line in text.lines() {
        if prev == Some(line) {
            repeat += 1;
            continue;
        }
        if repeat > 0 {
            out.push(format!("  (x{})", repeat + 1));
            repeat = 0;
        }
        out.push(line.to_string());
        prev = Some(line);
    }
    if repeat > 0 {
        out.push(format!("  (x{})", repeat + 1));
    }
    out.join(sep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_codes() {
        assert_eq!(strip_ansi("\x1b[32mPASS\x1b[0m ok"), "PASS ok");
    }

    #[test]
    fn collapses_blank_runs_to_one() {
        assert_eq!(collapse_blanks("a\n\n\n\nb"), "a\n\nb");
    }

    #[test]
    fn dedupes_identical_consecutive_lines() {
        assert_eq!(
            collapse_repeats("same\nsame\nsame\nend"),
            "same\n  (x3)\nend"
        );
    }

    #[test]
    fn does_not_dedupe_across_blank_gap() {
        // blanks update prev: "x\n\nx" stays three lines
        let composed = collapse_repeats(&collapse_blanks("x\n\n\nx"));
        assert_eq!(composed, "x\n\nx");
    }

    #[test]
    fn plain_prose_unchanged() {
        let prose = "first line\nsecond line\nthird line";
        assert_eq!(
            collapse_repeats(&collapse_blanks(&strip_ansi(prose))),
            prose
        );
    }

    #[test]
    fn trims_trailing_whitespace() {
        assert_eq!(collapse_blanks("line   \nnext"), "line\nnext");
    }

    #[test]
    fn crlf_input_keeps_crlf_output() {
        assert_eq!(collapse_blanks("a\r\n\r\n\r\nb"), "a\r\n\r\nb");
        assert_eq!(collapse_repeats("x\r\nx\r\ny"), "x\r\n  (x2)\r\ny");
    }
}
