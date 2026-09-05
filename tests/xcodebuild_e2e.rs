//! End-to-end xcodebuild-test adapter check. Requires Xcode and is therefore
//! `#[ignore]`d — it never runs in CI. Run manually on a Mac with Xcode:
//!
//!   cargo test --test xcodebuild_e2e -- --ignored
//!
//! It builds a throwaway SwiftPM package with one failing test, drives
//! `xcodebuild test` through the cartoon binary, and asserts the TOON summary
//! (not the raw xcodebuild log) is emitted.
use std::fs;
use std::process::Command;

/// Every run archives into XDG_STATE_HOME; point it at a per-test temp dir so
/// `cargo test` never writes into (or prunes) the developer's real archive.
fn isolated_state() -> &'static tempfile::TempDir {
    static STATE: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    STATE.get_or_init(|| tempfile::tempdir().expect("temp state dir"))
}

#[test]
#[ignore = "requires Xcode; run with --ignored on macOS"]
fn xcodebuild_test_emits_toon_summary() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("Sources/E2E")).unwrap();
    fs::create_dir_all(root.join("Tests/E2ETests")).unwrap();

    fs::write(
        root.join("Package.swift"),
        r#"// swift-tools-version:6.0
import PackageDescription
let package = Package(
    name: "E2E",
    platforms: [.macOS(.v13)],
    targets: [
        .target(name: "E2E"),
        .testTarget(name: "E2ETests", dependencies: ["E2E"]),
    ]
)
"#,
    )
    .unwrap();
    fs::write(
        root.join("Sources/E2E/Lib.swift"),
        "public func two() -> Int { 2 }\n",
    )
    .unwrap();
    fs::write(
        root.join("Tests/E2ETests/LibTests.swift"),
        r#"import XCTest
@testable import E2E
final class LibTests: XCTestCase {
    func testPass() { XCTAssertEqual(two(), 2) }
    func testFail() { XCTAssertEqual(two(), 3) }
}
"#,
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_cartoon"))
        .env("XDG_STATE_HOME", isolated_state().path())
        .current_dir(root)
        .args([
            "xcodebuild",
            "test",
            "-scheme",
            "E2E-Package",
            "-destination",
            "platform=macOS",
        ])
        .output()
        .expect("run cartoon xcodebuild test");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("runner: xcodebuild-test"),
        "expected TOON summary, got:\n{stdout}"
    );
    assert!(
        stdout.contains("failed: 1"),
        "expected one failure:\n{stdout}"
    );
    assert!(
        stdout.contains("testFail"),
        "expected failing test id:\n{stdout}"
    );
}
