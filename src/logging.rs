use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

use crate::{Config, DiscussError, Result, error::BoxedError};

const LOG_FILE_PREFIX: &str = "discuss";
const LOG_FILE_SUFFIX: &str = "log";

pub fn init_tracing(config: &Config) -> Result<()> {
    let log_dir = default_log_dir()?;
    init_tracing_in_dir(config, &log_dir)
}

fn init_tracing_in_dir(config: &Config, log_dir: &Path) -> Result<()> {
    fs::create_dir_all(log_dir).map_err(|source| logging_init_error(log_dir, source))?;

    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix(LOG_FILE_SUFFIX)
        .build(log_dir)
        .map_err(|source| logging_init_error(log_dir, source))?;

    let filter = EnvFilter::try_new(log_filter_directive(config))
        .map_err(|source| logging_init_error(log_dir, source))?;

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(file_appender)
        .try_init()
        .map_err(|source| logging_init_error(log_dir, source))?;

    Ok(())
}

fn default_log_dir() -> Result<PathBuf> {
    BaseDirs::new()
        .map(|base_dirs| base_dirs.home_dir().join(".discuss").join("logs"))
        .ok_or_else(|| {
            logging_init_error(
                Path::new("~/.discuss/logs"),
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "could not determine home directory",
                ),
            )
        })
}

fn log_filter_directive(config: &Config) -> String {
    log_filter_directive_from(config, env::var("DISCUSS_LOG").ok().as_deref())
}

fn log_filter_directive_from(config: &Config, discuss_log: Option<&str>) -> String {
    discuss_log
        .map(str::to_owned)
        .or_else(|| config.log_level.clone())
        .unwrap_or_else(|| "info".to_string())
}

fn logging_init_error(path: &Path, source: impl Into<BoxedError>) -> DiscussError {
    DiscussError::LoggingInitError {
        path: path.to_path_buf(),
        source: source.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    use tempfile::tempdir;

    use super::*;

    static ENV_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

    fn env_mutex() -> &'static Mutex<()> {
        ENV_MUTEX.get_or_init(|| Mutex::new(()))
    }

    struct EnvGuard {
        home: Option<OsString>,
        discuss_log: Option<OsString>,
    }

    impl EnvGuard {
        fn set_temp_home(home: &Path) -> Self {
            let guard = Self {
                home: env::var_os("HOME"),
                discuss_log: env::var_os("DISCUSS_LOG"),
            };

            // SAFETY: callers acquire `env_mutex()` before constructing an EnvGuard,
            // serializing all process-wide env mutations within these tests.
            unsafe {
                env::set_var("HOME", home);
                env::remove_var("DISCUSS_LOG");
            }

            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: same `env_mutex()` invariant as `set_temp_home`; the guard's
            // lifetime is bounded by the lock the test holds.
            unsafe {
                if let Some(home) = &self.home {
                    env::set_var("HOME", home);
                } else {
                    env::remove_var("HOME");
                }

                if let Some(discuss_log) = &self.discuss_log {
                    env::set_var("DISCUSS_LOG", discuss_log);
                } else {
                    env::remove_var("DISCUSS_LOG");
                }
            }
        }
    }

    #[test]
    fn log_filter_prefers_env_then_config_then_info() {
        let config = Config {
            log_level: Some("warn".to_string()),
            ..Config::default()
        };

        assert_eq!(log_filter_directive_from(&config, Some("debug")), "debug");
        assert_eq!(log_filter_directive_from(&config, None), "warn");
        assert_eq!(log_filter_directive_from(&Config::default(), None), "info");
    }

    #[test]
    fn init_tracing_creates_daily_log_file_for_temp_home() {
        let _env_guard = env_mutex().lock().expect("env mutex should lock");
        let temp_home = tempdir().expect("temp home should be created");
        let _env_vars = EnvGuard::set_temp_home(temp_home.path());

        init_tracing(&Config::default()).expect("tracing should initialize");
        tracing::info!("test log event");

        let log_dir = temp_home.path().join(".discuss").join("logs");
        let log_files = fs::read_dir(&log_dir)
            .expect("log dir should exist")
            .map(|entry| entry.expect("log entry should be readable").path())
            .collect::<Vec<_>>();

        assert_eq!(log_files.len(), 1);
        let log_file_name = log_files[0]
            .file_name()
            .expect("log file should have a file name")
            .to_string_lossy();
        assert!(log_file_name.starts_with("discuss."));
        assert!(log_file_name.ends_with(".log"));
        assert!(
            fs::read_to_string(&log_files[0])
                .expect("log file should be readable")
                .contains("test log event")
        );
    }
}
