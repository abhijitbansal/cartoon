//! End-to-end tests for the gradle, maven, dotnet-test, golangci-lint and
//! pkg-script adapters.
//!
//! The JVM/.NET build tools and golangci-lint are not installed in CI, so the
//! always-on tests drive the real cartoon binary against stand-in scripts on
//! PATH that replay output captured from the real tools (gradle 8.14,
//! maven 3.9 + surefire 3.2, dotnet 8.0 SDK, golangci-lint 2.5; see
//! tests/fixtures/<tool>/). The `#[ignore]`d `real_*` tests run the tools
//! themselves: `cargo test --test e2e_adapters2 -- --ignored`.
use assert_cmd::Command;
use std::path::{Path, PathBuf};

// `have()` panics instead of skipping under CARTOON_E2E_STRICT=1.
mod common;
use common::have;

fn cartoon() -> Command {
    Command::cargo_bin("cartoon").unwrap()
}

fn fixtures(tool: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(tool)
}

/// A cartoon invocation isolated from the developer's state, run in `cwd`
/// with `bin` (stand-in tools) first on PATH.
fn cartoon_in(tmp: &Path, cwd: &Path, bin: Option<&Path>) -> Command {
    let mut c = cartoon();
    c.env("XDG_STATE_HOME", tmp.join("state"))
        .env("XDG_CONFIG_HOME", tmp.join("config"))
        .current_dir(cwd);
    if let Some(bin) = bin {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![bin.to_path_buf()];
        dirs.extend(std::env::split_paths(&path));
        c.env("PATH", std::env::join_paths(dirs).unwrap());
    }
    c
}

/// Write an executable stand-in `name` into `bin`.
#[cfg(unix)]
fn stand_in(bin: &Path, name: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(bin).unwrap();
    let p = bin.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{script}")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn stdout_of(a: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(a.get_output().stdout.clone()).unwrap()
}

/// A stand-in gradle: writes `TEST-*.xml` into `<module>/build/test-results`
/// for each `module=file` pair, replays the captured console, exits 1.
#[cfg(unix)]
fn fake_gradle(bin: &Path, reports: &[(&str, &str)], console: &str) {
    let fx = fixtures("gradle");
    let mut s = String::new();
    for (module, file) in reports {
        s.push_str(&format!(
            "mkdir -p {module}/build/test-results/test && cp '{}' {module}/build/test-results/test/\n",
            fx.join(file).display()
        ));
    }
    s.push_str(&format!(
        "cat '{0}.stdout'\ncat '{0}.stderr' >&2\nexit 1\n",
        fx.join(console).display()
    ));
    stand_in(bin, "gradle", &s);
}

#[cfg(unix)]
#[test]
fn e2e_gradle_harvests_reports_written_by_the_run_only() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    // A report left over from an earlier run: never part of this report.
    let stale = proj.join("old/build/test-results/test");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(
        stale.join("TEST-old.StaleTest.xml"),
        r#"<testsuite><testcase classname="old.StaleTest" name="stale()"><failure message="stale"/></testcase></testsuite>"#,
    )
    .unwrap();
    let bin = tmp.path().join("bin");
    fake_gradle(
        &bin,
        &[
            ("app", "TEST-demo.AppTest.xml"),
            ("lib", "TEST-demo.LibTest.xml"),
        ],
        "test-fail",
    );
    let a = cartoon_in(tmp.path(), &proj, Some(&bin))
        .args(["gradle", "test", "--continue"])
        .assert()
        .code(1);
    let out = stdout_of(&a);
    assert!(out.contains("runner: gradle"), "{out}");
    assert!(out.contains("total: 7"), "{out}");
    assert!(out.contains("failed: 3"), "{out}");
    assert!(out.contains("demo.AppTest.subtracts()"), "{out}");
    assert!(!out.contains("stale"), "{out}");
}

#[cfg(unix)]
#[test]
fn e2e_gradle_partial_compile_failure_is_not_hidden_by_test_results() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let bin = tmp.path().join("bin");
    fake_gradle(&bin, &[("app", "TEST-demo.AppTest.xml")], "compile-partial");
    let a = cartoon_in(tmp.path(), &proj, Some(&bin))
        .args(["gradle", "test", "--continue"])
        .assert()
        .code(1);
    let out = stdout_of(&a);
    assert!(out.contains("runner: gradle"), "{out}");
    assert!(out.contains("(build)"), "{out}");
    assert!(
        out.contains("incompatible types: String cannot be converted to int"),
        "{out}"
    );
    assert!(
        out.contains("Execution failed for task ':lib:compileTestJava'"),
        "{out}"
    );
}

#[cfg(unix)]
#[test]
fn e2e_gradle_without_fresh_reports_falls_back_to_the_console() {
    // A build that failed before any test ran: gradle's own output is kept.
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let bin = tmp.path().join("bin");
    fake_gradle(&bin, &[], "compile-partial");
    let a = cartoon_in(tmp.path(), &proj, Some(&bin))
        .args(["gradle", "test"])
        .assert()
        .code(1);
    let out = stdout_of(&a);
    let err = String::from_utf8(a.get_output().stderr.clone()).unwrap();
    assert!(!out.contains("runner: gradle"), "{out}");
    assert!(err.contains("Compilation failed"), "{err}");
}

#[cfg(unix)]
#[test]
fn e2e_maven_harvests_surefire_reports_of_every_module() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let fx = fixtures("maven");
    let bin = tmp.path().join("bin");
    stand_in(
        &bin,
        "mvn",
        &format!(
            "mkdir -p core/target/surefire-reports web/target/surefire-reports\n\
             cp '{0}/TEST-demo.CoreTest.xml' core/target/surefire-reports/\n\
             cp '{0}/TEST-demo.WebTest.xml' web/target/surefire-reports/\n\
             cat '{0}/compile-partial.stdout'\nexit 1\n",
            fx.display()
        ),
    );
    let a = cartoon_in(tmp.path(), &proj, Some(&bin))
        .args(["mvn", "-B", "-fae", "test"])
        .assert()
        .code(1);
    let out = stdout_of(&a);
    assert!(out.contains("runner: maven"), "{out}");
    assert!(out.contains("total: 7"), "{out}");
    assert!(out.contains("demo.CoreTest.throwsUnexpectedly"), "{out}");
    assert!(
        out.contains("java.lang.IllegalStateException: boom"),
        "{out}"
    );
    // web's test sources did not compile: surfaced next to core's failures.
    assert!(
        out.contains("incompatible types: java.lang.String cannot be converted to int"),
        "{out}"
    );
}

#[cfg(unix)]
#[test]
fn e2e_dotnet_test_reads_every_trx_from_the_injected_results_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let fx = fixtures("dotnet");
    let bin = tmp.path().join("bin");
    // Copies one TRX per test project into --results-directory, the way
    // VSTest does, after checking the injected logger.
    stand_in(
        &bin,
        "dotnet",
        &format!(
            r#"dir=""; logger=""
prev=""
for a in "$@"; do
  [ "$prev" = "--results-directory" ] && dir="$a"
  [ "$prev" = "--logger" ] && logger="$a"
  prev="$a"
done
[ "$logger" = "trx" ] || {{ echo "no trx logger" >&2; exit 9; }}
mkdir -p "$dir"
cp '{0}/UnitA.trx' "$dir/_vm_1.trx"
cp '{0}/UnitB.trx' "$dir/_vm_1[1].trx"
cat '{0}/build-partial.stdout'
exit 1
"#,
            fx.display()
        ),
    );
    let a = cartoon_in(tmp.path(), &proj, Some(&bin))
        .args(["dotnet", "test"])
        .assert()
        .code(1);
    let out = stdout_of(&a);
    assert!(out.contains("runner: dotnet"), "{out}");
    assert!(out.contains("total: 9"), "{out}");
    assert!(out.contains("UnitB.StringTests.Lower"), "{out}");
    assert!(
        out.contains("\"UnitA.CalculatorTests.IsOdd(n: 2)\""),
        "{out}"
    );
    // The MSBuild error from the console is lifted into the report.
    assert!(out.contains("CS0029"), "{out}");
}

#[cfg(unix)]
#[test]
fn e2e_golangci_lint_injects_the_flag_of_the_installed_major_version() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let v2 = fixtures("golangci-lint").join("v2.stdout");
    for (version, flag) in [
        ("2.5.0", "--output.json.path=stdout"),
        ("1.64.8", "--out-format json"),
    ] {
        let bin = tmp.path().join(format!("bin{version}"));
        stand_in(
            &bin,
            "golangci-lint",
            &format!(
                r#"if [ "$1" = "--version" ]; then
  echo "golangci-lint has version {version} built with go1.25.1 from ff63786c on 2025-09-21T19:04:05Z"; exit 0
fi
case "$*" in
  *"{flag}"*) cat '{}'; exit 1 ;;
  *) echo "unknown flag" >&2; exit 3 ;;
esac
"#,
                v2.display()
            ),
        );
        let a = cartoon_in(tmp.path(), &proj, Some(&bin))
            .args(["golangci-lint", "run", "./..."])
            .assert()
            .code(1);
        let out = stdout_of(&a);
        assert!(out.contains("runner: golangci-lint"), "{version}: {out}");
        assert!(out.contains("errors: 3"), "{version}: {out}");
        assert!(out.contains("errcheck"), "{version}: {out}");
    }
}

/// A copy of the jest e2e project with a `test` script.
fn js_project(tmp: &Path, script: &str, test_file: &str) -> PathBuf {
    let proj = tmp.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let e2e = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/e2e");
    std::fs::copy(e2e.join(test_file), proj.join("sample.test.js")).unwrap();
    let pkg = serde_json::json!({
        "name": "cartoon-e2e-pkg-script",
        "private": true,
        "scripts": { "test": script },
        "jest": { "testEnvironment": "node" },
    });
    std::fs::write(proj.join("package.json"), pkg.to_string()).unwrap();
    proj
}

#[test]
fn e2e_npm_test_with_a_jest_script_gets_the_jest_report() {
    if !have("npm") || !have("jest") {
        eprintln!("SKIP: npm or jest not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let proj = js_project(tmp.path(), "jest", "jsproj/sample.test.js");
    let a = cartoon_in(tmp.path(), &proj, None)
        .args(["npm", "test"])
        .assert()
        .code(1);
    let out = stdout_of(&a);
    assert!(out.contains("runner: jest"), "{out}");
    assert!(out.contains("failed: 1"), "{out}");
    assert!(out.contains("fails"), "{out}");
    // npm's `> name test` banner is not part of the report.
    assert!(!out.contains("> jest"), "{out}");
}

#[test]
fn e2e_npm_test_forwards_user_args_after_the_separator() {
    if !have("npm") || !have("jest") {
        eprintln!("SKIP: npm or jest not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let proj = js_project(tmp.path(), "jest", "jsproj/sample.test.js");
    let a = cartoon_in(tmp.path(), &proj, None)
        .args(["npm", "test", "--", "-t", "passes"])
        .assert()
        .code(0);
    let out = stdout_of(&a);
    assert!(out.contains("runner: jest"), "{out}");
    assert!(out.contains("passed: 1"), "{out}");
    assert!(out.contains("skipped: 1"), "{out}");
}

#[test]
fn e2e_npm_test_with_a_vitest_script_gets_the_vitest_report() {
    if !have("npm") || !have("vitest") {
        eprintln!("SKIP: npm or vitest not installed");
        return;
    }
    // `vitest run`, and bare `vitest`: with stdin not a terminal (as here)
    // vitest runs once instead of watching.
    for script in ["vitest run", "vitest"] {
        let tmp = tempfile::tempdir().unwrap();
        let proj = js_project(tmp.path(), script, "vitestproj/sample.test.js");
        let a = cartoon_in(tmp.path(), &proj, None)
            .args(["npm", "run", "test"])
            .assert()
            .code(1);
        let out = stdout_of(&a);
        assert!(out.contains("runner: vitest"), "{script}: {out}");
        assert!(out.contains("failed: 1"), "{script}: {out}");
    }
}

#[test]
fn e2e_npm_test_with_a_compound_script_is_left_to_the_ladder() {
    if !have("npm") || !have("jest") {
        eprintln!("SKIP: npm or jest not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let proj = js_project(tmp.path(), "echo prep && jest", "jsproj/sample.test.js");
    let a = cartoon_in(tmp.path(), &proj, None)
        .args(["npm", "test"])
        .assert()
        .code(1);
    let out = stdout_of(&a);
    assert!(!out.contains("runner: jest"), "{out}");
}

// ---- real tools (not in CI): cargo test --test e2e_adapters2 -- --ignored

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

const JUNIT5_TEST: &str = r#"package demo;
import static org.junit.jupiter.api.Assertions.*;
import org.junit.jupiter.api.Test;
class CalcTest {
    @Test void adds() { assertEquals(4, 2 + 2); }
    @Test void subtracts() { assertEquals(1, 3 - 1, "subtraction is off"); }
}
"#;

#[test]
#[ignore = "needs gradle and network access to Maven Central"]
fn real_gradle_test_reports_failures() {
    if !have("gradle") {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    write(&proj.join("settings.gradle"), "rootProject.name = 'demo'\n");
    write(
        &proj.join("build.gradle"),
        "apply plugin: 'java'\nrepositories { mavenCentral() }\n\
         dependencies {\n  testImplementation 'org.junit.jupiter:junit-jupiter:5.10.2'\n  \
         testRuntimeOnly 'org.junit.platform:junit-platform-launcher'\n}\n\
         test { useJUnitPlatform() }\n",
    );
    write(&proj.join("src/test/java/demo/CalcTest.java"), JUNIT5_TEST);
    let a = cartoon_in(tmp.path(), &proj, None)
        .args(["gradle", "test", "--no-daemon"])
        .assert()
        .code(1);
    let out = stdout_of(&a);
    assert!(out.contains("runner: gradle"), "{out}");
    assert!(out.contains("failed: 1"), "{out}");
    assert!(out.contains("src/test/java/demo/CalcTest.java:6"), "{out}");
}

#[test]
#[ignore = "needs maven and network access to Maven Central"]
fn real_maven_test_reports_failures() {
    if !have("mvn") {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    write(
        &proj.join("pom.xml"),
        r#"<project xmlns="http://maven.apache.org/POM/4.0.0">
  <modelVersion>4.0.0</modelVersion>
  <groupId>demo</groupId><artifactId>demo</artifactId><version>1.0</version>
  <properties><maven.compiler.release>17</maven.compiler.release></properties>
  <dependencies><dependency>
    <groupId>org.junit.jupiter</groupId><artifactId>junit-jupiter</artifactId>
    <version>5.10.2</version><scope>test</scope>
  </dependency></dependencies>
  <build><plugins><plugin>
    <groupId>org.apache.maven.plugins</groupId><artifactId>maven-surefire-plugin</artifactId><version>3.2.5</version>
  </plugin></plugins></build>
</project>
"#,
    );
    write(&proj.join("src/test/java/demo/CalcTest.java"), JUNIT5_TEST);
    let a = cartoon_in(tmp.path(), &proj, None)
        .args(["mvn", "-B", "test"])
        .assert()
        .code(1);
    let out = stdout_of(&a);
    assert!(out.contains("runner: maven"), "{out}");
    assert!(out.contains("failed: 1"), "{out}");
    assert!(out.contains("demo.CalcTest.subtracts"), "{out}");
}

#[test]
#[ignore = "needs the .NET SDK and network access to NuGet"]
fn real_dotnet_test_reports_failures() {
    if !have("dotnet") {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let ok = std::process::Command::new("dotnet")
        .args(["new", "xunit", "-o", "Calc"])
        .current_dir(tmp.path())
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "dotnet new xunit failed");
    let proj = tmp.path().join("Calc");
    write(
        &proj.join("UnitTest1.cs"),
        "namespace Calc;\npublic class CalcTests {\n  [Fact] public void Adds() { Assert.Equal(4, 2 + 2); }\n  \
         [Fact] public void Subtracts() { Assert.Equal(1, 3 - 1); }\n}\n",
    );
    let a = cartoon_in(tmp.path(), &proj, None)
        .args(["dotnet", "test"])
        .assert()
        .code(1);
    let out = stdout_of(&a);
    assert!(out.contains("runner: dotnet"), "{out}");
    assert!(out.contains("failed: 1"), "{out}");
    assert!(out.contains("Calc.CalcTests.Subtracts"), "{out}");
}

#[test]
#[ignore = "needs golangci-lint"]
fn real_golangci_lint_reports_issues() {
    if !have("golangci-lint") {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("proj");
    write(&proj.join("go.mod"), "module example.com/demo\n\ngo 1.22\n");
    write(
        &proj.join("a.go"),
        "package demo\n\nimport \"os\"\n\nfunc unused() {}\n\nfunc Run() {\n\tos.Remove(\"x\")\n}\n",
    );
    let a = cartoon_in(tmp.path(), &proj, None)
        .args(["golangci-lint", "run"])
        .assert()
        .code(1);
    let out = stdout_of(&a);
    assert!(out.contains("runner: golangci-lint"), "{out}");
    assert!(out.contains("errcheck"), "{out}");
    assert!(out.contains("unused"), "{out}");
}
