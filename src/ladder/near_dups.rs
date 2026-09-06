use regex::Regex;
use std::sync::OnceLock;

const MIN_RUN: usize = 3;

fn normalize(line: &str) -> String {
    static NUM: OnceLock<Regex> = OnceLock::new();
    static HEX: OnceLock<Regex> = OnceLock::new();
    let hex = HEX.get_or_init(|| Regex::new(r"\b[0-9a-fA-F]{8,}\b").unwrap());
    let num = NUM.get_or_init(|| Regex::new(r"\d+").unwrap());
    num.replace_all(&hex.replace_all(line, "#"), "#")
        .into_owned()
}

/// Collapse runs of >= MIN_RUN consecutive lines that are identical after
/// numeric/id normalization into the first line + `  (xN similar)`.
/// Shorter runs are emitted verbatim.
pub fn collapse_near_dups(text: &str) -> String {
    let sep = super::safe::line_sep(text);
    let mut out: Vec<String> = Vec::new();
    let mut run: Vec<String> = Vec::new();
    let mut run_norm = String::new();

    fn flush(run: &mut Vec<String>, out: &mut Vec<String>) {
        if run.len() >= MIN_RUN {
            out.push(run[0].clone());
            out.push(format!("  (x{} similar)", run.len()));
        } else {
            out.append(run);
        }
        run.clear();
    }

    for raw in text.lines() {
        // Diagnostics are data, not noise: emit verbatim, never template.
        if super::diagnostics::is_diagnostic_line(raw) {
            flush(&mut run, &mut out);
            out.push(raw.to_string());
            continue;
        }
        let norm = normalize(raw);
        if !run.is_empty() && norm == run_norm {
            run.push(raw.to_string());
        } else {
            flush(&mut run, &mut out);
            run_norm = norm;
            run.push(raw.to_string());
        }
    }
    flush(&mut run, &mut out);
    out.join(sep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_numbered_run() {
        let input = "copied chunk 1 of 500\ncopied chunk 2 of 500\ncopied chunk 3 of 500\ncopied chunk 4 of 500\ndone";
        assert_eq!(
            collapse_near_dups(input),
            "copied chunk 1 of 500\n  (x4 similar)\ndone"
        );
    }

    #[test]
    fn run_of_two_kept_verbatim() {
        let input = "retry 1\nretry 2\nok";
        assert_eq!(collapse_near_dups(input), input);
    }

    #[test]
    fn distinct_lines_unchanged() {
        let input = "alpha\nbeta\ngamma";
        assert_eq!(collapse_near_dups(input), input);
    }

    #[test]
    fn diagnostics_differing_only_by_line_are_kept() {
        let input = "src/a.c:10:5: error: expected ';'\nsrc/a.c:20:5: error: expected ';'\nsrc/a.c:30:5: error: expected ';'\ndone";
        assert_eq!(collapse_near_dups(input), input);
    }

    #[test]
    fn hex_ids_normalize() {
        let input =
            "pulled layer a1b2c3d4e5f6\npulled layer 998877665544\npulled layer deadbeef0123\nok";
        assert_eq!(
            collapse_near_dups(input),
            "pulled layer a1b2c3d4e5f6\n  (x3 similar)\nok"
        );
    }
}
