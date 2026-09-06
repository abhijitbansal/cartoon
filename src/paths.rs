use std::path::{Path, PathBuf};

pub fn config_file() -> Option<PathBuf> {
    base("XDG_CONFIG_HOME", ".config").map(|d| d.join("cartoon/config.toml"))
}

/// Sourceable file of shell wrapper functions (`cartoon shim`).
pub fn shims_file() -> Option<PathBuf> {
    base("XDG_CONFIG_HOME", ".config").map(|d| d.join("cartoon/shims.sh"))
}

pub fn stats_file() -> Option<PathBuf> {
    base("XDG_STATE_HOME", ".local/state").map(|d| d.join("cartoon/stats.jsonl"))
}

pub fn runs_dir() -> Option<PathBuf> {
    base("XDG_STATE_HOME", ".local/state").map(|d| d.join("cartoon/runs"))
}

fn base(env: &str, fallback: &str) -> Option<PathBuf> {
    if let Ok(v) = std::env::var(env) {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    dirs::home_dir().map(|h| h.join(fallback))
}

/// Walk up from `start` looking for a project-local `.cartoon.toml`, stopping
/// after checking the first directory that contains `.git` (the repo
/// boundary) — a project config should never be picked up from an ancestor
/// outside the current repo. Returns the config file's path if found.
pub fn project_config_file(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        let candidate = dir.join(".cartoon.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if dir.join(".git").exists() {
            return None;
        }
        dir = dir.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_config_in_start_dir() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".cartoon.toml"), "").unwrap();
        assert_eq!(
            project_config_file(tmp.path()),
            Some(tmp.path().join(".cartoon.toml"))
        );
    }

    #[test]
    fn finds_config_by_walking_up_to_repo_root() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        fs::write(tmp.path().join(".cartoon.toml"), "").unwrap();
        let sub = tmp.path().join("a/b/c");
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(
            project_config_file(&sub),
            Some(tmp.path().join(".cartoon.toml"))
        );
    }

    #[test]
    fn stops_at_git_boundary_without_finding_ancestor_config() {
        let tmp = tempfile::tempdir().unwrap();
        // .cartoon.toml lives ABOVE the repo root — must not be picked up.
        fs::write(tmp.path().join(".cartoon.toml"), "").unwrap();
        let repo_root = tmp.path().join("repo");
        fs::create_dir_all(repo_root.join(".git")).unwrap();
        let sub = repo_root.join("src");
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(project_config_file(&sub), None);
    }

    #[test]
    fn returns_none_when_absent_and_no_git_boundary() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("x/y");
        fs::create_dir_all(&sub).unwrap();
        // No .cartoon.toml and no .git anywhere in this tree; walking up
        // terminates naturally at the real filesystem root (no infinite loop).
        assert_eq!(project_config_file(&sub), None);
    }
}
