//! `cartoon init` — scan a repo root for wrapper scripts that call a known
//! noisy dev-loop tool directly (`./build.sh` calling `xcodebuild`, etc.) and
//! print a ready-to-paste `.cartoon.toml` snippet: a `wrap_scripts` entry.
//! Declared scripts default to the aggressive tier (the safe tier does not
//! compress this kind of output). Not recursive, and content-sniffing only
//! happens here as a one-time suggestion — never in the hook's hot
//! PreToolUse path.
use anyhow::Result;
use std::path::Path;

/// Substrings whose presence in a script marks it as a noisy dev-loop
/// wrapper worth declaring in `wrap_scripts`.
const NOISY_MARKERS: &[&str] = &[
    "xcodebuild",
    "swift test",
    "swift build",
    "pytest",
    "cargo test",
    "cargo build",
];

pub fn run(root: &Path) -> Result<i32> {
    println!("{}", render(&scan(root)?));
    Ok(0)
}

/// `*.sh` files directly in `root` (not recursive) whose source mentions a
/// noisy marker. Returns `./<name>` argv0 forms, sorted, matching how the
/// hook will actually see them invoked.
fn scan(root: &Path) -> Result<Vec<String>> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(found);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("sh") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        if NOISY_MARKERS.iter().any(|m| src.contains(m)) {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                found.push(format!("./{name}"));
            }
        }
    }
    found.sort();
    Ok(found)
}

pub fn render(scripts: &[String]) -> String {
    if scripts.is_empty() {
        return "init: no wrapper scripts found in this directory — nothing to suggest".into();
    }
    let quoted: Vec<String> = scripts.iter().map(|s| format!("\"{s}\"")).collect();
    format!(
        "init: found {} script(s) invoking a noisy dev-loop tool directly\n\n\
         # paste into .cartoon.toml (repo root):\n\
         wrap_scripts = [{}]\n\n\
         Declared scripts compress at the aggressive tier by default (the safe tier \
         compresses none of this kind of output). Override per script with \
         [command.\"{}\"] level = \"safe\". The hook routes them deny-with-suggestion, \
         never auto-approved — a project script isn't a vetted read-mostly tool.\n",
        scripts.len(),
        quoted.join(", "),
        scripts[0]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn empty_dir_says_nothing_to_suggest() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(scan(tmp.path()).unwrap().is_empty());
        assert!(render(&scan(tmp.path()).unwrap()).contains("nothing to suggest"));
    }

    #[test]
    fn detects_script_invoking_xcodebuild() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("build.sh"),
            "#!/usr/bin/env bash\nxcodebuild build -scheme App\n",
        )
        .unwrap();
        assert_eq!(scan(tmp.path()).unwrap(), vec!["./build.sh".to_string()]);
    }

    #[test]
    fn ignores_script_without_a_noisy_marker() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("deploy.sh"),
            "#!/usr/bin/env bash\nrsync -av build/ prod:/var/www\n",
        )
        .unwrap();
        assert!(scan(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn ignores_non_sh_files_even_with_a_marker_inside() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("notes.txt"), "run xcodebuild manually").unwrap();
        assert!(scan(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn render_includes_wrap_scripts_and_explains_the_default_tier() {
        let out = render(&["./build.sh".to_string()]);
        assert!(
            out.contains(r#"wrap_scripts = ["./build.sh"]"#),
            "got:\n{out}"
        );
        assert!(out.contains("aggressive tier by default"), "got:\n{out}");
        // No per-script pin block: the default already covers it.
        assert!(!out.contains("level = \"aggressive\""), "got:\n{out}");
    }
}
