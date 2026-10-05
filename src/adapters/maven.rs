//! `mvn` / `mvnw` test phases. Nothing is injected: surefire (and
//! failsafe) write `TEST-<class>.xml` under each module's
//! `target/surefire-reports/` (`target/failsafe-reports/`), so after the run
//! every file written by it is harvested and merged. A run that wrote none
//! (a build that failed before testing) is a parse error and falls back to
//! the generic ladder over maven's own console output.
use super::jvm_reports::{self, add_build_failures};
use super::{basename, Adapter, AdapterReport, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::Result;
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub struct Maven;

/// Options that take the next token as their value (skipped when looking
/// for lifecycle phases).
const VALUE_OPTS: &[&str] = &[
    "-f",
    "--file",
    "-s",
    "--settings",
    "-gs",
    "--global-settings",
    "-t",
    "--toolchains",
    "-pl",
    "--projects",
    "-rf",
    "--resume-from",
    "-T",
    "--threads",
    "-l",
    "--log-file",
    "-P",
    "--activate-profiles",
    "-b",
    "--builder",
    "-D",
    "--define",
];

/// Phases (and plugin goals) that run tests.
const TEST_PHASES: &[&str] = &["test", "integration-test", "verify", "package", "install"];

impl Adapter for Maven {
    fn name(&self) -> &'static str {
        "maven"
    }
    fn matches(&self) -> &'static str {
        "mvn | ./mvnw test / verify / package / install (reads surefire/failsafe reports)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        let Some(first) = argv.first() else {
            return false;
        };
        if !matches!(basename(first), "mvn" | "mvnw" | "mvn.cmd" | "mvnw.cmd") {
            return false;
        }
        let args = &argv[1..];
        if args.iter().any(|a| skips_tests(a)) {
            return false;
        }
        phases(args).iter().any(|p| {
            TEST_PHASES.contains(p) || p.ends_with(":test") || p.ends_with(":integration-test")
        })
    }
    fn prepare(&self, argv: Vec<String>) -> Prepared {
        let artifact = jvm_reports::snapshot(&report_files(&argv));
        Prepared { argv, artifact }
    }
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        let fresh = jvm_reports::written_this_run(prepared, &report_files(&prepared.argv))?;
        let mut report = jvm_reports::harvest(&fresh, "maven")?;
        let mut found = build_failures(&captured.stdout);
        found.extend(build_failures(&captured.stderr));
        add_build_failures(&mut report, found);
        Ok(ParseOutcome {
            report: AdapterReport::Tests(report),
            // maven's console is reactor progress plus its own copy of every
            // failure; build failures were lifted into the report.
            passthrough_stdout: None,
            passthrough_stderr: None,
        })
    }
}

/// Every module's surefire and failsafe `TEST-*.xml`.
fn report_files(argv: &[String]) -> Vec<PathBuf> {
    let root = project_dir(argv.get(1..).unwrap_or(&[]));
    jvm_reports::find_report_files(&root, "target", &["surefire-reports", "failsafe-reports"])
}

/// `-DskipTests`, `-Dmaven.test.skip=true` (and `-DskipTests=true`): no
/// report will be written.
fn skips_tests(a: &str) -> bool {
    matches!(
        a,
        "-h" | "--help" | "-v" | "--version" | "-DskipTests" | "-DskipTests=true"
    ) || a == "-Dmaven.test.skip"
        || a == "-Dmaven.test.skip=true"
}

/// Positional phases / goals, skipping option values.
fn phases(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with('-') {
            skip = VALUE_OPTS.contains(&a.as_str());
            continue;
        }
        out.push(a.as_str());
    }
    out
}

/// The project root: the directory of `-f <pom>` / `--file[=]<pom|dir>`,
/// else the cwd.
fn project_dir(args: &[String]) -> PathBuf {
    let mut file: Option<&str> = None;
    for (i, a) in args.iter().enumerate() {
        if let Some(v) = a.strip_prefix("--file=") {
            file = Some(v);
        } else if a == "-f" || a == "--file" {
            file = args.get(i + 1).map(String::as_str);
        }
    }
    match file {
        Some(f) if Path::new(f).is_dir() => PathBuf::from(f),
        Some(f) => Path::new(f)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
        None => PathBuf::from("."),
    }
}

/// Build-level failures in maven's console: compiler errors
/// (`[ERROR] /p/A.java:[3,11] msg`) and every `Failed to execute goal`
/// except the one test failures cause (the report already shows those).
/// Returned as (loc, msg).
pub fn build_failures(text: &str) -> Vec<(String, String)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(concat!(
            r"^\[ERROR\] (?:file://)?(?P<f>[^\s\[][^\[]*?\.(?:java|kt|groovy|scala)):\[(?P<l>\d+),(?P<c>\d+)\] (?P<m>.+)$",
            r"|^\[ERROR\] Failed to execute goal (?P<g>.+)$",
        ))
        .unwrap()
    });
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        let Some(c) = re.captures(line) else {
            continue;
        };
        if let Some(g) = c.name("g") {
            let g = g.as_str();
            if g.contains("There are test failures") || g.contains("There were test failures") {
                continue;
            }
            out.push((String::new(), format!("Failed to execute goal {g}")));
        } else {
            out.push((
                format!("{}:{}:{}", &c["f"], &c["l"], &c["c"]),
                c["m"].into(),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn fixture(name: &str) -> String {
        let p = format!("{}/tests/fixtures/maven/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
    }

    #[test]
    fn detects_test_phases() {
        for a in [
            &["mvn", "test"][..],
            &["./mvnw", "-B", "verify"],
            &["mvn", "clean", "package"],
            &["mvn", "-pl", "core", "install"],
            &["mvn", "-Dtest=CoreTest", "test"],
            &["mvn", "surefire:test"],
            &["mvn", "-q", "-f", "sub/pom.xml", "test"],
        ] {
            assert!(Maven.detect(&argv(a)), "{a:?}");
        }
    }

    #[test]
    fn declines_runs_without_tests() {
        for a in [
            &["mvn", "compile"][..],
            &["mvn", "clean"],
            &["mvn", "package", "-DskipTests"],
            &["mvn", "install", "-Dmaven.test.skip=true"],
            &["mvn", "--version"],
            &["mvn"],
            // `-pl test`: a module named test, not the phase.
            &["mvn", "-pl", "test", "compile"],
        ] {
            assert!(!Maven.detect(&argv(a)), "{a:?}");
        }
    }

    #[test]
    fn prepare_injects_nothing() {
        let a = argv(&["mvn", "-B", "test"]);
        let p = Maven.prepare(a.clone());
        assert_eq!(p.argv, a);
        assert!(p.artifact_path().is_some(), "pre-run snapshot");
    }

    #[test]
    fn project_dir_follows_the_pom() {
        assert_eq!(project_dir(&argv(&["test"])), PathBuf::from("."));
        assert_eq!(
            project_dir(&argv(&["-f", "sub/pom.xml", "test"])),
            PathBuf::from("sub")
        );
        assert_eq!(
            project_dir(&argv(&["--file=pom.xml", "test"])),
            PathBuf::from(".")
        );
    }

    #[test]
    fn real_compile_failure_alongside_test_failures_is_surfaced() {
        // `mvn -fae test`: web's test sources did not compile, core's tests
        // ran and failed. The compiler errors and the goal failure surface;
        // the surefire "There are test failures" goal failure does not.
        let found = build_failures(&fixture("compile-partial.stdout"));
        let msgs: Vec<String> = found.iter().map(|(l, m)| format!("{l} {m}")).collect();
        assert!(
            msgs.iter().any(|m| m.ends_with(
                "web/src/test/java/demo/Broken.java:3:11 incompatible types: java.lang.String cannot be converted to int"
            )),
            "{msgs:#?}"
        );
        assert!(
            msgs.iter()
                .any(|m| m.contains("testCompile (default-testCompile) on project web")),
            "{msgs:#?}"
        );
        assert!(
            !msgs.iter().any(|m| m.contains("test failures")),
            "{msgs:#?}"
        );
        // Compiler errors are listed twice by maven; duplicates are kept
        // here and dropped by add_build_failures.
        assert!(found.len() >= 3, "{msgs:#?}");
    }

    #[test]
    fn test_failure_only_run_has_no_build_failures() {
        assert!(build_failures(&fixture("test-fail.stdout")).is_empty());
    }

    #[test]
    fn harvests_surefire_and_failsafe_reports_of_every_module() {
        let dir = tempfile::tempdir().unwrap();
        for (module, sub, file) in [
            ("core", "surefire-reports", "TEST-demo.CoreTest.xml"),
            ("web", "failsafe-reports", "TEST-demo.WebTest.xml"),
        ] {
            let d = dir.path().join(module).join("target").join(sub);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(file), fixture(file)).unwrap();
            // Not a JUnit report: never parsed.
            std::fs::write(d.join("failsafe-summary.xml"), "<x/>").unwrap();
        }
        let src = dir.path().join("core/src/test/java/demo");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("CoreTest.java"), fixture("CoreTest.java")).unwrap();
        let files = jvm_reports::find_report_files(
            dir.path(),
            "target",
            &["surefire-reports", "failsafe-reports"],
        );
        assert_eq!(files.len(), 2, "{files:?}");
        let r = jvm_reports::harvest(&files, "maven").unwrap();
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (7, 3, 3, 1));
        let boom = r
            .failures
            .iter()
            .find(|f| f.id == "demo.CoreTest.throwsUnexpectedly")
            .unwrap();
        assert_eq!(boom.msg, "java.lang.IllegalStateException: boom");
        let want = dir.path().join("core/src/test/java/demo/CoreTest.java:21");
        assert_eq!(boom.loc, want.display().to_string());
    }
}
