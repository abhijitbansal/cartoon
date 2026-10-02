use super::safe::{split_lines, Lines};
use regex::Regex;
use std::sync::OnceLock;

/// A progress-bar line: a run of bar glyphs (`█▓▒░`, `####`, `====>`)
/// together with a percentage or an `n/m` counter. A bare percentage is
/// not enough: `GPU0 util: 12%` / `GPU1 util: 99%` are distinct data rows.
pub(super) fn is_bar_line(line: &str) -> bool {
    static BAR: OnceLock<Regex> = OnceLock::new();
    static AMOUNT: OnceLock<Regex> = OnceLock::new();
    let bar = BAR.get_or_init(|| Regex::new(r"[█▓▒░]{2,}|[#=]{4,}>?|\[[=\-#>\s]{4,}\]").unwrap());
    let amount = AMOUNT.get_or_init(|| Regex::new(r"\d{1,3}(?:\.\d+)?\s*%|\d+\s*/\s*\d+").unwrap());
    bar.is_match(line) && amount.is_match(line)
}

/// Digits and bar glyphs removed: two frames of one progress bar
/// normalize to the same template; two different bars (other labels,
/// e.g. per-layer download bars) do not.
fn template(line: &str) -> String {
    line.chars()
        .filter(|c| {
            !matches!(
                c,
                '0'..='9' | '=' | '#' | '>' | '-' | '.' | ' ' | '\t' | '%' | '█' | '▓' | '▒' | '░'
            )
        })
        .collect()
}

/// The visible state of a line redrawn with `\r`: its last non-empty
/// segment. Returns `None` when the line was not redrawn (no CR, or only
/// trailing CRs such as a `\r\r\n` ending), in which case it is kept as is.
pub(super) fn redrawn_final(line: &str) -> Option<&str> {
    let body = line.trim_end_matches('\r');
    if !body.contains('\r') {
        return None;
    }
    Some(body.rsplit('\r').find(|s| !s.is_empty()).unwrap_or(""))
}

/// Keep only the final state of progress output:
/// 1. A line redrawn with `\r` keeps its last non-empty `\r` segment.
/// 2. Runs of >= 2 consecutive progress-bar frames of one bar (same
///    template) keep the last frame plus a `(xN progress frames)` marker.
///
/// Lines that merely share a template after digit stripping are never
/// folded unless they draw a bar, and a redrawn line never absorbs a
/// neighbouring line.
pub fn collapse_progress(text: &str) -> String {
    let mut out = Lines::new(text);
    // (frame, terminator, template, frames folded into it)
    let mut pending: Option<(&str, &str, String, usize)> = None;
    fn flush<'a>(pending: &mut Option<(&'a str, &'a str, String, usize)>, out: &mut Lines<'a>) {
        if let Some((line, term, _, n)) = pending.take() {
            out.push(line, term);
            if n > 1 {
                out.push_marker(format!("  (x{n} progress frames, last kept)"), term);
            }
        }
    }
    for (raw, term) in split_lines(text) {
        let line = redrawn_final(raw).unwrap_or(raw);
        if is_bar_line(line) {
            let tpl = template(line);
            match &mut pending {
                Some((p, t, ptpl, n)) if *ptpl == tpl => {
                    *p = line;
                    *t = term;
                    *n += 1;
                }
                _ => {
                    flush(&mut pending, &mut out);
                    pending = Some((line, term, tpl, 1));
                }
            }
            continue;
        }
        flush(&mut pending, &mut out);
        out.push(line, term);
    }
    flush(&mut pending, &mut out);
    out.join()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_text_after_last_carriage_return() {
        assert_eq!(collapse_progress("step 1\rstep 2\rdone"), "done");
    }

    #[test]
    fn plain_percentage_lines_are_kept() {
        // without `\r` or a bar these are indistinguishable from data rows
        let input = "Downloading 10%\nDownloading 55%\nDownloading 100%\nresolved";
        assert_eq!(collapse_progress(input), input);
    }

    #[test]
    fn distinct_lines_sharing_a_template_survive() {
        let input = "GPU0 util: 12%\nGPU1 util: 99%\nGPU2 util: 7%";
        assert_eq!(collapse_progress(input), input);
    }

    #[test]
    fn collapses_bar_run_to_last_with_disclosure() {
        let input = "[====>     ] 4/10\n[========> ] 8/10\n[==========] 10/10\nok";
        assert_eq!(
            collapse_progress(input),
            "[==========] 10/10\n  (x3 progress frames, last kept)\nok"
        );
    }

    #[test]
    fn collapses_block_glyph_bar() {
        let input = "train ███░░░░ 40%\ntrain █████░░ 70%\ntrain ███████ 100%\n";
        assert_eq!(
            collapse_progress(input),
            "train ███████ 100%\n  (x3 progress frames, last kept)\n"
        );
    }

    #[test]
    fn different_bars_are_not_folded_together() {
        let input = "layer a1: [===>  ] 30%\nlayer b2: [=>    ] 10%";
        assert_eq!(collapse_progress(input), input);
    }

    #[test]
    fn plain_prose_unchanged() {
        let prose = "compiling foo\ncompiling bar\nfinished";
        assert_eq!(collapse_progress(prose), prose);
    }

    #[test]
    fn distinct_percentage_rows_are_not_collapsed() {
        let table = "src/a.py  10  2  80%\nsrc/b.py  20  0  100%\nsrc/c.py  5   5  0%\nTOTAL     35  7  80%";
        assert_eq!(collapse_progress(table), table);
    }

    #[test]
    fn carriage_return_frames_collapse_regardless_of_text() {
        assert_eq!(
            collapse_progress("Fetching 10%\rUnpacking 50%\rDone 100%\nok"),
            "Done 100%\nok"
        );
    }

    #[test]
    fn cr_frame_never_absorbs_a_pending_line() {
        let input = "Coverage total: 45%\nDownloading 10%\rDownloading 100%\nok";
        assert_eq!(
            collapse_progress(input),
            "Coverage total: 45%\nDownloading 100%\nok"
        );
    }

    #[test]
    fn consecutive_redrawn_lines_each_keep_their_final_state() {
        let input = "a 10%\ra 100%\nb 10%\rb 100%";
        assert_eq!(collapse_progress(input), "a 100%\nb 100%");
    }

    #[test]
    fn trailing_carriage_returns_keep_the_line() {
        // `\r\r\n` endings used to blank every line
        let input = "one\r\r\ntwo\r\r\nthree";
        assert_eq!(collapse_progress(input), input);
        // a final unterminated `FATAL …\r` must survive
        assert_eq!(
            collapse_progress("ok\nFATAL disk full\r"),
            "ok\nFATAL disk full\r"
        );
        // last NON-EMPTY segment of a redrawn line
        assert_eq!(collapse_progress("10%\r100%\r\nnext"), "100%\r\nnext");
    }

    #[test]
    fn single_progress_line_amid_prose_is_kept() {
        let input = "coverage: 93%\nall files checked";
        assert_eq!(collapse_progress(input), "coverage: 93%\nall files checked");
    }

    #[test]
    fn mixed_line_endings_are_preserved() {
        let input = "a\nb\r\nc\n";
        assert_eq!(collapse_progress(input), input);
    }
}
