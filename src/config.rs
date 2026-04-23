use std::path::{Path, PathBuf};

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
}

impl Config {
    pub fn from_toml_str(source: &str, path: impl AsRef<Path>) -> Result<Self> {
        toml::from_str(source).map_err(|error| config_parse_error(path.as_ref(), source, error))
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
        }
    }
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
}
