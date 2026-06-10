use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Debug, Serialize, Deserialize)]
pub struct StatRecord {
    pub ts: String,
    pub cmd: String,
    pub adapter: String,
    pub tokens_in: usize,
    pub tokens_out: usize,
    pub saved: i64,
    pub exit: i32,
}

pub fn estimate_tokens(text: &str, tokenizer: &str) -> usize {
    match tokenizer {
        "approx" => text.len() / 4,
        _ => {
            use std::sync::OnceLock;
            static BPE: OnceLock<tiktoken_rs::CoreBPE> = OnceLock::new();
            BPE.get_or_init(|| tiktoken_rs::o200k_base().expect("bundled tokenizer"))
                .encode_with_special_tokens(text)
                .len()
        }
    }
}

/// Build + append a record. Failures are swallowed: stats must never break a call.
pub fn record_call(
    argv: &[String],
    adapter: &str,
    original: &str,
    emitted: &str,
    exit: i32,
    tokenizer: &str,
) {
    let tokens_in = estimate_tokens(original, tokenizer);
    let tokens_out = estimate_tokens(emitted, tokenizer);
    let rec = StatRecord {
        ts: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        cmd: argv.first().cloned().unwrap_or_default(),
        adapter: adapter.to_string(),
        tokens_in,
        tokens_out,
        saved: tokens_in as i64 - tokens_out as i64,
        exit,
    };
    let Some(path) = crate::paths::stats_file() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    if let Ok(line) = serde_json::to_string(&rec) {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

pub fn parse_since(s: &str) -> Result<Duration> {
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = num
        .parse()
        .context("--since wants <number><d|h|m>, e.g. 7d")?;
    match unit {
        "d" => Ok(Duration::days(n)),
        "h" => Ok(Duration::hours(n)),
        "m" => Ok(Duration::minutes(n)),
        _ => anyhow::bail!("--since wants <number><d|h|m>, e.g. 7d"),
    }
}

pub fn aggregate(recs: &[StatRecord]) -> Value {
    let mut by_adapter: Map<String, Value> = Map::new();
    let mut total_saved = 0i64;
    for r in recs {
        total_saved += r.saved;
        let entry = by_adapter
            .entry(r.adapter.clone())
            .or_insert_with(|| json!({"calls": 0, "saved": 0}));
        entry["calls"] = json!(entry["calls"].as_i64().unwrap() + 1);
        entry["saved"] = json!(entry["saved"].as_i64().unwrap() + r.saved);
    }
    json!({
        "calls": recs.len(),
        "tokens_saved": total_saved,
        "by_adapter": Value::Object(by_adapter),
    })
}

/// The `cartoon stats` report — output is itself TOON (dogfooding).
pub fn report(since: Option<&str>) -> Result<String> {
    let cutoff: Option<DateTime<Utc>> = match since {
        Some(s) => Some(Utc::now() - parse_since(s)?),
        None => None,
    };
    let Some(path) = crate::paths::stats_file() else {
        return Ok("calls: 0".into());
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let recs: Vec<StatRecord> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .filter(|r: &StatRecord| match cutoff {
            None => true,
            Some(c) => DateTime::parse_from_rfc3339(&r.ts)
                .map(|t| t.with_timezone(&Utc) >= c)
                .unwrap_or(false),
        })
        .collect();
    Ok(crate::toon::encode(&aggregate(&recs)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approx_estimate_is_quarter_of_bytes() {
        assert_eq!(estimate_tokens("abcdefgh", "approx"), 2);
    }

    #[test]
    fn o200k_estimate_counts_real_tokens() {
        let n = estimate_tokens("the quick brown fox jumps over the lazy dog", "o200k");
        assert!((5..=15).contains(&n), "got {n}");
    }

    #[test]
    fn since_parses_units() {
        assert_eq!(parse_since("7d").unwrap(), chrono::Duration::days(7));
        assert_eq!(parse_since("24h").unwrap(), chrono::Duration::hours(24));
        assert_eq!(parse_since("30m").unwrap(), chrono::Duration::minutes(30));
        assert!(parse_since("7x").is_err());
    }

    #[test]
    fn aggregate_sums_and_groups() {
        let recs = vec![
            StatRecord {
                ts: "2026-06-09T10:00:00Z".into(),
                cmd: "pytest".into(),
                adapter: "pytest".into(),
                tokens_in: 100,
                tokens_out: 10,
                saved: 90,
                exit: 0,
            },
            StatRecord {
                ts: "2026-06-09T11:00:00Z".into(),
                cmd: "ls".into(),
                adapter: "passthrough".into(),
                tokens_in: 5,
                tokens_out: 5,
                saved: 0,
                exit: 0,
            },
        ];
        let v = aggregate(&recs);
        assert_eq!(v["calls"], 2);
        assert_eq!(v["tokens_saved"], 90);
        assert_eq!(v["by_adapter"]["pytest"]["saved"], 90);
    }
}
