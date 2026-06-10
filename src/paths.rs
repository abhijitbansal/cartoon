use std::path::PathBuf;

pub fn config_file() -> Option<PathBuf> {
    base("XDG_CONFIG_HOME", ".config").map(|d| d.join("cartoon/config.toml"))
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
