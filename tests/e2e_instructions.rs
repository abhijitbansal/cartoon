use assert_cmd::Command;
use predicates::str::contains;
use std::fs;

fn cartoon() -> Command {
    Command::cargo_bin("cartoon").unwrap()
}

#[test]
fn install_creates_agents_md_with_the_directive() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .current_dir(tmp.path())
        .args(["instructions", "install"])
        .assert()
        .success()
        .stdout(contains("AGENTS.md"));

    let body = fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
    assert!(body.contains("BEGIN cartoon instructions"));
    assert!(body.contains("NEVER pipe"));
    assert!(body.contains("cartoon logs grep <pattern> --last"));
}

#[test]
fn reinstall_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        cartoon()
            .current_dir(tmp.path())
            .args(["instructions", "install"])
            .assert()
            .success();
    }
    let body = fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
    assert_eq!(
        body.matches("BEGIN cartoon instructions").count(),
        1,
        "directive must not be duplicated on re-install"
    );
}

#[test]
fn install_appends_and_uninstall_restores_existing_content() {
    let tmp = tempfile::tempdir().unwrap();
    let agents = tmp.path().join("AGENTS.md");
    fs::write(&agents, "# My rules\n\nDo the thing.\n").unwrap();

    cartoon()
        .current_dir(tmp.path())
        .args(["instructions", "install"])
        .assert()
        .success();
    let with_block = fs::read_to_string(&agents).unwrap();
    assert!(with_block.contains("# My rules"));
    assert!(with_block.contains("BEGIN cartoon instructions"));

    cartoon()
        .current_dir(tmp.path())
        .args(["instructions", "uninstall"])
        .assert()
        .success()
        .stdout(contains("removed"));
    let after = fs::read_to_string(&agents).unwrap();
    assert!(after.contains("# My rules"));
    assert!(after.contains("Do the thing."));
    assert!(!after.contains("BEGIN cartoon instructions"));
}

#[test]
fn uninstall_deletes_file_when_we_were_its_only_content() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .current_dir(tmp.path())
        .args(["instructions", "install"])
        .assert()
        .success();
    cartoon()
        .current_dir(tmp.path())
        .args(["instructions", "uninstall"])
        .assert()
        .success();
    assert!(
        !tmp.path().join("AGENTS.md").exists(),
        "a file containing only our block should be removed on uninstall"
    );
}

#[test]
fn install_prefers_claude_md_when_it_exists() {
    let tmp = tempfile::tempdir().unwrap();
    // A Claude Code project signals itself with a CLAUDE.md (here, still empty).
    fs::write(tmp.path().join("CLAUDE.md"), "").unwrap();

    cartoon()
        .current_dir(tmp.path())
        .args(["instructions", "install"])
        .assert()
        .success()
        .stdout(contains("CLAUDE.md"));

    let body = fs::read_to_string(tmp.path().join("CLAUDE.md")).unwrap();
    assert!(body.contains("BEGIN cartoon instructions"));
    // AGENTS.md is only the backup; it must not be created when CLAUDE.md wins.
    assert!(!tmp.path().join("AGENTS.md").exists());
}

#[test]
fn uninstall_auto_detects_claude_md() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("CLAUDE.md"), "# Claude project\n").unwrap();
    cartoon()
        .current_dir(tmp.path())
        .args(["instructions", "install"])
        .assert()
        .success();

    // No flag on uninstall either: it must resolve to the same CLAUDE.md.
    cartoon()
        .current_dir(tmp.path())
        .args(["instructions", "uninstall"])
        .assert()
        .success()
        .stdout(contains("removed"));

    let after = fs::read_to_string(tmp.path().join("CLAUDE.md")).unwrap();
    assert!(after.contains("# Claude project"));
    assert!(!after.contains("BEGIN cartoon instructions"));
}

#[test]
fn install_stays_in_agents_md_when_block_already_lives_there() {
    let tmp = tempfile::tempdir().unwrap();
    // First install with no CLAUDE.md → lands in AGENTS.md.
    cartoon()
        .current_dir(tmp.path())
        .args(["instructions", "install"])
        .assert()
        .success();
    assert!(tmp.path().join("AGENTS.md").exists());

    // A CLAUDE.md appears later; re-install must update AGENTS.md in place, not
    // strand the existing block by writing a second copy into CLAUDE.md.
    fs::write(tmp.path().join("CLAUDE.md"), "# notes\n").unwrap();
    cartoon()
        .current_dir(tmp.path())
        .args(["instructions", "install"])
        .assert()
        .success()
        .stdout(contains("AGENTS.md"));

    assert!(!fs::read_to_string(tmp.path().join("CLAUDE.md"))
        .unwrap()
        .contains("BEGIN cartoon instructions"));
    assert_eq!(
        fs::read_to_string(tmp.path().join("AGENTS.md"))
            .unwrap()
            .matches("BEGIN cartoon instructions")
            .count(),
        1
    );
}

#[test]
fn copilot_flag_targets_the_github_file() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .current_dir(tmp.path())
        .args(["instructions", "install", "--copilot"])
        .assert()
        .success();
    assert!(tmp.path().join(".github/copilot-instructions.md").exists());
    // default AGENTS.md untouched
    assert!(!tmp.path().join("AGENTS.md").exists());
}

#[test]
fn status_reports_each_file() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .current_dir(tmp.path())
        .args(["instructions", "install"])
        .assert()
        .success();
    cartoon()
        .current_dir(tmp.path())
        .args(["instructions", "status"])
        .assert()
        .success()
        .stdout(contains("AGENTS.md: installed"))
        .stdout(contains("CLAUDE.md: not installed"));
}

#[test]
fn print_emits_the_block_without_writing() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .current_dir(tmp.path())
        .args(["instructions", "print"])
        .assert()
        .success()
        .stdout(contains("BEGIN cartoon instructions"))
        .stdout(contains("NEVER pipe"));
    assert!(!tmp.path().join("AGENTS.md").exists());
}

#[test]
fn hook_install_hints_about_the_pipe_gap() {
    let tmp = tempfile::tempdir().unwrap();
    // --project keeps the hook in cwd/.claude, never the real home dir.
    cartoon()
        .current_dir(tmp.path())
        .args(["hook", "install", "--project"])
        .assert()
        .success()
        .stdout(contains("cartoon instructions install"));
    // Non-interactive: the directive is only *offered*, not written.
    assert!(!tmp.path().join("AGENTS.md").exists());
}

#[test]
fn hook_install_with_instructions_writes_the_directive() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .current_dir(tmp.path())
        .args(["hook", "install", "--project", "--instructions"])
        .assert()
        .success();
    let body = fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
    assert!(body.contains("BEGIN cartoon instructions"));
    // and the hook itself landed in the project settings
    assert!(tmp.path().join(".claude/settings.json").exists());
}

#[test]
fn hook_install_instructions_prefers_claude_md_when_present() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("CLAUDE.md"), "").unwrap();
    cartoon()
        .current_dir(tmp.path())
        .args(["hook", "install", "--project", "--instructions"])
        .assert()
        .success()
        .stdout(contains("CLAUDE.md"));
    assert!(fs::read_to_string(tmp.path().join("CLAUDE.md"))
        .unwrap()
        .contains("BEGIN cartoon instructions"));
    // AGENTS.md is the backup; CLAUDE.md present means it stays untouched.
    assert!(!tmp.path().join("AGENTS.md").exists());
}

#[test]
fn hook_install_copilot_instructions_targets_copilot_file() {
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .current_dir(tmp.path())
        .args([
            "hook",
            "install",
            "--copilot",
            "--project",
            "--instructions",
        ])
        .assert()
        .success();
    assert!(tmp.path().join(".github/copilot-instructions.md").exists());
}

#[test]
fn hook_install_copilot_hint_carries_the_copilot_flag() {
    // Regression: the piped-gap hint for a Copilot install must suggest
    // `cartoon instructions install --copilot` (not the bare command, which
    // auto-detects to AGENTS.md — a file Copilot never reads).
    let tmp = tempfile::tempdir().unwrap();
    cartoon()
        .current_dir(tmp.path())
        .args(["hook", "install", "--copilot", "--project"])
        .assert()
        .success()
        .stdout(contains("cartoon instructions install --copilot"))
        .stdout(contains("cartoon hook install --copilot --instructions"));
}

#[test]
fn hook_install_instructions_refreshes_a_stale_body() {
    // Regression: `hook install --instructions` must refresh a stale directive
    // body in place (like standalone `instructions install`), not short-circuit
    // when a marker block is merely present.
    let tmp = tempfile::tempdir().unwrap();
    // Seed AGENTS.md with a valid marker block but an outdated body.
    let cur = cartoon::instructions::block();
    let begin = cur.lines().next().unwrap();
    let end = cur.lines().last().unwrap();
    let stale = format!("{begin}\nOLD STALE DIRECTIVE BODY\n{end}\n");
    fs::write(tmp.path().join("AGENTS.md"), &stale).unwrap();

    cartoon()
        .current_dir(tmp.path())
        .args(["hook", "install", "--project", "--instructions"])
        .assert()
        .success()
        .stdout(contains("updated"));

    let body = fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
    assert!(
        !body.contains("OLD STALE DIRECTIVE BODY"),
        "stale body not removed"
    );
    assert!(
        body.contains("NEVER pipe"),
        "current directive body not written"
    );
}
