use std::net::SocketAddr;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::launch;

pub const AGENT_INSTRUCTIONS: [&str; 3] = [
    "Use payload.endpoints; do not assume port 7777.",
    "On thread.created, POST a take to addTakeTemplate with {threadId} replaced.",
    "Stop when session.done is received.",
];

#[derive(Debug, Serialize)]
pub struct Endpoints {
    pub state: String,
    pub events: String,
    #[serde(rename = "createThread")]
    pub create_thread: String,
    #[serde(rename = "addTakeTemplate")]
    pub add_take_template: String,
    pub done: String,
}

#[derive(Debug, Serialize)]
pub struct SessionStartedPayload {
    pub url: String,
    #[serde(rename = "apiBaseUrl")]
    pub api_base_url: String,
    #[serde(rename = "proxyUrl", skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    pub endpoints: Endpoints,
    #[serde(rename = "agentInstructions")]
    pub agent_instructions: Vec<String>,
    pub mode: String,
    pub source_file: String,
    pub files_count: usize,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_args: Option<Vec<String>>,
}

pub struct SessionFacts {
    pub mode: String,
    pub source_file: String,
    pub files_count: usize,
    pub git_args: Vec<String>,
}

pub fn session_started_payload(
    api_addr: SocketAddr,
    proxy_addr: Option<SocketAddr>,
    facts: &SessionFacts,
    started_at: DateTime<Utc>,
) -> Value {
    let api_base_url = launch::loopback_url(api_addr);
    let payload = SessionStartedPayload {
        url: api_base_url.clone(),
        api_base_url: api_base_url.clone(),
        proxy_url: proxy_addr.map(launch::loopback_url),
        endpoints: Endpoints {
            state: format!("{api_base_url}/api/state"),
            events: format!("{api_base_url}/api/events"),
            create_thread: format!("{api_base_url}/api/threads"),
            add_take_template: format!("{api_base_url}/api/threads/{{threadId}}/takes"),
            done: format!("{api_base_url}/api/done"),
        },
        agent_instructions: AGENT_INSTRUCTIONS
            .iter()
            .map(|value| value.to_string())
            .collect(),
        mode: facts.mode.clone(),
        source_file: facts.source_file.clone(),
        files_count: facts.files_count,
        started_at: started_at.to_rfc3339(),
        git_args: (!facts.git_args.is_empty()).then(|| facts.git_args.clone()),
    };

    serde_json::to_value(payload).expect("session.started payload should serialize")
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::TimeZone;
    use serde_json::json;

    fn facts(git_args: Vec<String>) -> SessionFacts {
        SessionFacts {
            mode: "diff".to_string(),
            source_file: "git --stat".to_string(),
            files_count: 2,
            git_args,
        }
    }

    fn started_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 25, 12, 30, 0)
            .single()
            .expect("valid test timestamp")
    }

    #[test]
    fn session_started_payload_reports_endpoints_for_bound_address() {
        let payload = session_started_payload(
            "127.0.0.1:49152".parse().expect("valid API address"),
            None,
            &facts(Vec::new()),
            started_at(),
        );

        assert_eq!(payload["url"], "http://127.0.0.1:49152");
        assert_eq!(payload["apiBaseUrl"], payload["url"]);
        assert_eq!(
            payload["endpoints"]["state"],
            "http://127.0.0.1:49152/api/state"
        );
        assert_eq!(
            payload["endpoints"]["events"],
            "http://127.0.0.1:49152/api/events"
        );
        assert_eq!(
            payload["endpoints"]["createThread"],
            "http://127.0.0.1:49152/api/threads"
        );
        assert_eq!(
            payload["endpoints"]["addTakeTemplate"],
            "http://127.0.0.1:49152/api/threads/{threadId}/takes"
        );
        assert_eq!(
            payload["endpoints"]["done"],
            "http://127.0.0.1:49152/api/done"
        );
    }

    #[test]
    fn session_started_payload_omits_proxy_url_without_secondary_listener() {
        let payload = session_started_payload(
            "127.0.0.1:49152".parse().expect("valid API address"),
            None,
            &facts(Vec::new()),
            started_at(),
        );

        assert!(payload.get("proxyUrl").is_none());
    }

    #[test]
    fn session_started_payload_reports_proxy_url_for_second_bound_address() {
        let payload = session_started_payload(
            "127.0.0.1:49152".parse().expect("valid API address"),
            Some("127.0.0.1:49153".parse().expect("valid proxy address")),
            &facts(Vec::new()),
            started_at(),
        );

        assert_eq!(payload["proxyUrl"], "http://127.0.0.1:49153");
    }

    #[test]
    fn session_started_payload_preserves_existing_keys_and_omits_empty_git_args() {
        let payload = session_started_payload(
            "127.0.0.1:49152".parse().expect("valid API address"),
            None,
            &facts(Vec::new()),
            started_at(),
        );

        assert_eq!(payload["mode"], "diff");
        assert_eq!(payload["source_file"], "git --stat");
        assert_eq!(payload["files_count"], 2);
        assert_eq!(payload["started_at"], "2026-08-25T12:30:00+00:00");
        assert_eq!(payload["agentInstructions"], json!(AGENT_INSTRUCTIONS));
        assert!(payload.get("git_args").is_none());

        let payload = session_started_payload(
            "127.0.0.1:49152".parse().expect("valid API address"),
            None,
            &facts(vec!["--stat".to_string()]),
            started_at(),
        );
        assert_eq!(payload["git_args"], json!(["--stat"]));
    }
}
