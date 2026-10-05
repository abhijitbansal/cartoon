//! `cartoon last` and `cartoon diff`: the agent's edit → run → fix loop.
//!
//! An adapter run stores its structured report next to the raw logs
//! (`report.json`, see `archive::StoredReport`). `last` re-shows the newest
//! run without re-running it; `diff` compares two runs of the same command
//! and says what got fixed, what still fails and what newly fails, so the
//! agent never re-reads a whole report to find out whether its edit worked.
use crate::adapters::{report, AdapterReport};
use crate::archive::{self, RunMeta, StoredItem, StoredReport};
use anyhow::Result;
use serde_json::{json, Map, Value};
use std::path::Path;

/// The on-disk form of an adapter report: failing tests (id/loc/msg) or
/// diagnostics (loc/rule/msg), with paths relative to the cwd as rendered.
pub fn stored_from(rep: &AdapterReport, rendered: &str) -> StoredReport {
    let rel = report::relativize;
    match rep {
        AdapterReport::Tests(r) => StoredReport {
            kind: "tests".into(),
            runner: r.runner.into(),
            failed: r.failed.max(r.failures.len() as u64),
            total: Some(r.total),
            items: r
                .failures
                .iter()
                .map(|f| StoredItem {
                    id: rel(&f.id),
                    loc: rel(&f.loc),
                    rule: String::new(),
                    msg: rel(&f.msg),
                })
                .collect(),
            rendered: rendered.into(),
        },
        AdapterReport::Value(v) => {
            let mut v = v.clone();
            report::relativize_value(&mut v);
            let text = |x: &Value| x.as_str().unwrap_or_default().to_string();
            let items: Vec<StoredItem> = v["diagnostics"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|d| StoredItem {
                            id: String::new(),
                            loc: text(&d["loc"]),
                            rule: text(&d["rule"]),
                            // Location-less tool errors are bare strings.
                            msg: d.as_str().map_or_else(|| text(&d["msg"]), String::from),
                        })
                        .collect()
                })
                .unwrap_or_default();
            StoredReport {
                kind: "diagnostics".into(),
                runner: text(&v["runner"]),
                failed: items.len() as u64,
                total: None,
                items,
                rendered: rendered.into(),
            }
        }
    }
}

/// Called by the adapter flow once the run is archived: persist the report,
/// and when an earlier run of the same command (same argv, same cwd) has
/// one, return the one-line `vs_previous` summary for the footer. None when
/// there is no earlier run, or neither run reported anything.
pub fn on_archived(
    run: &archive::RunRef,
    argv: &[String],
    rep: &AdapterReport,
    rendered: &str,
) -> Option<String> {
    let cur = stored_from(rep, rendered);
    if !archive::write_report(&run.dir, &cur) {
        return None;
    }
    let root = run.dir.parent()?;
    let cwd = std::env::current_dir().ok()?.display().to_string();
    let key = command_key(argv);
    let prev = ordered_runs(root)
        .into_iter()
        .filter(|m| m.id != run.id && m.cwd == cwd && command_key(&m.argv) == key)
        .find_map(|m| archive::load_report_at(root, &m.id))?;
    if prev.items.is_empty() && cur.items.is_empty() {
        return None;
    }
    let d = compare(&prev, &cur);
    Some(format!(
        "{} {}; still {}; new {} (cartoon diff)",
        if fewer_tests(&prev, &cur) {
            "fixed or not run"
        } else {
            "fixed"
        },
        d.fixed.len(),
        d.still.len(),
        d.new.len()
    ))
}

/// What identifies "the same command": the argv as the user typed it (the
/// archive never stores adapter-injected flags), with a `sh -c` script's
/// whitespace normalized.
pub fn command_key(argv: &[String]) -> String {
    argv.iter()
        .map(|a| a.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

fn display_command(argv: &[String]) -> String {
    shell_words::join(argv.iter().map(String::as_str))
}

/// Runs newest first. Ids order runs to the second; two runs within the same
/// second (their salt is per-process) are ordered by when their meta.json
/// was written.
fn ordered_runs(root: &Path) -> Vec<RunMeta> {
    let mut runs: Vec<(RunMeta, Option<std::time::SystemTime>)> = archive::list_at(root, None)
        .into_iter()
        .map(|m| {
            let t = std::fs::metadata(root.join(&m.id).join("meta.json"))
                .and_then(|md| md.modified())
                .ok();
            (m, t)
        })
        .collect();
    runs.sort_by(|(a, ta), (b, tb)| {
        let sec = |m: &RunMeta| m.id.get(..15).unwrap_or("").to_string();
        sec(b).cmp(&sec(a)).then(tb.cmp(ta)).then(b.id.cmp(&a.id))
    });
    runs.into_iter().map(|(m, _)| m).collect()
}

fn matches_cmd(m: &RunMeta, cmd: Option<&str>) -> bool {
    cmd.is_none_or(|c| display_command(&m.argv).contains(c) || m.argv.join(" ").contains(c))
}

/// The outcome of comparing two runs' reports.
#[derive(Debug, Default, PartialEq)]
pub struct Delta {
    pub fixed: Vec<StoredItem>,
    pub still: Vec<StoredItem>,
    pub new: Vec<StoredItem>,
}

/// Tests match by id. Diagnostics match by (file, rule, msg): editing a file
/// shifts line numbers, and a diagnostic that only moved is not "fixed".
/// Duplicates match one-to-one, so two identical warnings dropping to one
/// counts one fixed.
fn match_key(it: &StoredItem) -> (String, String, String) {
    if !it.id.is_empty() {
        return (it.id.clone(), String::new(), String::new());
    }
    (
        file_of(&it.loc).to_string(),
        it.rule.clone(),
        it.msg.clone(),
    )
}

/// `src/a.py:12:5` → `src/a.py` (strips trailing `:<digits>` segments).
fn file_of(loc: &str) -> &str {
    let mut s = loc;
    while let Some((head, tail)) = s.rsplit_once(':') {
        if tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
            break;
        }
        s = head;
    }
    s
}

/// The current run executed fewer tests than the previous one (stopped at
/// the first failure, a `-k` filter, a crash): a previous failure missing
/// from it may simply not have run.
fn fewer_tests(prev: &StoredReport, cur: &StoredReport) -> bool {
    matches!((prev.total, cur.total), (Some(p), Some(c)) if c < p)
}

pub fn compare(prev: &StoredReport, cur: &StoredReport) -> Delta {
    let mut unmatched: Vec<Option<&StoredItem>> = prev.items.iter().map(Some).collect();
    let mut d = Delta::default();
    for it in &cur.items {
        let k = match_key(it);
        let hit = unmatched
            .iter_mut()
            .find(|p| p.is_some_and(|p| match_key(p) == k));
        match hit {
            Some(slot) => {
                *slot = None;
                d.still.push(it.clone());
            }
            None => d.new.push(it.clone()),
        }
    }
    d.fixed = unmatched.into_iter().flatten().cloned().collect();
    d
}

/// `cartoon last [--cmd <substring>]`: the newest run's report as it was
/// printed, plus where its raw log is. A run without an adapter report gets
/// a short summary instead.
pub fn run_last(cmd: Option<&str>) -> Result<i32> {
    let root = crate::paths::runs_dir().ok_or_else(|| anyhow::anyhow!("no state directory"))?;
    match render_last(&root, cmd) {
        Some(out) => {
            println!("{out}");
            Ok(0)
        }
        None => {
            eprintln!("{}", no_runs_message(cmd));
            Ok(1)
        }
    }
}

fn no_runs_message(cmd: Option<&str>) -> String {
    match cmd {
        Some(c) => format!("cartoon: no archived run matching {c:?} — try `cartoon logs`"),
        None => "cartoon: no archived runs yet".to_string(),
    }
}

pub fn render_last(root: &Path, cmd: Option<&str>) -> Option<String> {
    let meta = ordered_runs(root)
        .into_iter()
        .find(|m| matches_cmd(m, cmd))?;
    let raw_log = root.join(&meta.id).display().to_string();
    let mut foot = Map::new();
    foot.insert("run".into(), json!(meta.id));
    foot.insert("command".into(), json!(display_command(&meta.argv)));
    match archive::load_report_at(root, &meta.id) {
        Some(rep) => {
            if meta.exit != 0 {
                foot.insert("exit_code".into(), json!(meta.exit));
            }
            foot.insert("raw_log".into(), json!(raw_log));
            Some(format!(
                "{}\n{}",
                rep.rendered.trim_end(),
                crate::toon::encode(&Value::Object(foot))
            ))
        }
        None => {
            foot.insert("ts".into(), json!(meta.ts));
            foot.insert("mode".into(), json!(meta.mode));
            foot.insert("exit_code".into(), json!(meta.exit));
            foot.insert("stdout_bytes".into(), json!(meta.stdout_bytes));
            foot.insert("stderr_bytes".into(), json!(meta.stderr_bytes));
            foot.insert("raw_log".into(), json!(raw_log));
            foot.insert(
                "note".into(),
                json!(format!(
                    "no adapter report for this run; read it with cartoon logs {}",
                    meta.id
                )),
            );
            Some(crate::toon::encode(&Value::Object(foot)))
        }
    }
}

/// `cartoon diff [<id-a> <id-b>] [--cmd <substring>]`. Exit 0 (a query),
/// or 1 with a message when there is no pair of comparable runs.
pub fn run_diff(ids: Option<(String, String)>, cmd: Option<&str>) -> Result<i32> {
    let root = crate::paths::runs_dir().ok_or_else(|| anyhow::anyhow!("no state directory"))?;
    match render_diff(&root, ids, cmd) {
        Ok(out) => {
            println!("{out}");
            Ok(0)
        }
        Err(msg) => {
            eprintln!("cartoon: {msg}");
            Ok(1)
        }
    }
}

/// The diff as TOON, or why no comparison is possible.
pub fn render_diff(
    root: &Path,
    ids: Option<(String, String)>,
    cmd: Option<&str>,
) -> std::result::Result<String, String> {
    let runs = ordered_runs(root);
    let find = |id: &str| runs.iter().find(|m| m.id == id);
    fn with_report<'a>(root: &Path, m: &'a RunMeta) -> Option<(&'a RunMeta, StoredReport)> {
        archive::load_report_at(root, &m.id).map(|r| (m, r))
    }
    let with_report = |m| with_report(root, m);
    let ((pm, prev), (cm, cur)) = match &ids {
        Some((a, b)) => {
            let load = |id: &str| -> std::result::Result<_, String> {
                let m =
                    find(id).ok_or_else(|| format!("no archived run {id} — try `cartoon logs`"))?;
                with_report(m).ok_or_else(|| {
                    format!("run {id} has no adapter report to compare (only test/lint/build runs a cartoon adapter parsed have one)")
                })
            };
            (load(a)?, load(b)?)
        }
        None => {
            let current = runs
                .iter()
                .filter(|m| matches_cmd(m, cmd))
                .find_map(with_report)
                .ok_or_else(|| match cmd {
                    Some(c) => format!("no archived adapter run matching {c:?} — nothing to diff"),
                    None => "no archived adapter run (test/lint/build) yet — nothing to diff"
                        .to_string(),
                })?;
            let key = command_key(&current.0.argv);
            let previous = runs
                .iter()
                .filter(|m| {
                    m.id != current.0.id
                        && m.cwd == current.0.cwd
                        && command_key(&m.argv) == key
                })
                .find_map(with_report)
                .ok_or_else(|| {
                    format!(
                        "only one run of `{}` in {} — run it again after your edit, then `cartoon diff`",
                        display_command(&current.0.argv),
                        current.0.cwd
                    )
                })?;
            (previous, current)
        }
    };
    let d = compare(&prev, &cur);
    let tests = cur.kind == "tests";
    let entry = |it: &StoredItem, with_msg: bool| -> Value {
        let mut o = Map::new();
        if tests {
            o.insert("id".into(), json!(it.id));
            o.insert("loc".into(), json!(it.loc));
            if with_msg {
                o.insert("msg".into(), json!(it.msg));
            }
        } else {
            // A diagnostic has no descriptive id: its message is what says
            // what was fixed.
            o.insert("loc".into(), json!(it.loc));
            o.insert("rule".into(), json!(it.rule));
            o.insert("msg".into(), json!(it.msg));
        }
        Value::Object(o)
    };
    let side =
        |m: &RunMeta, r: &StoredReport| json!({"id": m.id, "exit": m.exit, "failed": r.failed});
    let fewer = fewer_tests(&prev, &cur);
    let mut v = json!({
        "command": display_command(&cm.argv),
        "previous": side(pm, &prev),
        "current": side(cm, &cur),
        "still_failing": d.still.iter().map(|i| entry(i, true)).collect::<Vec<_>>(),
        "new_failures": d.new.iter().map(|i| entry(i, true)).collect::<Vec<_>>(),
    });
    let fixed: Vec<Value> = d.fixed.iter().map(|i| entry(i, false)).collect();
    let o = v.as_object_mut().expect("object");
    if fewer {
        o.insert(
            "note".into(),
            json!(format!(
                "current run executed {} tests vs {} before (stopped early or filtered): \
                 previous failures it lacks may not have run",
                cur.total.unwrap_or(0),
                prev.total.unwrap_or(0)
            )),
        );
        o.insert("fixed_or_not_run".into(), json!(fixed));
    } else {
        o.insert("fixed".into(), json!(fixed));
    }
    Ok(crate::toon::encode(&v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::report::{Failure, TestReport};

    fn test_item(id: &str, loc: &str) -> StoredItem {
        StoredItem {
            id: id.into(),
            loc: loc.into(),
            rule: String::new(),
            msg: format!("{id} failed"),
        }
    }

    fn diag(loc: &str, rule: &str, msg: &str) -> StoredItem {
        StoredItem {
            id: String::new(),
            loc: loc.into(),
            rule: rule.into(),
            msg: msg.into(),
        }
    }

    fn rep(kind: &str, items: Vec<StoredItem>) -> StoredReport {
        StoredReport {
            kind: kind.into(),
            runner: "x".into(),
            failed: items.len() as u64,
            total: None,
            items,
            rendered: format!("runner: {kind}\n"),
        }
    }

    #[test]
    fn tests_match_by_id_not_line() {
        let prev = rep(
            "tests",
            vec![test_item("t::a", "t.py:3"), test_item("t::b", "t.py:9")],
        );
        let cur = rep(
            "tests",
            vec![test_item("t::b", "t.py:12"), test_item("t::c", "t.py:20")],
        );
        let d = compare(&prev, &cur);
        assert_eq!(d.fixed, vec![test_item("t::a", "t.py:3")]);
        assert_eq!(
            d.still,
            vec![test_item("t::b", "t.py:12")],
            "current loc wins"
        );
        assert_eq!(d.new, vec![test_item("t::c", "t.py:20")]);
    }

    #[test]
    fn diagnostics_ignore_line_shifts_and_match_duplicates_one_to_one() {
        let prev = rep(
            "diagnostics",
            vec![
                diag("a.py:3:1", "F401", "`os` imported but unused"),
                diag("a.py:10:5", "E711", "comparison to None"),
                diag("a.py:11:5", "E711", "comparison to None"),
            ],
        );
        let cur = rep(
            "diagnostics",
            vec![
                diag("a.py:14:5", "E711", "comparison to None"), // moved down
                diag("b.py:1:1", "F401", "`os` imported but unused"), // other file
            ],
        );
        let d = compare(&prev, &cur);
        assert_eq!(d.still.len(), 1);
        assert_eq!(d.still[0].loc, "a.py:14:5");
        assert_eq!(d.new.len(), 1);
        assert_eq!(d.new[0].loc, "b.py:1:1");
        let fixed: Vec<&str> = d.fixed.iter().map(|i| i.loc.as_str()).collect();
        assert_eq!(fixed, vec!["a.py:3:1", "a.py:11:5"]);
    }

    #[test]
    fn file_of_strips_line_and_column_only() {
        assert_eq!(file_of("src/a.rs:12:5"), "src/a.rs");
        assert_eq!(file_of("t.py:3"), "t.py");
        assert_eq!(file_of("C:/x/a.rs:1"), "C:/x/a.rs");
        assert_eq!(file_of("a.rs"), "a.rs");
    }

    #[test]
    fn command_key_normalizes_shell_whitespace() {
        let a = vec!["sh".into(), "-c".into(), "pytest  -q".into()];
        let b = vec!["sh".into(), "-c".into(), "pytest -q".into()];
        assert_eq!(command_key(&a), command_key(&b));
        assert_ne!(
            command_key(&["pytest".into(), "-q".into()]),
            command_key(&["pytest -q".into()])
        );
    }

    #[test]
    fn stored_from_test_report_keeps_failures() {
        let r = AdapterReport::Tests(TestReport {
            runner: "pytest",
            total: 2,
            passed: 1,
            failed: 1,
            skipped: 0,
            duration_s: 0.1,
            failures: vec![Failure {
                id: "t.py::test_a".into(),
                loc: "t.py:3".into(),
                msg: "assert 1 == 2".into(),
                trace: vec!["E assert".into()],
            }],
        });
        let s = stored_from(&r, "rendered");
        assert_eq!(s.kind, "tests");
        assert_eq!(s.failed, 1);
        assert_eq!(
            s.items,
            vec![StoredItem {
                id: "t.py::test_a".into(),
                loc: "t.py:3".into(),
                rule: String::new(),
                msg: "assert 1 == 2".into(),
            }]
        );
        assert_eq!(s.rendered, "rendered");
    }

    #[test]
    fn stored_from_diagnostics_value_takes_the_diagnostics_array() {
        let v = json!({
            "runner": "ruff",
            "summary": {"errors": 2, "warnings": 0},
            "diagnostics": [
                {"loc": "a.py:1:1", "severity": "error", "rule": "F401", "msg": "unused"},
                "ld: symbol(s) not found",
            ],
        });
        let s = stored_from(&AdapterReport::Value(v), "r");
        assert_eq!(s.kind, "diagnostics");
        assert_eq!(s.runner, "ruff");
        assert_eq!(s.items[0], diag("a.py:1:1", "F401", "unused"));
        assert_eq!(s.items[1].msg, "ld: symbol(s) not found");
    }

    /// Archive a run with `argv`/`cwd` and (optionally) a report.
    fn put(root: &Path, id: &str, argv: &[&str], cwd: &str, exit: i32, r: Option<&StoredReport>) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let meta = json!({
            "id": id, "ts": "", "argv": argv, "mode": "pytest", "exit": exit,
            "cwd": cwd, "stdout_bytes": 10, "stderr_bytes": 0,
        });
        std::fs::write(dir.join("meta.json"), meta.to_string()).unwrap();
        if let Some(r) = r {
            assert!(archive::write_report(&dir, r));
        }
    }

    #[test]
    fn fewer_tests_in_the_current_run_is_not_called_fixed() {
        let mut prev = rep(
            "tests",
            vec![test_item("t::a", "t.py:3"), test_item("t::b", "t.py:9")],
        );
        prev.total = Some(40);
        let mut cur = rep("tests", vec![test_item("t::a", "t.py:3")]);
        cur.total = Some(3);
        assert!(fewer_tests(&prev, &cur));
        cur.total = Some(40);
        assert!(!fewer_tests(&prev, &cur));
        prev.total = None;
        assert!(!fewer_tests(&prev, &cur), "unknown totals: no claim");
    }

    #[test]
    fn diff_defaults_to_the_previous_run_of_the_same_command_and_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let r1 = rep(
            "tests",
            vec![test_item("t::a", "t.py:3"), test_item("t::b", "t.py:9")],
        );
        let r2 = rep("tests", vec![test_item("t::b", "t.py:9")]);
        let other = rep("tests", vec![test_item("z::z", "z.py:1")]);
        put(
            root,
            "20261005-100000-0001",
            &["pytest"],
            "/p",
            1,
            Some(&r1),
        );
        put(
            root,
            "20261005-100001-0001",
            &["pytest"],
            "/elsewhere",
            1,
            Some(&other),
        );
        put(
            root,
            "20261005-100002-0001",
            &["pytest", "-x"],
            "/p",
            1,
            Some(&other),
        );
        put(
            root,
            "20261005-100003-0001",
            &["pytest"],
            "/p",
            1,
            Some(&r2),
        );
        put(
            root,
            "20261005-100004-0001",
            &["git", "status"],
            "/p",
            0,
            None,
        );
        let out = render_diff(root, None, None).unwrap();
        assert!(out.contains("command: pytest"), "{out}");
        assert!(out.contains("20261005-100000-0001"), "previous: {out}");
        assert!(out.contains("20261005-100003-0001"), "current: {out}");
        assert!(
            out.contains("fixed[1]{id,loc}:\n  \"t::a\",\"t.py:3\""),
            "{out}"
        );
        assert!(out.contains("still_failing[1]{id,loc,msg}:"), "{out}");
        assert!(out.contains("new_failures: []"), "{out}");
        assert!(!out.contains("z::z"), "{out}");
    }

    #[test]
    fn diff_without_a_comparable_pair_says_why() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(render_diff(root, None, None)
            .unwrap_err()
            .contains("nothing to diff"));
        let r = rep("tests", vec![]);
        put(root, "20261005-100000-0001", &["pytest"], "/p", 0, Some(&r));
        let e = render_diff(root, None, None).unwrap_err();
        assert!(e.contains("only one run of `pytest`"), "{e}");
        put(root, "20261005-100001-0001", &["ls"], "/p", 0, None);
        let ids = Some(("20261005-100000-0001".into(), "20261005-100001-0001".into()));
        assert!(render_diff(root, ids, None)
            .unwrap_err()
            .contains("no adapter report"));
    }

    #[test]
    fn diff_with_explicit_ids_and_cmd_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let lint1 = rep("diagnostics", vec![diag("a.py:3:1", "F401", "unused")]);
        let lint2 = rep("diagnostics", vec![]);
        let t = rep("tests", vec![test_item("t::a", "t.py:3")]);
        put(
            root,
            "20261005-100000-0001",
            &["ruff", "check"],
            "/p",
            1,
            Some(&lint1),
        );
        put(
            root,
            "20261005-100001-0001",
            &["ruff", "check"],
            "/p",
            0,
            Some(&lint2),
        );
        put(root, "20261005-100002-0001", &["pytest"], "/p", 1, Some(&t));
        let out = render_diff(root, None, Some("ruff")).unwrap();
        assert!(
            out.contains("fixed[1]{loc,rule,msg}:\n  \"a.py:3:1\",F401,unused"),
            "{out}"
        );
        let ids = Some(("20261005-100001-0001".into(), "20261005-100000-0001".into()));
        let out = render_diff(root, ids, None).unwrap();
        assert!(out.contains("new_failures[1]{loc,rule,msg}"), "{out}");
    }

    #[test]
    fn last_reshows_the_report_or_summarizes_a_plain_run() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let r = rep("tests", vec![test_item("t::a", "t.py:3")]);
        put(
            root,
            "20261005-100000-0001",
            &["pytest", "-q"],
            "/p",
            1,
            Some(&r),
        );
        put(root, "20261005-100001-0001", &["ls"], "/p", 0, None);
        let out = render_last(root, None).unwrap();
        assert!(out.contains("note: no adapter report"), "{out}");
        assert!(out.contains("raw_log:"), "{out}");
        let out = render_last(root, Some("pytest")).unwrap();
        assert!(out.starts_with("runner: tests\n"), "{out}");
        assert!(out.contains("exit_code: 1"), "{out}");
        assert!(out.contains("command: pytest -q"), "{out}");
        assert!(render_last(root, Some("cargo")).is_none());
    }
}
