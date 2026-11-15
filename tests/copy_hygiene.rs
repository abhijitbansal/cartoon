//! Marketing copy must not carry savings figures we cannot back with a
//! committed measurement. `benchmarks/results/` and `docs/superpowers/` are
//! the only places such numbers may live.
use std::path::Path;

const BANNED: &[&str] = &["~70%", "≥70%", "43–89%", "12 lines instead of 800"];
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
