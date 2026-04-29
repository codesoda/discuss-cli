use std::path::PathBuf;

use clap::{Parser, Subcommand};

const HELP_FOOTER: &str = "\
Exit codes:
  0  Clean exit (Done, or update completed)
  1  Generic failure (file not found, render error, etc.)
  2  Configuration / parse error
  3  Port already in use (or other server bind failure)
  5  Interrupted (Ctrl+C)

Docs: https://github.com/codesoda/discuss-cli
LLM ref: https://github.com/codesoda/discuss-cli/blob/main/llms.txt";

#[derive(Debug, Parser)]
#[command(
    name = "discuss",
    version,
    about = "Launch a live bidirectional markdown review session.",
    after_help = HELP_FOOTER,
    after_long_help = HELP_FOOTER
)]
pub struct Args {
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u16).range(1..),
        help = "Bind the local review server to this port"
    )]
    pub port: Option<u16>,

    #[arg(long, help = "Do not open the browser after the server starts")]
    pub no_open: bool,

    #[arg(long, help = "Do not write a history archive when the review is done")]
    pub no_save: bool,

    #[arg(
        long,
        value_name = "PATH",
        help = "Write history archives under this directory for this invocation"
    )]
    pub history_dir: Option<PathBuf>,

    #[arg(
        value_name = "FILE",
        help = "One or more files to review. Use `-` to read from stdin; bare `discuss` with piped stdin also reads from stdin. Pass multiple paths to review them together."
    )]
    pub files: Vec<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    #[command(
        about = "Explicitly check for and install discuss updates.",
        long_about = "Explicitly check for and install discuss updates.\n\n\
Updates are explicit-only: discuss never checks for updates automatically, so no env opt-out is needed."
    )]
    Update(UpdateArgs),
}

#[derive(Debug, clap::Args)]
pub struct UpdateArgs {
    #[arg(
        long,
        conflicts_with = "yes",
        help = "Check GitHub Releases for a newer version. This is explicit-only; discuss never checks automatically."
    )]
    pub check: bool,

    #[arg(
        short = 'y',
        long = "yes",
        conflicts_with = "check",
        help = "Download and install the latest release without an interactive prompt"
    )]
    pub yes: bool,
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser, error::ErrorKind};

    use super::*;

    #[test]
    fn bare_command_parses_with_no_file_or_subcommand() {
        let args = Args::try_parse_from(["discuss"]).expect("bare command should parse");

        assert!(args.files.is_empty());
        assert!(args.command.is_none());
    }

    #[test]
    fn parses_stdin_dash_argument() {
        let args = Args::try_parse_from(["discuss", "-"]).expect("dash should parse as stdin");

        assert_eq!(args.files, vec![PathBuf::from("-")]);
    }

    #[test]
    fn parses_markdown_file_argument() {
        let args = Args::try_parse_from(["discuss", "plan.md"]).expect("file arg should parse");

        assert_eq!(args.port, None);
        assert!(!args.no_open);
        assert!(!args.no_save);
        assert_eq!(args.history_dir, None);
        assert_eq!(args.files, vec![PathBuf::from("plan.md")]);
        assert!(args.command.is_none());
    }

    #[test]
    fn parses_multiple_file_arguments_in_order() {
        let args = Args::try_parse_from(["discuss", "a.md", "b.md", "c.md"])
            .expect("multi file args should parse");

        assert_eq!(
            args.files,
            vec![
                PathBuf::from("a.md"),
                PathBuf::from("b.md"),
                PathBuf::from("c.md"),
            ]
        );
    }

    #[test]
    fn parses_mixed_stdin_and_file_arguments() {
        let args = Args::try_parse_from(["discuss", "a.md", "-", "b.md"])
            .expect("mixed stdin + files should parse");

        assert_eq!(
            args.files,
            vec![
                PathBuf::from("a.md"),
                PathBuf::from("-"),
                PathBuf::from("b.md"),
            ]
        );
    }

    #[test]
    fn parses_port_override() {
        let args = Args::try_parse_from(["discuss", "--port", "8888", "plan.md"])
            .expect("port arg should parse");

        assert_eq!(args.port, Some(8888));
        assert!(!args.no_open);
        assert!(!args.no_save);
        assert_eq!(args.history_dir, None);
        assert_eq!(args.files, vec![PathBuf::from("plan.md")]);
    }

    #[test]
    fn parses_no_open_flag() {
        let args = Args::try_parse_from(["discuss", "--no-open", "plan.md"])
            .expect("no-open arg should parse");

        assert!(args.no_open);
        assert!(!args.no_save);
        assert_eq!(args.history_dir, None);
        assert_eq!(args.files, vec![PathBuf::from("plan.md")]);
    }

    #[test]
    fn parses_history_archive_flags() {
        let args = Args::try_parse_from([
            "discuss",
            "--no-save",
            "--history-dir",
            "/tmp/discuss-history",
            "plan.md",
        ])
        .expect("history archive flags should parse");

        assert!(args.no_save);
        assert_eq!(
            args.history_dir,
            Some(PathBuf::from("/tmp/discuss-history"))
        );
        assert_eq!(args.files, vec![PathBuf::from("plan.md")]);
    }

    #[test]
    fn rejects_zero_port_override() {
        let error = Args::try_parse_from(["discuss", "--port", "0", "plan.md"])
            .expect_err("port 0 should be rejected");

        assert_eq!(error.kind(), ErrorKind::ValueValidation);
    }

    #[test]
    fn parses_update_subcommand() {
        let args = Args::try_parse_from(["discuss", "update"]).expect("update should parse");

        assert_eq!(args.port, None);
        assert!(!args.no_open);
        assert!(!args.no_save);
        assert_eq!(args.history_dir, None);
        assert!(args.files.is_empty());
        assert!(matches!(
            args.command,
            Some(Commands::Update(UpdateArgs {
                check: false,
                yes: false
            }))
        ));
    }

    #[test]
    fn parses_update_check_flag() {
        let args =
            Args::try_parse_from(["discuss", "update", "--check"]).expect("update check parses");

        assert!(matches!(
            args.command,
            Some(Commands::Update(UpdateArgs {
                check: true,
                yes: false
            }))
        ));
    }

    #[test]
    fn parses_update_yes_flag() {
        let args = Args::try_parse_from(["discuss", "update", "-y"]).expect("update yes parses");

        assert!(matches!(
            args.command,
            Some(Commands::Update(UpdateArgs {
                check: false,
                yes: true
            }))
        ));
    }

    #[test]
    fn rejects_update_check_with_yes() {
        let error = Args::try_parse_from(["discuss", "update", "--check", "--yes"])
            .expect_err("check and yes should conflict");

        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn help_contains_exit_codes_and_references() {
        let help = Args::command().render_long_help().to_string();

        for expected in [
            "Exit codes:",
            "0  Clean exit",
            "1  Generic failure",
            "2  Configuration / parse error",
            "3  Port already in use",
            "5  Interrupted",
            "Docs:",
            "LLM ref:",
            "--no-save",
            "--history-dir",
        ] {
            assert!(
                help.contains(expected),
                "expected help to contain {expected:?}\n{help}"
            );
        }
    }

    #[test]
    fn version_reports_package_version() {
        let error =
            Args::try_parse_from(["discuss", "--version"]).expect_err("--version should exit");

        assert_eq!(error.kind(), ErrorKind::DisplayVersion);
        assert!(error.to_string().contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn update_help_mentions_explicit_only_checks() {
        let mut command = Args::command();
        let update = command
            .find_subcommand_mut("update")
            .expect("update subcommand should exist");
        let help = update.render_long_help().to_string();

        for expected in [
            "Explicitly check for and install discuss updates.",
            "Updates are explicit-only",
            "no env opt-out is needed",
            "--check",
            "--yes",
        ] {
            assert!(
                help.contains(expected),
                "expected update help to contain {expected:?}\n{help}"
            );
        }
    }
}
