Build using Rust CLI with CLAP

Even though you are focused on a particular spec right now, make sure you consider the entire PRD, which can be found at tasks/prd-discuss-cli.md, it contains the full scope of what the current spec requirements are.

## Quality Checks

- Formatting checks are not sufficient validation. Rust changes must pass a strict compile check.
- Treat Rust warnings as errors for Cargo validation commands. Use `RUSTFLAGS="-D warnings"` with `cargo check`, `cargo build`, and `cargo test`.
- Run Clippy with warnings denied: `cargo clippy --all-targets -- -D warnings`.
