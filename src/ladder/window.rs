use regex::Regex;
use std::sync::OnceLock;

const HEAD_LINES: usize = 15;
const TAIL_LINES: usize = 15;
const ERROR_CONTEXT: usize = 3;
const MIN_TOTAL_LINES: usize = 80; // below this, windowing saves too little

fn is_error_line(line: &str) -> bool {
    static PAT: OnceLock<Regex> = OnceLock::new();
    let pat = PAT.get_or_init(|| {
        // Keyword list plus identifier-glued names (`KeyError:`,
        // `NullPointerException`) that `\b…\b` alone would miss.
        Regex::new(
            r"(?i)\b(error|err!|fail|failed|failure|exception|panic|fatal|traceback)\b|[A-Za-z_][A-Za-z0-9_]*(Error|Exception)\b",
        )
        .unwrap()
    });
    pat.is_match(line)
}

/// Keep head + tail + windows around error keywords; replace elided spans
/// with `  (skipped K lines, see raw_log)`.
pub fn window_errors(text: &str) -> String {
    let sep = super::safe::line_sep(text);
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < MIN_TOTAL_LINES {
        return text.to_string();
    }
    let mut keep = vec![false; lines.len()];
    for k in keep.iter_mut().take(HEAD_LINES.min(lines.len())) {
        *k = true;
    }
    for k in keep.iter_mut().rev().take(TAIL_LINES.min(lines.len())) {
        *k = true;
    }
    for (i, line) in lines.iter().enumerate() {
        if is_error_line(line) {
            let lo = i.saturating_sub(ERROR_CONTEXT);
            let hi = (i + ERROR_CONTEXT).min(lines.len() - 1);
            for k in keep.iter_mut().take(hi + 1).skip(lo) {
                *k = true;
            }
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    for (i, line) in lines.iter().enumerate() {
        if keep[i] {
            if skipped > 0 {
                out.push(format!("  (skipped {skipped} lines, see raw_log)"));
                skipped = 0;
            }
            out.push(line.to_string());
        } else {
            skipped += 1;
        }
    }
    if skipped > 0 {
        out.push(format!("  (skipped {skipped} lines, see raw_log)"));
    }
    out.join(sep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big_log() -> String {
        let mut s = String::new();
        for i in 0..200 {
            if i == 100 {
                s.push_str("step 100: ERROR widget exploded\n");
            } else {
                s.push_str(&format!("step {i}: ok\n"));
            }
        }
        s
    }

    #[test]
    fn keeps_head_tail_and_error_window() {
        let out = window_errors(&big_log());
        assert!(out.contains("step 0: ok"));
        assert!(out.contains("step 199: ok"));
        assert!(out.contains("ERROR widget exploded"));
        assert!(out.contains("step 98: ok")); // error context
        assert!(out.contains("skipped"));
        assert!(!out.contains("step 50: ok")); // elided middle
    }

    #[test]
    fn camelcase_exception_names_anchor_a_window() {
        let mut lines: Vec<String> = (0..120).map(|i| format!("line {i}")).collect();
        lines[60] = "KeyError: 'email'".into();
        lines[61] = "java.lang.NullPointerException: x".into();
        let out = window_errors(&lines.join("\n"));
        assert!(out.contains("KeyError"), "{out}");
        assert!(out.contains("NullPointerException"), "{out}");
        assert!(out.contains("line 58"), "context kept: {out}");
        assert!(!out.contains("line 30"), "middle elided: {out}");
    }

    #[test]
    fn small_input_unchanged() {
        let small = "a\nb\nc";
        assert_eq!(window_errors(small), small);
    }
}
