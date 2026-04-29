use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ThreadId(pub String);

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct FileId(pub String);

pub fn default_file_id() -> FileId {
    FileId("f-1".to_string())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    Markdown,
    Diff,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    pub id: FileId,
    pub path: String,
    pub kind: FileKind,
    pub content: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    #[serde(default)]
    pub files: Vec<File>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMeta {
    pub id: FileId,
    pub path: String,
    pub kind: FileKind,
}

impl From<&File> for FileMeta {
    fn from(file: &File) -> Self {
        Self {
            id: file.id.clone(),
            path: file.path.clone(),
            kind: file.kind,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadKind {
    User,
    Prepopulated,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: ThreadId,
    #[serde(default = "default_file_id")]
    pub file_id: FileId,
    pub anchor_start: usize,
    pub anchor_end: usize,
    pub snippet: String,
    pub breadcrumb: String,
    pub text: String,
    pub created_at: DateTime<Utc>,
    pub kind: ThreadKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_range: Option<LineRange>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reply {
    pub id: String,
    pub thread_id: ThreadId,
    pub text: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Take {
    pub id: String,
    pub thread_id: ThreadId,
    pub text: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    pub decision: Option<String>,
    pub resolved_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Drafts {
    #[serde(with = "anchor_range_keys")]
    pub new_thread: HashMap<(usize, usize), Draft>,
    pub followup: HashMap<ThreadId, Draft>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub text: String,
    pub updated_at: DateTime<Utc>,
}

mod anchor_range_keys {
    use std::collections::HashMap;

    use serde::Deserialize;
    use serde::de::{self, Deserializer};
    use serde::ser::{SerializeMap, Serializer};

    use super::Draft;

    pub fn serialize<S>(
        drafts: &HashMap<(usize, usize), Draft>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(drafts.len()))?;

        for ((anchor_start, anchor_end), draft) in drafts {
            map.serialize_entry(&format!("{anchor_start}-{anchor_end}"), draft)?;
        }

        map.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<HashMap<(usize, usize), Draft>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = HashMap::<String, Draft>::deserialize(deserializer)?;
        let mut drafts = HashMap::with_capacity(raw.len());

        for (key, draft) in raw {
            drafts.insert(parse_anchor_range_key(&key)?, draft);
        }

        Ok(drafts)
    }

    fn parse_anchor_range_key<E>(key: &str) -> Result<(usize, usize), E>
    where
        E: de::Error,
    {
        let (anchor_start, anchor_end) = key.split_once('-').ok_or_else(|| {
            E::custom(format!("expected draft key {key:?} to look like start-end"))
        })?;

        let anchor_start = anchor_start.parse().map_err(|error| {
            E::custom(format!(
                "invalid draft key {key:?}: start anchor is not a number: {error}"
            ))
        })?;
        let anchor_end = anchor_end.parse().map_err(|error| {
            E::custom(format!(
                "invalid draft key {key:?}: end anchor is not a number: {error}"
            ))
        })?;

        Ok((anchor_start, anchor_end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::TimeZone;
    use serde_json::{Value, json};

    fn timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 23, 2, 30, 0)
            .single()
            .expect("valid timestamp")
    }

    #[test]
    fn thread_serializes_with_template_compatible_camel_case_keys() {
        let thread = Thread {
            id: ThreadId("u-abc".to_string()),
            file_id: FileId("f-1".to_string()),
            anchor_start: 2,
            anchor_end: 4,
            snippet: "selected text".to_string(),
            breadcrumb: "Overview > Goals".to_string(),
            text: "Needs clarification".to_string(),
            created_at: timestamp(),
            kind: ThreadKind::User,
            line_range: None,
        };

        let value = serde_json::to_value(&thread).expect("serialize thread");

        assert_eq!(value["id"], "u-abc");
        assert_eq!(value["fileId"], "f-1");
        assert_eq!(value["anchorStart"], 2);
        assert_eq!(value["anchorEnd"], 4);
        assert_eq!(value["createdAt"], "2026-04-23T02:30:00Z");
        assert_eq!(value["kind"], "user");
        assert!(value.get("anchor_start").is_none());
        assert!(value.get("created_at").is_none());
        assert!(value.get("file_id").is_none());
        assert!(value.get("lineRange").is_none());

        let round_tripped: Thread = serde_json::from_value(value).expect("deserialize thread");
        assert_eq!(round_tripped, thread);
    }

    #[test]
    fn thread_round_trips_with_line_range_in_camel_case() {
        let thread = Thread {
            id: ThreadId("u-code".to_string()),
            file_id: FileId("f-1".to_string()),
            anchor_start: 7,
            anchor_end: 7,
            snippet: "fn main() {}".to_string(),
            breadcrumb: String::new(),
            text: "Why is this here?".to_string(),
            created_at: timestamp(),
            kind: ThreadKind::User,
            line_range: Some(LineRange { start: 3, end: 5 }),
        };

        let value = serde_json::to_value(&thread).expect("serialize thread");
        assert_eq!(value["lineRange"], json!({ "start": 3, "end": 5 }));
        assert!(value["lineRange"].get("Start").is_none());

        let round_tripped: Thread = serde_json::from_value(value).expect("deserialize thread");
        assert_eq!(round_tripped, thread);
    }

    #[test]
    fn thread_deserializes_default_file_id_when_missing() {
        let value = json!({
            "id": "u-old",
            "anchorStart": 1,
            "anchorEnd": 2,
            "snippet": "snippet",
            "breadcrumb": "",
            "text": "from an archive predating fileId",
            "createdAt": "2026-04-23T02:30:00Z",
            "kind": "user"
        });

        let thread: Thread = serde_json::from_value(value).expect("deserialize legacy thread");
        assert_eq!(thread.file_id, default_file_id());
    }

    #[test]
    fn file_and_file_meta_round_trip_in_camel_case() {
        let file = File {
            id: FileId("f-1".to_string()),
            path: "plan.md".to_string(),
            kind: FileKind::Markdown,
            content: "# heading".to_string(),
        };

        let value = serde_json::to_value(&file).expect("serialize file");
        assert_eq!(value["id"], "f-1");
        assert_eq!(value["path"], "plan.md");
        assert_eq!(value["kind"], "markdown");
        assert_eq!(value["content"], "# heading");

        let round_tripped: File = serde_json::from_value(value).expect("deserialize file");
        assert_eq!(round_tripped, file);

        let meta: FileMeta = (&file).into();
        let value = serde_json::to_value(&meta).expect("serialize file meta");
        assert_eq!(
            value,
            json!({ "id": "f-1", "path": "plan.md", "kind": "markdown" })
        );

        let round_tripped: FileMeta = serde_json::from_value(value).expect("deserialize file meta");
        assert_eq!(round_tripped, meta);
    }

    #[test]
    fn file_kind_serializes_to_lowercase_strings() {
        assert_eq!(
            serde_json::to_value(FileKind::Markdown).expect("serialize markdown"),
            "markdown"
        );
        assert_eq!(
            serde_json::to_value(FileKind::Diff).expect("serialize diff"),
            "diff"
        );
    }

    #[test]
    fn reply_take_and_resolution_use_camel_case_json() {
        let reply = Reply {
            id: "r-1".to_string(),
            thread_id: ThreadId("u-abc".to_string()),
            text: "User reply".to_string(),
            created_at: timestamp(),
        };
        let take = Take {
            id: "t-1".to_string(),
            thread_id: ThreadId("u-abc".to_string()),
            text: "Agent take".to_string(),
            created_at: timestamp(),
        };
        let resolution = Resolution {
            decision: Some("Ship it".to_string()),
            resolved_at: timestamp(),
        };

        let reply_value = serde_json::to_value(&reply).expect("serialize reply");
        let take_value = serde_json::to_value(&take).expect("serialize take");
        let resolution_value = serde_json::to_value(&resolution).expect("serialize resolution");

        assert_eq!(
            reply_value,
            json!({
                "id": "r-1",
                "threadId": "u-abc",
                "text": "User reply",
                "createdAt": "2026-04-23T02:30:00Z"
            })
        );
        assert_eq!(
            take_value,
            json!({
                "id": "t-1",
                "threadId": "u-abc",
                "text": "Agent take",
                "createdAt": "2026-04-23T02:30:00Z"
            })
        );
        assert_eq!(
            resolution_value,
            json!({
                "decision": "Ship it",
                "resolvedAt": "2026-04-23T02:30:00Z"
            })
        );

        assert_eq!(
            serde_json::from_value::<Reply>(reply_value).expect("deserialize reply"),
            reply
        );
        assert_eq!(
            serde_json::from_value::<Take>(take_value).expect("deserialize take"),
            take
        );
        assert_eq!(
            serde_json::from_value::<Resolution>(resolution_value).expect("deserialize resolution"),
            resolution
        );
    }

    #[test]
    fn thread_id_serializes_transparently() {
        let id = ThreadId("u-abc".to_string());

        assert_eq!(serde_json::to_value(&id).expect("serialize id"), "u-abc");
        assert_eq!(
            serde_json::from_value::<ThreadId>(Value::String("u-abc".to_string()))
                .expect("deserialize id"),
            id
        );
    }

    #[test]
    fn drafts_round_trip_with_anchor_range_and_thread_id_keys() {
        let mut drafts = Drafts::default();
        let draft = Draft {
            text: "New thread draft".to_string(),
            updated_at: timestamp(),
        };
        let followup = Draft {
            text: "Follow-up draft".to_string(),
            updated_at: timestamp(),
        };

        drafts.new_thread.insert((3, 5), draft.clone());
        drafts
            .followup
            .insert(ThreadId("u-abc".to_string()), followup.clone());

        let value = serde_json::to_value(&drafts).expect("serialize drafts");

        assert_eq!(value["newThread"]["3-5"]["text"], draft.text);
        assert_eq!(
            value["newThread"]["3-5"]["updatedAt"],
            "2026-04-23T02:30:00Z"
        );
        assert_eq!(value["followup"]["u-abc"]["text"], followup.text);
        assert!(value.get("new_thread").is_none());
        assert!(value["newThread"].get("3,5").is_none());

        let round_tripped: Drafts = serde_json::from_value(value).expect("deserialize drafts");
        assert_eq!(round_tripped, drafts);
    }

    #[test]
    fn invalid_new_thread_draft_key_is_rejected() {
        let error = serde_json::from_value::<Drafts>(json!({
            "newThread": {
                "not-a-range": {
                    "text": "draft",
                    "updatedAt": "2026-04-23T02:30:00Z"
                }
            },
            "followup": {}
        }))
        .expect_err("invalid key should fail");

        assert!(
            error.to_string().contains("invalid draft key"),
            "unexpected error: {error}"
        );
    }
}
