//! `dotnet test` (VSTest). The TRX logger is built into the SDK, so
//! `--logger trx --results-directory <temp dir>` is injected: every test
//! project writes its own uniquely named `.trx` there, and all of them are
//! parsed and merged. A user's own `--logger trx` / `--results-directory` is
//! respected (their directory is read, keeping only files this run wrote).
//! MSBuild errors in the console are lifted into the report, so a project
//! that failed to build is never hidden behind another project's results.
use super::report::{trim_trace, Failure, TestReport};
use super::{basename, jvm_reports, Adapter, AdapterReport, Artifact, ParseOutcome, Prepared};
use crate::runner::Captured;
use anyhow::{bail, Context, Result};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub struct DotnetTest;

/// Runs that list or describe tests instead of running them.
const NON_RUN_FLAGS: &[&str] = &["-t", "--list-tests", "-h", "--help", "-?"];

impl Adapter for DotnetTest {
    fn name(&self) -> &'static str {
        "dotnet-test"
    }
    fn matches(&self) -> &'static str {
        "dotnet test (VSTest; injected --logger trx --results-directory)"
    }
    fn detect(&self, argv: &[String]) -> bool {
        let is_test = matches!(argv, [first, second, ..]
            if matches!(basename(first), "dotnet" | "dotnet.exe") && second == "test");
        is_test
            && !argv[2..]
                .iter()
                .take_while(|a| *a != "--")
                .any(|a| NON_RUN_FLAGS.contains(&a.as_str()))
            // Microsoft.Testing.Platform mode has no VSTest `--logger`.
            && !uses_testing_platform(Path::new("."))
    }
    fn prepare(&self, mut argv: Vec<String>) -> Prepared {
        let insert_at = argv.iter().position(|a| a == "--").unwrap_or(argv.len());
        let user_args = &argv[2..insert_at];
        let mut inject: Vec<String> = Vec::new();
        if !has_trx_logger(user_args) {
            inject.extend(["--logger".to_string(), "trx".to_string()]);
        }
        let artifact = if let Some(dir) = user_results_dir(user_args) {
            // Their directory may hold older runs: record what is there.
            jvm_reports::snapshot(&trx_files(Path::new(dir)))
        } else {
            tempfile::Builder::new()
                .prefix("cartoon-dotnet-")
                .tempdir()
                .ok()
                .map(|guard| {
                    let path = guard.path().join("results");
                    inject.push("--results-directory".into());
                    inject.push(path.display().to_string());
                    Artifact::Dir {
                        _guard: guard,
                        path,
                    }
                })
        };
        for (i, a) in inject.into_iter().enumerate() {
            argv.insert(insert_at + i, a);
        }
        Prepared { argv, artifact }
    }
    fn parse(&self, captured: &Captured, prepared: &Prepared) -> Result<ParseOutcome> {
        let files = match &prepared.artifact {
            // Our own temp directory: everything in it is this run's.
            Some(Artifact::Dir { path, .. }) => trx_files(path),
            _ => {
                let end = prepared
                    .argv
                    .iter()
                    .position(|a| a == "--")
                    .unwrap_or(prepared.argv.len());
                let dir = user_results_dir(&prepared.argv[2..end])
                    .context("no results directory to read")?;
                jvm_reports::written_this_run(prepared, &trx_files(Path::new(dir)))?
            }
        };
        if files.is_empty() {
            bail!("no TRX file was written by this run");
        }
        let mut reports = Vec::new();
        for f in &files {
            let xml = std::fs::read_to_string(f).with_context(|| format!("{}", f.display()))?;
            reports.push(parse_trx(&xml)?);
        }
        let mut report = super::report::merge(reports).context("no TRX report")?;
        let mut found = build_errors(&captured.stdout);
        found.extend(build_errors(&captured.stderr));
        jvm_reports::add_build_failures(&mut report, found);
        Ok(ParseOutcome {
            report: AdapterReport::Tests(report),
            // The console repeats every failure (stdout) and xUnit's
            // `[FAIL]` lines (stderr); build errors were lifted into the
            // report.
            passthrough_stdout: None,
            passthrough_stderr: None,
        })
    }
}

/// A `global.json` (here or above) opting into Microsoft.Testing.Platform,
/// whose `dotnet test` rejects VSTest's `--logger`.
fn uses_testing_platform(start: &Path) -> bool {
    let Ok(abs) = std::fs::canonicalize(start) else {
        return false;
    };
    abs.ancestors().any(|d| {
        std::fs::read_to_string(d.join("global.json"))
            .is_ok_and(|s| s.contains("Microsoft.Testing.Platform"))
    })
}

/// The value of each `-l` / `--logger` (both `--logger x` and `--logger:x`
/// / `--logger=x` forms).
fn logger_values(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if a == "-l" || a == "--logger" {
            if let Some(v) = args.get(i + 1) {
                out.push(v.as_str());
            }
        } else if let Some(v) = a
            .strip_prefix("--logger:")
            .or_else(|| a.strip_prefix("--logger="))
        {
            out.push(v);
        }
    }
    out
}

fn has_trx_logger(args: &[String]) -> bool {
    logger_values(args)
        .iter()
        .any(|v| v.to_ascii_lowercase().starts_with("trx"))
}

fn user_results_dir(args: &[String]) -> Option<&str> {
    for (i, a) in args.iter().enumerate() {
        if a == "--results-directory" {
            return args.get(i + 1).map(String::as_str);
        }
        if let Some(v) = a
            .strip_prefix("--results-directory=")
            .or_else(|| a.strip_prefix("--results-directory:"))
        {
            return Some(v);
        }
    }
    None
}

/// Every `*.trx` under `dir`, sorted.
fn trx_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_trx(dir, &mut out, 0);
    out.sort();
    out
}

fn collect_trx(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            if depth < 3 {
                collect_trx(&p, out, depth + 1);
            }
        } else if p.extension().is_some_and(|x| x == "trx") {
            out.push(p);
        }
    }
}

/// .NET runtime / test-framework frames.
const DOTNET_NOISE: &[&str] = &[
    "at System.",
    "at Xunit.",
    "at NUnit.Framework.",
    "at Microsoft.VisualStudio.TestPlatform.",
    "at Microsoft.VisualStudio.TestTools.",
    "at Microsoft.TestPlatform.",
    "at InvokeStub_",
];

/// One VSTest TRX document. Top-level `UnitTestResult`s are counted (an
/// MSTest data row's inner results belong to their parent): `Passed`;
/// `Failed` / `Error` / `Timeout` / `Aborted` fail; anything else
/// (`NotExecuted`, `Inconclusive`, ...) is skipped.
pub fn parse_trx(xml: &str) -> Result<TestReport> {
    let xml = xml.trim_start_matches('\u{feff}');
    let doc = roxmltree::Document::parse(xml).context("invalid TRX xml")?;
    if !doc.root_element().has_tag_name("TestRun") {
        bail!("not a TRX document");
    }
    let (mut total, mut passed, mut failed, mut skipped) = (0u64, 0u64, 0u64, 0u64);
    let mut duration_s = 0.0;
    let mut failures = Vec::new();
    let results = doc.descendants().filter(|n| {
        n.has_tag_name("UnitTestResult") && n.parent().is_some_and(|p| p.has_tag_name("Results"))
    });
    for r in results {
        total += 1;
        duration_s += r.attribute("duration").map(parse_duration).unwrap_or(0.0);
        match r.attribute("outcome").unwrap_or("") {
            "Passed" | "PassedButRunAborted" => passed += 1,
            "Failed" | "Error" | "Timeout" | "Aborted" => {
                failed += 1;
                failures.push(failure(r));
            }
            _ => skipped += 1,
        }
    }
    // The run failed without any failing test (a crashed test host, a
    // data collector error): its RunInfos say why.
    if failures.is_empty() {
        let summary_failed = doc
            .descendants()
            .find(|n| n.has_tag_name("ResultSummary"))
            .and_then(|n| n.attribute("outcome"))
            .is_some_and(|o| matches!(o, "Failed" | "Error" | "Aborted" | "Timeout"));
        if summary_failed {
            for info in doc
                .descendants()
                .filter(|n| n.has_tag_name("RunInfo") && n.attribute("outcome") == Some("Error"))
            {
                let text = child_text(info, "Text");
                failed += 1;
                failures.push(Failure {
                    id: "(run)".into(),
                    loc: String::new(),
                    msg: text.lines().next().unwrap_or("").trim().to_string(),
                    trace: trim_trace(&text).into_iter().skip(1).collect(),
                });
            }
        }
    }
    Ok(TestReport {
        runner: "dotnet",
        total,
        passed,
        failed,
        skipped,
        duration_s,
        failures,
    })
}

/// Text of the first descendant element named `tag`.
fn child_text(n: roxmltree::Node, tag: &str) -> String {
    n.descendants()
        .find(|c| c.has_tag_name(tag))
        .and_then(|c| c.text())
        .unwrap_or("")
        .to_string()
}

fn failure(r: roxmltree::Node) -> Failure {
    let id = r.attribute("testName").unwrap_or("").to_string();
    let message = child_text(r, "Message");
    let stack = child_text(r, "StackTrace");
    let msg = message.lines().next().unwrap_or("").trim().to_string();
    let mut trace: Vec<String> = message
        .lines()
        .skip(1)
        .map(|l| l.trim_end().to_string())
        .filter(|l| !l.trim().is_empty())
        .collect();
    let mut frames: Vec<String> = stack
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !DOTNET_NOISE.iter().any(|n| l.starts_with(n)))
        .collect();
    let found = frames
        .iter()
        .enumerate()
        .find_map(|(i, f)| frame_loc(f).map(|loc| (i, loc)));
    let loc = match found {
        Some((i, loc)) => {
            // The frame that only repeats `loc` says nothing more.
            frames.remove(i);
            loc
        }
        None => String::new(),
    };
    trace.extend(frames);
    Failure {
        id,
        loc,
        msg,
        trace,
    }
}

/// `at Ns.Class.Method() in /p/File.cs:line 14` → `/p/File.cs:14`.
fn frame_loc(frame: &str) -> Option<String> {
    let (_, rest) = frame.rsplit_once(" in ")?;
    let (file, line) = rest.rsplit_once(":line ")?;
    line.trim().parse::<u64>().ok()?;
    Some(format!("{file}:{}", line.trim()))
}

/// `00:00:01.2345678` → seconds.
fn parse_duration(d: &str) -> f64 {
    let mut parts = d.split(':');
    let (Some(h), Some(m), Some(s)) = (parts.next(), parts.next(), parts.next()) else {
        return 0.0;
    };
    let num = |x: &str| x.parse::<f64>().unwrap_or(0.0);
    num(h) * 3600.0 + num(m) * 60.0 + num(s)
}

/// MSBuild errors (`/p/A.cs(2,31): error CS0029: msg [/p/A.csproj]`,
/// `MSBUILD : error MSB1009: msg`), as (loc, msg). MSBuild repeats them in
/// its summary; add_build_failures drops the duplicates.
pub fn build_errors(text: &str) -> Vec<(String, String)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(concat!(
            r"^\s*(?:(?P<f>[^\s(][^(]*)\((?P<l>\d+),(?P<c>\d+)\)|(?P<src>[^:]+?)) ?: error (?P<code>[A-Z]+\d+): ",
            r"(?P<m>.+?)(?: \[[^\]]+\])?$",
        ))
        .unwrap()
    });
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(c) = re.captures(line.trim_end()) else {
            continue;
        };
        let msg = format!("{}: {}", &c["code"], &c["m"]);
        let loc = match c.name("f") {
            Some(f) => format!("{}:{}:{}", f.as_str(), &c["l"], &c["c"]),
            None => String::new(),
        };
        out.push((loc, msg));
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
        let p = format!(
            "{}/tests/fixtures/dotnet/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
    }

    #[test]
    fn detects_dotnet_test_only() {
        assert!(DotnetTest.detect(&argv(&["dotnet", "test"])));
        assert!(DotnetTest.detect(&argv(&["dotnet", "test", "All.sln", "-c", "Release"])));
        assert!(!DotnetTest.detect(&argv(&["dotnet", "build"])));
        assert!(!DotnetTest.detect(&argv(&["dotnet", "test", "--list-tests"])));
        assert!(!DotnetTest.detect(&argv(&["dotnet", "test", "-h"])));
        assert!(!DotnetTest.detect(&argv(&["dotnet"])));
        // After `--` it is a RunSettings argument, not a dotnet flag.
        assert!(DotnetTest.detect(&argv(&["dotnet", "test", "--", "-t"])));
    }

    #[test]
    fn testing_platform_global_json_declines() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!uses_testing_platform(dir.path()));
        std::fs::write(
            dir.path().join("global.json"),
            r#"{"test": {"runner": "Microsoft.Testing.Platform"}}"#,
        )
        .unwrap();
        let sub = dir.path().join("src");
        std::fs::create_dir(&sub).unwrap();
        assert!(uses_testing_platform(&sub));
    }

    #[test]
    fn prepare_injects_trx_logger_and_temp_results_dir_before_separator() {
        let p = DotnetTest.prepare(argv(&["dotnet", "test", "x.sln", "--", "A=1"]));
        let dir = p.artifact_path().unwrap();
        assert_eq!(
            p.argv,
            argv(&[
                "dotnet",
                "test",
                "x.sln",
                "--logger",
                "trx",
                "--results-directory",
                &dir.display().to_string(),
                "--",
                "A=1"
            ])
        );
        assert!(matches!(p.artifact, Some(Artifact::Dir { .. })));
    }

    #[test]
    fn prepare_respects_user_logger_and_results_directory() {
        let a = argv(&[
            "dotnet",
            "test",
            "--logger",
            "trx;LogFileName=r.trx",
            "--results-directory",
            "out",
        ]);
        let p = DotnetTest.prepare(a.clone());
        assert_eq!(p.argv, a);
        assert!(
            matches!(p.artifact, Some(Artifact::File(_))),
            "pre-run snapshot"
        );
        // Another logger is kept, and trx added next to it.
        let p = DotnetTest.prepare(argv(&[
            "dotnet",
            "test",
            "-l",
            "console;verbosity=detailed",
            "--results-directory=out",
        ]));
        assert_eq!(&p.argv[5..], &argv(&["--logger", "trx"])[..]);
    }

    #[test]
    fn parses_real_trx_with_theory_skip_and_exception() {
        let r = parse_trx(&fixture("UnitA.trx")).unwrap();
        assert_eq!(r.runner, "dotnet");
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (7, 3, 3, 1));
        let odd = r
            .failures
            .iter()
            .find(|f| f.id == "UnitA.CalculatorTests.IsOdd(n: 2)")
            .unwrap();
        assert_eq!(odd.msg, "2 is not odd");
        assert_eq!(odd.loc, "/home/dev/dn/UnitA/UnitTest1.cs:27");
        // Runtime frames and the frame that only repeats `loc` are dropped.
        assert_eq!(odd.trace, vec!["Expected: True", "Actual:   False"]);
        let boom = r
            .failures
            .iter()
            .find(|f| f.id == "UnitA.CalculatorTests.Throws")
            .unwrap();
        assert_eq!(boom.msg, "System.InvalidOperationException : boom");
        assert!(r.duration_s > 0.0);
    }

    #[test]
    fn merges_one_trx_per_test_project() {
        let reports = vec![
            parse_trx(&fixture("UnitA.trx")).unwrap(),
            parse_trx(&fixture("UnitB.trx")).unwrap(),
        ];
        let r = super::super::report::merge(reports).unwrap();
        assert_eq!((r.total, r.passed, r.failed, r.skipped), (9, 4, 4, 1));
    }

    #[test]
    fn non_trx_xml_is_an_error() {
        assert!(parse_trx("<testsuite/>").is_err());
        assert!(parse_trx("not xml").is_err());
    }

    #[test]
    fn run_error_without_failed_tests_is_reported() {
        let xml = r#"<TestRun xmlns="http://microsoft.com/schemas/VisualStudio/TeamTest/2010">
  <Results>
    <UnitTestResult testName="A.B.ok" outcome="Passed" duration="00:00:00.01"/>
  </Results>
  <ResultSummary outcome="Failed">
    <RunInfos>
      <RunInfo outcome="Error"><Text>The active test run was aborted. Reason: Test host process crashed</Text></RunInfo>
    </RunInfos>
  </ResultSummary>
</TestRun>"#;
        let r = parse_trx(xml).unwrap();
        assert_eq!((r.total, r.passed, r.failed), (1, 1, 1));
        assert_eq!(
            r.failures[0].msg,
            "The active test run was aborted. Reason: Test host process crashed"
        );
    }

    #[test]
    fn real_build_error_in_one_project_is_surfaced() {
        // UnitB did not compile; UnitA's tests ran and failed.
        let found = build_errors(&fixture("build-partial.stdout"));
        assert_eq!(
            found,
            vec![(
                "/home/dev/dn/UnitB/Broken.cs:2:31".to_string(),
                "CS0029: Cannot implicitly convert type 'string' to 'int'".to_string()
            )]
        );
        assert!(build_errors(&fixture("test-fail.stdout")).is_empty());
        assert_eq!(
            build_errors("MSBUILD : error MSB1009: Project file does not exist."),
            vec![(
                String::new(),
                "MSB1009: Project file does not exist.".to_string()
            )]
        );
    }

    #[test]
    fn duration_parses_hms() {
        assert!((parse_duration("00:01:02.5000000") - 62.5).abs() < 1e-9);
        assert_eq!(parse_duration("garbage"), 0.0);
    }
}
