//! `--max-tokens`: a hard ceiling on what reaches the agent. Keeps the head
//! and tail of the output in whole lines and replaces the middle with ONE
//! disclosed marker that is itself a ready-to-run `cartoon logs grep`
//! command. Opt-in only — with a ceiling set, even passthrough output may be
//! cut, which is exactly what the flag is for ("no Bash result ever exceeds
//! N tokens"). The raw log is archived either way.

/// Share of the budget spent on the head; the rest goes to the tail (the
/// first failure usually sits early, the summary sits at the end).
const HEAD_SHARE_PERCENT: usize = 60;

pub fn cap_tokens(text: &str, max: usize, tokenizer: &str, run_id: Option<&str>) -> String {
    if crate::stats::estimate_tokens(text, tokenizer) <= max {
        return text.to_string();
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    // Per-line estimates plus a one-token margin each: splitting text into
    // lines can only round token counts up, so the sum bounds the whole.
    let cost = |l: &str| crate::stats::estimate_tokens(l, tokenizer).max(1) + 1;
    let sel = run_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "--last".into());
    let marker_for = |omitted: usize| {
        format!(
            "  (omitted {omitted} lines to stay under --max-tokens {max}; cartoon logs grep <pattern> {sel} -C 2)\n"
        )
    };
    // The marker is part of what the agent reads: reserve its cost first.
    let budget = max.saturating_sub(cost(&marker_for(lines.len())));
    let head_budget = budget * HEAD_SHARE_PERCENT / 100;
    let tail_budget = budget.saturating_sub(head_budget);

    let mut head_end = 0;
    let mut used = 0;
    for l in &lines {
        let c = cost(l);
        if used + c > head_budget {
            break;
        }
        used += c;
        head_end += 1;
    }
    let mut tail_start = lines.len();
    used = 0;
    while tail_start > head_end {
        let c = cost(lines[tail_start - 1]);
        if used + c > tail_budget {
            break;
        }
        used += c;
        tail_start -= 1;
    }
    let omitted = tail_start - head_end;
    if omitted == 0 {
        return text.to_string();
    }
    let marker = marker_for(omitted);
    let mut out = String::with_capacity(text.len().min(max * 8));
    out.extend(lines[..head_end].iter().copied());
    out.push_str(&marker);
    out.extend(lines[tail_start..].iter().copied());
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
}
