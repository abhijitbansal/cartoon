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

/// Replace `path` with `contents` atomically: write a temp file in the same
/// directory, then rename it over the target, so a crash or full disk can
/// never leave a half-written user file. An existing file's permissions are
/// kept; a new file gets 0644 (the temp file itself starts 0600).
pub fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    // Write through a symlink (CLAUDE.md -> AGENTS.md is common) instead of
    // replacing the link with a regular file.
    let resolved;
    let path = match std::fs::canonicalize(path) {
        Ok(p) => {
            resolved = p;
            resolved.as_path()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => path,
        Err(e) => return Err(e),
    };
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let existing_perms = match std::fs::metadata(path) {
        Ok(m) => Some(m.permissions()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let mut tmp = tempfile::Builder::new()
        .prefix(".cartoon-tmp-")
        .tempfile_in(dir)?;
    tmp.write_all(contents)?;
    tmp.as_file().sync_all()?;
    match existing_perms {
        Some(p) => tmp.as_file().set_permissions(p)?,
        #[cfg(unix)]
        None => {
            use std::os::unix::fs::PermissionsExt;
            tmp.as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o644))?
        }
        #[cfg(not(unix))]
        None => {}
    }
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn write_atomic_replaces_content_and_keeps_permissions() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("f.md");
        write_atomic(&p, b"one").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "one");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o644
            );
            fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        }
        write_atomic(&p, b"two").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // No temp files left behind.
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_writes_through_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("AGENTS.md");
        let link = tmp.path().join("CLAUDE.md");
        fs::write(&real, "old").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, b"new").unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
    }

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
