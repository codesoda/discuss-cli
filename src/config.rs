use std::fmt::Display;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::{env, str::FromStr};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};

use crate::{DiscussError, Result};

const DEFAULT_AUTO_OPEN: bool = true;
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 600;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub port: Option<u16>,
    pub auto_open: bool,
    pub idle_timeout_secs: u64,
    pub history_dir: Option<PathBuf>,
    pub no_save: bool,
    pub log_level: Option<String>,
    pub max_diff_bytes: Option<usize>,
}

impl Config {
    pub fn from_toml_str(source: &str, path: impl AsRef<Path>) -> Result<Self> {
        toml::from_str(source).map_err(|error| config_parse_error(path.as_ref(), source, error))
    }

    pub fn resolve(cli_overrides: ConfigOverrides) -> Result<Self> {
        let user_config_path = BaseDirs::new().map(|base_dirs| {
            base_dirs
                .home_dir()
                .join(".discuss")
                .join("discuss.config.toml")
        });
        let project_config_path = PathBuf::from("discuss.config.toml");

        Self::resolve_with_sources(
            cli_overrides,
            user_config_path.as_deref(),
            Some(project_config_path.as_path()),
            env::vars(),
        )
    }

    fn resolve_with_sources<I>(
        cli_overrides: ConfigOverrides,
        user_config_path: Option<&Path>,
        project_config_path: Option<&Path>,
        env_vars: I,
    ) -> Result<Self>
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut config = Config::default();

        for path in [user_config_path, project_config_path]
            .into_iter()
            .flatten()
        {
            if let Some(layer) = read_config_layer(path)? {
                layer.apply_to(&mut config);
            }
        }

        ConfigLayer::from_env(env_vars)?.apply_to(&mut config);
        cli_overrides.apply_to(&mut config);

        Ok(config)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: None,
            auto_open: DEFAULT_AUTO_OPEN,
            idle_timeout_secs: DEFAULT_IDLE_TIMEOUT_SECS,
            history_dir: None,
            no_save: false,
            log_level: None,
            max_diff_bytes: None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfigOverrides {
    pub port: Option<u16>,
    pub auto_open: Option<bool>,
    pub idle_timeout_secs: Option<u64>,
    pub history_dir: Option<PathBuf>,
    pub no_save: Option<bool>,
    pub log_level: Option<String>,
    pub max_diff_bytes: Option<usize>,
}

impl ConfigOverrides {
    fn apply_to(self, config: &mut Config) {
        if let Some(port) = self.port {
            config.port = Some(port);
        }

        if let Some(auto_open) = self.auto_open {
            config.auto_open = auto_open;
        }

        if let Some(idle_timeout_secs) = self.idle_timeout_secs {
            config.idle_timeout_secs = idle_timeout_secs;
        }

        if let Some(history_dir) = self.history_dir {
            config.history_dir = Some(history_dir);
        }

        if let Some(no_save) = self.no_save {
            config.no_save = no_save;
        }

        if let Some(log_level) = self.log_level {
            config.log_level = Some(log_level);
        }

        if let Some(max_diff_bytes) = self.max_diff_bytes {
            config.max_diff_bytes = Some(max_diff_bytes);
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct ConfigLayer {
    port: Option<u16>,
    auto_open: Option<bool>,
    idle_timeout_secs: Option<u64>,
    history_dir: Option<PathBuf>,
    no_save: Option<bool>,
    log_level: Option<String>,
    max_diff_bytes: Option<usize>,
}

impl ConfigLayer {
    fn from_toml_str(source: &str, path: &Path) -> Result<Self> {
        toml::from_str(source).map_err(|error| config_parse_error(path, source, error))
    }

    fn from_env<I>(env_vars: I) -> Result<Self>
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut layer = Self::default();

        for (name, value) in env_vars {
            match name.as_str() {
                "DISCUSS_PORT" => layer.port = Some(parse_env_var(&name, &value)?),
                "DISCUSS_AUTO_OPEN" => layer.auto_open = Some(parse_env_var(&name, &value)?),
                "DISCUSS_IDLE_TIMEOUT_SECS" => {
                    layer.idle_timeout_secs = Some(parse_env_var(&name, &value)?);
                }
                "DISCUSS_HISTORY_DIR" => layer.history_dir = Some(PathBuf::from(value)),
                "DISCUSS_NO_SAVE" => layer.no_save = Some(parse_env_var(&name, &value)?),
                "DISCUSS_LOG" => layer.log_level = Some(value),
                "DISCUSS_MAX_DIFF_BYTES" => {
                    layer.max_diff_bytes = Some(parse_env_var(&name, &value)?);
                }
                _ => {}
            }
        }

        Ok(layer)
    }

    fn apply_to(self, config: &mut Config) {
        if let Some(port) = self.port {
            config.port = Some(port);
        }

        if let Some(auto_open) = self.auto_open {
            config.auto_open = auto_open;
        }

        if let Some(idle_timeout_secs) = self.idle_timeout_secs {
            config.idle_timeout_secs = idle_timeout_secs;
        }

        if let Some(history_dir) = self.history_dir {
            config.history_dir = Some(history_dir);
        }

        if let Some(no_save) = self.no_save {
            config.no_save = no_save;
        }

        if let Some(log_level) = self.log_level {
            config.log_level = Some(log_level);
        }

        if let Some(max_diff_bytes) = self.max_diff_bytes {
            config.max_diff_bytes = Some(max_diff_bytes);
        }
    }
}

fn read_config_layer(path: &Path) -> Result<Option<ConfigLayer>> {
    match fs::read_to_string(path) {
        Ok(source) => ConfigLayer::from_toml_str(&source, path).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DiscussError::ConfigParseError {
            path: path.to_path_buf(),
            line: 0,
            col: 0,
            message: format!("could not read config file: {error}"),
        }),
    }
}

fn parse_env_var<T>(name: &str, value: &str) -> Result<T>
where
    T: FromStr,
    T::Err: Display,
{
    value
        .parse()
        .map_err(|error| DiscussError::ConfigParseError {
            path: PathBuf::from(name),
            line: 0,
            col: 0,
            message: format!("invalid value {value:?}: {error}"),
        })
}

fn config_parse_error(path: &Path, source: &str, error: toml::de::Error) -> DiscussError {
    let (line, col) = error
        .span()
        .map(|span| line_col_for_offset(source, span.start))
        .unwrap_or((0, 0));

    DiscussError::ConfigParseError {
        path: path.to_path_buf(),
        line,
        col,
        message: error.message().to_string(),
    }
}

fn line_col_for_offset(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;

    for (index, character) in source.char_indices() {
        if index >= offset {
            break;
        }

        if character == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }

    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    #[test]
    fn default_config_uses_documented_values() {
        assert_eq!(
            Config::default(),
            Config {
                port: None,
                auto_open: true,
                idle_timeout_secs: 600,
                history_dir: None,
                no_save: false,
                log_level: None,
                max_diff_bytes: None,
            }
        );
    }

    #[test]
    fn deserializes_valid_toml_with_defaults_for_omitted_fields() {
        let config = Config::from_toml_str(
            r#"
port = 8888
auto_open = false
idle_timeout_secs = 30
history_dir = "/tmp/discuss-history"
no_save = true
log_level = "debug"
"#,
            "discuss.config.toml",
        )
        .expect("valid config should parse");

        assert_eq!(config.port, Some(8888));
        assert!(!config.auto_open);
        assert_eq!(config.idle_timeout_secs, 30);
        assert_eq!(
            config.history_dir,
            Some(PathBuf::from("/tmp/discuss-history"))
        );
        assert!(config.no_save);
        assert_eq!(config.log_level.as_deref(), Some("debug"));
    }

    #[test]
    fn deserializes_partial_toml_using_config_defaults() {
        let config = Config::from_toml_str(
            r#"
port = 9999
"#,
            "discuss.config.toml",
        )
        .expect("partial config should parse");

        assert_eq!(
            config,
            Config {
                port: Some(9999),
                ..Config::default()
            }
        );
    }

    #[test]
    fn rejects_unknown_fields_as_config_parse_errors() {
        let error = Config::from_toml_str(
            r#"
port = 7777
porrt = 8888
"#,
            "/tmp/discuss.config.toml",
        )
        .expect_err("unknown fields should be rejected");

        match error {
            DiscussError::ConfigParseError {
                path,
                line,
                col,
                message,
            } => {
                assert_eq!(path, PathBuf::from("/tmp/discuss.config.toml"));
                assert_eq!(line, 3);
                assert!(col > 0);
                assert!(message.contains("unknown field"));
                assert!(message.contains("porrt"));
            }
            other => panic!("expected config parse error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_ignores_missing_config_files() {
        let temp_dir = tempdir().expect("tempdir should be created");
        let user_path = temp_dir.path().join("missing-user.toml");
        let project_path = temp_dir.path().join("missing-project.toml");

        let config = Config::resolve_with_sources(
            ConfigOverrides::default(),
            Some(&user_path),
            Some(&project_path),
            std::iter::empty::<(String, String)>(),
        )
        .expect("missing config files should be ignored");

        assert_eq!(config, Config::default());
    }

    #[test]
    fn project_toml_overrides_user_toml_without_resetting_omitted_fields() {
        let temp_dir = tempdir().expect("tempdir should be created");
        let user_path = temp_dir.path().join("user.toml");
        let project_path = temp_dir.path().join("project.toml");

        fs::write(
            &user_path,
            r#"
port = 1111
auto_open = false
idle_timeout_secs = 20
"#,
        )
        .expect("user config should be written");
        fs::write(
            &project_path,
            r#"
port = 2222
no_save = true
"#,
        )
        .expect("project config should be written");

        let config = Config::resolve_with_sources(
            ConfigOverrides::default(),
            Some(&user_path),
            Some(&project_path),
            std::iter::empty::<(String, String)>(),
        )
        .expect("layered config should resolve");

        assert_eq!(config.port, Some(2222));
        assert!(!config.auto_open);
        assert_eq!(config.idle_timeout_secs, 20);
        assert!(config.no_save);
    }

    #[test]
    fn env_vars_override_toml_layers() {
        let temp_dir = tempdir().expect("tempdir should be created");
        let project_path = temp_dir.path().join("project.toml");

        fs::write(
            &project_path,
            r#"
port = 2222
auto_open = false
idle_timeout_secs = 20
history_dir = "/project/history"
no_save = false
log_level = "warn"
"#,
        )
        .expect("project config should be written");

        let config = Config::resolve_with_sources(
            ConfigOverrides::default(),
            None,
            Some(&project_path),
            [
                ("DISCUSS_PORT".to_string(), "3333".to_string()),
                ("DISCUSS_AUTO_OPEN".to_string(), "true".to_string()),
                ("DISCUSS_IDLE_TIMEOUT_SECS".to_string(), "40".to_string()),
                (
                    "DISCUSS_HISTORY_DIR".to_string(),
                    "/env/history".to_string(),
                ),
                ("DISCUSS_NO_SAVE".to_string(), "true".to_string()),
                ("DISCUSS_LOG".to_string(), "debug".to_string()),
            ],
        )
        .expect("env config should resolve");

        assert_eq!(config.port, Some(3333));
        assert!(config.auto_open);
        assert_eq!(config.idle_timeout_secs, 40);
        assert_eq!(config.history_dir, Some(PathBuf::from("/env/history")));
        assert!(config.no_save);
        assert_eq!(config.log_level.as_deref(), Some("debug"));
    }

    #[test]
    fn cli_overrides_win_over_env_vars() {
        let config = Config::resolve_with_sources(
            ConfigOverrides {
                port: Some(4444),
                auto_open: Some(false),
                idle_timeout_secs: Some(55),
                history_dir: Some(PathBuf::from("/cli/history")),
                no_save: Some(false),
                log_level: Some("trace".to_string()),
                max_diff_bytes: None,
            },
            None,
            None,
            [
                ("DISCUSS_PORT".to_string(), "3333".to_string()),
                ("DISCUSS_AUTO_OPEN".to_string(), "true".to_string()),
                ("DISCUSS_IDLE_TIMEOUT_SECS".to_string(), "40".to_string()),
                (
                    "DISCUSS_HISTORY_DIR".to_string(),
                    "/env/history".to_string(),
                ),
                ("DISCUSS_NO_SAVE".to_string(), "true".to_string()),
                ("DISCUSS_LOG".to_string(), "debug".to_string()),
            ],
        )
        .expect("cli overrides should resolve");

        assert_eq!(config.port, Some(4444));
        assert!(!config.auto_open);
        assert_eq!(config.idle_timeout_secs, 55);
        assert_eq!(config.history_dir, Some(PathBuf::from("/cli/history")));
        assert!(!config.no_save);
        assert_eq!(config.log_level.as_deref(), Some("trace"));
    }

    #[test]
    fn malformed_config_file_returns_path_aware_parse_error() {
        let temp_dir = tempdir().expect("tempdir should be created");
        let project_path = temp_dir.path().join("project.toml");

        fs::write(&project_path, "port = 'not a number'\n")
            .expect("project config should be written");

        let error = Config::resolve_with_sources(
            ConfigOverrides::default(),
            None,
            Some(&project_path),
            std::iter::empty::<(String, String)>(),
        )
        .expect_err("malformed config should fail");

        match error {
            DiscussError::ConfigParseError {
                path,
                line,
                col,
                message,
            } => {
                assert_eq!(path, project_path);
                assert_eq!(line, 1);
                assert!(col > 0);
                assert!(
                    message.contains("invalid type") || message.contains("expected"),
                    "unexpected parse message: {message}"
                );
            }
            other => panic!("expected config parse error, got {other:?}"),
        }
    }

    #[test]
    fn invalid_env_var_returns_config_parse_error() {
        let error = Config::resolve_with_sources(
            ConfigOverrides::default(),
            None,
            None,
            [("DISCUSS_PORT".to_string(), "not-a-port".to_string())],
        )
        .expect_err("invalid env value should fail");

        match error {
            DiscussError::ConfigParseError { path, message, .. } => {
                assert_eq!(path, PathBuf::from("DISCUSS_PORT"));
                assert!(message.contains("invalid value"));
                assert!(message.contains("not-a-port"));
            }
            other => panic!("expected config parse error, got {other:?}"),
        }
    }
}
