use regex::Regex;
use std::sync::OnceLock;

const MIN_RUN: usize = 3;
/// Distinct values listed per varying slot before the rest is counted.
const MAX_SLOT_VALUES: usize = 6;

/// Numeric slots: long hex ids first (so `a1b2c3d4` is one slot), then
/// digit runs. Leftmost-first alternation keeps the slot order stable.
fn slot_pat() -> &'static Regex {
    static PAT: OnceLock<Regex> = OnceLock::new();
    PAT.get_or_init(|| Regex::new(r"\b[0-9a-fA-F]{8,}\b|\d+").unwrap())
}

fn normalize(line: &str) -> String {
    slot_pat().replace_all(line, "#").into_owned()
}

/// What a varying slot holds across a run.
enum Slot {
    /// Same value on every line: keep it literally.
    Constant,
    /// Counter or id (strictly increasing, or a hex id): show the first.
    Index,
    /// Data that differs between lines: list the distinct values.
    Values(Vec<String>),
}

fn classify(values: &[&str]) -> Slot {
    if values.iter().all(|v| *v == values[0]) {
        return Slot::Constant;
    }
    if values
        .iter()
        .any(|v| v.len() >= 8 && !v.bytes().all(|b| b.is_ascii_digit()))
    {
        return Slot::Index;
    }
    let nums: Option<Vec<u128>> = values.iter().map(|v| v.parse().ok()).collect();
    if let Some(nums) = nums {
        if nums.windows(2).all(|w| w[0] < w[1]) {
            return Slot::Index;
        }
    }
    let mut distinct: Vec<String> = Vec::new();
    for v in values {
        if !distinct.iter().any(|d| d == v) {
            distinct.push(v.to_string());
        }
    }
    Slot::Values(distinct)
}

/// The run's first line with each data slot replaced by `{v1,v2,…}`.
fn summarize(run: &[String]) -> String {
    let slots: Vec<Vec<&str>> = run
        .iter()
        .map(|l| slot_pat().find_iter(l).map(|m| m.as_str()).collect())
        .collect();
    let first = &run[0];
    let mut out = String::new();
    let mut last = 0;
    for (k, m) in slot_pat().find_iter(first).enumerate() {
        let column: Vec<&str> = slots.iter().map(|s| s[k]).collect();
        out.push_str(&first[last..m.start()]);
        match classify(&column) {
            Slot::Constant | Slot::Index => out.push_str(m.as_str()),
            Slot::Values(vals) => {
                let shown = vals.len().min(MAX_SLOT_VALUES);
                out.push('{');
                out.push_str(&vals[..shown].join(","));
                if vals.len() > shown {
                    out.push_str(&format!(",…+{}", vals.len() - shown));
                }
                out.push('}');
            }
        }
        last = m.end();
    }
    out.push_str(&first[last..]);
    out
}

/// Collapse runs of >= MIN_RUN consecutive lines that are identical after
/// numeric/id normalization into one summary line + `  (xN similar)`.
/// Counters and ids show their first value; slots that carry differing
/// data list their distinct values (`status {200,500,401}`), so a
/// collapse never hides which values occurred. Shorter runs are emitted
/// verbatim, and diagnostics and error lines are never templated.
pub fn collapse_near_dups(text: &str) -> String {
    let sep = super::safe::line_sep(text);
    let mut out: Vec<String> = Vec::new();
    let mut run: Vec<String> = Vec::new();
    let mut run_norm = String::new();

    fn flush(run: &mut Vec<String>, out: &mut Vec<String>) {
        if run.len() >= MIN_RUN {
            out.push(summarize(run));
            out.push(format!("  (x{} similar)", run.len()));
        } else {
            out.append(run);
        }
        run.clear();
    }

    for raw in text.lines() {
        // Diagnostics and failures are data, not noise: emit verbatim.
        if super::diagnostics::is_diagnostic_line(raw) || super::is_error_line(raw) {
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

    #[test]
    fn error_lines_are_never_templated() {
        let input: String = (0..10)
            .map(|i| {
                format!(
                    "FAILED test_v{i} - assert {} == 200\n",
                    [200, 500, 401][i % 3]
                )
            })
            .collect();
        assert_eq!(collapse_near_dups(&input), input.trim_end());
    }

    #[test]
    fn varying_data_slots_list_their_distinct_values() {
        let input: String = (0..10)
            .map(|i| format!("GET /api/v{i} -> status {}\n", [200, 500, 401][i % 3]))
            .collect();
        assert_eq!(
            collapse_near_dups(&input),
            "GET /api/v0 -> status {200,500,401}\n  (x10 similar)"
        );
    }

    #[test]
    fn many_distinct_values_are_capped_with_a_count() {
        let input: String = (0..10)
            .map(|i| format!("request took {}ms\n", [9, 3, 7, 1, 8, 2, 6, 4, 5, 0][i]))
            .collect();
        assert_eq!(
            collapse_near_dups(&input),
            "request took {9,3,7,1,8,2,…+4}ms\n  (x10 similar)"
        );
    }
}
