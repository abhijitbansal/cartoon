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
/// Matched case-insensitively. The spec drops the "any CLI" pitch: cartoon is
/// pitched at Xcode/Swift runs for coding agents, not every command.
const BANNED_PHRASES: &[&str] = &["any cli"];
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
        let lower = text.to_lowercase();
        for phrase in BANNED_PHRASES {
            if lower.contains(phrase) {
                hits.push(format!("{} contains {phrase:?}", path.display()));
            }
        }
    }
}

#[test]
fn no_unbacked_savings_figures_or_banned_pitch_in_copy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut hits = Vec::new();
    scan(root, &mut hits);
    assert!(
        hits.is_empty(),
        "unbacked figures or banned pitch:\n{}",
        hits.join("\n")
    );
}

/// `hook install --deny` was removed in 0.8; marketplace text must not tell
/// users to run it.
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

/// Features removed in 0.8. Only the "Upgrading from 0.7" section of
/// `docs/usage.md` may name them (to tell users what replaced them).
/// Matched case-insensitively.
const REMOVED_FEATURE_TERMS: &[&str] = &[
    "shim",
    "--fast",
    "--copilot",
    "--vscode",
    "--deny",
    "--heuristic",
    "cartoon --tag",
];

/// User-facing docs: the README, contributor guide, `docs/` (minus the
/// engineering specs under `docs/superpowers/`), the skill and the npm README.
fn user_facing_docs(root: &Path) -> Vec<std::path::PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "superpowers") {
                    continue;
                }
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut out = vec![
        root.join("README.md"),
        root.join("CONTRIBUTING.md"),
        root.join("benchmarks/README.md"),
        root.join("packages/npm/cartoon-wrap/README.md"),
    ];
    walk(&root.join("docs"), &mut out);
    walk(&root.join("skills"), &mut out);
    out
}

/// Text outside `## Upgrading from 0.7` (that section runs to the next `## `).
fn without_upgrade_section(text: &str) -> String {
    let Some(start) = text.find("## Upgrading from 0.7") else {
        return text.to_string();
    };
    let rest = &text[start + 3..];
    let end = rest.find("\n## ").map_or(text.len(), |i| start + 3 + i);
    format!("{}{}", &text[..start], &text[end..])
}

#[test]
fn docs_do_not_document_removed_features() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut hits = Vec::new();
    for path in user_facing_docs(root) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue; // binary asset
        };
        let lower = without_upgrade_section(&text).to_lowercase();
        for term in REMOVED_FEATURE_TERMS {
            if lower.contains(term) {
                hits.push(format!("{} mentions removed {term:?}", path.display()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "removed features documented:\n{}",
        hits.join("\n")
    );
}

#[test]
fn readme_is_short_and_points_at_the_reference_docs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    assert!(
        readme.lines().count() < 150,
        "README is {} lines",
        readme.lines().count()
    );
    for link in ["docs/usage.md", "docs/agents.md", "benchmarks/"] {
        assert!(readme.contains(link), "README does not link {link}");
    }
}

#[test]
fn usage_doc_explains_upgrading_from_0_7() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let usage = std::fs::read_to_string(root.join("docs/usage.md")).unwrap_or_default();
    assert!(
        usage.contains("## Upgrading from 0.7"),
        "docs/usage.md lacks the upgrade section"
    );
}
