use regex::Regex;
use std::borrow::Cow;
use std::sync::OnceLock;

/// Strip ANSI escape sequences. Pure; returns a new String.
/// Covers full CSI (private/intermediate bytes, `38:2:r:g:b` truecolor),
/// OSC strings terminated by BEL or ST (an OSC 8 hyperlink loses its URL
/// and keeps the link text, which sits between the two OSCs), charset
/// designation (`ESC ( B`) and keypad mode (`ESC =`, `ESC >`).
pub fn strip_ansi(text: &str) -> String {
    static ANSI: OnceLock<Regex> = OnceLock::new();
    let ansi = ANSI.get_or_init(|| {
        Regex::new(
            r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-?]*[ -/]*[@-~]|\x1b[()][0-9A-Za-z]|\x1b[=>]",
        )
        .unwrap()
    });
    ansi.replace_all(text, "").into_owned()
}

/// The line terminator to re-join with: CRLF input stays CRLF (Windows tools,
/// some CI logs) so the ladder never silently rewrites line endings.
pub(crate) fn line_sep(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Split into `(content, terminator)` pairs. Each line keeps its own
/// terminator (`\r\n`, `\n`, or `""` for an unterminated last line) so the
/// safe rules re-join mixed line endings exactly as they came in.
pub(crate) fn split_lines(text: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        match rest.find('\n') {
            Some(i) if i > 0 && rest.as_bytes()[i - 1] == b'\r' => {
                out.push((&rest[..i - 1], &rest[i - 1..=i]));
                rest = &rest[i + 1..];
            }
            Some(i) => {
                out.push((&rest[..i], &rest[i..=i]));
                rest = &rest[i + 1..];
            }
            None => {
                out.push((rest, ""));
                rest = "";
            }
        }
    }
    out
}

/// Output of a safe rule: lines with their own terminators.
pub(crate) struct Lines<'a> {
    sep: &'static str,
    items: Vec<(Cow<'a, str>, &'a str)>,
}

impl<'a> Lines<'a> {
    pub(crate) fn new(text: &str) -> Self {
        Lines {
            sep: line_sep(text),
            items: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, line: impl Into<Cow<'a, str>>, term: &'a str) {
        self.items.push((line.into(), term));
    }

    /// Append a disclosure line terminated by `term`. When the line before
    /// it was the unterminated last line, that line gets the document's
    /// terminator so the marker starts on its own line.
    pub(crate) fn push_marker(&mut self, marker: String, term: &'a str) {
        if let Some((_, t)) = self.items.last_mut() {
            if t.is_empty() {
                *t = self.sep;
            }
        }
        self.items.push((marker.into(), term));
    }

    pub(crate) fn join(self) -> String {
        let mut s = String::new();
        for (line, term) in self.items {
            s.push_str(&line);
            s.push_str(term);
        }
        s
    }
}

/// Collapse runs of empty lines to one. Only truly empty lines count:
/// whitespace-only lines and trailing whitespace are content (`git diff`
/// context lines, whitespace-only changes) and are kept verbatim.
pub fn collapse_blanks(text: &str) -> String {
    let mut out = Lines::new(text);
    let mut blank_run = 0usize;
    for (line, term) in split_lines(text) {
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push(line, term);
    }
    out.join()
}

/// Collapse exact consecutive duplicate lines to `line` + `  (xN)`.
/// Blank lines participate in tracking, so duplicates separated by a
/// blank are intentionally NOT collapsed across the gap.
pub fn collapse_repeats(text: &str) -> String {
    let mut out = Lines::new(text);
    let mut prev: Option<&str> = None;
    let mut repeat = 0usize;
    let mut last_term = "";
    for (line, term) in split_lines(text) {
        if prev == Some(line) {
            repeat += 1;
            last_term = term;
            continue;
        }
        if repeat > 0 {
            out.push_marker(format!("  (x{})", repeat + 1), last_term);
            repeat = 0;
        }
        out.push(line, term);
        prev = Some(line);
    }
    if repeat > 0 {
        out.push_marker(format!("  (x{})", repeat + 1), last_term);
    }
    out.join()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_codes() {
        assert_eq!(strip_ansi("\x1b[32mPASS\x1b[0m ok"), "PASS ok");
    }

    #[test]
    fn strips_osc8_hyperlink_keeping_text() {
        // gcc: `-Wunused-variable` wrapped in an OSC 8 link to the docs
        let s = "warning: unused [\x1b]8;;https://gcc.gnu.org/onlinedocs/gcc/Warning-Options.html#index-Wunused-variable\x07-Wunused-variable\x1b]8;;\x07]";
        assert_eq!(strip_ansi(s), "warning: unused [-Wunused-variable]");
        let st = "\x1b]8;id=1;file:///tmp/x\x1b\\x.rs\x1b]8;;\x1b\\ ok";
        assert_eq!(strip_ansi(st), "x.rs ok");
    }

    #[test]
    fn strips_window_title_charset_keypad_and_truecolor() {
        assert_eq!(strip_ansi("\x1b]0;title\x07a"), "a");
        assert_eq!(strip_ansi("\x1b(Bplain\x1b)0"), "plain");
        assert_eq!(strip_ansi("\x1b=x\x1b>"), "x");
        assert_eq!(strip_ansi("\x1b[38:2::255:0:0mred\x1b[m"), "red");
        assert_eq!(strip_ansi("\x1b[38;2;255;0;0mred\x1b[0m"), "red");
        assert_eq!(strip_ansi("\x1b[?25l\x1b[2Kline\x1b[1G"), "line");
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
    fn keeps_trailing_whitespace() {
        // a `git diff` context line and a whitespace-only change
        let diff = " fn main() {   \n-\tx\n+\tx  \n \n";
        assert_eq!(collapse_blanks(diff), diff);
    }

    #[test]
    fn whitespace_only_lines_are_not_blank() {
        assert_eq!(collapse_blanks("a\n  \n\t\n\n\nb"), "a\n  \n\t\n\nb");
    }

    #[test]
    fn crlf_input_keeps_crlf_output() {
        assert_eq!(collapse_blanks("a\r\n\r\n\r\nb"), "a\r\n\r\nb");
        assert_eq!(collapse_repeats("x\r\nx\r\ny"), "x\r\n  (x2)\r\ny");
    }

    #[test]
    fn mixed_line_endings_are_preserved() {
        let mixed = "a\nb\r\nc\nd\n";
        assert_eq!(collapse_blanks(mixed), mixed);
        assert_eq!(collapse_repeats(mixed), mixed);
        assert_eq!(collapse_repeats("x\nx\r\ny\n"), "x\n  (x2)\r\ny\n");
    }

    #[test]
    fn trailing_newline_is_kept() {
        assert_eq!(collapse_repeats("a\na\n"), "a\n  (x2)\n");
        assert_eq!(collapse_repeats("a\na"), "a\n  (x2)");
    }
}
