use serde::{Deserialize, Serialize};

use crate::{DiscussError, Result};

/// The GitHub side used by an inline review anchor.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiffSide {
    /// The pre-change side of the diff.
    Left,
    /// The post-change side of the diff.
    Right,
}

/// The change classification detected from a per-file diff.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffStatus {
    /// An existing file changed in place.
    Modified,
    /// A new file was added.
    Added,
    /// An existing file was deleted.
    Deleted,
    /// A file was renamed.
    Renamed,
}

/// A GitHub line associated with one rendered diff row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiffLine {
    /// The old or new side of the diff.
    pub side: DiffSide,
    /// The one-based line number on that side.
    pub line: u32,
}

/// One row in a rendered hunk fence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffRow {
    /// The one-based row in the hunk fence; row one is the hunk header.
    pub row: usize,
    /// The GitHub line, absent for headers and no-newline markers.
    pub target: Option<DiffLine>,
}

/// A parsed unified-diff hunk and its render-row map.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffHunk {
    /// The zero-based hunk index in the file.
    pub index: usize,
    /// The first old-side line from the hunk header.
    pub old_start: u32,
    /// The old-side line count from the hunk header.
    pub old_count: u32,
    /// The first new-side line from the hunk header.
    pub new_start: u32,
    /// The new-side line count from the hunk header.
    pub new_count: u32,
    /// Rows in the rendered hunk fence.
    pub rows: Vec<DiffRow>,
}

/// A resolved row suitable for a GitHub inline review comment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MappedDiffRow {
    /// The old or new side of the diff.
    pub side: DiffSide,
    /// The one-based line number on that side.
    pub line: u32,
    /// The zero-based containing hunk.
    pub hunk_index: usize,
    /// The one-based resolved rendered row.
    pub row: usize,
    /// Whether the requested row had to move within its hunk.
    pub approximate: bool,
}

/// Parsed paths, status, binary metadata, and hunk row mappings for one file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffMap {
    /// Path from the `a/` side of the git header.
    pub old_path: String,
    /// Path from the `b/` side of the git header.
    pub new_path: String,
    /// Detected file status.
    pub status: DiffStatus,
    /// Whether the block represents a binary change.
    pub binary: bool,
    /// Parsed hunks in source order.
    pub hunks: Vec<DiffHunk>,
}

impl DiffMap {
    /// Parses exactly one git-style per-file unified diff block.
    pub fn parse(diff: &str) -> Result<Self> {
        let headers: Vec<_> = diff
            .lines()
            .filter(|line| line.starts_with("diff --git "))
            .collect();
        if headers.len() != 1 || !diff.starts_with("diff --git ") {
            return Err(diff_error(format!(
                "expected exactly one leading `diff --git` block, found {}",
                headers.len()
            )));
        }

        let (mut old_path, mut new_path) = parse_diff_header(headers[0])?;
        let mut status = DiffStatus::Modified;
        let mut binary = false;
        for line in diff.lines().take_while(|line| !line.starts_with("@@ ")) {
            if line.starts_with("new file mode ") {
                status = DiffStatus::Added;
            } else if line.starts_with("deleted file mode ") {
                status = DiffStatus::Deleted;
            } else if let Some(path) = line.strip_prefix("rename from ") {
                old_path = parse_header_path(path)?;
                status = DiffStatus::Renamed;
            } else if let Some(path) = line.strip_prefix("rename to ") {
                new_path = parse_header_path(path)?;
                status = DiffStatus::Renamed;
            } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
                binary = true;
            }
        }

        let mut hunks = Vec::new();
        let mut current: Option<DiffHunk> = None;
        let mut old_line = 0;
        let mut new_line = 0;

        for line in diff.lines() {
            if line.starts_with("@@ ") {
                if let Some(hunk) = current.take() {
                    validate_hunk_extent(&hunk, old_line, new_line)?;
                    hunks.push(hunk);
                }
                let (old_start, old_count, new_start, new_count) = parse_hunk_header(line)?;
                if let Some(previous) = hunks.last() {
                    let previous_old_end = previous
                        .old_start
                        .checked_add(previous.old_count)
                        .ok_or_else(|| diff_error("old-side hunk range overflow"))?;
                    let previous_new_end = previous
                        .new_start
                        .checked_add(previous.new_count)
                        .ok_or_else(|| diff_error("new-side hunk range overflow"))?;
                    if old_start < previous_old_end || new_start < previous_new_end {
                        return Err(diff_error("hunks overlap or are out of source order"));
                    }
                }
                old_line = old_start;
                new_line = new_start;
                current = Some(DiffHunk {
                    index: hunks.len(),
                    old_start,
                    old_count,
                    new_start,
                    new_count,
                    rows: vec![DiffRow {
                        row: 1,
                        target: None,
                    }],
                });
                continue;
            }

            let Some(hunk) = current.as_mut() else {
                continue;
            };
            let target = if line.starts_with("\\ No newline at end of file") {
                None
            } else if line.starts_with('+') {
                let target = DiffLine {
                    side: DiffSide::Right,
                    line: new_line,
                };
                new_line = new_line
                    .checked_add(1)
                    .ok_or_else(|| diff_error("new-side hunk line number overflow"))?;
                Some(target)
            } else if line.starts_with('-') {
                let target = DiffLine {
                    side: DiffSide::Left,
                    line: old_line,
                };
                old_line = old_line
                    .checked_add(1)
                    .ok_or_else(|| diff_error("old-side hunk line number overflow"))?;
                Some(target)
            } else if line.starts_with(' ') {
                let target = DiffLine {
                    side: DiffSide::Right,
                    line: new_line,
                };
                old_line = old_line
                    .checked_add(1)
                    .ok_or_else(|| diff_error("old-side hunk line number overflow"))?;
                new_line = new_line
                    .checked_add(1)
                    .ok_or_else(|| diff_error("new-side hunk line number overflow"))?;
                Some(target)
            } else {
                return Err(diff_error(format!(
                    "unrecognized line in hunk {}: {line:?}",
                    hunk.index
                )));
            };
            hunk.rows.push(DiffRow {
                row: hunk.rows.len() + 1,
                target,
            });
        }
        if let Some(hunk) = current {
            validate_hunk_extent(&hunk, old_line, new_line)?;
            hunks.push(hunk);
        }

        Ok(Self {
            old_path,
            new_path,
            status,
            binary,
            hunks,
        })
    }

    /// Resolves an exact rendered row, without approximation.
    pub fn map_row(&self, hunk_index: usize, row: usize) -> Option<MappedDiffRow> {
        let hunk = self.hunks.get(hunk_index)?;
        let target = hunk
            .rows
            .iter()
            .find(|candidate| candidate.row == row)?
            .target?;
        Some(MappedDiffRow {
            side: target.side,
            line: target.line,
            hunk_index,
            row,
            approximate: false,
        })
    }

    /// Resolves a row or the nearest valid row within the same hunk.
    pub fn nearest_valid_row(&self, hunk_index: usize, row: usize) -> Option<MappedDiffRow> {
        if let Some(exact) = self.map_row(hunk_index, row) {
            return Some(exact);
        }
        let hunk = self.hunks.get(hunk_index)?;
        let nearest = hunk
            .rows
            .iter()
            .filter_map(|candidate| candidate.target.map(|target| (candidate.row, target)))
            .min_by_key(|(candidate_row, _)| (candidate_row.abs_diff(row), *candidate_row))?;
        Some(MappedDiffRow {
            side: nearest.1.side,
            line: nearest.1.line,
            hunk_index,
            row: nearest.0,
            approximate: true,
        })
    }
}

fn validate_hunk_extent(hunk: &DiffHunk, old_end: u32, new_end: u32) -> Result<()> {
    let expected_old_end = hunk
        .old_start
        .checked_add(hunk.old_count)
        .ok_or_else(|| diff_error("old-side hunk range overflow"))?;
    let expected_new_end = hunk
        .new_start
        .checked_add(hunk.new_count)
        .ok_or_else(|| diff_error("new-side hunk range overflow"))?;
    if old_end != expected_old_end || new_end != expected_new_end {
        return Err(diff_error(format!(
            "hunk {} body ends at old/new {old_end}/{new_end}, expected {expected_old_end}/{expected_new_end}",
            hunk.index
        )));
    }
    Ok(())
}

fn parse_diff_header(header: &str) -> Result<(String, String)> {
    let rest = header
        .strip_prefix("diff --git ")
        .ok_or_else(|| diff_error("missing `diff --git` header"))?;
    let tokens = parse_git_tokens(rest)?;
    if tokens.len() != 2 {
        return Err(diff_error(format!(
            "expected two paths in `diff --git` header, found {}",
            tokens.len()
        )));
    }
    let old_path = tokens[0]
        .strip_prefix("a/")
        .ok_or_else(|| diff_error("old path in `diff --git` header must start with `a/`"))?;
    let new_path = tokens[1]
        .strip_prefix("b/")
        .ok_or_else(|| diff_error("new path in `diff --git` header must start with `b/`"))?;
    if old_path.is_empty() || new_path.is_empty() {
        return Err(diff_error("diff paths must not be empty"));
    }
    Ok((old_path.to_string(), new_path.to_string()))
}

fn parse_header_path(path: &str) -> Result<String> {
    let tokens = parse_git_tokens(path)?;
    if tokens.len() != 1 || tokens[0].is_empty() {
        return Err(diff_error(
            "rename path must contain exactly one nonempty path",
        ));
    }
    Ok(tokens.into_iter().next().expect("one token was checked"))
}

fn parse_git_tokens(input: &str) -> Result<Vec<String>> {
    let bytes = input.as_bytes();
    let mut index = 0;
    let mut tokens = Vec::new();
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index == bytes.len() {
            break;
        }
        let quoted = bytes[index] == b'"';
        if quoted {
            index += 1;
        }
        let mut closed = !quoted;
        let mut token = Vec::new();
        while index < bytes.len() {
            let byte = bytes[index];
            if quoted && byte == b'"' {
                index += 1;
                closed = true;
                break;
            }
            if !quoted && byte.is_ascii_whitespace() {
                break;
            }
            if byte == b'\\' {
                index += 1;
                if index == bytes.len() {
                    return Err(diff_error("unterminated escape in git path"));
                }
                let escaped = bytes[index];
                if escaped.is_ascii_digit() && escaped < b'8' {
                    let mut value = 0u16;
                    let mut digits = 0;
                    while index < bytes.len() && digits < 3 {
                        let digit = bytes[index];
                        if !(b'0'..=b'7').contains(&digit) {
                            break;
                        }
                        value = value * 8 + u16::from(digit - b'0');
                        index += 1;
                        digits += 1;
                    }
                    token
                        .push(u8::try_from(value).map_err(|_| {
                            diff_error("octal escape in git path exceeds one byte")
                        })?);
                    continue;
                }
                token.push(match escaped {
                    b'a' => 0x07,
                    b'b' => 0x08,
                    b't' => b'\t',
                    b'n' => b'\n',
                    b'v' => 0x0b,
                    b'f' => 0x0c,
                    b'r' => b'\r',
                    other => other,
                });
                index += 1;
                continue;
            }
            token.push(byte);
            index += 1;
        }
        if !closed {
            return Err(diff_error("unterminated quoted git path"));
        }
        if quoted && index < bytes.len() && !bytes[index].is_ascii_whitespace() {
            return Err(diff_error("unexpected material after quoted git path"));
        }
        let token = String::from_utf8(token)
            .map_err(|_| diff_error("git path is not valid UTF-8 after unescaping"))?;
        tokens.push(token);
    }
    Ok(tokens)
}

fn parse_hunk_header(header: &str) -> Result<(u32, u32, u32, u32)> {
    let rest = header
        .strip_prefix("@@ ")
        .ok_or_else(|| diff_error(format!("invalid hunk header {header:?}")))?;
    let mut fields = rest.split_whitespace();
    let old = fields
        .next()
        .ok_or_else(|| diff_error(format!("missing old range in hunk header {header:?}")))?;
    let new = fields
        .next()
        .ok_or_else(|| diff_error(format!("missing new range in hunk header {header:?}")))?;
    let marker = fields
        .next()
        .ok_or_else(|| diff_error(format!("missing closing marker in hunk header {header:?}")))?;
    if marker != "@@" {
        return Err(diff_error(format!(
            "invalid closing marker in hunk header {header:?}"
        )));
    }
    let (old_start, old_count) = parse_range(old, '-', header)?;
    let (new_start, new_count) = parse_range(new, '+', header)?;
    Ok((old_start, old_count, new_start, new_count))
}

fn parse_range(range: &str, prefix: char, header: &str) -> Result<(u32, u32)> {
    let range = range
        .strip_prefix(prefix)
        .ok_or_else(|| diff_error(format!("invalid range in hunk header {header:?}")))?;
    let (start, count) = range.split_once(',').unwrap_or((range, "1"));
    let start = start
        .parse()
        .map_err(|_| diff_error(format!("invalid line number in hunk header {header:?}")))?;
    let count = count
        .parse()
        .map_err(|_| diff_error(format!("invalid line count in hunk header {header:?}")))?;
    Ok((start, count))
}

fn diff_error(message: impl Into<String>) -> DiscussError {
    DiscussError::ConfigError {
        message: format!("invalid imported per-file diff: {}", message.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_modified_file_rows_on_left_and_right() {
        let map = DiffMap::parse(
            "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -10,2 +10,2 @@ fn f() {\n same\n-old\n+new\n",
        )
        .unwrap();
        assert_eq!(map.status, DiffStatus::Modified);
        assert_eq!(map.old_path, "src/lib.rs");
        assert_eq!(map.new_path, "src/lib.rs");
        assert_eq!(map.map_row(0, 1), None);
        assert_eq!(map.map_row(0, 2).unwrap().side, DiffSide::Right);
        assert_eq!(map.map_row(0, 2).unwrap().line, 10);
        assert_eq!(map.map_row(0, 3).unwrap().side, DiffSide::Left);
        assert_eq!(map.map_row(0, 3).unwrap().line, 11);
        assert_eq!(map.map_row(0, 4).unwrap().side, DiffSide::Right);
        assert_eq!(map.map_row(0, 4).unwrap().line, 11);
    }

    #[test]
    fn detects_added_deleted_renamed_and_binary_files() {
        let added = DiffMap::parse(
            "diff --git a/new.rs b/new.rs\nnew file mode 100644\n--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1 @@\n+new\n",
        )
        .unwrap();
        assert_eq!(added.status, DiffStatus::Added);
        assert_eq!(added.map_row(0, 2).unwrap().side, DiffSide::Right);

        let deleted = DiffMap::parse(
            "diff --git a/old.rs b/old.rs\ndeleted file mode 100644\n--- a/old.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n",
        )
        .unwrap();
        assert_eq!(deleted.status, DiffStatus::Deleted);
        assert_eq!(deleted.map_row(0, 2).unwrap().side, DiffSide::Left);

        let renamed = DiffMap::parse(
            "diff --git \"a/old name.rs\" \"b/new name.rs\"\nsimilarity index 90%\nrename from \"old name.rs\"\nrename to \"new name.rs\"\n@@ -1 +1 @@\n-old\n+new\n",
        )
        .unwrap();
        assert_eq!(renamed.status, DiffStatus::Renamed);
        assert_eq!(renamed.old_path, "old name.rs");
        assert_eq!(renamed.new_path, "new name.rs");

        let binary = DiffMap::parse(
            "diff --git a/image.png b/image.png\nindex 123..456 100644\nBinary files a/image.png and b/image.png differ\n",
        )
        .unwrap();
        assert!(binary.binary);
        assert!(binary.hunks.is_empty());
    }

    #[test]
    fn parses_git_quoted_escapes() {
        let map = DiffMap::parse(
            "diff --git \"a/a\\ b\\\"c\\\\d.rs\" \"b/a\\ b\\\"c\\\\d.rs\"\n@@ -1 +1 @@\n-old\n+new\n",
        )
        .unwrap();
        assert_eq!(map.old_path, "a b\"c\\d.rs");
        assert_eq!(map.new_path, "a b\"c\\d.rs");
    }

    #[test]
    fn hunk_header_and_no_newline_marker_are_unanchorable() {
        let map = DiffMap::parse(
            "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n",
        )
        .unwrap();
        assert_eq!(map.map_row(0, 1), None);
        assert_eq!(map.map_row(0, 3), None);
        assert_eq!(map.map_row(0, 5), None);
    }

    #[test]
    fn rejects_truncated_or_overlapping_hunks() {
        let truncated = "diff --git a/a b/a\n@@ -1,2 +1,2 @@\n-old\n+new\n";
        assert!(DiffMap::parse(truncated).is_err());

        let overlapping =
            "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n@@ -1 +1 @@\n-old again\n+new again\n";
        assert!(DiffMap::parse(overlapping).is_err());
    }

    #[test]
    fn approximation_stays_within_the_requested_hunk() {
        let map = DiffMap::parse(
            "diff --git a/a b/a\n@@ -1 +1 @@\n-one\n+one changed\n@@ -20 +20 @@\n-twenty\n+twenty changed\n",
        )
        .unwrap();
        let first = map.nearest_valid_row(0, 1).unwrap();
        assert_eq!((first.hunk_index, first.row, first.line), (0, 2, 1));
        assert!(first.approximate);
        let second = map.nearest_valid_row(1, 1).unwrap();
        assert_eq!((second.hunk_index, second.row, second.line), (1, 2, 20));
        assert!(map.nearest_valid_row(2, 1).is_none());
        assert!(!map.nearest_valid_row(0, 2).unwrap().approximate);
    }
}
