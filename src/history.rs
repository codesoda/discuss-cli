use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use directories::BaseDirs;
use serde_json::Value;

pub fn default_history_dir() -> PathBuf {
    BaseDirs::new()
        .map(|base_dirs| base_dirs.home_dir().join(".discuss").join("history"))
        .unwrap_or_else(|| PathBuf::from(".discuss").join("history"))
}

pub fn history_archive_path(
    history_dir: &Path,
    source_path: Option<&Path>,
    files_count: usize,
    completed_at: DateTime<Utc>,
) -> PathBuf {
    let timestamp = completed_at.to_rfc3339_opts(SecondsFormat::Secs, true);

    history_dir
        .join(source_name_for_history(source_path, files_count))
        .join(format!("{timestamp}.json"))
}

pub fn write_history_archive(path: &Path, transcript_json: &Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let bytes = serde_json::to_vec(transcript_json).map_err(io::Error::other)?;
    fs::write(path, bytes)
}

fn source_name_for_history(source_path: Option<&Path>, files_count: usize) -> String {
    if files_count > 1 {
        return format!("multi-{files_count}-files");
    }

    let Some(source_path) = source_path else {
        return "unnamed".to_string();
    };
    let Some(stem) = source_path.file_stem().and_then(|stem| stem.to_str()) else {
        return "unnamed".to_string();
    };

    let sanitized = stem
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();

    if sanitized.is_empty() {
        "unnamed".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::TimeZone;
    use serde_json::json;

    fn timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 23, 2, 30, 0)
            .single()
            .expect("valid timestamp")
    }

    #[test]
    fn archive_path_uses_sanitized_source_stem_and_iso_timestamp() {
        let path = history_archive_path(
            Path::new("/tmp/history"),
            Some(Path::new("docs/review:plan.md")),
            1,
            timestamp(),
        );

        assert_eq!(
            path,
            PathBuf::from("/tmp/history")
                .join("review_plan")
                .join("2026-04-23T02:30:00Z.json")
        );
    }

    #[test]
    fn archive_path_falls_back_to_unnamed_without_source_path() {
        let path = history_archive_path(Path::new("/tmp/history"), None, 1, timestamp());

        assert_eq!(
            path,
            PathBuf::from("/tmp/history")
                .join("unnamed")
                .join("2026-04-23T02:30:00Z.json")
        );
    }

    #[test]
    fn archive_path_uses_multi_label_when_files_count_above_one() {
        let path = history_archive_path(
            Path::new("/tmp/history"),
            Some(Path::new("docs/first.md")),
            3,
            timestamp(),
        );

        assert_eq!(
            path,
            PathBuf::from("/tmp/history")
                .join("multi-3-files")
                .join("2026-04-23T02:30:00Z.json")
        );
    }

    #[test]
    fn write_history_archive_creates_parent_directory_and_writes_payload_json() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir
            .path()
            .join("review")
            .join("2026-04-23T02:30:00Z.json");
        let payload = json!({ "threads": [{ "id": "u-1" }] });

        write_history_archive(&path, &payload).expect("write archive");

        let archived =
            fs::read_to_string(path).expect("archive file should be written as utf-8 JSON");
        assert_eq!(
            serde_json::from_str::<Value>(&archived).expect("archive JSON"),
            payload
        );
    }
}
