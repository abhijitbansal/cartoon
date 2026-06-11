use crate::archive::{self, RunMeta};
use crate::cli::{LogsQuery, RunSel, StreamSel};
use anyhow::Result;
use serde_json::json;

/// Entry point for `cartoon logs ...`. Returns the process exit code.
pub fn run(query: LogsQuery) -> Result<i32> {
    match query {
        LogsQuery::List { tag } => {
            println!("{}", render_list(&archive::list(tag.as_deref())));
            Ok(0)
        }
        LogsQuery::Show { sel, stream } => {
            let id = resolve_sel(sel)?;
            let (meta, stdout, stderr) = archive::load(&id)?;
            println!("{}", render_show(&meta, &stdout, &stderr, &stream));
            Ok(0)
        }
        LogsQuery::Grep {
            sel,
            pattern,
            context,
        } => {
            let id = resolve_sel(sel)?;
            let (_, stdout, stderr) = archive::load(&id)?;
            let re =
                regex::Regex::new(&pattern).map_err(|e| anyhow::anyhow!("invalid pattern: {e}"))?;
            print!("{}", render_grep(&id, &stdout, &stderr, &re, context));
            Ok(0)
        }
    }
}

fn resolve_sel(sel: RunSel) -> Result<String> {
    match sel {
        RunSel::Id(id) => Ok(id),
        RunSel::Last => archive::last_id().ok_or_else(|| anyhow::anyhow!("no archived runs yet")),
    }
}

/// Cap on emitted match blocks: grep exists to AVOID re-reading a huge log.
const MAX_MATCHES: usize = 50;

pub fn render_grep(
    id: &str,
    stdout: &str,
    stderr: &str,
    re: &regex::Regex,
    context: usize,
) -> String {
    let mut out = String::new();
    let mut total = 0usize;
    for (stream, text) in [("stdout", stdout), ("stderr", stderr)] {
        let lines: Vec<&str> = text.lines().collect();
        let hits: Vec<usize> = (0..lines.len())
            .filter(|&i| re.is_match(lines[i]))
            .collect();
        if hits.is_empty() {
            continue;
        }
        out.push_str(&format!("--- {stream} ({} matches) ---\n", hits.len()));
        let mut last_end: Option<usize> = None;
        for &i in hits.iter().take(MAX_MATCHES.saturating_sub(total)) {
            let lo = i.saturating_sub(context);
            let hi = (i + context).min(lines.len() - 1);
            if let Some(le) = last_end {
                if lo > le + 1 {
                    out.push_str("...\n");
                }
            }
            let start = last_end.map_or(lo, |le| lo.max(le + 1));
            for (j, line) in lines.iter().enumerate().take(hi + 1).skip(start) {
                out.push_str(&format!("{}:{}\n", j + 1, line));
            }
            last_end = Some(hi);
        }
        total += hits.len();
        if hits.len() > MAX_MATCHES {
            out.push_str(&format!(
                "  (capped at {MAX_MATCHES} matches; cartoon logs {id} for the full log)\n"
            ));
        }
    }
    if out.is_empty() {
        out = format!("no matches in run {id}\n");
    }
    out
}

pub fn render_list(metas: &[RunMeta]) -> String {
    let rows: Vec<serde_json::Value> = metas
        .iter()
        .map(|m| {
            json!({
                "id": m.id,
                "ts": m.ts,
                "cmd": m.argv.first().cloned().unwrap_or_default(),
                "mode": m.mode,
                "exit": m.exit,
                "tags": m.tags.join(","),
            })
        })
        .collect();
    crate::toon::encode(&json!({ "runs": rows }))
}

pub fn render_show(meta: &RunMeta, stdout: &str, stderr: &str, stream: &StreamSel) -> String {
    match stream {
        StreamSel::Stdout => stdout.to_string(),
        StreamSel::Stderr => stderr.to_string(),
        StreamSel::Both => {
            let head = crate::toon::encode(&json!({
                "id": meta.id,
                "ts": meta.ts,
                "cmd": meta.argv.join(" "),
                "mode": meta.mode,
                "exit": meta.exit,
                "tags": meta.tags.join(","),
            }));
            format!("{head}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::RunMeta;

    fn meta(id: &str, cmd: &str, tags: &[&str]) -> RunMeta {
        RunMeta {
            id: id.into(),
            ts: "2026-06-10T05:12:03Z".into(),
            argv: vec![cmd.into(), "-q".into()],
            mode: "pytest".into(),
            exit: 1,
            cwd: "/proj".into(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            stdout_bytes: 10,
            stderr_bytes: 0,
        }
    }

    #[test]
    fn list_renders_tabular_toon() {
        let out = render_list(&[meta("20260610-051203-ab12", "pytest", &["api", "ci"])]);
        assert!(
            out.contains("runs[1]{id,ts,cmd,mode,exit,tags}:"),
            "got:\n{out}"
        );
        assert!(out.contains("20260610-051203-ab12"));
        assert!(out.contains("\"api,ci\""));
    }

    #[test]
    fn empty_list_renders_zero() {
        assert_eq!(render_list(&[]), "runs[0]:");
    }

    #[test]
    fn show_both_streams_has_sections() {
        let out = render_show(
            &meta("id1", "pytest", &[]),
            "RAW OUT",
            "RAW ERR",
            &crate::cli::StreamSel::Both,
        );
        assert!(out.contains("id: id1"));
        assert!(out.contains("--- stdout ---\nRAW OUT"));
        assert!(out.contains("--- stderr ---\nRAW ERR"));
    }

    #[test]
    fn show_single_stream_is_raw_only() {
        let out = render_show(
            &meta("id1", "pytest", &[]),
            "RAW OUT",
            "RAW ERR",
            &crate::cli::StreamSel::Stdout,
        );
        assert_eq!(out, "RAW OUT");
    }
}
