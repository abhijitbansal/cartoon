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
            let id = match sel {
                RunSel::Id(id) => id,
                RunSel::Last => {
                    archive::last_id().ok_or_else(|| anyhow::anyhow!("no archived runs yet"))?
                }
            };
            let (meta, stdout, stderr) = archive::load(&id)?;
            println!("{}", render_show(&meta, &stdout, &stderr, &stream));
            Ok(0)
        }
    }
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
