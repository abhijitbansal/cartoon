use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub heuristic: bool,
    pub tokenizer: String,
    pub trace_lines: usize,
    pub keep_runs: usize,
    pub max_archive_mb: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            heuristic: false,
            tokenizer: "o200k".into(),
            trace_lines: 20,
            keep_runs: 50,
            max_archive_mb: 50,
        }
    }
}

pub fn load() -> Config {
    let Some(path) = crate::paths::config_file() else {
        return Config::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => parse_or_default(&s, &path.display().to_string()),
        Err(_) => Config::default(), // no config file is normal
    }
}

fn parse_or_default(s: &str, path: &str) -> Config {
    toml::from_str(s).unwrap_or_else(|e| {
        eprintln!("cartoon: invalid config {path}: {e}; using defaults");
        Config::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = Config::default();
        assert!(!c.heuristic);
        assert_eq!(c.tokenizer, "o200k");
        assert_eq!(c.trace_lines, 20);
    }

    #[test]
    fn partial_toml_overrides() {
        let c: Config = toml::from_str("heuristic = true").unwrap();
        assert!(c.heuristic);
        assert_eq!(c.tokenizer, "o200k");
    }

    #[test]
    fn bad_toml_falls_back_to_defaults() {
        let c = parse_or_default("not [ valid toml", "/tmp/x");
        assert!(!c.heuristic);
    }

    #[test]
    fn archive_defaults() {
        let c = Config::default();
        assert_eq!(c.keep_runs, 50);
        assert_eq!(c.max_archive_mb, 50);
    }

    #[test]
    fn archive_keys_override() {
        let c: Config = toml::from_str("keep_runs = 5\nmax_archive_mb = 10").unwrap();
        assert_eq!(c.keep_runs, 5);
        assert_eq!(c.max_archive_mb, 10);
        assert_eq!(c.tokenizer, "o200k"); // other defaults intact
    }
}
