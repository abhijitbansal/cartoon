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
    #[serde(default)]
    pub run_id: Option<String>,
    /// For `sh -c <string>` runs, the command inside the string (`cmd` is
    /// then just `sh`). Lets `learn`/`logs` see what actually ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inner_cmd: Option<String>,
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

/// Estimate both streams once and append a record.
/// Failures are swallowed: stats must never break a call.
pub fn record_call(
    argv: &[String],
    adapter: &str,
    original: &str,
    emitted: &str,
    exit: i32,
    tokenizer: &str,
    run_id: Option<&str>,
) {
    let tokens_in = estimate_tokens(original, tokenizer);
    let tokens_out = estimate_tokens(emitted, tokenizer);
    record_counts(argv, adapter, tokens_in, tokens_out, exit, run_id);
}

/// Append a record from counts the caller already computed (avoids
/// tokenizing full-size output twice). Failures are swallowed.
pub fn record_counts(
    argv: &[String],
    adapter: &str,
    tokens_in: usize,
    tokens_out: usize,
    exit: i32,
    run_id: Option<&str>,
) {
    let rec = build_record(argv, adapter, tokens_in, tokens_out, exit, run_id);
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
    // One write_all of record+newline: a single O_APPEND write of a small
    // record is atomic on POSIX, so concurrent cartoon processes can no
    // longer interleave two records into one corrupt line.
    if let Ok(mut line) = serde_json::to_string(&rec) {
        line.push('\n');
        use std::io::Write;
        let _ = f.write_all(line.as_bytes());
    }
}

pub fn build_record(
    argv: &[String],
    adapter: &str,
    tokens_in: usize,
    tokens_out: usize,
    exit: i32,
    run_id: Option<&str>,
) -> StatRecord {
    StatRecord {
        ts: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        cmd: argv.first().cloned().unwrap_or_default(),
        adapter: adapter.to_string(),
        tokens_in,
        tokens_out,
        saved: tokens_in as i64 - tokens_out as i64,
        exit,
        run_id: run_id.map(String::from),
        inner_cmd: crate::cli::inner_command(argv),
    }
}

pub fn parse_since(s: &str) -> Result<Duration> {
    const USAGE: &str = "--since wants <number><d|h|m>, e.g. 7d";
    let (idx, unit_char) = s.char_indices().next_back().context(USAGE)?;
    let n: i64 = s[..idx].parse().context(USAGE)?;
    if n <= 0 {
        anyhow::bail!(USAGE);
    }
    match unit_char {
        'd' => Ok(Duration::days(n)),
        'h' => Ok(Duration::hours(n)),
        'm' => Ok(Duration::minutes(n)),
        _ => anyhow::bail!(USAGE),
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
        entry["calls"] = json!(entry["calls"].as_i64().unwrap_or(0) + 1);
        entry["saved"] = json!(entry["saved"].as_i64().unwrap_or(0) + r.saved);
    }
    json!({
        "calls": recs.len(),
        "tokens_saved": total_saved,
        "by_adapter": Value::Object(by_adapter),
    })
}

/// Parse ledger text. Concatenated records on one line (a pre-fix
/// interleaved write) are all recovered; a line that is not JSON counts as
/// malformed and is skipped — never silently.
pub fn parse_ledger(text: &str) -> (Vec<StatRecord>, usize) {
    let mut recs = Vec::new();
    let mut malformed = 0usize;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let before = recs.len();
        let mut broke = false;
        for item in serde_json::Deserializer::from_str(line).into_iter::<StatRecord>() {
            match item {
                Ok(r) => recs.push(r),
                Err(_) => {
                    broke = true;
                    break;
                }
            }
        }
        if broke || recs.len() == before {
            malformed += 1;
        }
    }
    (recs, malformed)
}

/// Whole ledger plus its malformed-line count (for `stats` and `doctor`).
pub fn read_ledger() -> (Vec<StatRecord>, usize) {
    let Some(path) = crate::paths::stats_file() else {
        return (Vec::new(), 0);
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    parse_ledger(&text)
}

/// Read stat records, optionally filtered by a `--since` window.
pub fn read_records(since: Option<&str>) -> Result<Vec<StatRecord>> {
    let cutoff: Option<DateTime<Utc>> = match since {
        Some(s) => Some(Utc::now() - parse_since(s)?),
        None => None,
    };
    let (recs, _) = read_ledger();
    Ok(recs
        .into_iter()
        .filter(|r: &StatRecord| match cutoff {
            None => true,
            Some(c) => DateTime::parse_from_rfc3339(&r.ts)
                .map(|t| t.with_timezone(&Utc) >= c)
                .unwrap_or(false),
        })
        .collect())
}

/// The `cartoon stats` report — output is itself TOON (dogfooding).
pub fn report(since: Option<&str>) -> Result<String> {
    let recs = read_records(since)?;
    let mut agg = aggregate(&recs);
    let (_, malformed) = read_ledger();
    if malformed > 0 {
        agg["malformed_lines"] = json!(malformed);
    }
    Ok(crate::toon::encode(&agg))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> StatRecord {
        StatRecord {
            ts: "2026-06-09T10:00:00Z".into(),
            cmd: "pytest".into(),
            adapter: "pytest".into(),
            tokens_in: 100,
            tokens_out: 10,
            saved: 90,
            exit: 0,
            run_id: None,
            inner_cmd: None,
        }
    }

    #[test]
    fn reader_recovers_concatenated_records_and_counts_malformed_lines() {
        let a = serde_json::to_string(&sample_record()).unwrap();
        let text = format!("{a}{a}\n\n{{not json\n{a}\n");
        let (recs, malformed) = parse_ledger(&text);
        assert_eq!(recs.len(), 3);
        assert_eq!(malformed, 1);
    }

    #[test]
    fn build_record_captures_inner_command_for_shell_strings() {
        let argv: Vec<String> = ["sh", "-c", "xcodebuild test -scheme A | tail"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let r = build_record(&argv, "passthrough", 10, 10, 0, None);
        assert_eq!(r.cmd, "sh");
        assert_eq!(r.inner_cmd.as_deref(), Some("xcodebuild"));
        let plain: Vec<String> = vec!["pytest".into()];
        assert_eq!(
            build_record(&plain, "pytest", 1, 1, 0, None).inner_cmd,
            None
        );
    }

    #[test]
    fn inner_cmd_is_omitted_from_json_when_absent() {
        let json = serde_json::to_string(&sample_record()).unwrap();
        assert!(!json.contains("inner_cmd"));
    }

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
    fn since_rejects_multibyte_and_nonpositive() {
        assert!(parse_since("7é").is_err());
        assert!(parse_since("é").is_err());
        assert!(parse_since("-1d").is_err());
        assert!(parse_since("0d").is_err());
        assert!(parse_since("").is_err());
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
                run_id: None,
                inner_cmd: None,
            },
            StatRecord {
                ts: "2026-06-09T11:00:00Z".into(),
                cmd: "ls".into(),
                adapter: "passthrough".into(),
                tokens_in: 5,
                tokens_out: 5,
                saved: 0,
                exit: 0,
                run_id: None,
                inner_cmd: None,
            },
        ];
        let v = aggregate(&recs);
        assert_eq!(v["calls"], 2);
        assert_eq!(v["tokens_saved"], 90);
        assert_eq!(v["by_adapter"]["pytest"]["saved"], 90);
    }
}
