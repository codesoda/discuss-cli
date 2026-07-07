use std::process::Command;

use crate::error::{DiscussError, Result};

pub const DEFAULT_DIFF_SIZE_LIMIT_BYTES: usize = 5 * 1024 * 1024;

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

pub fn run_git_diff(unstaged: bool, extra: &[String], limit_bytes: usize) -> Result<DiffOutput> {
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

    if output.stdout.len() > limit_bytes {
        return Err(DiscussError::DiffError {
            message: format!(
                "diff is {} bytes, which exceeds the {} byte limit. Narrow the range (e.g. fewer commits, a path filter, or smaller -U context), or raise the cap via --max-diff-bytes / max_diff_bytes in discuss.config.toml / DISCUSS_MAX_DIFF_BYTES.",
                output.stdout.len(),
                limit_bytes
            ),
        });
    }

    let stdout = String::from_utf8(output.stdout).map_err(|source| DiscussError::DiffError {
        message: format!("git diff produced non-UTF-8 output: {source}"),
    })?;

    Ok(DiffOutput {
        git_args,
        files: split_into_files(&stdout),
    })
}

pub fn split_into_files(unified: &str) -> Vec<DiffFile> {
    let mut starts: Vec<usize> = Vec::new();
    let mut offset = 0;
    for line in unified.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            starts.push(offset);
        }
        offset += line.len();
    }

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

/// Converts raw unified-diff content into markdown that renders through the
/// normal markdown pipeline: a heading with the file path and +/- counts,
/// then one fenced `diff-<lang>` block per hunk so Prism's diff-highlight
/// plugin (plus the autoloaded language grammar) can highlight it. Each hunk
/// becomes its own commentable block anchor in the review UI.
///
/// `content` may contain one or more `diff --git` sections (e.g. a `.patch`
/// file passed on the command line); each section gets its own heading.
pub fn diff_content_to_markdown(fallback_path: &str, content: &str) -> String {
    let files = split_into_files(content);

    if files.is_empty() {
        // Not a git-style diff (e.g. plain unified diff without headers).
        // Render the whole thing as a single diff block.
        let mut markdown = format!("### `{fallback_path}`\n\n");
        push_fenced_diff(&mut markdown, language_for_path(fallback_path), content);
        return markdown;
    }

    let mut markdown = String::new();
    for file in &files {
        diff_file_section(&mut markdown, file);
    }
    markdown
}

fn diff_file_section(markdown: &mut String, file: &DiffFile) {
    let lang = language_for_path(&file.path);
    let (header_lines, hunks) = split_header_and_hunks(&file.content);

    let mut additions = 0usize;
    let mut deletions = 0usize;
    for hunk in &hunks {
        for line in hunk.lines().skip(1) {
            if line.starts_with('+') {
                additions += 1;
            } else if line.starts_with('-') {
                deletions += 1;
            }
        }
    }

    markdown.push_str(&format!("### `{}`\n\n", file.path));

    let mut notes: Vec<String> = Vec::new();
    if !hunks.is_empty() {
        notes.push(format!("+{additions} −{deletions}"));
    }
    for line in header_lines.lines() {
        if line.starts_with("new file mode") {
            notes.push("new file".to_string());
        } else if line.starts_with("deleted file mode") {
            notes.push("deleted file".to_string());
        } else if let Some(to) = line.strip_prefix("rename to ") {
            notes.push(format!("renamed to `{to}`"));
        } else if line.starts_with("old mode") || line.starts_with("new mode") {
            notes.push(line.to_string());
        } else if line.starts_with("Binary files") {
            notes.push("binary file changed".to_string());
        }
    }
    if !notes.is_empty() {
        markdown.push_str(&notes.join(" · "));
        markdown.push_str("\n\n");
    }

    if hunks.is_empty() {
        markdown.push_str("_No textual changes to review._\n\n");
        return;
    }

    for hunk in hunks {
        push_fenced_diff(markdown, lang, hunk);
    }
}

fn push_fenced_diff(markdown: &mut String, lang: Option<&'static str>, body: &str) {
    let info = match lang {
        Some(lang) => format!("diff-{lang}"),
        None => "diff".to_string(),
    };

    // A closing fence may be indented up to three spaces, and diff context
    // lines start with a single space — so a fence inside the diff body could
    // terminate our block early. Always use one more backtick than the
    // longest backtick run in the body (minimum four).
    let longest_run = body
        .lines()
        .map(|line| {
            let trimmed = line.trim_start_matches([' ', '+', '-']);
            trimmed.chars().take_while(|&c| c == '`').count()
        })
        .max()
        .unwrap_or(0);
    let fence = "`".repeat((longest_run + 1).max(4));

    markdown.push_str(&format!("{fence}{info}\n"));
    markdown.push_str(body);
    if !body.ends_with('\n') {
        markdown.push('\n');
    }
    markdown.push_str(&fence);
    markdown.push_str("\n\n");
}

/// Splits a single-file diff block into its header (everything before the
/// first `@@`) and one string per hunk (each starting at its `@@` line).
fn split_header_and_hunks(content: &str) -> (String, Vec<&str>) {
    let mut hunk_starts: Vec<usize> = Vec::new();
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        if line.starts_with("@@") {
            hunk_starts.push(offset);
        }
        offset += line.len();
    }

    let Some(&first) = hunk_starts.first() else {
        return (content.to_string(), Vec::new());
    };

    let header = content[..first].to_string();
    let hunks = hunk_starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = hunk_starts.get(i + 1).copied().unwrap_or(content.len());
            &content[start..end]
        })
        .collect();

    (header, hunks)
}

fn language_for_path(path: &str) -> Option<&'static str> {
    let extension = path.rsplit('.').next()?.to_ascii_lowercase();

    Some(match extension.as_str() {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "jsx",
        "py" => "python",
        "rb" => "ruby",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => "cpp",
        "cs" => "csharp",
        "php" => "php",
        "md" | "markdown" => "markdown",
        "json" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "sh" | "bash" | "zsh" => "bash",
        "html" | "htm" | "xml" | "svg" => "markup",
        "css" => "css",
        "scss" => "scss",
        "sql" => "sql",
        "lua" => "lua",
        "ex" | "exs" => "elixir",
        _ => return None,
    })
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
    fn split_into_files_ignores_diff_git_text_inside_hunk_lines() {
        let sample = "\
diff --git a/doc.md b/doc.md
index 1111..2222 100644
--- a/doc.md
+++ b/doc.md
@@ -1 +1 @@
-run `diff --git a/x b/x` to inspect
+diff --git looks like a header but is hunk content
";

        // "+diff --git ..." starts with '+', not at column 0, so only the
        // real header at column 0 counts.
        let files = split_into_files(sample);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "doc.md");
    }

    #[test]
    fn parse_path_from_block_handles_renames_with_b_prefix() {
        let block =
            "diff --git a/old.md b/new/path.md\nrename from old.md\nrename to new/path.md\n";

        assert_eq!(parse_path_from_block(block).as_deref(), Some("new/path.md"));
    }

    #[test]
    fn default_diff_size_limit_is_five_megabytes() {
        assert_eq!(DEFAULT_DIFF_SIZE_LIMIT_BYTES, 5 * 1024 * 1024);
    }

    #[test]
    fn diff_content_to_markdown_emits_heading_stats_and_fenced_hunks() {
        let files = split_into_files(SAMPLE);
        let markdown = diff_content_to_markdown("foo.rs", &files[0].content);

        assert!(markdown.contains("### `foo.rs`"));
        assert!(markdown.contains("+1 −1"));
        assert!(markdown.contains("````diff-rust\n@@ -1 +1 @@\n-old\n+new\n````"));
    }

    #[test]
    fn diff_content_to_markdown_renders_each_hunk_as_its_own_fence() {
        let sample = "\
diff --git a/multi.py b/multi.py
index 1111..2222 100644
--- a/multi.py
+++ b/multi.py
@@ -1,2 +1,2 @@
-first = 1
+first = 2
 keep
@@ -10,2 +10,2 @@
 keep
-second = 1
+second = 2
";

        let markdown = diff_content_to_markdown("multi.py", sample);

        assert_eq!(markdown.matches("````diff-python").count(), 2);
        assert!(markdown.contains("+2 −2"));
    }

    #[test]
    fn diff_content_to_markdown_flags_new_deleted_and_renamed_files() {
        let files = split_into_files(SAMPLE);
        let markdown = diff_content_to_markdown("bar.md", &files[1].content);
        assert!(markdown.contains("new file"));

        let rename = "\
diff --git a/old.md b/new.md
similarity index 100%
rename from old.md
rename to new.md
";
        let markdown = diff_content_to_markdown("new.md", rename);
        assert!(markdown.contains("renamed to `new.md`"));
        assert!(markdown.contains("_No textual changes to review._"));

        let binary = "\
diff --git a/img.png b/img.png
index 1111..2222 100644
Binary files a/img.png and b/img.png differ
";
        let markdown = diff_content_to_markdown("img.png", binary);
        assert!(markdown.contains("binary file changed"));
    }

    #[test]
    fn diff_content_to_markdown_falls_back_to_plain_diff_fence_for_unknown_extensions() {
        let sample = "\
diff --git a/Makefile b/Makefile
index 1111..2222 100644
--- a/Makefile
+++ b/Makefile
@@ -1 +1 @@
-all:
+all: build
";

        let markdown = diff_content_to_markdown("Makefile", sample);
        assert!(markdown.contains("````diff\n@@"));
    }

    #[test]
    fn diff_content_to_markdown_wraps_non_git_content_in_single_fence() {
        let markdown = diff_content_to_markdown("change.patch", "@@ -1 +1 @@\n-a\n+b\n");

        assert!(markdown.contains("### `change.patch`"));
        assert!(markdown.contains("````diff\n@@ -1 +1 @@\n-a\n+b\n````"));
    }

    #[test]
    fn fences_grow_past_backtick_runs_inside_diff_content() {
        let sample = "\
diff --git a/doc.md b/doc.md
index 1111..2222 100644
--- a/doc.md
+++ b/doc.md
@@ -1,3 +1,3 @@
 ````diff
-old fence content
+new fence content
 ````
";

        let markdown = diff_content_to_markdown("doc.md", sample);

        // Body contains a 4-backtick run, so our fence must use 5.
        assert!(markdown.contains("`````diff-markdown\n"));
        assert!(markdown.contains("\n`````\n"));
    }
}
