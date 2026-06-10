use crate::config::Config;
use crate::runner::Captured;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct RunRef {
    pub id: String,
    pub dir: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RunMeta {
    pub id: String,
    pub ts: String,
    pub argv: Vec<String>,
    pub mode: String,
    pub exit: i32,
    pub cwd: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
}

/// `YYYYMMDD-HHMMSS-<4 hex>` UTC; lexicographic order == time order.
/// Salt: process-local monotonic counter lazily seeded from pid ^ nanos,
/// so parallel processes start at different offsets (collision-resistant)
/// while calls within one process stay strictly ordered.
pub fn new_run_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = chrono::Utc::now();
    let _ = COUNTER.compare_exchange(
        0,
        (std::process::id() as u64 ^ now.timestamp_subsec_nanos() as u64) | 1,
        Ordering::Relaxed,
        Ordering::Relaxed,
    );
    let salt = COUNTER.fetch_add(1, Ordering::Relaxed) & 0xffff;
    format!("{}-{:04x}", now.format("%Y%m%d-%H%M%S"), salt)
}

/// Public wrapper: archive under the XDG runs dir. Failures swallowed → None.
pub fn record(
    argv: &[String],
    mode: &str,
    captured: &Captured,
    exit: i32,
    tags: &[String],
    cfg: &Config,
) -> Option<RunRef> {
    let root = crate::paths::runs_dir()?;
    record_at(&root, argv, mode, captured, exit, tags, cfg)
}

pub fn list(tag: Option<&str>) -> Vec<RunMeta> {
    match crate::paths::runs_dir() {
        Some(root) => list_at(&root, tag),
        None => Vec::new(),
    }
}

pub fn load(id: &str) -> Result<(RunMeta, String, String)> {
    let root = crate::paths::runs_dir().context("no state directory")?;
    load_at(&root, id)
}

/// Newest run id, if any.
pub fn last_id() -> Option<String> {
    list(None).into_iter().next().map(|m| m.id)
}

pub fn record_at(
    root: &Path,
    argv: &[String],
    mode: &str,
    captured: &Captured,
    exit: i32,
    tags: &[String],
    cfg: &Config,
) -> Option<RunRef> {
    if cfg.keep_runs == 0 {
        return None; // archiving disabled
    }
    let now = chrono::Utc::now();
    let id = new_run_id();
    let dir = root.join(&id);
    let meta = RunMeta {
        id: id.clone(),
        ts: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        argv: argv.to_vec(),
        mode: mode.to_string(),
        exit,
        cwd: std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        tags: tags.to_vec(),
        stdout_bytes: captured.stdout.len() as u64,
        stderr_bytes: captured.stderr.len() as u64,
    };
    let write_all = || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("stdout.log"), &captured.stdout)?;
        std::fs::write(dir.join("stderr.log"), &captured.stderr)?;
        let json = serde_json::to_string_pretty(&meta).map_err(std::io::Error::other)?;
        std::fs::write(dir.join("meta.json"), json)?;
        Ok(())
    };
    if write_all().is_err() {
        // Partial write: best-effort cleanup, then report failure.
        let _ = std::fs::remove_dir_all(&dir);
        return None;
    }
    prune_at(root, cfg);
    Some(RunRef { id, dir })
}

pub fn list_at(root: &Path, tag: Option<&str>) -> Vec<RunMeta> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut metas: Vec<RunMeta> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let text = std::fs::read_to_string(e.path().join("meta.json")).ok()?;
            match serde_json::from_str::<RunMeta>(&text) {
                Ok(m) => Some(m),
                Err(_) => {
                    eprintln!(
                        "cartoon: skipping corrupt archive entry {}",
                        e.path().display()
                    );
                    None
                }
            }
        })
        .filter(|m| match tag {
            Some(t) => m.tags.iter().any(|x| x == t),
            None => true,
        })
        .collect();
    metas.sort_by(|a, b| b.id.cmp(&a.id)); // newest first
    metas
}

pub fn load_at(root: &Path, id: &str) -> Result<(RunMeta, String, String)> {
    // Run ids are [0-9a-z-] only; reject anything path-like.
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        anyhow::bail!("invalid run id {id:?} — try `cartoon logs` to list runs");
    }
    let dir = root.join(id);
    let meta: RunMeta = serde_json::from_str(
        &std::fs::read_to_string(dir.join("meta.json"))
            .with_context(|| format!("no archived run {id} — try `cartoon logs`"))?,
    )
    .with_context(|| format!("corrupt meta for run {id}"))?;
    let read_stream = |name: &str| -> String {
        std::fs::read_to_string(dir.join(name)).unwrap_or_else(|_| {
            eprintln!("cartoon: archived {name} missing for run {id}");
            String::new()
        })
    };
    let stdout = read_stream("stdout.log");
    let stderr = read_stream("stderr.log");
    Ok((meta, stdout, stderr))
}

/// Delete oldest runs while count > keep_runs OR total bytes > max_archive_mb.
/// Errors ignored: deletion is idempotent and retried implicitly next run.
fn prune_at(root: &Path, cfg: &Config) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort(); // run-ids sort oldest-first lexicographically

    let dir_size = |d: &Path| -> u64 {
        std::fs::read_dir(d)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| e.metadata().ok())
                    .map(|m| m.len())
                    .sum()
            })
            .unwrap_or(0)
    };
    let sizes: Vec<u64> = dirs.iter().map(|d| dir_size(d)).collect();
    let mut total: u64 = sizes.iter().sum();
    let max_bytes = cfg.max_archive_mb * 1024 * 1024;

    let mut i = 0;
    while i < dirs.len() && (dirs.len() - i > cfg.keep_runs || total > max_bytes) {
        let _ = std::fs::remove_dir_all(&dirs[i]);
        total = total.saturating_sub(sizes[i]);
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{list_at, load_at, new_run_id, record_at};
    use crate::runner::Captured;

    fn captured(stdout: &str, stderr: &str) -> Captured {
        use std::process::Command;
        let status = Command::new("true").status().unwrap();
        Captured {
            stdout: stdout.into(),
            stderr: stderr.into(),
            status,
        }
    }

    fn cfg() -> crate::config::Config {
        crate::config::Config::default()
    }

    #[test]
    fn run_ids_are_time_ordered_and_unique() {
        let a = new_run_id();
        let b = new_run_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), "20260610-051203-ab12".len());
        // Salt can wrap at 0xffff, so compare only the 15-char timestamp prefix
        // (YYYYMMDD-HHMMSS) which is always non-decreasing.
        assert!(
            a[..15] <= b[..15],
            "timestamp prefix must follow time order: {a} vs {b}"
        );
    }

    #[test]
    fn load_rejects_path_like_ids() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_at(tmp.path(), "../../etc").is_err());
        assert!(load_at(tmp.path(), "/etc/passwd").is_err());
        assert!(load_at(tmp.path(), "a/b").is_err());
        assert!(load_at(tmp.path(), "").is_err());
    }

    #[test]
    fn record_then_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let argv = vec!["pytest".to_string(), "-q".to_string()];
        let cap = captured("OUT bytes\n", "ERR bytes\n");
        let tags = vec!["api".to_string(), "ci".to_string()];
        let run = record_at(tmp.path(), &argv, "pytest", &cap, 1, &tags, &cfg()).unwrap();

        let (meta, out, err) = load_at(tmp.path(), &run.id).unwrap();
        assert_eq!(meta.id, run.id);
        assert_eq!(meta.argv, argv);
        assert_eq!(meta.mode, "pytest");
        assert_eq!(meta.exit, 1);
        assert_eq!(meta.tags, tags);
        assert_eq!(meta.stdout_bytes, 10);
        assert_eq!(meta.stderr_bytes, 10);
        assert_eq!(out, "OUT bytes\n");
        assert_eq!(err, "ERR bytes\n");
        assert!(run.dir.join("meta.json").exists());
    }

    #[test]
    fn list_is_newest_first_and_tag_filtered() {
        let tmp = tempfile::tempdir().unwrap();
        let cap = captured("x", "");
        record_at(
            tmp.path(),
            &["a".into()],
            "passthrough",
            &cap,
            0,
            &[],
            &cfg(),
        )
        .unwrap();
        record_at(
            tmp.path(),
            &["b".into()],
            "json",
            &cap,
            0,
            &["t1".into()],
            &cfg(),
        )
        .unwrap();

        let all = list_at(tmp.path(), None);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].argv[0], "b", "newest first");

        let tagged = list_at(tmp.path(), Some("t1"));
        assert_eq!(tagged.len(), 1);
        assert_eq!(tagged[0].argv[0], "b");
    }

    #[test]
    fn load_unknown_id_errors() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_at(tmp.path(), "20990101-000000-dead").is_err());
    }

    #[test]
    fn corrupt_meta_is_skipped_in_list() {
        let tmp = tempfile::tempdir().unwrap();
        let cap = captured("x", "");
        record_at(tmp.path(), &["ok".into()], "json", &cap, 0, &[], &cfg()).unwrap();
        let bad = tmp.path().join("20000101-000000-beef");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("meta.json"), "not json").unwrap();
        let all = list_at(tmp.path(), None);
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn prunes_beyond_keep_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let cap = captured("x", "");
        let mut small = cfg();
        small.keep_runs = 2;
        for name in ["a", "b", "c"] {
            record_at(
                tmp.path(),
                &[name.to_string()],
                "json",
                &cap,
                0,
                &[],
                &small,
            )
            .unwrap();
        }
        let all = list_at(tmp.path(), None);
        assert_eq!(all.len(), 2, "oldest pruned");
        assert_eq!(all[1].argv[0], "b", "a was deleted");
    }

    #[test]
    fn prunes_beyond_max_size() {
        let tmp = tempfile::tempdir().unwrap();
        let big = "x".repeat(1024 * 1024); // 1 MiB stdout per run
        let cap = captured(&big, "");
        let mut small = cfg();
        small.keep_runs = 100;
        small.max_archive_mb = 2;
        for name in ["a", "b", "c"] {
            record_at(
                tmp.path(),
                &[name.to_string()],
                "json",
                &cap,
                0,
                &[],
                &small,
            )
            .unwrap();
        }
        let all = list_at(tmp.path(), None);
        assert!(all.len() <= 2, "size cap enforced, got {}", all.len());
    }

    #[test]
    fn keep_runs_zero_disables_archiving() {
        let tmp = tempfile::tempdir().unwrap();
        let cap = captured("x", "");
        let mut off = cfg();
        off.keep_runs = 0;
        assert!(record_at(tmp.path(), &["a".into()], "json", &cap, 0, &[], &off).is_none());
        assert!(list_at(tmp.path(), None).is_empty());
    }
}
