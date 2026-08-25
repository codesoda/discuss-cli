use std::io::Write;
use std::net::SocketAddr;

use chrono::Utc;
use tokio::net::TcpListener;

use crate::endpoints::{SessionFacts, session_started_payload};
use crate::events::{Event, EventEmitter, EventKind};
use crate::launch::{self, BrowserLauncher};
use crate::{Result, server};

pub struct BoundListener {
    pub addr: SocketAddr,
    pub listener: TcpListener,
}

pub struct StartedListeners {
    pub api: BoundListener,
    pub proxy: Option<BoundListener>,
}

pub async fn start_session<E, W, L>(
    api_addr: SocketAddr,
    proxy_addr: Option<SocketAddr>,
    facts: &SessionFacts,
    emitter: &EventEmitter<E>,
    stderr: &mut W,
    launcher: &L,
    auto_open: bool,
) -> Result<StartedListeners>
where
    E: Write,
    W: Write,
    L: BrowserLauncher,
{
    let (api_listener, api_addr) = server::bind_listener(api_addr).await?;
    let proxy = if let Some(proxy_addr) = proxy_addr {
        let (listener, addr) = server::bind_listener(proxy_addr).await?;
        Some(BoundListener { addr, listener })
    } else {
        None
    };

    let started_at = Utc::now();
    let payload = session_started_payload(
        api_addr,
        proxy.as_ref().map(|listener| listener.addr),
        facts,
        started_at,
    );
    let review_url = launch::loopback_url(api_addr);

    if let Err(error) = emitter.emit(&Event {
        kind: EventKind::SessionStarted,
        at: started_at,
        payload,
    }) {
        tracing::warn!(
            url = %review_url,
            error = %error,
            "failed to emit session.started event"
        );
    }

    let proxy_url = proxy
        .as_ref()
        .map(|listener| launch::loopback_url(listener.addr));
    if let Err(error) = launch::announce_endpoints(
        stderr,
        launcher,
        &review_url,
        proxy_url.as_deref(),
        auto_open,
    ) {
        tracing::warn!(
            url = %review_url,
            error = %error,
            "failed to write endpoint URLs to stderr"
        );
    }

    Ok(StartedListeners {
        api: BoundListener {
            addr: api_addr,
            listener: api_listener,
        },
        proxy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;
    use std::io;
    use std::net::{Ipv4Addr, TcpListener as StdTcpListener};

    use serde_json::Value;

    #[derive(Debug, Default)]
    struct FakeLauncher {
        opened_urls: RefCell<Vec<String>>,
    }

    impl BrowserLauncher for FakeLauncher {
        fn open(&self, url: &str) -> io::Result<()> {
            self.opened_urls.borrow_mut().push(url.to_string());
            Ok(())
        }
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("write failed"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn facts() -> SessionFacts {
        SessionFacts {
            mode: "markdown".to_string(),
            source_file: "review.md".to_string(),
            files_count: 1,
            git_args: Vec::new(),
        }
    }

    fn free_loopback_addr() -> SocketAddr {
        let listener = StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("allocate free port");
        listener.local_addr().expect("read free port")
    }

    #[tokio::test]
    async fn start_session_partial_bind_failure_produces_no_side_effects() {
        let occupied_proxy =
            StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("occupy proxy port");
        let proxy_addr = occupied_proxy.local_addr().expect("read proxy port");
        let api_addr = free_loopback_addr();
        let emitter = EventEmitter::new(Vec::new());
        let mut stderr = Vec::new();
        let launcher = FakeLauncher::default();

        let result = start_session(
            api_addr,
            Some(proxy_addr),
            &facts(),
            &emitter,
            &mut stderr,
            &launcher,
            true,
        )
        .await;

        assert!(result.is_err());
        assert!(emitter.into_inner().expect("read event sink").is_empty());
        assert!(stderr.is_empty());
        assert!(launcher.opened_urls.borrow().is_empty());
        TcpListener::bind(api_addr)
            .await
            .expect("API listener should be released after proxy bind failure");
    }

    #[tokio::test]
    async fn start_session_emits_payload_announces_and_opens_browser() {
        let emitter = EventEmitter::new(Vec::new());
        let mut stderr = Vec::new();
        let launcher = FakeLauncher::default();

        let started = start_session(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            None,
            &facts(),
            &emitter,
            &mut stderr,
            &launcher,
            true,
        )
        .await
        .expect("session should start");
        let review_url = launch::loopback_url(started.api.addr);
        let emitted = String::from_utf8(emitter.into_inner().expect("read event sink"))
            .expect("event should be UTF-8");
        let event: Value = serde_json::from_str(emitted.trim()).expect("event should be JSON");

        assert_eq!(event["kind"], "session.started");
        assert_eq!(event["payload"]["apiBaseUrl"], review_url);
        assert_eq!(
            String::from_utf8(stderr).expect("stderr should be UTF-8"),
            format!("review UI/API: {review_url}\n")
        );
        assert_eq!(launcher.opened_urls.borrow().as_slice(), [review_url]);
    }

    #[tokio::test]
    async fn start_session_reports_proxy_endpoint_when_second_listener_binds() {
        let emitter = EventEmitter::new(Vec::new());
        let mut stderr = Vec::new();
        let launcher = FakeLauncher::default();

        let started = start_session(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))),
            &facts(),
            &emitter,
            &mut stderr,
            &launcher,
            false,
        )
        .await
        .expect("both listeners should bind");
        let review_url = launch::loopback_url(started.api.addr);
        let proxy_url = launch::loopback_url(
            started
                .proxy
                .as_ref()
                .expect("proxy listener should be returned")
                .addr,
        );
        let emitted = String::from_utf8(emitter.into_inner().expect("read event sink"))
            .expect("event should be UTF-8");
        let event: Value = serde_json::from_str(emitted.trim()).expect("event should be JSON");

        assert_eq!(event["payload"]["proxyUrl"], proxy_url);
        assert_eq!(
            String::from_utf8(stderr).expect("stderr should be UTF-8"),
            format!("review UI/API: {review_url}\nwebsite proxy: {proxy_url}\n")
        );
        assert!(launcher.opened_urls.borrow().is_empty());
    }

    #[tokio::test]
    async fn start_session_suppresses_browser_open_when_auto_open_disabled() {
        let emitter = EventEmitter::new(Vec::new());
        let mut stderr = Vec::new();
        let launcher = FakeLauncher::default();

        let started = start_session(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            None,
            &facts(),
            &emitter,
            &mut stderr,
            &launcher,
            false,
        )
        .await
        .expect("session should start");
        let review_url = launch::loopback_url(started.api.addr);

        assert_eq!(
            String::from_utf8(stderr).expect("stderr should be UTF-8"),
            format!("review UI/API: {review_url}\n")
        );
        assert!(launcher.opened_urls.borrow().is_empty());
    }

    #[tokio::test]
    async fn start_session_warns_and_keeps_serving_when_event_emission_fails() {
        let emitter = EventEmitter::new(FailingWriter);
        let mut stderr = Vec::new();
        let launcher = FakeLauncher::default();

        let started = start_session(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            None,
            &facts(),
            &emitter,
            &mut stderr,
            &launcher,
            true,
        )
        .await
        .expect("event failure should not fail startup");
        let review_url = launch::loopback_url(started.api.addr);

        assert!(TcpListener::bind(started.api.addr).await.is_err());
        assert_eq!(
            String::from_utf8(stderr).expect("stderr should be UTF-8"),
            format!("review UI/API: {review_url}\n")
        );
        assert_eq!(launcher.opened_urls.borrow().as_slice(), [review_url]);
    }

    #[tokio::test]
    async fn start_session_warns_and_keeps_serving_when_stderr_write_fails() {
        let emitter = EventEmitter::new(Vec::new());
        let mut stderr = FailingWriter;
        let launcher = FakeLauncher::default();

        let started = start_session(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            None,
            &facts(),
            &emitter,
            &mut stderr,
            &launcher,
            true,
        )
        .await
        .expect("stderr failure should not fail startup");

        assert!(TcpListener::bind(started.api.addr).await.is_err());
        assert!(!emitter.into_inner().expect("read event sink").is_empty());
        assert!(launcher.opened_urls.borrow().is_empty());
    }
}
