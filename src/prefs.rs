//! Cross-session UI preferences.
//!
//! Every session binds a fresh ephemeral port, so the browser sees a new
//! origin each run and `localStorage` starts empty. Preferences the reviewer
//! sets in the UI therefore live here instead: one small JSON file in the
//! user's `~/.discuss` directory, read when the page is rendered and written
//! back through `POST /api/prefs`.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use serde_json::{Map, Value};

/// Overrides the prefs file location. Tests set this so they never read or
/// write the developer's real preferences.
pub const PREFS_PATH_ENV: &str = "DISCUSS_PREFS_PATH";

const THEME_KEY: &str = "theme";
const CMD_ENTER_KEY: &str = "cmdEnterToSend";
const FILES_COLLAPSED_KEY: &str = "filesCollapsed";
const THEME_MODES: [&str; 3] = ["light", "dark", "system"];

/// Stored as a JSON object rather than a struct so a file written by a newer
/// build keeps its extra keys when an older build merges a patch into it.
pub type UiPrefs = Map<String, Value>;

pub fn prefs_path() -> PathBuf {
    if let Some(path) = env::var_os(PREFS_PATH_ENV) {
        return PathBuf::from(path);
    }

    BaseDirs::new()
        .map(|base_dirs| base_dirs.home_dir().join(".discuss").join("prefs.json"))
        .unwrap_or_else(|| PathBuf::from(".discuss").join("prefs.json"))
}

/// Reads the stored preferences, falling back to an empty set. A missing,
/// unreadable, or corrupt file is not an error: a review must still open.
pub fn load() -> UiPrefs {
    load_from(&prefs_path())
}

pub fn load_from(path: &Path) -> UiPrefs {
    let Ok(contents) = fs::read_to_string(path) else {
        return UiPrefs::new();
    };

    match serde_json::from_str::<Value>(&contents) {
        Ok(Value::Object(prefs)) => prefs,
        _ => UiPrefs::new(),
    }
}

/// Applies `patch` over the stored preferences and returns the merged set.
///
/// Read-modify-write so a concurrent session toggling a different setting
/// keeps its change. `null` in the patch clears a key.
pub fn merge_and_save(patch: &UiPrefs) -> io::Result<UiPrefs> {
    merge_and_save_at(&prefs_path(), patch)
}

pub fn merge_and_save_at(path: &Path, patch: &UiPrefs) -> io::Result<UiPrefs> {
    let merged = merge(load_from(path), patch);
    write_atomically(path, &merged)?;
    Ok(merged)
}

/// Merge without touching disk, for demo sessions that must stay out of the
/// user's home directory while still answering with a coherent value.
pub fn merge(mut prefs: UiPrefs, patch: &UiPrefs) -> UiPrefs {
    for (key, value) in patch {
        if value.is_null() {
            prefs.remove(key);
        } else {
            prefs.insert(key.clone(), value.clone());
        }
    }

    prefs
}

/// Rejects anything the UI would not understand, so a stray client can't turn
/// the prefs file into a junk drawer.
pub fn validate_patch(patch: &UiPrefs) -> Result<(), String> {
    if patch.is_empty() {
        return Err("prefs patch must set at least one key".to_string());
    }

    for (key, value) in patch {
        if value.is_null() {
            continue;
        }

        let valid = match key.as_str() {
            THEME_KEY => value
                .as_str()
                .is_some_and(|mode| THEME_MODES.contains(&mode)),
            CMD_ENTER_KEY | FILES_COLLAPSED_KEY => value.is_boolean(),
            _ => return Err(format!("unknown preference: {key}")),
        };

        if !valid {
            return Err(format!("invalid value for preference: {key}"));
        }
    }

    Ok(())
}

/// Write to a sibling temp file and rename, so an interrupted write can never
/// leave a half-written prefs file behind.
fn write_atomically(path: &Path, prefs: &UiPrefs) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let temp_path = path.with_extension(format!("tmp-{}", std::process::id()));
    let bytes = serde_json::to_vec_pretty(prefs).map_err(io::Error::other)?;
    fs::write(&temp_path, bytes)?;

    match fs::rename(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    fn patch(value: Value) -> UiPrefs {
        match value {
            Value::Object(map) => map,
            _ => panic!("patch fixture must be an object"),
        }
    }

    #[test]
    fn merge_applies_only_the_keys_the_patch_names() {
        let stored = patch(json!({"theme": "dark", "cmdEnterToSend": true}));

        let merged = merge(stored, &patch(json!({"cmdEnterToSend": false})));

        assert_eq!(merged["theme"], json!("dark"));
        assert_eq!(merged["cmdEnterToSend"], json!(false));
    }

    #[test]
    fn merge_keeps_keys_written_by_a_newer_build() {
        let stored = patch(json!({"somethingNewer": {"nested": 1}}));

        let merged = merge(stored, &patch(json!({"theme": "light"})));

        assert_eq!(merged["somethingNewer"], json!({"nested": 1}));
        assert_eq!(merged["theme"], json!("light"));
    }

    #[test]
    fn merge_treats_null_as_clearing_the_key() {
        let stored = patch(json!({"theme": "dark"}));

        let merged = merge(stored, &patch(json!({"theme": null})));

        assert!(!merged.contains_key("theme"));
    }

    #[test]
    fn validate_accepts_every_known_preference() {
        assert!(
            validate_patch(&patch(json!({
                "theme": "system",
                "cmdEnterToSend": false,
                "filesCollapsed": true
            })))
            .is_ok()
        );
    }

    #[test]
    fn missing_file_loads_as_empty_prefs() {
        let dir = tempfile::tempdir().expect("temp dir");

        assert!(load_from(&dir.path().join("prefs.json")).is_empty());
    }

    #[test]
    fn corrupt_file_loads_as_empty_prefs_instead_of_failing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("prefs.json");
        fs::write(&path, "{not json").expect("write corrupt prefs");

        assert!(load_from(&path).is_empty());
    }

    #[test]
    fn saved_prefs_round_trip_through_the_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("nested").join("prefs.json");

        merge_and_save_at(&path, &patch(json!({"theme": "dark"}))).expect("first save");
        let merged = merge_and_save_at(&path, &patch(json!({"cmdEnterToSend": false})))
            .expect("second save");

        assert_eq!(merged["theme"], json!("dark"));
        assert_eq!(load_from(&path)["cmdEnterToSend"], json!(false));
        assert!(
            fs::read_dir(path.parent().expect("parent"))
                .expect("read dir")
                .filter_map(std::result::Result::ok)
                .all(|entry| entry.file_name() == "prefs.json"),
            "atomic write must not leave a temp file behind",
        );
    }

    #[test]
    fn validate_rejects_unknown_keys_and_bad_values() {
        assert!(validate_patch(&patch(json!({"nope": 1}))).is_err());
        assert!(validate_patch(&patch(json!({"theme": "neon"}))).is_err());
        assert!(validate_patch(&patch(json!({"cmdEnterToSend": "true"}))).is_err());
        assert!(validate_patch(&UiPrefs::new()).is_err());
    }
}
