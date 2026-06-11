use regex::Regex;
use std::sync::OnceLock;

const MIN_LEVELED_LINES: usize = 10;
const MIN_LEVELED_RATIO: f64 = 0.5;
const CONTEXT_LINES: usize = 2;

fn level_of(line: &str) -> Option<&'static str> {
    static PAT: OnceLock<Regex> = OnceLock::new();
    let pat = PAT.get_or_init(|| {
        Regex::new(
            r"(?x)^\s*
              (?:\[?\d{4}-\d{2}-\d{2}[T\ ][\d:.,+Z\-]+\]?\s+)?   # optional timestamp
              \[?\b(TRACE|DEBUG|INFO|WARN|WARNING|ERROR|FATAL|CRITICAL)\b\]?",
        )
        .unwrap()
    });
    let caps = pat.captures(line)?;
    Some(match caps.get(1).unwrap().as_str() {
        "TRACE" => "TRACE",
        "DEBUG" => "DEBUG",
        "INFO" => "INFO",
        "WARN" | "WARNING" => "WARN",
        "ERROR" => "ERROR",
        "FATAL" | "CRITICAL" => "FATAL",
        _ => unreachable!(),
    })
}

fn is_noise(level: &str) -> bool {
    matches!(level, "TRACE" | "DEBUG" | "INFO")
}

/// Collapse DEBUG/INFO/TRACE lines to counts; keep WARN+ verbatim with
/// +-CONTEXT_LINES of surrounding context. No-op unless leveled lines
/// dominate the input (see MIN_* thresholds).
pub fn filter_levels(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let levels: Vec<Option<&'static str>> = lines.iter().map(|l| level_of(l)).collect();
    let non_blank = lines.iter().filter(|l| !l.trim().is_empty()).count();
    let leveled = levels.iter().flatten().count();
    if leveled < MIN_LEVELED_LINES
        || non_blank == 0
        || (leveled as f64) / (non_blank as f64) < MIN_LEVELED_RATIO
    {
        return text.to_string();
    }
    // Mark keepers: non-leveled lines, WARN+ lines, and context around WARN+.
    let mut keep = vec![false; lines.len()];
    for (i, lv) in levels.iter().enumerate() {
        match lv {
            None => keep[i] = true,
            Some(l) if !is_noise(l) => {
                let lo = i.saturating_sub(CONTEXT_LINES);
                let hi = (i + CONTEXT_LINES).min(lines.len() - 1);
                for k in keep.iter_mut().take(hi + 1).skip(lo) {
                    *k = true;
                }
            }
            Some(_) => {}
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut dropped: Vec<&'static str> = Vec::new();
    fn flush(dropped: &mut Vec<&'static str>, out: &mut Vec<String>) {
        if dropped.is_empty() {
            return;
        }
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for l in dropped.iter() {
            match counts.iter_mut().find(|(name, _)| name == l) {
                Some((_, n)) => *n += 1,
                None => counts.push((l, 1)),
            }
        }
        let summary = counts
            .iter()
            .map(|(name, n)| format!("{n} {name}"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(format!("  (filtered {summary} lines, see raw_log)"));
        dropped.clear();
    }
    for (i, line) in lines.iter().enumerate() {
        if keep[i] {
            flush(&mut dropped, &mut out);
            out.push(line.to_string());
        } else {
            dropped.push(levels[i].unwrap());
        }
    }
    flush(&mut dropped, &mut out);
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leveled_log() -> String {
        let mut s = String::new();
        for i in 0..20 {
            s.push_str(&format!(
                "2026-06-11T10:00:{i:02} INFO worker heartbeat {i}\n"
            ));
        }
        s.push_str("2026-06-11T10:00:20 ERROR connection refused to db:5432\n");
        for i in 21..30 {
            s.push_str(&format!("2026-06-11T10:00:{i:02} DEBUG retry queue {i}\n"));
        }
        s
    }

    #[test]
    fn collapses_noise_keeps_error_with_context() {
        let out = filter_levels(&leveled_log());
        assert!(out.contains("ERROR connection refused"));
        assert!(out.contains("INFO worker heartbeat 18")); // -2 context
        assert!(out.contains("DEBUG retry queue 22")); // +2 context
        assert!(out.contains("filtered"));
        assert!(!out.contains("worker heartbeat 5")); // dropped noise
    }

    #[test]
    fn prose_without_levels_unchanged() {
        let prose = "Compiling cartoon v0.1.0\nFinished dev profile\n";
        assert_eq!(filter_levels(prose), prose);
    }

    #[test]
    fn few_leveled_lines_do_not_trigger() {
        let mixed = "INFO start\nplain a\nplain b\nplain c\nplain d\n";
        assert_eq!(filter_levels(mixed), mixed);
    }
}
