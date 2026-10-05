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
/// so parallel processes start at different offsets while calls within one
/// process stay strictly ordered. Uniqueness is not assumed: `create_run_dir`
/// claims the id with an exclusive `create_dir` and retries on a collision.
pub fn new_run_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    static SEEDED: std::sync::Once = std::sync::Once::new();
    let now = chrono::Utc::now();
    SEEDED.call_once(|| {
        COUNTER.store(
            std::process::id() as u64 ^ now.timestamp_subsec_nanos() as u64,
            Ordering::Relaxed,
        )
    });
    let salt = COUNTER.fetch_add(1, Ordering::Relaxed) & 0xffff;
    format!("{}-{:04x}", now.format("%Y%m%d-%H%M%S"), salt)
}

/// Archive dirs are 0700 and files 0600: `meta.json` holds full argv + cwd
/// and the logs hold whatever the command printed (tokens, env dumps).
#[cfg(unix)]
const DIR_MODE: u32 = 0o700;
#[cfg(unix)]
const FILE_MODE: u32 = 0o600;

/// Create `dir` and any missing parents, private to the user.
fn create_private_dir_all(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, DIR_MODE);
    b.create(dir)
}

/// Claim a fresh run dir under `root`. `create_dir` (not `create_dir_all`)
/// fails on an existing dir, so two processes can never share a run id;
/// on `AlreadyExists` a new id is drawn.
fn create_run_dir(root: &Path) -> std::io::Result<RunRef> {
    create_run_dir_with(root, new_run_id)
}

fn create_run_dir_with(
    root: &Path,
    mut next_id: impl FnMut() -> String,
) -> std::io::Result<RunRef> {
    create_private_dir_all(root)?;
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, DIR_MODE);
    let mut last = None;
    for _ in 0..64 {
        let id = next_id();
        let dir = root.join(&id);
        match b.create(&dir) {
            Ok(()) => return Ok(RunRef { id, dir }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no free run id")))
}

/// Write a new archive file readable only by the user.
fn write_private(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, FILE_MODE);
    o.open(path)?.write_all(contents.as_ref())
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

/// Reserve a run slot (id + empty dir, claimed exclusively) without writing
/// any logs, so callers can know the final `raw_log` path before committing
/// to a transform. None when archiving is disabled, no state dir exists, or
/// the dir cannot be created.
pub fn reserve(cfg: &Config) -> Option<RunRef> {
    if cfg.keep_runs == 0 {
        return None;
    }
    let root = crate::paths::runs_dir()?;
    create_run_dir(&root).ok()
}

/// Write a previously reserved run. Failures swallowed → None.
pub fn write_reserved(
    run: RunRef,
    argv: &[String],
    mode: &str,
    captured: &Captured,
    exit: i32,
    tags: &[String],
    cfg: &Config,
) -> Option<RunRef> {
    let root = run.dir.parent()?.to_path_buf();
    write_at(&root, run, argv, mode, captured, exit, tags, cfg)
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
    let run = match create_run_dir(root) {
        Ok(run) => run,
        Err(e) => {
            eprintln!(
                "cartoon: could not archive raw output under {}: {e}",
                root.display()
            );
            return None;
        }
    };
    write_at(root, run, argv, mode, captured, exit, tags, cfg)
}

#[allow(clippy::too_many_arguments)]
fn write_at(
    root: &Path,
    run: RunRef,
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
    let RunRef { id, dir } = run;
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
        // The dir was claimed by `create_run_dir`; recreate it only if it
        // vanished in between (e.g. a manual cleanup).
        if !dir.is_dir() {
            create_private_dir_all(&dir)?;
        }
        write_private(&dir.join("stdout.log"), &captured.stdout)?;
        write_private(&dir.join("stderr.log"), &captured.stderr)?;
        let json = serde_json::to_string_pretty(&meta).map_err(std::io::Error::other)?;
        write_private(&dir.join("meta.json"), json.as_bytes())?;
        Ok(())
    };
    if let Err(e) = write_all() {
        // Partial write: best-effort cleanup, then report failure — loudly,
        // because a missing archive breaks the raw_log escape hatch.
        eprintln!(
            "cartoon: could not archive raw output to {}: {e}",
            dir.display()
        );
        let _ = std::fs::remove_dir_all(&dir);
        return None;
    }
    prune_at(root, cfg, chrono::Utc::now());
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

/// The structured report an adapter produced for a run, stored next to its
/// raw logs as `report.json` so `cartoon last` can re-show it and `cartoon
/// diff` can compare it with another run of the same command, without
/// re-parsing the raw streams. Paths are already cwd-relative.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredReport {
    /// `tests` (a test runner's failures) or `diagnostics` (lint/typecheck/build).
    pub kind: String,
    pub runner: String,
    /// Failed tests, or diagnostics reported.
    pub failed: u64,
    /// Tests the run executed (test reports only). A re-run that executed
    /// fewer (`pytest -x`, `-k`) can't tell "fixed" from "not run".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    pub items: Vec<StoredItem>,
    /// The TOON report exactly as the run printed it (before footers).
    pub rendered: String,
}

/// One failing test (`id`) or one diagnostic (`rule`), with its location.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredItem {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(default)]
    pub loc: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rule: String,
    #[serde(default)]
    pub msg: String,
}

pub const REPORT_FILE: &str = "report.json";

/// Write `report.json` into an archived run's dir (private, like the logs).
/// Best effort: a run without one simply has nothing to diff.
pub fn write_report(dir: &Path, report: &StoredReport) -> bool {
    serde_json::to_string(report)
        .ok()
        .is_some_and(|json| write_private(&dir.join(REPORT_FILE), json).is_ok())
}

/// The stored report of run `id` under `root`; None when the run had no
/// adapter report (or the id is not a run id).
pub fn load_report_at(root: &Path, id: &str) -> Option<StoredReport> {
    if !looks_like_run_id(id) {
        return None;
    }
    let text = std::fs::read_to_string(root.join(id).join(REPORT_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// `YYYYMMDD-HHMMSS-xxxx`: the shape `new_run_id` produces.
pub fn looks_like_run_id(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 20
        && b[8] == b'-'
        && b[15] == b'-'
        && b[..8].iter().chain(&b[9..15]).all(u8::is_ascii_digit)
        && b[16..]
            .iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
}

/// The newest runs that the size budget never deletes: one oversized log
/// must not wipe out every older `raw_log` an agent may still hold.
const MIN_KEEP_RUNS: usize = 5;
/// Runs younger than this are never pruned: a concurrent cartoon process
/// may have just reserved or written one and printed its `raw_log` path.
const MIN_PRUNE_AGE_SECS: i64 = 60;

/// When a run was created, from its id (`YYYYMMDD-HHMMSS-xxxx`, UTC).
fn run_time(dir: &Path) -> Option<chrono::DateTime<chrono::Utc>> {
    let name = dir.file_name()?.to_str()?;
    let ts = chrono::NaiveDateTime::parse_from_str(name.get(..15)?, "%Y%m%d-%H%M%S").ok()?;
    Some(ts.and_utc())
}

/// Delete oldest runs while count > keep_runs OR the archive is over
/// max_archive_mb, with three guards:
/// - the newest run is always kept and does not count toward the size
///   budget (its raw_log footer was just emitted);
/// - the size budget never deletes any of the newest `MIN_KEEP_RUNS`;
/// - runs younger than `MIN_PRUNE_AGE_SECS` are never deleted (a sibling
///   process's run in flight).
///
/// Errors ignored: deletion is idempotent and retried implicitly next run.
fn prune_at(root: &Path, cfg: &Config, now: chrono::DateTime<chrono::Utc>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort(); // run-ids sort oldest-first lexicographically
    let n = dirs.len();
    if n < 2 {
        return;
    }

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
    // The newest run is outside the budget: it is always kept anyway.
    let mut total: u64 = sizes[..n - 1].iter().sum();
    let max_bytes = cfg.max_archive_mb * 1024 * 1024;
    let too_young =
        |d: &Path| run_time(d).is_some_and(|t| (now - t).num_seconds() < MIN_PRUNE_AGE_SECS);

    // `i` is the oldest surviving run; `n - i` runs remain.
    let mut i = 0;
    while i + 1 < n {
        let remaining = n - i;
        let over_count = remaining > cfg.keep_runs;
        let over_size = total > max_bytes && remaining > MIN_KEEP_RUNS;
        // Ids are time-ordered: once one is too young, all newer ones are.
        if !(over_count || over_size) || too_young(&dirs[i]) {
            break;
        }
        let _ = std::fs::remove_dir_all(&dirs[i]);
        total = total.saturating_sub(sizes[i]);
        i += 1;
    }
    if total > max_bytes {
        eprintln!(
            "cartoon: archive is {} MB, over max_archive_mb = {}; keeping the newest {} runs so their raw_log paths stay valid",
            total / (1024 * 1024),
            cfg.max_archive_mb,
            n - i
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        create_run_dir_with, list_at, load_at, load_report_at, looks_like_run_id, new_run_id,
        prune_at, record_at, write_report, StoredItem, StoredReport,
    };
    use crate::runner::Captured;
    use std::path::Path;

    /// An archived run with an explicit id (so its age is under the test's
    /// control) and `bytes` of stdout.
    fn fake_run(root: &Path, id: &str, bytes: usize) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("stdout.log"), "x".repeat(bytes)).unwrap();
        std::fs::write(
            dir.join("meta.json"),
            format!(
                r#"{{"id":"{id}","ts":"","argv":["a"],"mode":"safe","exit":0,"cwd":"","stdout_bytes":{bytes},"stderr_bytes":0}}"#
            ),
        )
        .unwrap();
    }

    fn old_id(i: usize) -> String {
        format!("20200101-000000-{i:04x}")
    }

    fn ids(root: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

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
        for i in 1..=3 {
            fake_run(tmp.path(), &old_id(i), 1);
        }
        let mut small = cfg();
        small.keep_runs = 2;
        prune_at(tmp.path(), &small, chrono::Utc::now());
        assert_eq!(ids(tmp.path()), vec![old_id(2), old_id(3)], "oldest pruned");
    }

    #[test]
    fn size_budget_keeps_the_newest_runs() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 1..=8 {
            fake_run(tmp.path(), &old_id(i), 1024 * 1024);
        }
        let mut small = cfg();
        small.max_archive_mb = 2;
        prune_at(tmp.path(), &small, chrono::Utc::now());
        let left = ids(tmp.path());
        assert_eq!(left.len(), super::MIN_KEEP_RUNS, "{left:?}");
        assert_eq!(left.last(), Some(&old_id(8)));
    }

    #[test]
    fn one_oversized_run_does_not_wipe_older_runs() {
        // Regression: a single run larger than max_archive_mb used to prune
        // every older run, breaking raw_log pointers the agent still holds.
        let tmp = tempfile::tempdir().unwrap();
        for i in 1..=8 {
            fake_run(tmp.path(), &old_id(i), 1024);
        }
        fake_run(tmp.path(), &old_id(9), 3 * 1024 * 1024);
        let mut small = cfg();
        small.max_archive_mb = 1;
        prune_at(tmp.path(), &small, chrono::Utc::now());
        assert_eq!(
            ids(tmp.path()).len(),
            9,
            "the newest run is outside the budget"
        );
    }

    #[test]
    fn oversized_runs_never_prune_below_the_minimum_kept() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 1..=8 {
            fake_run(tmp.path(), &old_id(i), 4096);
        }
        let mut c = cfg();
        c.max_archive_mb = 0; // every byte is over the cap
        prune_at(tmp.path(), &c, chrono::Utc::now());
        let left = ids(tmp.path());
        assert_eq!(left.len(), super::MIN_KEEP_RUNS, "{left:?}");
        assert_eq!(left.last(), Some(&old_id(8)), "newest survives");
    }

    #[test]
    fn young_runs_are_never_pruned() {
        // A sibling process may have just reserved or written these.
        let tmp = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        fake_run(tmp.path(), &old_id(1), 1);
        for i in 0..3 {
            let id = format!("{}-{i:04x}", now.format("%Y%m%d-%H%M%S"));
            std::fs::create_dir_all(tmp.path().join(id)).unwrap(); // reserved, empty
        }
        let mut c = cfg();
        c.keep_runs = 1;
        prune_at(tmp.path(), &c, now);
        let left = ids(tmp.path());
        assert_eq!(left.len(), 3, "only the old run goes: {left:?}");
        assert!(!left.contains(&old_id(1)));
    }

    #[test]
    fn records_written_back_to_back_are_all_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let cap = captured("x", "");
        let mut c = cfg();
        c.keep_runs = 1;
        let a = record_at(tmp.path(), &["a".into()], "safe", &cap, 0, &[], &c).unwrap();
        let b = record_at(tmp.path(), &["b".into()], "safe", &cap, 0, &[], &c).unwrap();
        assert!(a.dir.exists() && b.dir.exists(), "neither is 60s old yet");
    }

    #[test]
    fn run_dir_creation_never_reuses_an_existing_id() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("taken")).unwrap();
        std::fs::write(tmp.path().join("taken/stdout.log"), "sibling's log").unwrap();
        let mut ids = vec!["fresh".to_string(), "taken".to_string()];
        let run = create_run_dir_with(tmp.path(), || ids.pop().unwrap()).unwrap();
        assert_eq!(run.id, "fresh");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("taken/stdout.log")).unwrap(),
            "sibling's log"
        );
    }

    #[cfg(unix)]
    #[test]
    fn archive_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("state/cartoon/runs");
        let cap = captured("secret token\n", "err\n");
        let run = record_at(&root, &["env".into()], "safe", &cap, 0, &[], &cfg()).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(&tmp.path().join("state/cartoon")), 0o700);
        assert_eq!(mode(&run.dir), 0o700);
        for f in ["stdout.log", "stderr.log", "meta.json"] {
            assert_eq!(mode(&run.dir.join(f)), 0o600, "{f}");
        }
    }

    #[test]
    fn unwritable_root_yields_none_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let not_a_dir = tmp.path().join("file");
        std::fs::write(&not_a_dir, "x").unwrap();
        let cap = captured("out", "");
        assert!(record_at(&not_a_dir, &["a".to_string()], "safe", &cap, 0, &[], &cfg()).is_none());
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

    #[test]
    fn report_round_trips_next_to_the_logs() {
        let tmp = tempfile::tempdir().unwrap();
        let cap = captured("x", "");
        let argv = ["pytest".to_string()];
        let run = record_at(tmp.path(), &argv, "pytest", &cap, 1, &[], &cfg()).unwrap();
        assert_eq!(load_report_at(tmp.path(), &run.id), None, "no report yet");
        let report = StoredReport {
            kind: "tests".into(),
            runner: "pytest".into(),
            failed: 1,
            total: Some(2),
            items: vec![StoredItem {
                id: "t.py::test_a".into(),
                loc: "t.py:3".into(),
                rule: String::new(),
                msg: "assert 1 == 2".into(),
            }],
            rendered: "runner: pytest".into(),
        };
        assert!(write_report(&run.dir, &report));
        assert_eq!(load_report_at(tmp.path(), &run.id), Some(report));
        assert_eq!(load_report_at(tmp.path(), "../x"), None);
    }

    #[test]
    fn run_id_shape() {
        assert!(looks_like_run_id(&new_run_id()));
        assert!(looks_like_run_id("20261005-120102-0a9f"));
        assert!(!looks_like_run_id("a.txt"));
        assert!(!looks_like_run_id("20261005-120102-0A9F"));
        assert!(!looks_like_run_id("20261005-120102-0a9f/.."));
    }
}
