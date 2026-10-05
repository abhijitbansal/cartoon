//! `cartoon last` / `cartoon diff` against real pytest runs: fail, edit the
//! tests, re-run, and ask what changed.
mod common;
use assert_cmd::Command;
use common::have;
use std::path::Path;

fn cartoon(state: &Path, cwd: &Path) -> Command {
    let mut c = Command::cargo_bin("cartoon").unwrap();
    c.env("XDG_STATE_HOME", state)
        .env("XDG_CONFIG_HOME", state)
        .current_dir(cwd);
    c
}

fn stdout(a: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(a.get_output().stdout.clone()).unwrap()
}

fn stderr(a: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(a.get_output().stderr.clone()).unwrap()
}

/// A failing test with a traceback long enough that the TOON report beats
/// pytest's own output (so the adapter report, not passthrough, is shown).
fn failing(name: &str) -> String {
    format!(
        "def test_{name}():\n    data = {{'k%d' % i: i for i in range(40)}}\n    assert data == {{'k0': 1}}, '{name} is broken'\n\n"
    )
}

fn passing(name: &str) -> String {
    format!("def test_{name}():\n    assert True\n\n")
}

fn pytest(state: &Path, proj: &Path) -> String {
    let a = cartoon(state, proj)
        .args(["pytest", "-p", "no:cacheprovider", "test_loop.py"])
        .assert()
        .code(1);
    stdout(&a)
}

#[test]
fn e2e_last_and_diff_follow_an_edit_run_fix_loop() {
    if !have("pytest") {
        eprintln!("SKIP: pytest not installed");
        return;
    }
    let state = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let file = proj.path().join("test_loop.py");

    // Nothing archived yet: both queries say so and exit 1.
    let a = cartoon(state.path(), proj.path())
        .arg("diff")
        .assert()
        .code(1);
    assert!(stderr(&a).contains("nothing to diff"), "{}", stderr(&a));
    cartoon(state.path(), proj.path())
        .arg("last")
        .assert()
        .code(1);

    // Run 1: alpha and beta fail.
    let src = [failing("alpha"), failing("beta"), passing("gamma")].concat();
    std::fs::write(&file, src).unwrap();
    let out1 = pytest(state.path(), proj.path());
    assert!(out1.contains("runner: pytest"), "{out1}");
    assert!(!out1.contains("vs_previous"), "no earlier run: {out1}");

    // Only one run of the command: diff explains why it cannot compare.
    let a = cartoon(state.path(), proj.path())
        .arg("diff")
        .assert()
        .code(1);
    assert!(stderr(&a).contains("only one run of"), "{}", stderr(&a));

    // Run 2: alpha fixed, beta still failing (moved down a few lines),
    // gamma newly broken.
    let src = [
        passing("alpha"),
        "# an edit that shifts line numbers\n\n\n".to_string(),
        failing("beta"),
        failing("gamma"),
    ]
    .concat();
    std::fs::write(&file, src).unwrap();
    let out2 = pytest(state.path(), proj.path());
    assert!(
        out2.contains("vs_previous: fixed 1; still 1; new 1 (cartoon diff)"),
        "{out2}"
    );

    let a = cartoon(state.path(), proj.path())
        .arg("diff")
        .assert()
        .code(0);
    let diff = stdout(&a);
    assert!(
        diff.contains("command: \"pytest -p no:cacheprovider test_loop.py\""),
        "{diff}"
    );
    assert!(diff.contains("fixed[1]{id,loc}:"), "{diff}");
    assert!(diff.contains("test_alpha"), "{diff}");
    let still = diff.split("still_failing").nth(1).unwrap();
    let (still, new) = still.split_once("new_failures").unwrap();
    assert!(
        still.contains("test_beta") && still.contains("beta is broken"),
        "{diff}"
    );
    assert!(
        new.contains("test_gamma") && new.contains("gamma is broken"),
        "{diff}"
    );
    assert!(
        !new.contains("test_beta"),
        "a moved failure is not new: {diff}"
    );

    // `last` re-shows run 2's report without re-running pytest.
    let a = cartoon(state.path(), proj.path())
        .arg("last")
        .assert()
        .code(0);
    let last = stdout(&a);
    assert!(last.starts_with("runner: pytest"), "{last}");
    assert!(
        last.contains("test_gamma") && !last.contains("test_alpha"),
        "{last}"
    );
    assert!(
        last.contains("exit_code: 1") && last.contains("raw_log:"),
        "{last}"
    );

    // Explicit ids, reversed: what run 1 had that run 2 did not is "new".
    let ids: Vec<String> = [&out1, &out2]
        .iter()
        .map(|o| {
            let line = o.lines().find(|l| l.starts_with("raw_log:")).unwrap();
            let path = line.split_once(' ').unwrap().1.trim().trim_matches('"');
            Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let a = cartoon(state.path(), proj.path())
        .args(["diff", &ids[1], &ids[0]])
        .assert()
        .code(0);
    let rev = stdout(&a);
    let new = rev.split("new_failures").nth(1).unwrap();
    assert!(new.contains("test_alpha"), "{rev}");
}

#[test]
fn e2e_last_summarizes_a_run_without_a_report() {
    let state = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    cartoon(state.path(), cwd.path())
        .args(["sh", "-c", "echo hello"])
        .assert()
        .success();
    let a = cartoon(state.path(), cwd.path())
        .arg("last")
        .assert()
        .code(0);
    let out = stdout(&a);
    assert!(out.contains("command: sh -c 'echo hello'"), "{out}");
    assert!(out.contains("note: no adapter report"), "{out}");
    assert!(out.contains("raw_log:"), "{out}");
    let a = cartoon(state.path(), cwd.path())
        .args(["last", "--cmd", "nope"])
        .assert()
        .code(1);
    assert!(
        stderr(&a).contains("no archived run matching"),
        "{}",
        stderr(&a)
    );
}

#[test]
fn diff_with_file_args_still_wraps_the_system_diff() {
    if !have("diff") {
        eprintln!("SKIP: diff not installed");
        return;
    }
    let state = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(cwd.path().join("a.txt"), "one\n").unwrap();
    std::fs::write(cwd.path().join("b.txt"), "two\n").unwrap();
    let a = cartoon(state.path(), cwd.path())
        .args(["diff", "a.txt", "b.txt"])
        .assert()
        .code(1);
    let out = stdout(&a);
    assert!(out.contains("< one") && out.contains("> two"), "{out}");
}
