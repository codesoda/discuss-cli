use std::process::Command;

use crate::error::{DiscussError, Result};

#[derive(Debug)]
pub struct DiffOutput {
    pub git_args: Vec<String>,
    pub files: Vec<DiffFile>,
}

#[derive(Debug, Clone)]
pub struct DiffFile {
    pub path: String,
    pub content: String,
}

pub fn run_git_diff(unstaged: bool, extra: &[String]) -> Result<DiffOutput> {
    let mut git_args: Vec<String> =
        vec!["diff".into(), "--no-color".into(), "--no-ext-diff".into()];
    if extra.is_empty() && !unstaged {
        git_args.push("--cached".into());
    }
    if !extra.is_empty() {
        git_args.extend(extra.iter().cloned());
    }

    let output = Command::new("git")
        .args(&git_args)
        .output()
        .map_err(|source| DiscussError::DiffError {
            message: format!("failed to spawn `git`: {source}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if stderr.is_empty() {
            format!("git exited with status {}", output.status)
        } else {
            stderr
        };
        return Err(DiscussError::DiffError { message });
    }

    let stdout = String::from_utf8(output.stdout).map_err(|source| DiscussError::DiffError {
        message: format!("git diff produced non-UTF-8 output: {source}"),
    })?;

    Ok(DiffOutput {
        git_args,
        files: split_into_files(&stdout),
    })
}

fn split_into_files(unified: &str) -> Vec<DiffFile> {
    let starts: Vec<usize> = unified
        .match_indices("diff --git ")
        .map(|(idx, _)| idx)
        .collect();

    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(unified.len());
            let block = &unified[start..end];
            let path = parse_path_from_block(block).unwrap_or_else(|| format!("diff-{}", i + 1));
            DiffFile {
                path,
                content: block.to_string(),
            }
        })
        .collect()
}

fn parse_path_from_block(block: &str) -> Option<String> {
    let header = block.lines().next()?;
    let after = header.strip_prefix("diff --git ")?;
    let mut tokens = after.split_whitespace();
    let _a = tokens.next()?;
    let b = tokens.next()?;
    let stripped = b.strip_prefix("b/").unwrap_or(b);
    Some(stripped.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
diff --git a/foo.rs b/foo.rs
index 1234..5678 100644
--- a/foo.rs
+++ b/foo.rs
@@ -1 +1 @@
-old
+new
diff --git a/bar.md b/bar.md
new file mode 100644
index 0000..89ab
--- /dev/null
+++ b/bar.md
@@ -0,0 +1 @@
+hello
";

    #[test]
    fn split_into_files_returns_one_per_diff_header() {
        let files = split_into_files(SAMPLE);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "foo.rs");
        assert!(files[0].content.contains("@@ -1 +1 @@"));
        assert!(files[0].content.contains("+new"));
        assert_eq!(files[1].path, "bar.md");
        assert!(files[1].content.contains("+hello"));
    }

    #[test]
    fn split_into_files_returns_empty_when_no_diff_headers() {
        assert!(split_into_files("").is_empty());
        assert!(split_into_files("not a diff\n").is_empty());
    }

    #[test]
    fn parse_path_from_block_handles_renames_with_b_prefix() {
        let block =
            "diff --git a/old.md b/new/path.md\nrename from old.md\nrename to new/path.md\n";
        assert_eq!(parse_path_from_block(block).as_deref(), Some("new/path.md"));
    }
}
