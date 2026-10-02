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

/// Text is tokenized in line-aligned pieces of about this size, so a huge
/// log never materializes one token vector for the whole thing.
const TOKENIZE_CHUNK: usize = 1 << 20;

pub fn estimate_tokens(text: &str, tokenizer: &str) -> usize {
    if text.is_empty() {
        return 0; // never build the ~170 ms tokenizer for nothing
    }
    match tokenizer {
        "approx" => text.len() / 4,
        _ => {
            use std::sync::OnceLock;
            static BPE: OnceLock<tiktoken_rs::CoreBPE> = OnceLock::new();
            let bpe = BPE.get_or_init(|| tiktoken_rs::o200k_base().expect("bundled tokenizer"));
            let mut total = 0;
            let mut rest = text;
            while !rest.is_empty() {
                let mut end = rest.len().min(TOKENIZE_CHUNK);
                if end < rest.len() {
                    // Cut after a newline (tokens never span one in o200k's
                    // pre-split for ordinary text); fall back to a char boundary.
                    end = match rest[..end].rfind('\n') {
                        Some(i) => i + 1,
                        None => (end..rest.len())
                            .find(|&i| rest.is_char_boundary(i))
                            .unwrap_or(rest.len()),
                    };
                }
                total += bpe.encode_with_special_tokens(&rest[..end]).len();
                rest = &rest[end..];
            }
            total
        }
    }
}

/// Below this many bytes of total output, token counts use the `len/4`
/// estimate unless a decision hinges on them: building the o200k tokenizer
/// costs ~170 ms, which dwarfs `cartoon true` itself.
pub const SMALL_OUTPUT_BYTES: usize = 4096;

/// Above this many bytes the estimate is used too: exact o200k over tens of
/// megabytes costs seconds, and at that size the guard's decision is never
/// close (close calls still go through `exact`).
pub const LARGE_OUTPUT_BYTES: usize = 4 << 20;

/// Token counting for one run: exact with the configured tokenizer for
/// real output, the cheap estimate for tiny output (stats stay within a few
/// tokens there). `exact` forces the configured tokenizer for close calls.
#[derive(Debug, Clone, Copy)]
pub struct Counter<'a> {
    tokenizer: &'a str,
    cheap: bool,
}

impl<'a> Counter<'a> {
    /// `total_bytes`: everything the run captured; `need_exact`: a ceiling
    /// (`--max-tokens`) or similar depends on precise counts.
    pub fn new(tokenizer: &'a str, total_bytes: usize, need_exact: bool) -> Self {
        Counter {
            tokenizer,
            cheap: !need_exact && !(SMALL_OUTPUT_BYTES..=LARGE_OUTPUT_BYTES).contains(&total_bytes),
        }
    }

    pub fn count(&self, text: &str) -> usize {
        if self.cheap {
            text.len().div_ceil(4)
        } else {
            estimate_tokens(text, self.tokenizer)
        }
    }

    /// Always the configured tokenizer.
    pub fn exact(&self, text: &str) -> usize {
        estimate_tokens(text, self.tokenizer)
    }

    pub fn is_cheap(&self) -> bool {
        self.cheap
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
    // The ledger records every command line run under cartoon: private to
    // the user, like the raw-log archive (dir 0700, file 0600).
    if let Some(parent) = path.parent() {
        let mut dir = std::fs::DirBuilder::new();
        dir.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut dir, 0o700);
        let _ = dir.create(parent);
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let Ok(mut f) = opts.open(&path) else {
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
    Ok(read_records_counted(since)?.0)
}

/// `read_records` plus the ledger's malformed-line count, from ONE parse.
fn read_records_counted(since: Option<&str>) -> Result<(Vec<StatRecord>, usize)> {
    let cutoff: Option<DateTime<Utc>> = match since {
        Some(s) => Some(Utc::now() - parse_since(s)?),
        None => None,
    };
    let (recs, malformed) = read_ledger();
    Ok((filter_since(recs, cutoff), malformed))
}

fn filter_since(recs: Vec<StatRecord>, cutoff: Option<DateTime<Utc>>) -> Vec<StatRecord> {
    recs.into_iter()
        .filter(|r: &StatRecord| match cutoff {
            None => true,
            Some(c) => DateTime::parse_from_rfc3339(&r.ts)
                .map(|t| t.with_timezone(&Utc) >= c)
                .unwrap_or(false),
        })
        .collect()
}

/// The `cartoon stats` report — output is itself TOON (dogfooding).
pub fn report(since: Option<&str>) -> Result<String> {
    let (recs, malformed) = read_records_counted(since)?;
    Ok(render_report(&recs, malformed))
}

fn render_report(recs: &[StatRecord], malformed: usize) -> String {
    let mut agg = aggregate(recs);
    if malformed > 0 {
        agg["malformed_lines"] = json!(malformed);
    }
    crate::toon::encode(&agg)
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
    fn report_takes_records_and_malformed_count_from_one_parse() {
        let a = serde_json::to_string(&sample_record()).unwrap();
        let (recs, malformed) = parse_ledger(&format!("{a}\nnot json\n{a}\n"));
        let since = filter_since(recs, None);
        let out = render_report(&since, malformed);
        assert!(out.contains("calls: 2"), "{out}");
        assert!(out.contains("malformed_lines: 1"), "{out}");
    }

    #[test]
    fn empty_text_costs_zero_tokens_without_a_tokenizer() {
        assert_eq!(estimate_tokens("", "o200k"), 0);
    }

    #[test]
    fn chunked_o200k_count_matches_whole_count_closely() {
        let text: String = (0..200_000).map(|i| format!("line {i} ok\n")).collect();
        assert!(text.len() > TOKENIZE_CHUNK);
        let bpe = tiktoken_rs::o200k_base().unwrap();
        let whole = bpe.encode_with_special_tokens(&text).len();
        assert_eq!(estimate_tokens(&text, "o200k"), whole);
    }

    #[test]
    fn counter_is_cheap_only_for_small_output_without_a_ceiling() {
        assert!(Counter::new("o200k", 10, false).is_cheap());
        assert!(!Counter::new("o200k", 10, true).is_cheap());
        assert!(!Counter::new("o200k", SMALL_OUTPUT_BYTES, false).is_cheap());
        assert_eq!(Counter::new("o200k", 10, false).count("abcde"), 2);
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
