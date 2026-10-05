//! `gradle` / `gradlew` test tasks. Nothing is injected: gradle already
//! writes one JUnit XML file per test class under each module's
//! `build/test-results/`, so after the run every file written by it is
//! harvested and merged. A run that wrote none (an up-to-date task, a build
//! that failed before testing) is a parse error and falls back to the
//! generic ladder over gradle's own console output.
use super::jvm_reports::{self, add_build_failures};
use super::{basename, Adapter, AdapterReport, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::Result;
use regex::Regex;
use std::path::PathBuf;
use std::sync::OnceLock;

pub struct Gradle;

/// Options that take the next token as their value: skipped when looking
/// for task names (`-x test` EXCLUDES the test task).
const VALUE_OPTS: &[&str] = &[
    "-x",
    "--exclude-task",
    "-p",
    "--project-dir",
    "-c",
    "--settings-file",
    "-b",
    "--build-file",
    "-I",
    "--init-script",
    "-g",
    "--gradle-user-home",
    "--console",
    "--warning-mode",
    "--include-build",
    "--project-cache-dir",
    "--priority",
    "--tests",
    "-M",
    "--write-verification-metadata",
];

/// Runs that never execute tests (or never end).
const NON_RUN_FLAGS: &[&str] = &[
    "-t",
    "--continuous",
    "-h",
    "--help",
    "-?",
    "-v",
    "--version",
    "-m",
    "--dry-run",
];

impl Adapter for Gradle {
    fn name(&self) -> &'static str {
        "gradle"
    }
    fn matches(&self) -> &'static str {
        "gradle | ./gradlew test / check / build / *Test (reads build/test-results)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        let Some(first) = argv.first() else {
            return false;
        };
        if !matches!(
            basename(first),
            "gradle" | "gradlew" | "gradle.bat" | "gradlew.bat"
        ) {
            return false;
        }
        if argv[1..]
            .iter()
            .any(|a| NON_RUN_FLAGS.contains(&a.as_str()))
        {
            return false;
        }
        tasks(&argv[1..]).iter().any(|t| runs_tests(t))
    }
    fn prepare(&self, argv: Vec<String>) -> Prepared {
        let artifact = jvm_reports::snapshot(&report_files(&argv));
        Prepared { argv, artifact }
    }
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        let fresh = jvm_reports::written_this_run(prepared, &report_files(&prepared.argv))?;
        let mut report = jvm_reports::harvest(&fresh, "gradle")?;
        let mut found = build_failures(&captured.stdout);
        found.extend(build_failures(&captured.stderr));
        add_build_failures(&mut report, found);
        Ok(ParseOutcome {
            report: AdapterReport::Tests(report),
            // gradle's console is task progress plus a per-failure line
            // that the report replaces; build failures were lifted into it.
            passthrough_stdout: None,
            passthrough_stderr: None,
        })
    }
}

/// Every module's `build/test-results/**/TEST-*.xml`.
fn report_files(argv: &[String]) -> Vec<PathBuf> {
    let root = project_dir(argv.get(1..).unwrap_or(&[]));
    jvm_reports::find_report_files(&root, "build", &["test-results"])
}

/// Positional task names, skipping option values.
fn tasks(args: &[String]) -> Vec<&str> {
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

/// `test`, `check`, `build`, `:app:test`, `integrationTest`,
/// `testDebugUnitTest`: the last path segment names the task.
fn runs_tests(task: &str) -> bool {
    let name = task.rsplit(':').next().unwrap_or(task);
    matches!(name, "test" | "check" | "build") || name.ends_with("Test")
}

/// The project root: `-p <dir>` / `--project-dir[=]<dir>`, else the cwd.
fn project_dir(args: &[String]) -> PathBuf {
    for (i, a) in args.iter().enumerate() {
        if let Some(v) = a.strip_prefix("--project-dir=") {
            return PathBuf::from(v);
        }
        if a == "-p" || a == "--project-dir" {
            if let Some(v) = args.get(i + 1) {
                return PathBuf::from(v);
            }
        }
    }
    PathBuf::from(".")
}

/// Build-level failures in gradle's console: javac / kotlinc errors and
/// every `* What went wrong:` block except the one test failures cause
/// (the report already shows those). Returned as (loc, msg).
pub fn build_failures(text: &str) -> Vec<(String, String)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(concat!(
            // javac / groovyc / scalac: `/p/A.java:3: error: msg`
            r"^(?P<jf>[^\s:][^:]*\.(?:java|groovy|scala)):(?P<jl>\d+): error: (?P<jm>.+)$",
            // kotlinc 2.x: `e: file:///p/A.kt:3:5 msg`
            r"|^e: (?:file://)?(?P<kf>\S+?\.kts?):(?P<kl>\d+):(?P<kc>\d+) (?P<km>.+)$",
            // kotlinc 1.x: `e: /p/A.kt: (3, 5): msg`
            r"|^e: (?P<of>\S+?\.kts?): \((?P<ol>\d+), (?P<oc>\d+)\): (?P<om>.+)$",
        ))
        .unwrap()
    });
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        let Some(c) = re.captures(line) else {
            continue;
        };
        let get = |n: &str| c.name(n).map(|m| m.as_str()).unwrap_or("");
        if c.name("jf").is_some() {
            out.push((format!("{}:{}", get("jf"), get("jl")), get("jm").into()));
        } else if c.name("kf").is_some() {
            out.push((
                format!("{}:{}:{}", get("kf"), get("kl"), get("kc")),
                get("km").into(),
            ));
        } else {
            out.push((
                format!("{}:{}:{}", get("of"), get("ol"), get("oc")),
                get("om").into(),
            ));
        }
    }
    out.extend(what_went_wrong(text));
    out
}

/// `* What went wrong:` blocks: the header line plus its first `> reason`.
fn what_went_wrong(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(l) = lines.next() {
        if l.trim() != "* What went wrong:" {
            continue;
        }
        let mut block: Vec<&str> = Vec::new();
        for b in lines.by_ref() {
            let t = b.trim();
            if t.is_empty() || t.starts_with("* ") {
                break;
            }
            block.push(t);
        }
        if block.iter().any(|b| b.contains("There were failing tests")) {
            continue;
        }
        let Some(head) = block.first() else {
            continue;
        };
        let reason = block
            .iter()
            .skip(1)
            .find_map(|b| b.strip_prefix("> "))
            .map(|r| format!(" {r}"))
            .unwrap_or_default();
        out.push((String::new(), format!("{head}{reason}")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn fixture(name: &str) -> String {
        let p = format!(
            "{}/tests/fixtures/gradle/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
    }

    #[test]
    fn detects_test_tasks() {
        for a in [
            &["gradle", "test"][..],
            &["./gradlew", "check"],
            &["./gradlew", "clean", "build"],
            &["gradle", ":app:test", "--tests", "demo.AppTest"],
            &["./gradlew", "testDebugUnitTest"],
            &["gradlew.bat", "integrationTest"],
            &["gradle", "-p", "sub", "test"],
        ] {
            assert!(Gradle.detect(&argv(a)), "{a:?}");
        }
    }

    #[test]
    fn declines_non_test_runs() {
        for a in [
            &["gradle", "assemble"][..],
            &["gradle", "tasks"],
            &["gradle", "test", "--continuous"],
            &["gradle", "-t", "test"],
            &["gradle", "--version"],
            &["gradle", "test", "--dry-run"],
            &["gradle"],
            &["make", "test"],
        ] {
            assert!(!Gradle.detect(&argv(a)), "{a:?}");
        }
        // `-x test` excludes the task; its value is not a task name.
        assert!(!Gradle.detect(&argv(&["gradle", "assemble", "-x", "test"])));
        // `--tests <filter>` value is not a task either.
        assert!(!Gradle.detect(&argv(&["gradle", "jar", "--tests", "Test"])));
    }

    #[test]
    fn prepare_injects_nothing() {
        let a = argv(&["./gradlew", "test", "--info"]);
        let p = Gradle.prepare(a.clone());
        assert_eq!(p.argv, a);
        assert!(p.artifact_path().is_some(), "pre-run snapshot");
    }

    #[test]
    fn project_dir_option_forms() {
        assert_eq!(project_dir(&argv(&["test"])), PathBuf::from("."));
        assert_eq!(
            project_dir(&argv(&["-p", "sub", "test"])),
            PathBuf::from("sub")
        );
        assert_eq!(
            project_dir(&argv(&["--project-dir=x/y", "test"])),
            PathBuf::from("x/y")
        );
    }

    #[test]
    fn real_compile_failure_alongside_test_failures_is_surfaced() {
        // `gradle test --continue`: :app:test failed, :lib:compileTestJava
        // did not compile. Both javac errors and the task failure surface;
        // the failing-tests block (already in the report) does not.
        let mut found = build_failures(&fixture("compile-partial.stdout"));
        found.extend(build_failures(&fixture("compile-partial.stderr")));
        let msgs: Vec<String> = found.iter().map(|(l, m)| format!("{l} {m}")).collect();
        assert!(
            msgs.iter().any(|m| m.ends_with(
                "lib/src/test/java/demo/Broken.java:3 incompatible types: String cannot be converted to int"
            )),
            "{msgs:#?}"
        );
        assert!(
            msgs.iter()
                .any(|m| m.ends_with("Broken.java:4 cannot find symbol")),
            "{msgs:#?}"
        );
        assert!(
            msgs.iter().any(|m| m
                .contains("Execution failed for task ':lib:compileTestJava'. Compilation failed")),
            "{msgs:#?}"
        );
        assert!(!msgs.iter().any(|m| m.contains(":app:test")), "{msgs:#?}");
        // The indented copies inside the block are not counted again.
        assert_eq!(found.len(), 3, "{msgs:#?}");
    }

    #[test]
    fn test_failure_only_run_has_no_build_failures() {
        let mut found = build_failures(&fixture("test-fail.stdout"));
        found.extend(build_failures(&fixture("test-fail.stderr")));
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn kotlin_compiler_errors_both_formats() {
        let text = "\
e: file:///p/app/src/main/kotlin/A.kt:3:5 Unresolved reference 'foo'.
e: /p/app/src/main/kotlin/B.kt: (7, 9): Type mismatch: inferred type is String but Int was expected
w: file:///p/app/src/main/kotlin/C.kt:1:1 Unused variable
";
        let found = build_failures(text);
        assert_eq!(
            found,
            vec![
                (
                    "/p/app/src/main/kotlin/A.kt:3:5".to_string(),
                    "Unresolved reference 'foo'.".to_string()
                ),
                (
                    "/p/app/src/main/kotlin/B.kt:7:9".to_string(),
                    "Type mismatch: inferred type is String but Int was expected".to_string()
                ),
            ]
        );
    }

    /// Lay the real multi-module report files out as gradle does.
    fn multi_project(dir: &Path) {
        for (module, file) in [
            ("app", "TEST-demo.AppTest.xml"),
            ("lib", "TEST-demo.LibTest.xml"),
        ] {
            let d = dir.join(module).join("build/test-results/test");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(file), fixture(file)).unwrap();
        }
        let src = dir.join("app/src/test/java/demo");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("AppTest.java"), fixture("AppTest.java")).unwrap();
    }

    #[test]
    fn harvests_every_module_written_by_this_run() {
        let dir = tempfile::tempdir().unwrap();
        multi_project(dir.path());
        let files = jvm_reports::find_report_files(dir.path(), "build", &["test-results"]);
        assert_eq!(files.len(), 2, "{files:?}");
        let r = jvm_reports::harvest(&files, "gradle").unwrap();
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (7, 3, 3, 1));
        let sub = r
            .failures
            .iter()
            .find(|f| f.id == "demo.AppTest.subtracts()")
            .unwrap();
        assert_eq!(
            sub.msg,
            "org.opentest4j.AssertionFailedError: subtraction is off ==> expected: <1> but was: <2>"
        );
        // Located in the test's source file, found from the stack frame.
        let want = dir.path().join("app/src/test/java/demo/AppTest.java:16");
        assert_eq!(sub.loc, want.display().to_string());
        // Framework frames, the repeated message and the frame that only
        // repeats `loc` are dropped.
        assert!(sub.trace.is_empty(), "{:?}", sub.trace);
    }
}
