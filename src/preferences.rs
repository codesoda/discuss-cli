//! Browser UI preferences that outlive a single review session.
//!
//! The browser used to keep these in `localStorage`, which is keyed by origin.
//! A session without an explicit `--port` binds `127.0.0.1:0`, so the OS hands
//! out a different port every launch, every launch is a different origin, and
//! every preference read comes back empty. Storing them next to the history
//! archives instead makes one answer that survives launches and reinstalls.

use std::fs;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};

/// Preferences the browser owns. Every field is optional so a file written by
/// an older or newer build still loads, and an absent field means "the page
/// decides".
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    /// `light`, `dark`, or `system`. Any other value is ignored by the page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Whether the multi-file sidebar is collapsed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_collapsed: Option<bool>,
    /// Whether the composer needs Cmd+Enter rather than Enter to send.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmd_enter_to_send: Option<bool>,
}

impl Preferences {
    /// Overwrite only the fields the caller supplied, so a page that knows
    /// about one preference cannot erase another.
    pub fn merge(&mut self, update: Preferences) {
        if update.theme.is_some() {
            self.theme = update.theme;
        }
        if update.files_collapsed.is_some() {
            self.files_collapsed = update.files_collapsed;
        }
        if update.cmd_enter_to_send.is_some() {
            self.cmd_enter_to_send = update.cmd_enter_to_send;
        }
    }
}

/// `~/.discuss/preferences.json`, beside the history archives.
pub fn default_preferences_path() -> PathBuf {
    BaseDirs::new()
        .map(|base_dirs| base_dirs.home_dir().join(".discuss"))
        .unwrap_or_else(|| PathBuf::from(".discuss"))
        .join("preferences.json")
}

/// Read the stored preferences.
///
/// A missing, unreadable, or corrupt file is not an error: preferences are a
/// convenience, and a review session must still start without them.
pub fn load(path: &Path) -> Preferences {
    let Ok(contents) = fs::read_to_string(path) else {
        return Preferences::default();
    };
    serde_json::from_str(&contents).unwrap_or_else(|error| {
        tracing::warn!(
            path = %path.display(),
            error = %error,
            "ignoring unreadable preferences file"
        );
        Preferences::default()
    })
}

/// Write the preferences, creating the parent directory when it is absent.
pub fn save(path: &Path, preferences: &Preferences) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(preferences)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_path_sits_beside_the_history_archives() {
        let path = default_preferences_path();
        assert!(path.ends_with(".discuss/preferences.json"), "{path:?}");
    }

    #[test]
    fn missing_file_loads_as_defaults() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("absent.json");

        assert_eq!(load(&path), Preferences::default());
    }

    #[test]
    fn corrupt_file_loads_as_defaults_rather_than_failing() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("preferences.json");
        fs::write(&path, "{ not json").expect("write");

        assert_eq!(load(&path), Preferences::default());
    }

    #[test]
    fn saved_preferences_round_trip_through_the_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("nested").join("preferences.json");
        let preferences = Preferences {
            theme: Some("dark".to_string()),
            files_collapsed: Some(true),
            cmd_enter_to_send: Some(false),
        };

        save(&path, &preferences).expect("save");

        assert_eq!(load(&path), preferences);
    }

    #[test]
    fn stored_json_uses_the_camel_case_wire_names() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("preferences.json");

        save(
            &path,
            &Preferences {
                theme: None,
                files_collapsed: Some(true),
                cmd_enter_to_send: Some(true),
            },
        )
        .expect("save");

        let written = fs::read_to_string(&path).expect("read");
        assert!(written.contains("\"filesCollapsed\""), "{written}");
        assert!(written.contains("\"cmdEnterToSend\""), "{written}");
    }

    #[test]
    fn a_file_from_an_older_build_keeps_the_fields_it_knows() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("preferences.json");
        fs::write(&path, r#"{"theme":"light"}"#).expect("write");

        assert_eq!(
            load(&path),
            Preferences {
                theme: Some("light".to_string()),
                files_collapsed: None,
                cmd_enter_to_send: None,
            }
        );
    }

    #[test]
    fn merge_overwrites_only_the_supplied_fields() {
        let mut stored = Preferences {
            theme: Some("dark".to_string()),
            files_collapsed: Some(true),
            cmd_enter_to_send: Some(true),
        };

        stored.merge(Preferences {
            theme: None,
            files_collapsed: None,
            cmd_enter_to_send: Some(false),
        });

        assert_eq!(
            stored,
            Preferences {
                theme: Some("dark".to_string()),
                files_collapsed: Some(true),
                cmd_enter_to_send: Some(false),
            }
        );
    }
}
