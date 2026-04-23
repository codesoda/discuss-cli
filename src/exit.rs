use crate::DiscussError;

pub const EXIT_SUCCESS: i32 = 0;
pub const EXIT_GENERIC_FAILURE: i32 = 1;
pub const EXIT_CONFIG_ERROR: i32 = 2;
pub const EXIT_SERVER_ERROR: i32 = 3;
pub const EXIT_INTERRUPTED: i32 = 5;

pub fn exit_code_for_error(error: &DiscussError) -> i32 {
    match error {
        DiscussError::ConfigParseError { .. } => EXIT_CONFIG_ERROR,
        DiscussError::PortInUse { .. } | DiscussError::ServerBindError { .. } => EXIT_SERVER_ERROR,
        DiscussError::FileNotFound { .. }
        | DiscussError::FileNotReadable { .. }
        | DiscussError::RenderError { .. }
        | DiscussError::LoggingInitError { .. }
        | DiscussError::UpdateCheckError { .. } => EXIT_GENERIC_FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::net::SocketAddr;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn maps_port_in_use_to_server_exit_code() {
        assert_eq!(
            exit_code_for_error(&DiscussError::PortInUse { port: 7777 }),
            EXIT_SERVER_ERROR
        );
    }

    #[test]
    fn maps_all_current_error_variants_to_documented_codes() {
        let cases = [
            (
                DiscussError::FileNotFound {
                    path: PathBuf::from("missing.md"),
                },
                EXIT_GENERIC_FAILURE,
            ),
            (
                DiscussError::FileNotReadable {
                    path: PathBuf::from("private.md"),
                    source: io::Error::new(io::ErrorKind::PermissionDenied, "permission denied"),
                },
                EXIT_GENERIC_FAILURE,
            ),
            (DiscussError::PortInUse { port: 7777 }, EXIT_SERVER_ERROR),
            (
                DiscussError::ConfigParseError {
                    path: PathBuf::from("discuss.config.toml"),
                    line: 1,
                    col: 1,
                    message: "bad config".to_string(),
                },
                EXIT_CONFIG_ERROR,
            ),
            (
                DiscussError::RenderError {
                    source: Box::new(io::Error::other("render failed")),
                },
                EXIT_GENERIC_FAILURE,
            ),
            (
                DiscussError::ServerBindError {
                    addr: SocketAddr::from(([127, 0, 0, 1], 7777)),
                    source: io::Error::new(io::ErrorKind::AddrInUse, "address already in use"),
                },
                EXIT_SERVER_ERROR,
            ),
            (
                DiscussError::LoggingInitError {
                    path: PathBuf::from("/tmp/discuss/logs"),
                    source: Box::new(io::Error::other("permission denied")),
                },
                EXIT_GENERIC_FAILURE,
            ),
            (
                DiscussError::UpdateCheckError {
                    message: "GitHub did not return a Location header".to_string(),
                },
                EXIT_GENERIC_FAILURE,
            ),
        ];

        for (error, expected_code) in cases {
            assert_eq!(exit_code_for_error(&error), expected_code);
        }
    }
}
