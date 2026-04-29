use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::state::{LineRange, Reply, Resolution, State, Take, ThreadId, ThreadKind};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub threads: Vec<TranscriptThread>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptThread {
    pub id: ThreadId,
    pub anchor_start: usize,
    pub anchor_end: usize,
    pub snippet: String,
    pub breadcrumb: String,
    pub text: String,
    pub kind: ThreadKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_range: Option<LineRange>,
    pub replies: Vec<Reply>,
    pub takes: Vec<Take>,
    pub resolution: Option<Resolution>,
    pub created_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

pub fn build_transcript(state: &State) -> Transcript {
    let mut threads = state
        .all_threads()
        .iter()
        .map(|thread| TranscriptThread {
            id: thread.id.clone(),
            anchor_start: thread.anchor_start,
            anchor_end: thread.anchor_end,
            snippet: thread.snippet.clone(),
            breadcrumb: thread.breadcrumb.clone(),
            text: thread.text.clone(),
            kind: thread.kind.clone(),
            line_range: thread.line_range,
            replies: state.replies_for_thread(&thread.id),
            takes: state.takes_for_thread(&thread.id),
            resolution: state.resolution_for_thread(&thread.id),
            created_at: thread.created_at,
            deleted_at: state.deleted_at_for_thread(&thread.id),
        })
        .collect::<Vec<_>>();

    threads.sort_by_key(|thread| (thread.anchor_start, thread.anchor_end));

    Transcript { threads }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::TimeZone;
    use serde_json::json;

    use crate::state::{Reply, Resolution, State, Take, Thread, ThreadId, ThreadKind};

    fn timestamp(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 23, 2, 30, second)
            .single()
            .expect("valid timestamp")
    }

    fn thread(id: &str, anchor_start: usize, anchor_end: usize) -> Thread {
        Thread {
            id: ThreadId(id.to_string()),
            anchor_start,
            anchor_end,
            snippet: format!("snippet {id}"),
            breadcrumb: "Overview > Goals".to_string(),
            text: format!("initial comment {id}"),
            created_at: timestamp(anchor_start as u32),
            kind: ThreadKind::User,
            line_range: None,
        }
    }

    #[test]
    fn threads_are_sorted_by_document_order() {
        let mut state = State::default();
        state.add_thread(thread("u-later", 5, 5));
        state.add_thread(thread("u-earlier", 1, 1));
        state.add_thread(thread("u-middle", 3, 4));

        let transcript = build_transcript(&state);

        assert_eq!(
            transcript
                .threads
                .iter()
                .map(|thread| thread.id.0.as_str())
                .collect::<Vec<_>>(),
            vec!["u-earlier", "u-middle", "u-later"]
        );
    }

    #[test]
    fn attaches_replies_takes_and_resolution_to_their_threads() {
        let mut state = State::default();
        let first_id = ThreadId("u-1".to_string());
        let second_id = ThreadId("u-2".to_string());
        state.add_thread(thread("u-1", 2, 2));
        state.add_thread(thread("u-2", 4, 4));

        let reply = Reply {
            id: "r-1".to_string(),
            thread_id: first_id.clone(),
            text: "reply on first".to_string(),
            created_at: timestamp(10),
        };
        let take = Take {
            id: "t-1".to_string(),
            thread_id: second_id.clone(),
            text: "take on second".to_string(),
            created_at: timestamp(11),
        };
        let resolution = Resolution {
            decision: Some("accepted".to_string()),
            resolved_at: timestamp(12),
        };
        state.add_reply(reply.clone());
        state.add_take(take.clone());
        state.set_resolution(first_id.clone(), resolution.clone());

        let transcript = build_transcript(&state);
        let first = transcript
            .threads
            .iter()
            .find(|thread| thread.id == first_id)
            .expect("first thread in transcript");
        let second = transcript
            .threads
            .iter()
            .find(|thread| thread.id == second_id)
            .expect("second thread in transcript");

        assert_eq!(first.replies, vec![reply]);
        assert!(first.takes.is_empty());
        assert_eq!(first.resolution, Some(resolution));
        assert_eq!(second.takes, vec![take]);
        assert!(second.replies.is_empty());
        assert!(second.resolution.is_none());
    }

    #[test]
    fn includes_soft_deleted_threads_with_deleted_timestamp() {
        let mut state = State::default();
        let thread_id = ThreadId("u-deleted".to_string());
        state.add_thread(thread("u-deleted", 1, 1));
        state.soft_delete_thread(&thread_id);

        let transcript = build_transcript(&state);

        assert_eq!(transcript.threads.len(), 1);
        assert_eq!(transcript.threads[0].id, thread_id);
        assert!(transcript.threads[0].deleted_at.is_some());
    }

    #[test]
    fn serializes_with_camel_case_keys_and_preserved_iso_timestamps() {
        let mut state = State::default();
        let thread_id = ThreadId("u-1".to_string());
        state.add_thread(thread("u-1", 2, 3));
        state.add_reply(Reply {
            id: "r-1".to_string(),
            thread_id: thread_id.clone(),
            text: "reply".to_string(),
            created_at: timestamp(10),
        });
        state.set_resolution(
            thread_id,
            Resolution {
                decision: None,
                resolved_at: timestamp(11),
            },
        );

        let value = serde_json::to_value(build_transcript(&state)).expect("serialize transcript");

        assert_eq!(
            value,
            json!({
                "threads": [{
                    "id": "u-1",
                    "anchorStart": 2,
                    "anchorEnd": 3,
                    "snippet": "snippet u-1",
                    "breadcrumb": "Overview > Goals",
                    "text": "initial comment u-1",
                    "kind": "user",
                    "replies": [{
                        "id": "r-1",
                        "threadId": "u-1",
                        "text": "reply",
                        "createdAt": "2026-04-23T02:30:10Z"
                    }],
                    "takes": [],
                    "resolution": {
                        "decision": null,
                        "resolvedAt": "2026-04-23T02:30:11Z"
                    },
                    "createdAt": "2026-04-23T02:30:02Z",
                    "deletedAt": null
                }]
            })
        );
        assert!(value["threads"][0].get("anchor_start").is_none());
        assert!(value["threads"][0].get("created_at").is_none());
    }

    #[test]
    fn transcript_preserves_line_range_for_code_block_threads() {
        use crate::state::LineRange;

        let mut thread_with_range = thread("u-code", 5, 5);
        thread_with_range.line_range = Some(LineRange { start: 2, end: 4 });

        let mut state = State::default();
        state.add_thread(thread_with_range);

        let transcript = build_transcript(&state);
        assert_eq!(
            transcript.threads[0].line_range,
            Some(LineRange { start: 2, end: 4 })
        );

        let value = serde_json::to_value(&transcript).expect("serialize");
        assert_eq!(value["threads"][0]["lineRange"]["start"], 2);
        assert_eq!(value["threads"][0]["lineRange"]["end"], 4);
    }

    #[test]
    fn same_state_builds_the_same_transcript() {
        let mut state = State::default();
        let thread_id = ThreadId("u-1".to_string());
        state.add_thread(thread("u-1", 1, 1));
        state.add_take(Take {
            id: "t-1".to_string(),
            thread_id,
            text: "take".to_string(),
            created_at: timestamp(10),
        });

        let first = build_transcript(&state);
        let second = build_transcript(&state);

        assert_eq!(first, second);
        assert_eq!(
            serde_json::to_value(&first).expect("serialize first"),
            serde_json::to_value(&second).expect("serialize second")
        );
    }
}
