//! Cross-session UI preferences.
//!
//! Every session binds a fresh ephemeral port, so the browser sees a new
//! origin each run and `localStorage` starts empty. Preferences the reviewer
//! sets in the UI therefore live here instead: one small JSON file in the
//! user's `~/.discuss` directory, read when the page is rendered and written
//! back through `POST /api/prefs`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};

/// The per-user directory under the home directory that holds discuss data.
pub const DISCUSS_DIR_NAME: &str = ".discuss";
const PREFS_FILE_NAME: &str = "prefs.json";

/// Serializes every load → merge → save in this process, so two concurrent
/// patches can't both read the old file and have the later save erase the
/// other change.
static SAVE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    System,
}

/// Both the stored file and a `POST /api/prefs` patch. An unset field means
/// "keep the first-run default" in the file and "leave unchanged" in a patch.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiPrefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd_enter_to_send: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_collapsed: Option<bool>,
}

impl UiPrefs {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Applies every field `patch` sets and keeps the rest.
    pub fn merged_with(self, patch: &UiPrefs) -> UiPrefs {
        UiPrefs {
            theme: patch.theme.or(self.theme),
            cmd_enter_to_send: patch.cmd_enter_to_send.or(self.cmd_enter_to_send),
            files_collapsed: patch.files_collapsed.or(self.files_collapsed),
        }
    }
}

/// Resolved on each call rather than cached, so a test that points `HOME`
/// somewhere else sees its own directory.
pub fn default_prefs_path() -> PathBuf {
    BaseDirs::new()
        .map(|base_dirs| base_dirs.home_dir().join(DISCUSS_DIR_NAME))
        .unwrap_or_else(|| PathBuf::from(DISCUSS_DIR_NAME))
        .join(PREFS_FILE_NAME)
}

/// Reads the stored preferences, falling back to defaults. A missing,
/// unreadable, or corrupt file is not an error: a review must still open.
pub fn load(path: &Path) -> UiPrefs {
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

/// Applies `patch` over the stored preferences, saves, and returns the merged set.
pub fn merge_and_save(path: &Path, patch: &UiPrefs) -> io::Result<UiPrefs> {
    let _guard = SAVE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let merged = load(path).merged_with(patch);
    write_atomically(path, &merged)?;
    Ok(merged)
}

/// Write to a sibling temp file and rename, so a reader never sees a
/// half-written prefs file.
fn write_atomically(path: &Path, prefs: &UiPrefs) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let temp_path = path.with_extension(format!("tmp-{}", std::process::id()));
    let bytes = serde_json::to_vec_pretty(prefs).map_err(io::Error::other)?;
    fs::write(&temp_path, bytes)?;

    fs::rename(&temp_path, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp_path);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    fn prefs(value: serde_json::Value) -> UiPrefs {
        serde_json::from_value(value).expect("valid prefs fixture")
    }

    #[test]
    fn merge_applies_only_the_fields_the_patch_sets() {
        let stored = prefs(json!({"theme": "dark", "cmdEnterToSend": true}));

        let merged = stored.merged_with(&prefs(json!({"cmdEnterToSend": false})));

        assert_eq!(merged.theme, Some(Theme::Dark));
        assert_eq!(merged.cmd_enter_to_send, Some(false));
        assert_eq!(merged.files_collapsed, None);
    }

    #[test]
    fn accepts_every_known_preference() {
        let parsed = prefs(json!({
            "theme": "system",
            "cmdEnterToSend": false,
            "filesCollapsed": true
        }));

        assert_eq!(parsed.theme, Some(Theme::System));
        assert_eq!(parsed.cmd_enter_to_send, Some(false));
        assert_eq!(parsed.files_collapsed, Some(true));
    }

    #[test]
    fn rejects_unknown_keys_and_bad_values() {
        for bad in [
            json!({"nope": 1}),
            json!({"theme": "neon"}),
            json!({"cmdEnterToSend": "true"}),
        ] {
            assert!(
                serde_json::from_value::<UiPrefs>(bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn serializes_only_the_fields_that_are_set() {
        let json = serde_json::to_string(&prefs(json!({"theme": "light"}))).expect("serialize");

        assert_eq!(json, r#"{"theme":"light"}"#);
    }

    #[test]
    fn missing_file_loads_as_defaults() {
        let dir = tempfile::tempdir().expect("temp dir");

        assert!(load(&dir.path().join("prefs.json")).is_empty());
    }

    #[test]
    fn corrupt_file_loads_as_defaults_instead_of_failing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("prefs.json");
        fs::write(&path, "{not json").expect("write corrupt prefs");

        assert!(load(&path).is_empty());
    }

    #[test]
    fn default_path_is_in_the_discuss_directory() {
        let path = default_prefs_path();

        assert!(path.ends_with(Path::new(DISCUSS_DIR_NAME).join(PREFS_FILE_NAME)));
    }

    #[test]
    fn saved_prefs_round_trip_through_the_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("nested").join("prefs.json");

        merge_and_save(&path, &prefs(json!({"theme": "dark"}))).expect("first save");
        let merged =
            merge_and_save(&path, &prefs(json!({"cmdEnterToSend": false}))).expect("second save");

        assert_eq!(merged.theme, Some(Theme::Dark));
        assert_eq!(load(&path).cmd_enter_to_send, Some(false));
        assert!(
            fs::read_dir(path.parent().expect("parent"))
                .expect("read dir")
                .filter_map(std::result::Result::ok)
                .all(|entry| entry.file_name() == "prefs.json"),
            "atomic write must not leave a temp file behind",
        );
    }

    #[test]
    fn concurrent_saves_keep_every_change() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("prefs.json");

        std::thread::scope(|scope| {
            scope.spawn(|| merge_and_save(&path, &prefs(json!({"theme": "dark"}))));
            scope.spawn(|| merge_and_save(&path, &prefs(json!({"cmdEnterToSend": false}))));
            scope.spawn(|| merge_and_save(&path, &prefs(json!({"filesCollapsed": true}))));
        });

        assert_eq!(
            load(&path),
            prefs(json!({"theme": "dark", "cmdEnterToSend": false, "filesCollapsed": true}))
        );
    }
}
