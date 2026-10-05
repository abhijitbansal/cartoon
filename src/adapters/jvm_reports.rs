//! Shared harvest for build tools that write JUnit XML on their own
//! (gradle's `build/test-results`, maven's surefire/failsafe reports):
//! nothing is injected, the report files present before the run are
//! recorded, and every one written during the run is parsed and merged.
//! Build failures (compiler errors, a failed non-test task) are scanned
//! from the console streams so a partial build never reads as a clean test
//! report.
use super::report::{Failure, TestReport};
use super::{Artifact, Prepared};
use anyhow::{bail, Result};
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

/// Id given to every build-level failure, so it never collides with a
/// test's `Class.method` id.
pub const BUILD_FAILURE_ID: &str = "(build)";

/// Directories visited at most while looking for report directories: a
/// bound for huge monorepos (fail open: what was found is used).
const MAX_DIRS: usize = 20_000;
/// How deep below the project root module directories are searched.
const MAX_DEPTH: usize = 8;

/// A file's identity for change detection: mtime (ns) and size.
fn stamp(path: &Path) -> Option<(u128, u64)> {
    let m = std::fs::metadata(path).ok()?;
    let t = m
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((t, m.len()))
}

/// Record the report files that exist before the run (path, mtime, size)
/// in a temp file. After the run, only files that are new or changed since
/// count: an mtime cutoff with clock slack would let a report from a run
/// that finished a second earlier (gradle's daemon is that fast) pass as
/// this run's.
pub fn snapshot(files: &[PathBuf]) -> Option<Artifact> {
    use std::io::Write;
    let mut f = tempfile::Builder::new()
        .prefix("cartoon-reports-")
        .tempfile()
        .ok()?;
    for p in files {
        if let Some((t, len)) = stamp(p) {
            writeln!(f, "{t}\t{len}\t{}", p.display()).ok()?;
        }
    }
    f.flush().ok()?;
    Some(Artifact::File(f))
}

/// The files written by this run: those absent from the `snapshot`, or
/// whose mtime or size changed. Errors when the snapshot is missing.
pub fn written_this_run(prepared: &Prepared, files: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let Some(Artifact::File(snap)) = &prepared.artifact else {
        bail!("no pre-run snapshot of report files");
    };
    let before: HashMap<String, (u128, u64)> = std::fs::read_to_string(snap.path())?
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            let t = it.next()?.parse().ok()?;
            let len = it.next()?.parse().ok()?;
            Some((it.next()?.to_string(), (t, len)))
        })
        .collect();
    Ok(files
        .iter()
        .filter(|p| {
            let now = stamp(p);
            now.is_some() && before.get(&p.display().to_string()) != now.as_ref()
        })
        .cloned()
        .collect())
}

/// Every `TEST-*.xml` under `root` whose parent chain contains `<anchor>/<sub>`
/// (gradle: `build/test-results`; maven: `target/surefire-reports`), for
/// the project at `root` and its modules. `src`, hidden directories and
/// `node_modules` are never entered.
pub fn find_report_files(root: &Path, anchor: &str, subs: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut queue = vec![(root.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((dir, depth)) = queue.pop() {
        visited += 1;
        if visited > MAX_DIRS {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            if !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name == anchor {
                for sub in subs {
                    collect_xml(&e.path().join(sub), &mut out, 0);
                }
                // Build output never holds another module.
                continue;
            }
            if depth + 1 < MAX_DEPTH
                && !name.starts_with('.')
                && name != "node_modules"
                && name != "src"
            {
                queue.push((e.path(), depth + 1));
            }
        }
    }
    out.sort();
    out
}

fn collect_xml(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            if depth < 4 {
                collect_xml(&p, out, depth + 1);
            }
        } else if p.extension().is_some_and(|x| x == "xml")
            // Both tools name report files `TEST-<class>.xml`; maven's
            // `failsafe-summary.xml` and similar are not JUnit reports.
            && e.file_name().to_string_lossy().starts_with("TEST-")
        {
            out.push(p);
        }
    }
}

/// Parse and merge the report files written by this run. Errors when there
/// are none (an up-to-date task, a build that failed before testing) or
/// none parses: the pipeline then compresses the console output instead.
pub fn harvest(fresh: &[PathBuf], runner: &'static str) -> Result<TestReport> {
    if fresh.is_empty() {
        bail!("no test report was written by this run");
    }
    let mut reports = Vec::new();
    for f in fresh {
        let Ok(xml) = std::fs::read_to_string(f) else {
            continue;
        };
        // A module with no tests, or a half-written file, is skipped.
        let Ok(mut r) = super::pytest::parse_junit_named(&xml, runner) else {
            continue;
        };
        for fail in &mut r.failures {
            tidy_failure(fail, f);
        }
        reports.push(r);
    }
    match super::report::merge(reports) {
        Some(r) => Ok(r),
        None => bail!("no parseable test report was written by this run"),
    }
}

/// JVM stack frames from the test framework, the JDK and the build tool.
const JVM_NOISE: &[&str] = &[
    "org.junit.",
    "org.opentest4j.",
    "junit.framework.",
    "org.testng.internal.",
    "org.gradle.",
    "org.apache.maven.surefire.",
    "java.base/",
    "java.lang.reflect.",
    "java.util.",
    "jdk.internal.",
    "jdk.proxy",
    "sun.reflect.",
    "kotlin.coroutines.",
];

fn frame_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^at (?:[\w.$-]+(?:@[\w.-]+)?/+)?(?P<fq>[\w$.]+)\.(?P<m>[\w$<>-]+)\((?P<file>[\w$-]+\.(?:java|kt|groovy|scala)):(?P<line>\d+)\)",
        )
        .unwrap()
    })
}

/// Fill in a location from the first stack frame in the test's own class,
/// drop framework frames, and drop the trace line that repeats `msg`.
fn tidy_failure(f: &mut Failure, report_file: &Path) {
    if f.loc.is_empty() {
        let found = f
            .trace
            .iter()
            .enumerate()
            .find_map(|(i, l)| frame_loc(l, &f.id, report_file).map(|loc| (i, loc)));
        if let Some((i, loc)) = found {
            f.loc = loc;
            // A frame that only repeats `loc` costs tokens and says nothing.
            f.trace.remove(i);
        }
    }
    f.trace.retain(|l| {
        let t = l.trim_start();
        !(t.starts_with("at ") && JVM_NOISE.iter().any(|n| t.contains(n)))
    });
    // surefire's `message` drops the exception type that the trace's first
    // line carries (`boom` vs `java.lang.IllegalStateException: boom`):
    // prefer the typed line.
    if let Some(first) = f.trace.first() {
        if *first == f.msg || first.ends_with(&format!(": {}", f.msg)) {
            f.msg = f.trace.remove(0);
        }
    }
}

/// `at demo.AppTest.subtracts(AppTest.java:16)` for the test whose id is
/// `demo.AppTest.subtracts()` → `<module>/src/test/java/demo/AppTest.java:16`
/// when that source file exists, else `AppTest.java:16`.
fn frame_loc(line: &str, id: &str, report_file: &Path) -> Option<String> {
    let caps = frame_regex().captures(line.trim_start())?;
    let fq = &caps["fq"];
    // Only a frame in the test's own class (or a nested class of it).
    let outer = fq.split('$').next().unwrap_or(fq);
    if !id.starts_with(&format!("{outer}.")) && !id.starts_with(&format!("{fq}.")) {
        return None;
    }
    let file = &caps["file"];
    let line_no = &caps["line"];
    let pkg_dir = outer
        .rsplit_once('.')
        .map(|(p, _)| p.replace('.', "/"))
        .unwrap_or_default();
    let path = module_dir(report_file)
        .and_then(|m| find_source(&m, &pkg_dir, file))
        .map(|p| {
            // A cwd-relative root yields `./app/src/...`: drop the `./`.
            p.strip_prefix(".").unwrap_or(&p).display().to_string()
        })
        .unwrap_or_else(|| file.to_string());
    Some(format!("{path}:{line_no}"))
}

/// The module directory a report belongs to: the parent of its `build/`
/// or `target/` ancestor.
fn module_dir(report_file: &Path) -> Option<PathBuf> {
    report_file
        .ancestors()
        .find(|a| a.file_name().is_some_and(|n| n == "build" || n == "target"))
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

/// `<module>/src/<set>/<lang>/<pkg>/<file>` for any source set (`test`,
/// `integrationTest`, ...) and JVM language directory.
fn find_source(module: &Path, pkg_dir: &str, file: &str) -> Option<PathBuf> {
    let src = module.join("src");
    let sets = std::fs::read_dir(&src).ok()?;
    for set in sets.flatten() {
        for lang in ["java", "kotlin", "groovy", "scala"] {
            let p = set.path().join(lang).join(pkg_dir).join(file);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Append build-level failures to a test report, deduplicated by
/// (loc, msg): the console repeats compiler errors in its summary.
pub fn add_build_failures(report: &mut TestReport, found: Vec<(String, String)>) {
    let mut seen: Vec<(String, String)> = Vec::new();
    for (loc, msg) in found {
        let key = (loc.clone(), msg.clone());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        report.failures.push(Failure {
            id: BUILD_FAILURE_ID.into(),
            loc,
            msg,
            trace: Vec::new(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_loc_takes_the_tests_own_class_only() {
        let rf = Path::new("/nonexistent/app/build/test-results/test/TEST-x.xml");
        let id = "demo.AppTest.subtracts()";
        assert_eq!(
            frame_loc("at app//demo.AppTest.subtracts(AppTest.java:16)", id, rf),
            Some("AppTest.java:16".into())
        );
        assert_eq!(
            frame_loc("at demo.AppTest$Inner.run(AppTest.java:20)", id, rf),
            Some("AppTest.java:20".into())
        );
        assert_eq!(
            frame_loc(
                "at app//org.junit.jupiter.api.Assertions.assertEquals(Assertions.java:563)",
                id,
                rf
            ),
            None
        );
        assert_eq!(
            frame_loc("at demo.Other.helper(Other.java:3)", id, rf),
            None
        );
    }

    #[test]
    fn module_dir_is_the_parent_of_build_or_target() {
        assert_eq!(
            module_dir(Path::new("/p/lib/build/test-results/test/TEST-a.xml")),
            Some(PathBuf::from("/p/lib"))
        );
        assert_eq!(
            module_dir(Path::new("/p/core/target/surefire-reports/TEST-a.xml")),
            Some(PathBuf::from("/p/core"))
        );
    }

    #[test]
    fn build_failures_are_deduplicated() {
        let mut r = TestReport {
            runner: "gradle",
            total: 0,
            passed: 0,
            failed: 0,
            skipped: 0,
            duration_s: 0.0,
            failures: Vec::new(),
        };
        let e = ("a.java:3".to_string(), "boom".to_string());
        add_build_failures(&mut r, vec![e.clone(), e]);
        assert_eq!(r.failures.len(), 1);
        assert_eq!(r.failures[0].id, BUILD_FAILURE_ID);
    }

    #[test]
    fn only_reports_written_during_the_run_are_harvested() {
        let dir = tempfile::tempdir().unwrap();
        let xml =
            |n: &str| format!(r#"<testsuite><testcase classname="a.B" name="{n}()"/></testsuite>"#);
        let old = dir.path().join("TEST-old.xml");
        let rerun = dir.path().join("TEST-rerun.xml");
        std::fs::write(&old, xml("old")).unwrap();
        std::fs::write(&rerun, xml("rerun")).unwrap();
        let prepared = Prepared {
            argv: Vec::new(),
            artifact: snapshot(&[old.clone(), rerun.clone()]),
        };
        // The run rewrites one file (size differs) and adds another.
        std::fs::write(&rerun, xml("rerun_again")).unwrap();
        let new = dir.path().join("TEST-new.xml");
        std::fs::write(&new, xml("new")).unwrap();
        let all = vec![new.clone(), old, rerun.clone()];
        let fresh = written_this_run(&prepared, &all).unwrap();
        assert_eq!(fresh, vec![new, rerun]);
        let r = harvest(&fresh, "gradle").unwrap();
        assert_eq!((r.total, r.passed), (2, 2));
        assert!(harvest(&[], "gradle").is_err());
        let none = Prepared {
            argv: Vec::new(),
            artifact: None,
        };
        assert!(written_this_run(&none, &all).is_err());
    }
}
