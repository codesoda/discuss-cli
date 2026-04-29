use std::net::SocketAddr;
use std::path::PathBuf;
use std::{error::Error, io};

use thiserror::Error;

pub type Result<T> = std::result::Result<T, DiscussError>;
pub type BoxedError = Box<dyn Error + Send + Sync + 'static>;

#[derive(Debug, Error)]
pub enum DiscussError {
    #[error("file not found: {path} - check the path and try again")]
    FileNotFound { path: PathBuf },

    #[error(
        "file is not readable: {path} - check file permissions or choose another markdown file: {source}"
    )]
    FileNotReadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("port {port} is already in use - pass --port <N> or stop the other instance")]
    PortInUse { port: u16 },

    #[error(
        "config parse error in {path} at line {line}, column {col}: {message} - fix the TOML syntax or remove the invalid setting"
    )]
    ConfigParseError {
        path: PathBuf,
        line: usize,
        col: usize,
        message: String,
    },

    #[error("render error: {source} - check the markdown input and try again")]
    RenderError {
        #[source]
        source: BoxedError,
    },

    #[error(
        "server bind error at {addr}: {source} - pass --port <N> or check local networking permissions"
    )]
    ServerBindError {
        addr: SocketAddr,
        #[source]
        source: io::Error,
    },

    #[error(
        "logging initialization failed for {path}: {source} - check directory permissions or set HOME to a writable location"
    )]
    LoggingInitError {
        path: PathBuf,
        #[source]
        source: BoxedError,
    },

    #[error(
        "update check failed: {message} - check your network connection or GitHub release metadata, then run `discuss update --check` again"
    )]
    UpdateCheckError { message: String },

    #[error("update failed: {message}")]
    UpdateError { message: String },

    #[error("duplicate file argument: {path} - each input path can only be passed once")]
    DuplicateInputPath { path: PathBuf },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_display_contains(error: DiscussError, expected_parts: &[&str]) {
        let message = error.to_string();

        for expected_part in expected_parts {
            assert!(
                message.contains(expected_part),
                "expected {message:?} to contain {expected_part:?}"
            );
        }
    }

    #[test]
    fn file_not_found_message_names_path_and_suggestion() {
        assert_display_contains(
            DiscussError::FileNotFound {
                path: PathBuf::from("/tmp/missing.md"),
            },
            &["file not found", "/tmp/missing.md", "check the path"],
        );
    }

    #[test]
    fn file_not_readable_message_names_path_source_and_suggestion() {
        assert_display_contains(
            DiscussError::FileNotReadable {
                path: PathBuf::from("/tmp/private.md"),
                source: io::Error::new(io::ErrorKind::PermissionDenied, "permission denied"),
            },
            &[
                "file is not readable",
                "/tmp/private.md",
                "permission denied",
                "check file permissions",
            ],
        );
    }

    #[test]
    fn port_in_use_message_names_port_and_suggestion() {
        assert_display_contains(
            DiscussError::PortInUse { port: 7777 },
            &["port 7777", "already in use", "pass --port <N>"],
        );
    }

    #[test]
    fn config_parse_error_message_names_path_location_and_suggestion() {
        assert_display_contains(
            DiscussError::ConfigParseError {
                path: PathBuf::from("/tmp/discuss.config.toml"),
                line: 4,
                col: 12,
                message: "expected integer".to_string(),
            },
            &[
                "config parse error",
                "/tmp/discuss.config.toml",
                "line 4",
                "column 12",
                "expected integer",
                "fix the TOML syntax",
            ],
        );
    }

    #[test]
    fn render_error_message_names_source_and_suggestion() {
        assert_display_contains(
            DiscussError::RenderError {
                source: Box::new(io::Error::other("invalid markdown")),
            },
            &[
                "render error",
                "invalid markdown",
                "check the markdown input",
            ],
        );
    }

    #[test]
    fn server_bind_error_message_names_addr_source_and_suggestion() {
        assert_display_contains(
            DiscussError::ServerBindError {
                addr: SocketAddr::from(([127, 0, 0, 1], 7777)),
                source: io::Error::new(io::ErrorKind::AddrInUse, "address already in use"),
            },
            &[
                "server bind error",
                "127.0.0.1:7777",
                "address already in use",
                "pass --port <N>",
            ],
        );
    }

    #[test]
    fn logging_init_error_message_names_path_source_and_suggestion() {
        assert_display_contains(
            DiscussError::LoggingInitError {
                path: PathBuf::from("/tmp/discuss/logs"),
                source: Box::new(io::Error::other("permission denied")),
            },
            &[
                "logging initialization failed",
                "/tmp/discuss/logs",
                "permission denied",
                "check directory permissions",
            ],
        );
    }

    #[test]
    fn update_check_error_message_names_problem_and_suggestion() {
        assert_display_contains(
            DiscussError::UpdateCheckError {
                message: "GitHub did not return a Location header".to_string(),
            },
            &[
                "update check failed",
                "Location header",
                "network connection",
                "discuss update --check",
            ],
        );
    }

    #[test]
    fn update_error_message_names_problem_and_suggestion() {
        assert_display_contains(
            DiscussError::UpdateError {
                message: "stdin is not a TTY - rerun with `discuss update -y`".to_string(),
            },
            &["update failed", "stdin is not a TTY", "discuss update -y"],
        );
    }
}
