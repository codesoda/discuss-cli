//! "Agent is working" indicators.
//!
//! The agent posts a short label before a long job (a search, a rewrite) and
//! clears it when done. Statuses are browser-only: they go out over SSE as
//! `agent.status`, never to stdout, and are not part of review state. A take
//! on a thread clears any status tied to that thread, and stale statuses
//! expire so a forgotten one cannot spin forever.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State as AxumState;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::sse::BroadcastEvent;

use super::app_state::AppState;
use super::response::api_error_response;

const STATUS_TTL_MINUTES: i64 = 10;
const MAX_LABEL_CHARS: usize = 120;
const DEFAULT_STATUS_ID: &str = "main";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentStatus {
    id: String,
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_id: Option<String>,
    started_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
pub(super) struct AgentStatusStore {
    statuses: BTreeMap<String, AgentStatus>,
}

impl AgentStatusStore {
    fn prune(&mut self, now: DateTime<Utc>) {
        let cutoff = now - Duration::minutes(STATUS_TTL_MINUTES);
        self.statuses.retain(|_, status| status.started_at > cutoff);
    }

    fn list(&mut self, now: DateTime<Utc>) -> Vec<AgentStatus> {
        self.prune(now);
        self.statuses.values().cloned().collect()
    }

    /// Drops statuses tied to `thread_id`; returns true when any were removed.
    pub(super) fn clear_thread(&mut self, thread_id: &str) -> bool {
        let before = self.statuses.len();
        self.statuses
            .retain(|_, status| status.thread_id.as_deref() != Some(thread_id));
        self.statuses.len() != before
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AgentStatusRequest {
    id: Option<String>,
    label: Option<String>,
    thread_id: Option<String>,
    #[serde(default)]
    done: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct AgentStatusResponse {
    statuses: Vec<AgentStatus>,
}

pub(super) async fn get_api_agent_status(AxumState(app_state): AxumState<AppState>) -> Response {
    let statuses = lock(&app_state).list(Utc::now());
    Json(AgentStatusResponse { statuses }).into_response()
}

pub(super) async fn post_api_agent_status(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<AgentStatusRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                rejection.body_text(),
            );
        }
    };

    let id = request
        .id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| DEFAULT_STATUS_ID.to_string());
    let label = request
        .label
        .map(|label| {
            label
                .trim()
                .chars()
                .take(MAX_LABEL_CHARS)
                .collect::<String>()
        })
        .filter(|label| !label.is_empty());
    let thread_id = request.thread_id.filter(|thread_id| !thread_id.is_empty());

    let now = Utc::now();
    let statuses = {
        let mut store = lock(&app_state);
        match label {
            Some(label) if !request.done => {
                store.statuses.insert(
                    id.clone(),
                    AgentStatus {
                        id,
                        label,
                        thread_id,
                        started_at: now,
                    },
                );
            }
            _ => {
                store.statuses.remove(&id);
            }
        }
        store.list(now)
    };

    publish(&app_state, &statuses);
    Json(AgentStatusResponse { statuses }).into_response()
}

/// Clears statuses tied to a thread once the agent has answered it.
pub(super) fn clear_thread_status(app_state: &AppState, thread_id: &str) {
    let statuses = {
        let mut store = lock(app_state);
        if !store.clear_thread(thread_id) {
            return;
        }
        store.list(Utc::now())
    };
    publish(app_state, &statuses);
}

fn publish(app_state: &AppState, statuses: &[AgentStatus]) {
    app_state.bus.publish(BroadcastEvent {
        kind: "agent.status".to_string(),
        payload: serde_json::json!({ "statuses": statuses }),
    });
}

fn lock(app_state: &AppState) -> std::sync::MutexGuard<'_, AgentStatusStore> {
    app_state
        .agent_status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(id: &str, thread_id: Option<&str>, started_at: DateTime<Utc>) -> AgentStatus {
        AgentStatus {
            id: id.to_string(),
            label: format!("working on {id}"),
            thread_id: thread_id.map(str::to_string),
            started_at,
        }
    }

    #[test]
    fn list_drops_expired_statuses() {
        let now = Utc::now();
        let mut store = AgentStatusStore::default();
        store.statuses.insert(
            "old".into(),
            status("old", None, now - Duration::minutes(11)),
        );
        store.statuses.insert(
            "new".into(),
            status("new", None, now - Duration::minutes(1)),
        );

        let ids: Vec<_> = store.list(now).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["new".to_string()]);
    }

    #[test]
    fn clear_thread_only_removes_that_threads_statuses() {
        let now = Utc::now();
        let mut store = AgentStatusStore::default();
        store
            .statuses
            .insert("a".into(), status("a", Some("u-1"), now));
        store
            .statuses
            .insert("b".into(), status("b", Some("u-2"), now));
        store.statuses.insert("c".into(), status("c", None, now));

        assert!(store.clear_thread("u-1"));
        assert!(!store.clear_thread("u-1"));
        let ids: Vec<_> = store.list(now).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["b".to_string(), "c".to_string()]);
    }
}
