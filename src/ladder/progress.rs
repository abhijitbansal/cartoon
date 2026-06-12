use regex::Regex;
use std::sync::OnceLock;

/// A line counts as a progress frame if it contains a percentage or a
/// bar segment (====>, ----, ###) alongside a number.
fn is_progress_line(line: &str) -> bool {
    static PAT: OnceLock<Regex> = OnceLock::new();
    let pat =
        PAT.get_or_init(|| Regex::new(r"(\d{1,3}\s*%)|(\[[=\-#>\s]{4,}\])|([=#]{4,}>)").unwrap());
    pat.is_match(line)
}

/// Keep only the final state of progress output:
/// 1. Within each physical line, keep the text after the last `\r`.
/// 2. For runs of >=2 consecutive progress lines, keep only the last.
pub fn collapse_progress(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut pending_progress: Option<&str> = None;
    for raw in text.lines() {
        let line = raw.rsplit('\r').next().unwrap_or(raw);
        if is_progress_line(line) {
            pending_progress = Some(line);
            continue;
        }
        if let Some(p) = pending_progress.take() {
            out.push(p);
        }
        out.push(line);
    }
    if let Some(p) = pending_progress {
        out.push(p);
    }
    out.join("\n")
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
    fn single_progress_line_amid_prose_is_kept() {
        let input = "coverage: 93%\nall files checked";
        assert_eq!(collapse_progress(input), "coverage: 93%\nall files checked");
    }
}
