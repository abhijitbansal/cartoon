use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct CompressCfg {
    pub level: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct CommandCfg {
    pub level: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub heuristic: bool,
    pub tokenizer: String,
    pub trace_lines: usize,
    pub keep_runs: usize,
    pub max_archive_mb: u64,
    pub compress: CompressCfg,
    pub command: HashMap<String, CommandCfg>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            heuristic: false,
            tokenizer: "o200k".into(),
            trace_lines: 20,
            keep_runs: 50,
            max_archive_mb: 50,
            compress: CompressCfg::default(),
            command: HashMap::new(),
        }
    }
}

/// Level precedence: CLI --compress > CLI --heuristic > config
/// [command.<argv0>] > config [compress] > legacy `heuristic = true` > Safe.
pub fn resolve_level(
    flag: Option<&str>,
    heuristic_flag: bool,
    argv0: &str,
    cfg: &Config,
) -> anyhow::Result<crate::ladder::CompressLevel> {
    use crate::ladder::CompressLevel;
    let parse = |s: &str| s.parse::<CompressLevel>().map_err(|e| anyhow::anyhow!(e));
    if let Some(s) = flag {
        return parse(s);
    }
    if heuristic_flag {
        return Ok(CompressLevel::Aggressive);
    }
    if let Some(s) = cfg.command.get(argv0).and_then(|c| c.level.as_deref()) {
        return parse(s);
    }
    if let Some(s) = cfg.compress.level.as_deref() {
        return parse(s);
    }
    if cfg.heuristic {
        return Ok(CompressLevel::Aggressive);
    }
    Ok(CompressLevel::Safe)
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

    #[test]
    fn compress_level_parses() {
        let c: Config = toml::from_str("[compress]\nlevel = \"aggressive\"").unwrap();
        assert_eq!(c.compress.level.as_deref(), Some("aggressive"));
    }

    #[test]
    fn per_command_level_parses() {
        let c: Config = toml::from_str("[command.docker]\nlevel = \"aggressive\"").unwrap();
        assert_eq!(c.command["docker"].level.as_deref(), Some("aggressive"));
    }

    mod resolve {
        use super::*;
        use crate::ladder::CompressLevel;

        fn cfg(toml_src: &str) -> Config {
            toml::from_str(toml_src).unwrap()
        }

        #[test]
        fn default_is_safe() {
            let l = resolve_level(None, false, "make", &Config::default()).unwrap();
            assert_eq!(l, CompressLevel::Safe);
        }

        #[test]
        fn flag_beats_per_command_config() {
            let c = cfg("[command.make]\nlevel = \"aggressive\"");
            let l = resolve_level(Some("safe"), false, "make", &c).unwrap();
            assert_eq!(l, CompressLevel::Safe);
        }

        #[test]
        fn heuristic_flag_maps_to_aggressive() {
            let l = resolve_level(None, true, "make", &Config::default()).unwrap();
            assert_eq!(l, CompressLevel::Aggressive);
        }

        #[test]
        fn per_command_beats_global() {
            let c = cfg("[compress]\nlevel = \"aggressive\"\n[command.make]\nlevel = \"safe\"");
            let l = resolve_level(None, false, "make", &c).unwrap();
            assert_eq!(l, CompressLevel::Safe);
        }

        #[test]
        fn global_beats_legacy() {
            let c = cfg("heuristic = true\n[compress]\nlevel = \"safe\"");
            let l = resolve_level(None, false, "make", &c).unwrap();
            assert_eq!(l, CompressLevel::Safe);
        }

        #[test]
        fn legacy_heuristic_maps_to_aggressive() {
            let c = cfg("heuristic = true");
            let l = resolve_level(None, false, "make", &c).unwrap();
            assert_eq!(l, CompressLevel::Aggressive);
        }

        #[test]
        fn invalid_level_errors() {
            assert!(resolve_level(Some("turbo"), false, "make", &Config::default()).is_err());
        }
    }
}
