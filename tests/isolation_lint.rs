//! Every integration test that runs the real cartoon binary must point
//! XDG_STATE_HOME at a temp dir, or `cargo test` archives fixture runs into
//! the developer's real ~/.local/state/cartoon and prunes genuine logs.
#[test]
fn every_e2e_test_isolates_xdg_state_home() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let runs_binary =
            src.contains("CARGO_BIN_EXE_cartoon") || src.contains("cargo_bin(\"cartoon\")");
        if runs_binary && !src.contains("XDG_STATE_HOME") {
            offenders.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    assert!(
        offenders.is_empty(),
        "e2e tests without XDG_STATE_HOME isolation: {offenders:?}"
    );
}
