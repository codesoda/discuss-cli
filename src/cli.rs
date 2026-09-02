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
    about = "Launch a live bidirectional document or website review session.",
    subcommand_precedence_over_arg = true,
    after_help = HELP_FOOTER,
    after_long_help = HELP_FOOTER
)]
pub struct Args {
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u16).range(1..),
        help = "Bind the local review server to this exact port; when omitted, the OS assigns a port"
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
        long,
        value_name = "SPEC",
        help = "Configure review-finish verdict options. Must appear before the diff subcommand; quote '|' in shells."
    )]
    pub verdict_options: Option<String>,

    #[arg(
        long,
        value_name = "TEXT",
        help = "Prompt shown above verdict options. Ignored unless --verdict-options is also set."
    )]
    pub verdict_prompt: Option<String>,

    #[arg(
        value_name = "FILE_OR_URL",
        help = "One or more files to review together, or one HTTP/S URL for live website review. Use `-` to read from stdin; bare `discuss` with piped stdin also reads from stdin."
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

    #[command(
        about = "Review a git diff in the browser.",
        long_about = "Review a git diff in the browser.\n\n\
Defaults to the staged diff (git diff --cached). Pass --unstaged for the working tree, or pass\n\
<range> arguments through to `git diff` (e.g. HEAD~3..HEAD or main...feature).\n\n\
Combines with the file list: `discuss plan.md diff HEAD~1..HEAD` reviews the markdown file and\n\
the diff together in one session."
    )]
    Diff(DiffArgs),

    #[command(
        about = "Review a GitHub pull request privately before publishing.",
        long_about = "Review a GitHub pull request privately before publishing.\n\n\
Accepts a full https://github.com/OWNER/REPO/pull/NUMBER URL only. discuss starts a\n\
local session and prints machine-readable instructions for the active agent, which uses\n\
the authenticated gh CLI to import the PR. Nothing is posted to GitHub until the reviewer\n\
selects items, previews the exact GFM payload, and confirms publication.\n\n\
Top-level flags must come first: `discuss --no-open pr https://github.com/acme/app/pull/123`."
    )]
    Pr(PrArgs),

    #[command(
        about = "Open a self-contained demo review session with bundled example files.",
        long_about = "Open a self-contained demo review session with bundled example files.\n\n\
Every file is embedded in the binary: a feature-tour GIF, two revised markdown documents\n\
pre-annotated with agent takes, a diff, an image, and an HTML prototype. A deterministic\n\
Demo agent replies to comments you leave, entirely in-process: no agent session, no LLM,\n\
and no history archive is written.\n\n\
The review page behaves like any other session, so it still loads Prism syntax\n\
highlighting from a CDN and checks for a newer release. With no network the demo still\n\
runs; code fences lose highlighting and per-line diff comments.\n\n\
Top-level flags must come first: `discuss --port 4000 --no-open demo`."
    )]
    Demo,
}

#[derive(Debug, clap::Args)]
pub struct PrArgs {
    #[arg(
        value_name = "FULL_GITHUB_PR_URL",
        help = "Full GitHub pull request URL: https://github.com/OWNER/REPO/pull/NUMBER"
    )]
    pub url: String,
}

#[derive(Debug, clap::Args)]
pub struct DiffArgs {
    #[arg(
        long,
        conflicts_with = "args",
        help = "Show the working tree diff (git diff) instead of the staged diff (git diff --cached)."
    )]
    pub unstaged: bool,

    #[arg(
        long,
        value_name = "BYTES",
        help = "Override the diff size cap (default 5 MB). Use 0 to disable the cap."
    )]
    pub max_diff_bytes: Option<usize>,

    #[arg(
        value_name = "ARG",
        trailing_var_arg = true,
        allow_hyphen_values = true,
        help = "Additional arguments forwarded to `git diff` (e.g. HEAD~3..HEAD)."
    )]
    pub args: Vec<String>,
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
        assert_eq!(args.verdict_options, None);
        assert_eq!(args.verdict_prompt, None);
        assert_eq!(args.files, vec![PathBuf::from("plan.md")]);
        assert!(args.command.is_none());
    }

    #[test]
    fn parses_multiple_file_arguments() {
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
        assert!(args.command.is_none());
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
    fn parses_diff_subcommand_with_defaults() {
        let args = Args::try_parse_from(["discuss", "diff"]).expect("diff should parse");

        assert!(args.files.is_empty());
        let Some(Commands::Diff(diff_args)) = args.command else {
            panic!("expected diff subcommand");
        };
        assert!(!diff_args.unstaged);
        assert!(diff_args.args.is_empty());
        assert_eq!(diff_args.max_diff_bytes, None);
    }

    #[test]
    fn parses_diff_subcommand_with_unstaged_flag() {
        let args =
            Args::try_parse_from(["discuss", "diff", "--unstaged"]).expect("unstaged parses");

        let Some(Commands::Diff(diff_args)) = args.command else {
            panic!("expected diff subcommand");
        };
        assert!(diff_args.unstaged);
        assert!(diff_args.args.is_empty());
    }

    #[test]
    fn parses_diff_subcommand_with_range_args() {
        let args = Args::try_parse_from(["discuss", "diff", "HEAD~3..HEAD"]).expect("range parses");

        let Some(Commands::Diff(diff_args)) = args.command else {
            panic!("expected diff subcommand");
        };
        assert!(!diff_args.unstaged);
        assert_eq!(diff_args.args, vec!["HEAD~3..HEAD".to_string()]);
    }

    #[test]
    fn parses_diff_subcommand_with_max_diff_bytes() {
        let args = Args::try_parse_from(["discuss", "diff", "--max-diff-bytes", "1048576"])
            .expect("max-diff-bytes parses");

        let Some(Commands::Diff(diff_args)) = args.command else {
            panic!("expected diff subcommand");
        };
        assert_eq!(diff_args.max_diff_bytes, Some(1_048_576));
    }

    #[test]
    fn rejects_unstaged_with_range_args() {
        let error = Args::try_parse_from(["discuss", "diff", "--unstaged", "HEAD~3..HEAD"])
            .expect_err("unstaged with range should conflict");

        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parses_files_combined_with_diff_subcommand() {
        let args = Args::try_parse_from(["discuss", "plan.md", "notes.md", "diff", "HEAD~1..HEAD"])
            .expect("files + diff should parse");

        assert_eq!(
            args.files,
            vec![PathBuf::from("plan.md"), PathBuf::from("notes.md")]
        );
        let Some(Commands::Diff(diff_args)) = args.command else {
            panic!("expected diff subcommand");
        };
        assert_eq!(diff_args.args, vec!["HEAD~1..HEAD".to_string()]);
    }

    #[test]
    fn parses_files_combined_with_bare_diff() {
        let args =
            Args::try_parse_from(["discuss", "plan.md", "diff"]).expect("file + diff parses");

        assert_eq!(args.files, vec![PathBuf::from("plan.md")]);
        assert!(matches!(args.command, Some(Commands::Diff(_))));
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
    fn parses_verdict_flags_before_file() {
        let args = Args::try_parse_from([
            "discuss",
            "--verdict-options",
            "approved:Approve:positive|declined:Decline:negative!",
            "--verdict-prompt",
            "Final verdict?",
            "plan.md",
        ])
        .expect("verdict args should parse");

        assert_eq!(
            args.verdict_options,
            Some("approved:Approve:positive|declined:Decline:negative!".to_string())
        );
        assert_eq!(args.verdict_prompt, Some("Final verdict?".to_string()));
        assert_eq!(args.files, vec![PathBuf::from("plan.md")]);
    }

    #[test]
    fn parses_verdict_flags_before_diff_subcommand() {
        let args = Args::try_parse_from([
            "discuss",
            "--verdict-options",
            "approved|declined",
            "diff",
            "HEAD~1..HEAD",
        ])
        .expect("verdict args before diff should parse");

        assert_eq!(args.verdict_options, Some("approved|declined".to_string()));
        let Some(Commands::Diff(diff_args)) = args.command else {
            panic!("expected diff subcommand");
        };
        assert_eq!(diff_args.args, vec!["HEAD~1..HEAD".to_string()]);
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
    fn parses_pr_subcommand_with_full_url() {
        let args = Args::try_parse_from([
            "discuss",
            "pr",
            "https://github.com/codesoda/discuss-cli/pull/51",
        ])
        .expect("PR command should parse");

        let Some(Commands::Pr(pr_args)) = args.command else {
            panic!("expected pr subcommand");
        };
        assert_eq!(
            pr_args.url,
            "https://github.com/codesoda/discuss-cli/pull/51"
        );
    }

    #[test]
    fn pr_subcommand_requires_url() {
        let error = Args::try_parse_from(["discuss", "pr"])
            .expect_err("PR command without URL should fail");

        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn pr_help_documents_private_first_agent_workflow() {
        let mut command = Args::command();
        let pr = command
            .find_subcommand_mut("pr")
            .expect("pr subcommand should exist");
        let help = pr.render_long_help().to_string();

        for expected in [
            "full https://github.com/OWNER/REPO/pull/NUMBER URL only",
            "authenticated gh CLI",
            "Nothing is posted to GitHub",
            "previews the exact GFM payload",
        ] {
            assert!(
                help.contains(expected),
                "expected pr help to contain {expected:?}\n{help}"
            );
        }
    }

    #[test]
    fn parses_demo_subcommand() {
        let args = Args::try_parse_from(["discuss", "demo"]).expect("demo should parse");

        assert!(args.files.is_empty());
        assert!(matches!(args.command, Some(Commands::Demo)));
    }

    #[test]
    fn parses_demo_with_flags_before_subcommand() {
        let args = Args::try_parse_from(["discuss", "--port", "4000", "--no-open", "demo"])
            .expect("flags before demo should parse");

        assert_eq!(args.port, Some(4000));
        assert!(args.no_open);
        assert!(matches!(args.command, Some(Commands::Demo)));
    }

    #[test]
    fn parses_verdict_options_before_demo_subcommand() {
        let args = Args::try_parse_from(["discuss", "--verdict-options", "Ship it,Hold", "demo"])
            .expect("verdict options before demo should parse");

        assert_eq!(args.verdict_options, Some("Ship it,Hold".to_string()));
        assert!(matches!(args.command, Some(Commands::Demo)));
    }

    #[test]
    fn help_lists_demo_subcommand() {
        let help = Args::command().render_long_help().to_string();

        assert!(
            help.contains("demo") && help.contains("self-contained demo review session"),
            "expected help to list the demo subcommand\n{help}"
        );
    }

    #[test]
    fn demo_help_documents_flag_ordering() {
        let mut command = Args::command();
        let demo = command
            .find_subcommand_mut("demo")
            .expect("demo subcommand should exist");
        let help = demo.render_long_help().to_string();

        for expected in [
            "Open a self-contained demo review session with bundled example files.",
            "no agent session, no LLM",
            // The offline claim is scoped: the page still fetches Prism and the
            // version check, so the help must not promise a network-free run.
            "loads Prism syntax",
            "Top-level flags must come first",
            "discuss --port 4000 --no-open demo",
        ] {
            assert!(
                help.contains(expected),
                "expected demo help to contain {expected:?}\n{help}"
            );
        }
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
            "--verdict-options",
            "--verdict-prompt",
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
