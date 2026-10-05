//! `--max-tokens`: a hard ceiling on what reaches the agent. Keeps the lines
//! around errors first, then the head and tail of the output, in whole
//! lines; every cut is replaced by ONE disclosed marker that is itself a
//! ready-to-run `cartoon logs grep` command. Opt-in only — with a ceiling
//! set, even passthrough output may be cut, which is exactly what the flag
//! is for ("no Bash result ever exceeds N tokens"). The ceiling covers
//! everything emitted (stdout and stderr together, see `cap_streams`). The
//! raw log is archived either way.

/// Share of the budget spent on the head; the rest goes to the tail (the
/// first failure usually sits early, the summary sits at the end).
const HEAD_SHARE_PERCENT: usize = 60;

/// Share of the budget that lines around errors may claim before the
/// head/tail split (a log of nothing but errors must still keep its tail).
const ERROR_SHARE_PERCENT: usize = 50;

/// Lines of context kept on each side of an error line.
const ERROR_CONTEXT: usize = 1;

/// Lines that must survive a cut. Deliberately local and broad: it decides
/// what an agent loses under a ceiling, so a false positive only costs a
/// little budget while a miss hides the reason a run failed.
pub fn is_error_line(line: &str) -> bool {
    if line.starts_with("E   ") || line.trim_start().starts_with("npm ERR!") {
        return true; // pytest assertion detail, npm failure block
    }
    let l = line.to_ascii_lowercase();
    [
        "error",
        "fail",
        "panic",
        "exception",
        "traceback",
        "fatal",
        "segfault",
        "segmentation fault",
        "killed",
        "denied",
        "abort",
    ]
    .iter()
    .any(|k| l.contains(k))
}

/// Cap stdout and stderr TOGETHER at `max` tokens. Each stream keeps a
/// share of the ceiling proportional to its size, so the sum stays under
/// the ceiling. Identity when both fit.
pub fn cap_streams(
    out: &str,
    err: &str,
    max: usize,
    tokenizer: &str,
    run_id: Option<&str>,
) -> (String, String) {
    let t_out = crate::stats::estimate_tokens(out, tokenizer);
    let t_err = crate::stats::estimate_tokens(err, tokenizer);
    if t_out + t_err <= max {
        return (out.to_string(), err.to_string());
    }
    if t_err == 0 {
        return (cap_tokens(out, max, tokenizer, run_id), err.to_string());
    }
    if t_out == 0 {
        return (out.to_string(), cap_tokens(err, max, tokenizer, run_id));
    }
    // A stream that fits in its proportional share keeps everything and
    // donates the rest to the other one.
    let err_share = max * t_err / (t_out + t_err);
    let (out_budget, err_budget) = if t_err <= err_share {
        (max - t_err, t_err)
    } else if t_out <= max - err_share {
        (t_out, max - t_out)
    } else {
        (max - err_share, err_share)
    };
    (
        cap_tokens(out, out_budget, tokenizer, run_id),
        cap_tokens(err, err_budget, tokenizer, run_id),
    )
}

pub fn cap_tokens(text: &str, max: usize, tokenizer: &str, run_id: Option<&str>) -> String {
    if crate::stats::estimate_tokens(text, tokenizer) <= max {
        return text.to_string();
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let n = lines.len();
    // Per-line estimates plus a one-token margin each: splitting text into
    // lines can only round token counts up, so the sum bounds the whole.
    let cost = |l: &str| crate::stats::estimate_tokens(l, tokenizer).max(1) + 1;
    let sel = run_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "--last".into());
    // Markers from most to least helpful; the first that fits the ceiling
    // with room to spare is used for every cut, so a marker can never be
    // what breaks the ceiling. Sized with `n` (the widest possible count).
    let full = |k: usize| {
        format!(
            "  (omitted {k} lines to stay under --max-tokens {max}; cartoon logs grep <pattern> {sel} -C 2)\n"
        )
    };
    let short = |k: usize| format!("  (omitted {k} lines; cartoon logs grep {sel})\n");
    let tiny = |k: usize| format!("(+{k} lines)\n");
    let styles: [&dyn Fn(usize) -> String; 3] = [&full, &short, &tiny];
    let Some((marker_for, mcost)) = styles
        .iter()
        .map(|f| (*f, cost(&f(n))))
        .find(|(_, c)| c * 2 <= max)
        .or_else(|| {
            styles
                .iter()
                .map(|f| (*f, cost(&f(n))))
                .find(|(_, c)| *c <= max)
        })
    else {
        return String::new(); // not even a marker fits: emit nothing
    };

    let mut keep = vec![false; n];
    // The cut between head and tail always needs one marker.
    let mut budget = max - mcost;

    // 1. Error lines with a little context, each new island paying for the
    //    extra marker it may create.
    let err_budget = budget * ERROR_SHARE_PERCENT / 100;
    let mut used = 0;
    for i in (0..n).filter(|&i| is_error_line(lines[i])) {
        let lo = i.saturating_sub(ERROR_CONTEXT);
        let hi = (i + ERROR_CONTEXT).min(n - 1);
        let try_window = |lo: usize, hi: usize, keep: &[bool]| {
            let new_island = !(lo..=hi).any(|j| keep[j])
                && !(lo > 0 && keep[lo - 1])
                && !(hi + 1 < n && keep[hi + 1]);
            let lines_cost: usize = (lo..=hi)
                .filter(|&j| !keep[j])
                .map(|j| cost(lines[j]))
                .sum();
            lines_cost + if new_island { mcost } else { 0 }
        };
        let (lo, hi, c) = match try_window(lo, hi, &keep) {
            c if used + c <= err_budget => (lo, hi, c),
            _ => match try_window(i, i, &keep) {
                c if used + c <= err_budget => (i, i, c),
                _ => continue,
            },
        };
        used += c;
        keep[lo..=hi].iter_mut().for_each(|k| *k = true);
    }
    budget -= used;

    // 2. Head and tail from what is left; lines already kept cost nothing.
    let head_budget = budget * HEAD_SHARE_PERCENT / 100;
    let tail_budget = budget - head_budget;
    let mut head_end = 0;
    used = 0;
    while head_end < n {
        let c = if keep[head_end] {
            0
        } else {
            cost(lines[head_end])
        };
        if used + c > head_budget {
            break;
        }
        used += c;
        keep[head_end] = true;
        head_end += 1;
    }
    let mut tail_start = n;
    used = 0;
    while tail_start > head_end {
        let i = tail_start - 1;
        let c = if keep[i] { 0 } else { cost(lines[i]) };
        if used + c > tail_budget {
            break;
        }
        used += c;
        keep[i] = true;
        tail_start -= 1;
    }
    if keep.iter().all(|&k| k) {
        return text.to_string();
    }

    // 3. Emit kept lines; each run of dropped lines becomes one marker.
    let mut out = String::with_capacity(text.len().min(max * 8));
    let mut i = 0;
    while i < n {
        if keep[i] {
            out.push_str(lines[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < n && !keep[i] {
            i += 1;
        }
        out.push_str(&marker_for(i - start));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::estimate_tokens;

    #[test]
    fn cap_keeps_head_and_tail_and_discloses() {
        let text: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        let out = cap_tokens(&text, 200, "approx", Some("20260905-1200-abcd"));
        let n = estimate_tokens(&out, "approx");
        assert!(n <= 200, "{n} tokens over the 200 ceiling");
        assert!(out.starts_with("line 0\n"));
        assert!(out.trim_end().ends_with("line 999"));
        assert!(out.contains("omitted") && out.contains("cartoon logs grep"));
        assert!(out.contains("20260905-1200-abcd"));
    }

    #[test]
    fn cap_is_identity_under_budget() {
        assert_eq!(cap_tokens("small\n", 50, "approx", None), "small\n");
    }

    #[test]
    fn cap_without_run_id_points_at_last() {
        let text: String = (0..400).map(|i| format!("row {i}\n")).collect();
        let out = cap_tokens(&text, 100, "approx", None);
        assert!(out.contains("--last"), "{out}");
    }

    #[test]
    fn cap_keeps_a_mid_log_error() {
        let text: String = (0..300)
            .map(|i| {
                if i == 150 {
                    "ERROR: database migration 0042 failed\n".to_string()
                } else {
                    format!("step {i} compiled\n")
                }
            })
            .collect();
        for tok in ["approx", "o200k"] {
            let out = cap_tokens(&text, 300, tok, None);
            assert!(out.contains("migration 0042 failed"), "{tok}: {out}");
            assert!(estimate_tokens(&out, tok) <= 300, "{tok}");
            assert!(out.starts_with("step 0 compiled"));
            assert!(out.trim_end().ends_with("step 299 compiled"));
        }
    }

    #[test]
    fn marker_never_exceeds_a_tiny_ceiling() {
        let text: String = (0..500).map(|i| format!("line number {i}\n")).collect();
        for max in [0, 1, 2, 3, 5, 8, 12, 20, 40] {
            for tok in ["approx", "o200k"] {
                let out = cap_tokens(&text, max, tok, Some("20260905-1200-abcd"));
                let n = estimate_tokens(&out, tok);
                assert!(n <= max, "{tok} max={max}: {n} tokens: {out:?}");
            }
        }
    }

    #[test]
    fn cap_streams_bounds_the_total() {
        let out: String = (0..400).map(|i| format!("out {i}\n")).collect();
        let err: String = (0..2000).map(|i| format!("err {i}\n")).collect();
        let (o, e) = cap_streams(&out, &err, 150, "approx", None);
        let n = estimate_tokens(&o, "approx") + estimate_tokens(&e, "approx");
        assert!(n <= 150, "{n}");
        assert!(o.contains("out 0") && e.contains("err 0"));
        // A small stream that fits its share is kept whole.
        let (o, e) = cap_streams("tiny\n", &err, 150, "approx", None);
        assert_eq!(o, "tiny\n");
        assert!(estimate_tokens(&e, "approx") <= 149);
    }

    #[test]
    fn error_heuristic() {
        for l in [
            "E   assert 1 == 2",
            "npm ERR! code ELIFECYCLE",
            "thread 'main' panicked at src/x.rs:1:1",
            "Segmentation fault (core dumped)",
            "Killed",
            "bash: ./x: Permission denied",
            "Traceback (most recent call last):",
            "FAILED tests/test_x.py::test_y",
            "error[E0425]: cannot find value",
            "java.lang.NullPointerException",
        ] {
            assert!(is_error_line(l), "{l}");
        }
        for l in ["step 3 compiled", "Downloading crates", "test result: ok"] {
            assert!(!is_error_line(l), "{l}");
        }
    }
}
