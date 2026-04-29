use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{
    Draft, Drafts, FileMeta, NewThreadDraftKey, Reply, Resolution, Take, Thread, ThreadId,
};

pub type SharedState = Arc<RwLock<State>>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct State {
    threads: Vec<Thread>,
    replies: HashMap<ThreadId, Vec<Reply>>,
    takes: HashMap<ThreadId, Vec<Take>>,
    resolutions: HashMap<ThreadId, Resolution>,
    drafts: Drafts,
    deleted_at: HashMap<ThreadId, DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateSnapshot {
    pub threads: Vec<Thread>,
    pub replies: HashMap<ThreadId, Vec<Reply>>,
    pub takes: HashMap<ThreadId, Vec<Take>>,
    pub resolutions: HashMap<ThreadId, Resolution>,
    pub drafts: Drafts,
    #[serde(default)]
    pub files: Vec<FileMeta>,
}

impl State {
    pub fn new_shared() -> SharedState {
        Arc::new(RwLock::new(Self::default()))
    }

    pub fn get_threads(&self) -> Vec<Thread> {
        self.threads
            .iter()
            .filter(|thread| !self.deleted_at.contains_key(&thread.id))
            .cloned()
            .collect()
    }

    pub(crate) fn all_threads(&self) -> &[Thread] {
        &self.threads
    }

    pub(crate) fn replies_for_thread(&self, thread_id: &ThreadId) -> Vec<Reply> {
        self.replies.get(thread_id).cloned().unwrap_or_default()
    }

    pub(crate) fn takes_for_thread(&self, thread_id: &ThreadId) -> Vec<Take> {
        self.takes.get(thread_id).cloned().unwrap_or_default()
    }

    pub(crate) fn resolution_for_thread(&self, thread_id: &ThreadId) -> Option<Resolution> {
        self.resolutions.get(thread_id).cloned()
    }

    pub(crate) fn deleted_at_for_thread(&self, thread_id: &ThreadId) -> Option<DateTime<Utc>> {
        self.deleted_at.get(thread_id).copied()
    }

    pub fn add_thread(&mut self, thread: Thread) -> Thread {
        self.threads.push(thread.clone());
        thread
    }

    pub fn soft_delete_thread(&mut self, thread_id: &ThreadId) {
        if self.threads.iter().any(|thread| &thread.id == thread_id) {
            self.deleted_at.insert(thread_id.clone(), Utc::now());
        }
    }

    pub fn add_reply(&mut self, reply: Reply) -> Reply {
        self.replies
            .entry(reply.thread_id.clone())
            .or_default()
            .push(reply.clone());
        reply
    }

    pub fn add_take(&mut self, take: Take) -> Take {
        self.takes
            .entry(take.thread_id.clone())
            .or_default()
            .push(take.clone());
        take
    }

    pub fn set_resolution(&mut self, thread_id: ThreadId, resolution: Resolution) -> Resolution {
        self.resolutions.insert(thread_id, resolution.clone());
        resolution
    }

    pub fn clear_resolution(&mut self, thread_id: &ThreadId) {
        self.resolutions.remove(thread_id);
    }

    pub fn upsert_new_thread_draft(&mut self, key: NewThreadDraftKey, draft: Draft) -> Draft {
        self.drafts.new_thread.insert(key, draft.clone());
        draft
    }

    pub fn clear_new_thread_draft(&mut self, key: &NewThreadDraftKey) {
        self.drafts.new_thread.remove(key);
    }

    pub fn upsert_followup_draft(&mut self, thread_id: ThreadId, draft: Draft) -> Draft {
        self.drafts.followup.insert(thread_id, draft.clone());
        draft
    }

    pub fn clear_followup_draft(&mut self, thread_id: &ThreadId) {
        self.drafts.followup.remove(thread_id);
    }

    pub fn snapshot(&self) -> StateSnapshot {
        let threads = self.get_threads();
        let active_thread_ids = threads
            .iter()
            .map(|thread| thread.id.clone())
            .collect::<HashSet<_>>();

        StateSnapshot {
            threads,
            replies: active_vec_map(&self.replies, &active_thread_ids),
            takes: active_vec_map(&self.takes, &active_thread_ids),
            resolutions: active_value_map(&self.resolutions, &active_thread_ids),
            drafts: Drafts {
                new_thread: self.drafts.new_thread.clone(),
                followup: active_value_map(&self.drafts.followup, &active_thread_ids),
            },
            files: Vec::new(),
        }
    }
}

fn active_vec_map<T: Clone>(
    map: &HashMap<ThreadId, Vec<T>>,
    active_thread_ids: &HashSet<ThreadId>,
) -> HashMap<ThreadId, Vec<T>> {
    map.iter()
        .filter(|(thread_id, _)| active_thread_ids.contains(*thread_id))
        .map(|(thread_id, values)| (thread_id.clone(), values.clone()))
        .collect()
}

fn active_value_map<T: Clone>(
    map: &HashMap<ThreadId, T>,
    active_thread_ids: &HashSet<ThreadId>,
) -> HashMap<ThreadId, T> {
    map.iter()
        .filter(|(thread_id, _)| active_thread_ids.contains(*thread_id))
        .map(|(thread_id, value)| (thread_id.clone(), value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::TimeZone;

    use crate::state::{ThreadKind, types::default_file_id};

    fn timestamp(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 23, 2, 30, second)
            .single()
            .expect("valid timestamp")
    }

    fn thread(id: &str, anchor_start: usize) -> Thread {
        Thread {
            id: ThreadId(id.to_string()),
            file_id: default_file_id(),
            anchor_start,
            anchor_end: anchor_start + 1,
            snippet: format!("snippet {id}"),
            breadcrumb: "Overview".to_string(),
            text: format!("thread {id}"),
            created_at: timestamp(0),
            kind: ThreadKind::User,
            line_range: None,
        }
    }

    fn draft(text: &str) -> Draft {
        Draft {
            text: text.to_string(),
            updated_at: timestamp(1),
        }
    }

    #[test]
    fn shared_state_wraps_state_in_arc_rw_lock() {
        let shared = State::new_shared();

        shared
            .write()
            .expect("state lock should not be poisoned")
            .add_thread(thread("u-1", 1));

        assert_eq!(
            shared
                .read()
                .expect("state lock should not be poisoned")
                .get_threads()
                .len(),
            1
        );
    }

    #[test]
    fn add_then_get_thread_returns_active_threads() {
        let mut state = State::default();
        let added = state.add_thread(thread("u-1", 1));

        assert_eq!(added.id, ThreadId("u-1".to_string()));
        assert_eq!(state.get_threads(), vec![thread("u-1", 1)]);
    }

    #[test]
    fn soft_delete_removes_from_reads_but_preserves_thread() {
        let mut state = State::default();
        let thread = state.add_thread(thread("u-1", 1));

        state.soft_delete_thread(&thread.id);

        assert!(state.get_threads().is_empty());
        assert_eq!(state.threads, vec![thread.clone()]);
        assert!(state.deleted_at.contains_key(&thread.id));
    }

    #[test]
    fn reply_take_and_resolution_mutators_return_stored_objects() {
        let mut state = State::default();
        let thread_id = ThreadId("u-1".to_string());
        state.add_thread(thread("u-1", 1));

        let reply = Reply {
            id: "r-1".to_string(),
            thread_id: thread_id.clone(),
            text: "reply".to_string(),
            created_at: timestamp(2),
        };
        let take = Take {
            id: "t-1".to_string(),
            thread_id: thread_id.clone(),
            text: "take".to_string(),
            created_at: timestamp(3),
        };
        let resolution = Resolution {
            decision: Some("accepted".to_string()),
            resolved_at: timestamp(4),
        };

        assert_eq!(state.add_reply(reply.clone()), reply);
        assert_eq!(state.add_take(take.clone()), take);
        assert_eq!(
            state.set_resolution(thread_id.clone(), resolution.clone()),
            resolution
        );

        let snapshot = state.snapshot();
        assert_eq!(snapshot.replies[&thread_id], vec![reply]);
        assert_eq!(snapshot.takes[&thread_id], vec![take]);
        assert_eq!(snapshot.resolutions[&thread_id], resolution);

        state.clear_resolution(&thread_id);

        assert!(!state.snapshot().resolutions.contains_key(&thread_id));
    }

    #[test]
    fn drafts_upsert_and_clear() {
        let mut state = State::default();
        let thread_id = ThreadId("u-1".to_string());
        state.add_thread(thread("u-1", 1));

        let key = NewThreadDraftKey::new(default_file_id(), 3, 5);
        let new_thread_draft = state.upsert_new_thread_draft(key.clone(), draft("new thread"));
        let followup_draft = state.upsert_followup_draft(thread_id.clone(), draft("followup"));

        assert_eq!(state.snapshot().drafts.new_thread[&key], new_thread_draft);
        assert_eq!(state.snapshot().drafts.followup[&thread_id], followup_draft);

        state.clear_new_thread_draft(&key);
        state.clear_followup_draft(&thread_id);

        let snapshot = state.snapshot();
        assert!(snapshot.drafts.new_thread.is_empty());
        assert!(snapshot.drafts.followup.is_empty());
    }

    #[test]
    fn snapshot_returns_independent_clone_of_active_state() {
        let mut state = State::default();
        let thread_id = ThreadId("u-1".to_string());
        state.add_thread(thread("u-1", 1));
        state.upsert_followup_draft(thread_id.clone(), draft("first"));

        let snapshot = state.snapshot();

        state.add_thread(thread("u-2", 2));
        state.upsert_followup_draft(thread_id.clone(), draft("second"));

        assert_eq!(snapshot.threads, vec![thread("u-1", 1)]);
        assert_eq!(snapshot.drafts.followup[&thread_id].text, "first");
    }

    #[test]
    fn snapshot_excludes_deleted_thread_related_values() {
        let mut state = State::default();
        let thread_id = ThreadId("u-1".to_string());
        state.add_thread(thread("u-1", 1));
        state.add_reply(Reply {
            id: "r-1".to_string(),
            thread_id: thread_id.clone(),
            text: "reply".to_string(),
            created_at: timestamp(2),
        });
        state.upsert_followup_draft(thread_id.clone(), draft("followup"));

        state.soft_delete_thread(&thread_id);

        let snapshot = state.snapshot();
        assert!(snapshot.threads.is_empty());
        assert!(snapshot.replies.is_empty());
        assert!(snapshot.drafts.followup.is_empty());
        assert_eq!(state.replies[&thread_id].len(), 1);
    }
}
