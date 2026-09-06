use regex::Regex;
use std::sync::OnceLock;

/// A line counts as a progress frame if it draws a bar segment (====>,
/// ----, ###) or carries a percentage as its main content. A percentage
/// next to several other numbers is a table row (coverage report, `df`,
/// test timings), not a progress indicator, and is never treated as one.
fn is_progress_line(line: &str) -> bool {
    static BAR: OnceLock<Regex> = OnceLock::new();
    static PCT: OnceLock<Regex> = OnceLock::new();
    let bar = BAR.get_or_init(|| Regex::new(r"(\[[=\-#>\s]{4,}\])|([=#]{4,}>)").unwrap());
    let pct = PCT.get_or_init(|| Regex::new(r"\d{1,3}\s*%").unwrap());
    bar.is_match(line) || (pct.is_match(line) && number_runs(line) <= MAX_NUMBERS_IN_FRAME)
}

/// A percentage frame may carry the percentage plus one more number
/// (`3/10 (30%)`); a third number means a data row.
const MAX_NUMBERS_IN_FRAME: usize = 2;

fn number_runs(line: &str) -> usize {
    let mut runs = 0;
    let mut in_digits = false;
    for c in line.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                runs += 1;
                in_digits = true;
            }
        } else {
            in_digits = false;
        }
    }
    runs
}

/// Digits and bar glyphs removed: two frames of one progress indicator
/// normalize to the same template; two rows of a coverage table (different
/// file names) do not, so distinct data is never collapsed as "progress".
fn template(line: &str) -> String {
    line.chars()
        .filter(|c| !matches!(c, '0'..='9' | '=' | '#' | '>' | '-' | '.' | ' ' | '\t'))
        .collect()
}

/// Keep only the final state of progress output:
/// 1. Within each physical line, keep the text after the last `\r`.
/// 2. For runs of >=2 consecutive progress *frames* — lines that redrew via
///    `\r`, or percentage/bar lines sharing one template — keep only the last.
pub fn collapse_progress(text: &str) -> String {
    let sep = super::safe::line_sep(text);
    let mut out: Vec<&str> = Vec::new();
    let mut pending: Option<(&str, String, bool)> = None; // (line, template, had_cr)
    for raw in text.lines() {
        let had_cr = raw.contains('\r');
        let line = raw.rsplit('\r').next().unwrap_or(raw);
        if is_progress_line(line) {
            let tpl = template(line);
            match &pending {
                // Same indicator redrawing: replace the pending frame.
                Some((_, ptpl, pcr)) if had_cr || *pcr || *ptpl == tpl => {
                    pending = Some((line, tpl, had_cr));
                }
                // A different %-bearing line: flush the old one, keep this.
                Some((p, _, _)) => {
                    out.push(p);
                    pending = Some((line, tpl, had_cr));
                }
                None => pending = Some((line, tpl, had_cr)),
            }
            continue;
        }
        if let Some((p, _, _)) = pending.take() {
            out.push(p);
        }
        out.push(line);
    }
    if let Some((p, _, _)) = pending {
        out.push(p);
    }
    out.join(sep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_text_after_last_carriage_return() {
        assert_eq!(collapse_progress("step 1\rstep 2\rdone"), "done");
    }

    #[test]
    fn collapses_percentage_run_to_last() {
        let input = "Downloading 10%\nDownloading 55%\nDownloading 100%\nresolved";
        assert_eq!(collapse_progress(input), "Downloading 100%\nresolved");
    }

    #[test]
    fn collapses_bar_run_to_last() {
        let input = "[====>     ] 4/10\n[========> ] 8/10\n[==========] 10/10\nok";
        assert_eq!(collapse_progress(input), "[==========] 10/10\nok");
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
    fn single_progress_line_amid_prose_is_kept() {
        let input = "coverage: 93%\nall files checked";
        assert_eq!(collapse_progress(input), "coverage: 93%\nall files checked");
    }
}
