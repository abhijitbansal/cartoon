//! Marketing copy must not carry savings figures we cannot back with a
//! committed measurement. `benchmarks/results/` and `docs/superpowers/` are
//! the only places such numbers may live.
use std::path::Path;

const BANNED: &[&str] = &[
    "~70%",
    "≥70%",
    "43–89%",
    "12 lines instead of 800",
    // Hero/compare demo figures: one illustrative run, not in benchmarks/results.
    "~800",
    "~4,800",
    "~4800",
    "~300 tokens",
    "93.8",
    "12 structured",
    "12 TOON",
    "12 lines",
    // Golden-corpus test figures: real, but not benchmarks/results.
    "92.5",
    "61.5",
];
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".scratch",
    ".venv",
    ".superpowers",
    "target",
    "node_modules",
    "superpowers",
    "results",
];

fn scan(dir: &Path, hits: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                scan(&path, hits);
            }
            continue;
        }
        if name == "copy_hygiene.rs" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue; // binary file
        };
        for banned in BANNED {
            if text.contains(banned) {
                hits.push(format!("{} contains {banned:?}", path.display()));
            }
        }
    }
}

#[test]
fn no_unbacked_savings_figures_in_copy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut hits = Vec::new();
    scan(root, &mut hits);
    assert!(hits.is_empty(), "unbacked figures:\n{}", hits.join("\n"));
}

/// `hook install --deny` was removed in 0.8; marketplace text must not tell
/// users to run it. (The README still documents it until the docs rewrite.)
#[test]
fn plugin_manifests_do_not_advertise_removed_flags() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for rel in [
        ".claude-plugin/plugin.json",
        ".claude-plugin/marketplace.json",
    ] {
        let Ok(text) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        assert!(
            !text.contains("--deny"),
            "{rel} mentions removed --deny flag"
        );
    }
}
