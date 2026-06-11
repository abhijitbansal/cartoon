#![cfg(unix)]
use std::process::Command;

fn cartoon_bin() -> &'static str {
    env!("CARGO_BIN_EXE_cartoon")
}

#[test]
fn safe_tier_compresses_and_mirrors_exit_code() {
    // ANSI + a long duplicate run: the safe tier must fire AND beat the
    // net-savings guard (compression + raw_log footer < original tokens).
    let out = Command::new(cartoon_bin())
        .args([
            "sh",
            "-c",
            r"printf '\033[32mok\033[0m\n'; for i in $(seq 60); do echo 'same noisy line of output'; done; exit 3",
        ])
        .output()
        .expect("run cartoon");
    assert_eq!(out.status.code(), Some(3), "exit code mirrored");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("(x60)"), "repeats collapsed: {stdout}");
    assert!(!stdout.contains("\x1b["), "ANSI stripped");
    assert!(stdout.contains("raw_log"), "disclosure footer present");
}

#[test]
fn raw_flag_bypasses_ladder() {
    let out = Command::new(cartoon_bin())
        .args(["--raw", "sh", "-c", r"printf 'same\nsame\n'"])
        .output()
        .expect("run cartoon");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "same\nsame\n", "byte-identical in raw mode");
}

#[test]
fn aggressive_flag_extracts_diagnostics() {
    // Repeated make noise (collapsed by the safe tier) plus distinct
    // diagnostics: together they decisively beat the net-savings guard.
    // Messages are textually distinct (like real compiler output) so
    // collapse_near_dups does not template them away first.
    let script = r#"for i in $(seq 40); do echo "make[1]: Entering directory /work/build/objects"; done; printf '%s\n' \
      "src/lexer.c:10:5: error: unknown type name uint128_t in declaration" \
      "src/lexer.c:42:11: error: expected semicolon after expression statement" \
      "src/parser.c:7:1: warning: implicit declaration of function tokenize_buffer" \
      "src/parser.c:99:23: error: too few arguments to function call expected 3 have 1" \
      "src/ast.c:15:9: warning: unused variable depth_counter in this scope" \
      "src/ast.c:81:2: error: redefinition of node_free with different signature" \
      "src/eval.c:3:14: error: use of undeclared identifier global_env_table" \
      "src/eval.c:55:30: warning: comparison of integers of different signs" \
      "src/main.c:120:8: error: incompatible pointer types passing FILE to const char" \
      "src/main.c:131:4: note: did you mean to call fopen_checked instead" \
      "src/util.c:22:17: error: array subscript is not an integer literal" \
      "src/util.c:60:6: warning: control reaches end of non-void function here""#;
    let out = Command::new(cartoon_bin())
        .args(["--compress", "aggressive", "sh", "-c", script])
        .output()
        .expect("run cartoon");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("diagnostics"),
        "TOON table emitted: {stdout}"
    );
    assert!(
        stdout.contains("src/lexer.c:10:5"),
        "locations kept: {stdout}"
    );
}

#[test]
fn invalid_compress_level_exits_2() {
    let out = Command::new(cartoon_bin())
        .args(["--compress", "turbo", "echo", "hi"])
        .output()
        .expect("run cartoon");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid compress level"), "{stderr}");
}
