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
    /// JUnit XML file (or directory of them) the command writes; rendered as
    /// a test report after the run (`--junit` on the CLI does the same).
    pub junit: Option<String>,
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
    /// Hard ceiling on emitted tokens (`--max-tokens` / `CARTOON_MAX_TOKENS`
    /// override). None = no ceiling.
    pub max_tokens: Option<usize>,
    /// Project-scoped scripts (e.g. `./build.sh`) the hook should always
    /// route through cartoon, matched by argv0 basename. Populated from
    /// a global and/or project-local config; see `merge`.
    pub wrap_scripts: Vec<String>,
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
            max_tokens: None,
            wrap_scripts: Vec::new(),
        }
    }
}

/// Layer a project-local config over the global one. Scoped deliberately
/// narrow: only `wrap_scripts` (extended) and `command` (project wins on key
/// collision) are merged — every other field comes from `global` untouched.
/// A wider merge is unsafe here: `#[serde(default)]` means a project file
/// that only declares `wrap_scripts` still deserializes with
/// `tokenizer: "o200k"`, `keep_runs: 50`, etc. — indistinguishable from a
/// file that set those explicitly, so blindly overriding scalars would
/// silently reset a customized global value.
pub fn merge(mut global: Config, project: Config) -> Config {
    global.wrap_scripts.extend(project.wrap_scripts);
    global.command.extend(project.command);
    global
}

/// Load the global config, then merge in a project-local `.cartoon.toml`
/// discovered by walking up from `cwd` (see `paths::project_config_file`).
/// A missing or invalid project file is a no-op (fail-open, matching `load`).
pub fn load_merged(cwd: &std::path::Path) -> Config {
    let global = load();
    match crate::paths::project_config_file(cwd) {
        Some(path) => {
            let project = std::fs::read_to_string(&path)
                .map(|s| parse_or_default(&s, &path.display().to_string()))
                .unwrap_or_default();
            merge(global, project)
        }
        None => global,
    }
}

/// The config a wrapped run should use: global + the project-local
/// `.cartoon.toml` for the current directory. Falls back to global-only
/// when the cwd is unreadable (fail-open).
pub fn load_for_cwd() -> Config {
    match std::env::current_dir() {
        Ok(cwd) => load_merged(&cwd),
        Err(_) => load(),
    }
}

/// Level precedence: CLI --compress > CLI --heuristic > config
/// [command.<argv0>] > `wrap_scripts` member (Aggressive) > config [compress]
/// > legacy `heuristic = true` > Safe.
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
    // A declared wrapper script is, by definition, noisy build/test output
    // the safe tier does nothing for; default it to aggressive.
    if cfg.wrap_scripts.iter().any(|s| s == argv0) {
        return Ok(CompressLevel::Aggressive);
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

/// Parse without falling back — for `cartoon doctor`, which wants the error.
pub fn check(s: &str) -> Result<(), String> {
    toml::from_str::<Config>(s)
        .map(|_| ())
        .map_err(|e| e.to_string())
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
    fn wrap_scripts_defaults_to_empty() {
        assert!(Config::default().wrap_scripts.is_empty());
    }

    #[test]
    fn wrap_script_defaults_to_aggressive_without_explicit_pin() {
        use crate::ladder::CompressLevel;
        let cfg: Config = toml::from_str(r#"wrap_scripts = ["./build.sh"]"#).unwrap();
        assert_eq!(
            resolve_level(None, false, "./build.sh", &cfg).unwrap(),
            CompressLevel::Aggressive
        );
        assert_eq!(
            resolve_level(None, false, "pytest", &cfg).unwrap(),
            CompressLevel::Safe
        );
        let pinned: Config = toml::from_str(
            "wrap_scripts = [\"./build.sh\"]\n[command.\"./build.sh\"]\nlevel = \"safe\"",
        )
        .unwrap();
        assert_eq!(
            resolve_level(None, false, "./build.sh", &pinned).unwrap(),
            CompressLevel::Safe
        );
    }

    #[test]
    fn wrap_scripts_parses_from_toml() {
        let c: Config = toml::from_str(r#"wrap_scripts = ["./build.sh"]"#).unwrap();
        assert_eq!(c.wrap_scripts, vec!["./build.sh".to_string()]);
    }

    mod merging {
        use super::*;

        fn cfg(toml_src: &str) -> Config {
            toml::from_str(toml_src).unwrap()
        }

        #[test]
        fn extends_wrap_scripts_with_project_entries() {
            let global = cfg(r#"wrap_scripts = ["a"]"#);
            let project = cfg(r#"wrap_scripts = ["b"]"#);
            let merged = merge(global, project);
            assert_eq!(merged.wrap_scripts, vec!["a".to_string(), "b".to_string()]);
        }

        #[test]
        fn project_command_wins_on_key_collision() {
            let global = cfg("[command.\"./build.sh\"]\nlevel = \"safe\"");
            let project = cfg("[command.\"./build.sh\"]\nlevel = \"aggressive\"");
            let merged = merge(global, project);
            assert_eq!(
                merged.command["./build.sh"].level.as_deref(),
                Some("aggressive")
            );
        }

        #[test]
        fn project_command_adds_without_dropping_global_keys() {
            let global = cfg("[command.pytest]\nlevel = \"safe\"");
            let project = cfg("[command.\"./build.sh\"]\nlevel = \"aggressive\"");
            let merged = merge(global, project);
            assert_eq!(merged.command["pytest"].level.as_deref(), Some("safe"));
            assert_eq!(
                merged.command["./build.sh"].level.as_deref(),
                Some("aggressive")
            );
        }

        #[test]
        fn load_merged_wires_project_file_into_global_config() {
            // Isolated global config dir so this never touches the real
            // ~/.config/cartoon/config.toml or races other tests (nothing
            // else in this suite reads/writes XDG_CONFIG_HOME).
            let xdg = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(xdg.path().join("cartoon")).unwrap();
            std::fs::write(
                xdg.path().join("cartoon/config.toml"),
                "[command.pytest]\nlevel = \"safe\"",
            )
            .unwrap();
            let prev = std::env::var("XDG_CONFIG_HOME").ok();
            std::env::set_var("XDG_CONFIG_HOME", xdg.path());

            let repo = tempfile::tempdir().unwrap();
            std::fs::write(
                repo.path().join(".cartoon.toml"),
                "wrap_scripts = [\"./build.sh\"]\n[command.\"./build.sh\"]\nlevel = \"aggressive\"",
            )
            .unwrap();

            let merged = load_merged(repo.path());

            match prev {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }

            assert_eq!(merged.wrap_scripts, vec!["./build.sh".to_string()]);
            assert_eq!(merged.command["pytest"].level.as_deref(), Some("safe"));
            assert_eq!(
                merged.command["./build.sh"].level.as_deref(),
                Some("aggressive")
            );
        }

        #[test]
        fn merge_does_not_touch_global_scalars() {
            let global = cfg("keep_runs = 5\nmax_archive_mb = 10");
            let project = cfg(r#"wrap_scripts = ["./build.sh"]"#);
            let merged = merge(global, project);
            // A project file that only declares wrap_scripts must not reset
            // a customized global scalar back to its TOML-deserialization
            // default (keep_runs 50 / max_archive_mb 50).
            assert_eq!(merged.keep_runs, 5);
            assert_eq!(merged.max_archive_mb, 10);
        }
    }

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
