//! Fixed-upstream loopback reverse proxy used by live website review sessions.

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};

use axum::Router;
use axum::body::Body;
use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, State as AxumState};
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::http::{Request, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::any;
use futures_util::{SinkExt, StreamExt};
use reqwest::redirect::Policy;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use url::Url;

use crate::{DiscussError, Result};

pub const MAX_HTML_BYTES: usize = 25 * 1024 * 1024;
const SERVICE_WORKER_GUARD: &str = r#"<script data-discuss-service-worker-guard>(function(){if(!('serviceWorker' in navigator))return;try{navigator.serviceWorker.getRegistrations().then(function(rs){rs.forEach(function(r){r.unregister();});});navigator.serviceWorker.register=function(){return Promise.reject(new DOMException('Service workers are disabled during Discuss live review','SecurityError'));};}catch(_){}})();</script>"#;

#[derive(Clone, Debug)]
pub struct LiveProxy {
    upstream: Url,
    upstream_origin: String,
    proxy_origin: String,
    api_origin: String,
    client: reqwest::Client,
}

impl LiveProxy {
    pub fn new(upstream: Url, proxy_origin: String, api_origin: String) -> Result<Self> {
        if !matches!(upstream.scheme(), "http" | "https") || upstream.host_str().is_none() {
            return Err(DiscussError::ConfigError {
                message: format!("live review requires an http:// or https:// URL, got {upstream}"),
            });
        }
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()
            .map_err(|error| DiscussError::ConfigError {
                message: format!("failed to initialize live website proxy: {error}"),
            })?;
        let upstream_origin = upstream.origin().ascii_serialization();
        Ok(Self {
            upstream,
            upstream_origin,
            proxy_origin,
            api_origin,
            client,
        })
    }

    fn target_url(&self, path_and_query: &str) -> Url {
        let mut target = self.upstream.clone();
        let (path, query) = path_and_query
            .split_once('?')
            .map_or((path_and_query, None), |(path, query)| (path, Some(query)));
        target.set_path(if path.is_empty() { "/" } else { path });
        target.set_query(query);
        target.set_fragment(None);
        target
    }

    fn websocket_url(&self, path_and_query: &str) -> Url {
        let mut target = self.target_url(path_and_query);
        let scheme = if target.scheme() == "https" {
            "wss"
        } else {
            "ws"
        };
        target
            .set_scheme(scheme)
            .expect("http and https URLs accept websocket schemes");
        target
    }
}

pub async fn serve_proxy_listener(
    listener: TcpListener,
    proxy: LiveProxy,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let listening_addr = listener
        .local_addr()
        .map_err(|source| DiscussError::ServerBindError {
            addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            source,
        })?;
    if listening_addr.ip() != Ipv4Addr::LOCALHOST {
        return Err(DiscussError::ServerBindError {
            addr: listening_addr,
            source: std::io::Error::new(
                std::io::ErrorKind::AddrNotAvailable,
                "discuss only binds to 127.0.0.1",
            ),
        });
    }

    let router = Router::new().fallback(any(proxy_request)).with_state(proxy);
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            while shutdown.changed().await.is_ok() {
                if *shutdown.borrow() {
                    break;
                }
            }
        })
        .await
        .map_err(|source| DiscussError::ServerBindError {
            addr: listening_addr,
            source,
        })
}

async fn proxy_request(
    AxumState(proxy): AxumState<LiveProxy>,
    request: Request<Body>,
) -> Response<Body> {
    if request
        .headers()
        .get("service-worker")
        .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"script"))
    {
        return proxy_error(
            StatusCode::FORBIDDEN,
            "service workers are disabled during Discuss live review",
        );
    }

    if request.headers().contains_key(header::UPGRADE) {
        let (mut parts, body) = request.into_parts();
        match WebSocketUpgrade::from_request_parts(&mut parts, &proxy).await {
            Ok(websocket) => {
                return proxy_websocket(proxy, websocket, Request::from_parts(parts, body)).await;
            }
            Err(error) => return error.into_response(),
        }
    }

    proxy_http(proxy, request).await
}

async fn proxy_http(proxy: LiveProxy, request: Request<Body>) -> Response<Body> {
    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let target = proxy.target_url(path_and_query);
    let method = request.method().clone();
    let mut builder = proxy.client.request(method, target);
    builder = copy_request_headers(builder, request.headers(), &proxy);
    let body = reqwest::Body::wrap_stream(request.into_body().into_data_stream());

    let upstream = match builder.body(body).send().await {
        Ok(response) => response,
        Err(error) => {
            return proxy_error(
                StatusCode::BAD_GATEWAY,
                &format!("live upstream request failed: {error}"),
            );
        }
    };

    proxy_response(upstream, &proxy).await
}

fn copy_request_headers(
    mut builder: reqwest::RequestBuilder,
    headers: &HeaderMap,
    proxy: &LiveProxy,
) -> reqwest::RequestBuilder {
    let nominated = connection_header_names(headers);
    for (name, value) in headers {
        if is_hop_by_hop(name)
            || nominated.contains(name)
            || *name == header::HOST
            || *name == header::CONTENT_LENGTH
            || *name == header::ACCEPT_ENCODING
            || *name == header::IF_MATCH
            || *name == header::IF_NONE_MATCH
            || *name == header::IF_MODIFIED_SINCE
            || *name == header::IF_UNMODIFIED_SINCE
        {
            continue;
        }
        let rewritten = rewrite_request_header(name, value, proxy);
        builder = builder.header(name, rewritten);
    }
    builder.header(header::ACCEPT_ENCODING, "identity")
}

fn rewrite_request_header(
    name: &HeaderName,
    value: &HeaderValue,
    proxy: &LiveProxy,
) -> HeaderValue {
    if let Ok(text) = value.to_str() {
        let suffix = if *name == header::ORIGIN && text == proxy.proxy_origin {
            Some("")
        } else if *name == header::REFERER {
            text.strip_prefix(&proxy.proxy_origin)
                .filter(|suffix| suffix.is_empty() || suffix.starts_with('/'))
        } else {
            None
        };
        if let Some(suffix) = suffix {
            return HeaderValue::from_str(&format!("{}{}", proxy.upstream_origin, suffix))
                .unwrap_or_else(|_| value.clone());
        }
    }
    value.clone()
}

async fn proxy_response(mut upstream: reqwest::Response, proxy: &LiveProxy) -> Response<Body> {
    let status = upstream.status();
    if status.is_redirection()
        && let Some(location) = upstream.headers().get(header::LOCATION).cloned()
    {
        let response_url = upstream.url().clone();
        match classify_redirect(&location, &response_url, proxy) {
            Redirect::Relative => {}
            Redirect::SameOrigin(rewritten) => {
                upstream.headers_mut().insert(header::LOCATION, rewritten);
            }
            Redirect::External(destination) => {
                return external_navigation_page(&destination, &proxy.api_origin);
            }
        }
    }

    let is_html = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/html"));
    if !is_html {
        let mut response = Response::builder().status(status);
        copy_response_headers(
            response.headers_mut().expect("response headers"),
            upstream.headers(),
        );
        return response
            .body(Body::from_stream(upstream.bytes_stream()))
            .expect("valid proxied response");
    }

    if upstream.headers().contains_key(header::CONTENT_ENCODING) {
        return proxy_error(
            StatusCode::BAD_GATEWAY,
            "live upstream returned compressed HTML despite requesting identity encoding",
        );
    }

    let headers = upstream.headers().clone();
    let mut bytes = Vec::new();
    let mut stream = upstream.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                return proxy_error(
                    StatusCode::BAD_GATEWAY,
                    &format!("failed to read live upstream HTML: {error}"),
                );
            }
        };
        if bytes.len().saturating_add(chunk.len()) > MAX_HTML_BYTES {
            return proxy_error(
                StatusCode::BAD_GATEWAY,
                "live upstream HTML exceeds the 25 MiB rewrite limit",
            );
        }
        bytes.extend_from_slice(&chunk);
    }
    let source = match decode_html(&bytes, &headers) {
        Ok(source) => source,
        Err(message) => return proxy_error(StatusCode::BAD_GATEWAY, message),
    };
    let html = rewrite_html(&source, &proxy.api_origin, &proxy.upstream_origin);
    let mut response = Response::builder().status(status);
    let output_headers = response.headers_mut().expect("response headers");
    copy_response_headers(output_headers, &headers);
    for name in [
        header::CONTENT_LENGTH,
        header::CONTENT_SECURITY_POLICY,
        HeaderName::from_static("content-security-policy-report-only"),
        HeaderName::from_static("x-frame-options"),
        header::ETAG,
        header::LAST_MODIFIED,
    ] {
        output_headers.remove(name);
    }
    output_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    output_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    output_headers.insert(
        HeaderName::from_static("clear-site-data"),
        HeaderValue::from_static("\"storage\""),
    );
    response
        .body(Body::from(html))
        .expect("valid HTML response")
}

fn decode_html(bytes: &[u8], headers: &HeaderMap) -> std::result::Result<String, &'static str> {
    let header_label = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|content_type| {
            content_type.split(';').skip(1).find_map(|parameter| {
                let (name, value) = parameter.trim().split_once('=')?;
                name.eq_ignore_ascii_case("charset")
                    .then(|| value.trim().trim_matches(['\'', '"']))
            })
        });
    let bom_encoding = encoding_rs::Encoding::for_bom(bytes).map(|(encoding, _)| encoding);
    let meta_label = header_label
        .is_none()
        .then(|| sniff_meta_charset(bytes))
        .flatten();
    let encoding = if let Some(encoding) = bom_encoding {
        encoding
    } else {
        let label = header_label.or(meta_label.as_deref()).unwrap_or("utf-8");
        let Some(encoding) = encoding_rs::Encoding::for_label(label.as_bytes()) else {
            return Err("live upstream HTML declares an unsupported character encoding");
        };
        encoding
    };
    let (decoded, _, had_errors) = encoding.decode(bytes);
    if had_errors {
        return Err("live upstream HTML contains invalid bytes for its declared encoding");
    }
    Ok(decoded.into_owned())
}

fn sniff_meta_charset(bytes: &[u8]) -> Option<String> {
    let prefix = bytes
        .iter()
        .take(1024)
        .map(|byte| {
            if byte.is_ascii() {
                char::from(byte.to_ascii_lowercase())
            } else {
                ' '
            }
        })
        .collect::<String>();
    let mut cursor = 0;
    while let Some((start, end)) = find_start_tag(&prefix, "meta", cursor) {
        let tag = &prefix[start..end];
        if let Some(charset) = charset_label_in_meta(tag) {
            return Some(charset.to_string());
        }
        cursor = end;
    }
    None
}

fn charset_label_in_meta(tag: &str) -> Option<&str> {
    let charset = tag.find("charset")?;
    let mut rest = tag[charset + "charset".len()..].trim_start();
    rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest
        .as_bytes()
        .first()
        .copied()
        .filter(|byte| matches!(byte, b'\'' | b'"'));
    if quote.is_some() {
        rest = &rest[1..];
    }
    let end = rest
        .bytes()
        .position(|byte| {
            if let Some(quote) = quote {
                byte == quote
            } else {
                byte.is_ascii_whitespace() || matches!(byte, b';' | b'>' | b'/')
            }
        })
        .unwrap_or(rest.len());
    (end > 0).then_some(&rest[..end])
}

fn copy_response_headers(output: &mut HeaderMap, source: &HeaderMap) {
    let nominated = connection_header_names(source);
    for (name, value) in source {
        if !is_hop_by_hop(name) && !nominated.contains(name) {
            output.append(name, value.clone());
        }
    }
}

fn connection_header_names(headers: &HeaderMap) -> HashSet<HeaderName> {
    headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect()
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    *name == header::CONNECTION
        || *name == header::TRANSFER_ENCODING
        || *name == header::UPGRADE
        || name.as_str().eq_ignore_ascii_case("keep-alive")
        || name.as_str().eq_ignore_ascii_case("proxy-authenticate")
        || name.as_str().eq_ignore_ascii_case("proxy-authorization")
        || name.as_str().eq_ignore_ascii_case("te")
        || name.as_str().eq_ignore_ascii_case("trailer")
}

enum Redirect {
    Relative,
    SameOrigin(HeaderValue),
    External(String),
}

fn classify_redirect(location: &HeaderValue, response_url: &Url, proxy: &LiveProxy) -> Redirect {
    let Ok(location) = location.to_str() else {
        return Redirect::Relative;
    };
    let Ok(target) = response_url.join(location) else {
        return Redirect::External(location.to_string());
    };
    let ordinary_relative =
        Url::parse(location).is_err() && !location.starts_with("//") && !location.starts_with('\\');
    if ordinary_relative {
        return Redirect::Relative;
    }
    if target.origin() == proxy.upstream.origin() {
        let mut rewritten = format!("{}{}", proxy.proxy_origin, target.path());
        if let Some(query) = target.query() {
            rewritten.push('?');
            rewritten.push_str(query);
        }
        if let Some(fragment) = target.fragment() {
            rewritten.push('#');
            rewritten.push_str(fragment);
        }
        return HeaderValue::from_str(&rewritten)
            .map(Redirect::SameOrigin)
            .unwrap_or(Redirect::Relative);
    }
    Redirect::External(target.to_string())
}

fn external_navigation_page(destination: &str, parent_origin: &str) -> Response<Body> {
    let parent_origin_json = script_safe_json(parent_origin);
    let escaped = escape_html(destination);
    let is_http = Url::parse(destination)
        .ok()
        .is_some_and(|url| matches!(url.scheme(), "http" | "https"));
    let choice = if is_http {
        format!(
            "<p><a href=\"{escaped}\" target=\"_blank\" rel=\"noopener noreferrer\">{escaped}</a></p>"
        )
    } else {
        format!("<p>{escaped}</p>")
    };
    let notification = if is_http {
        let destination_json = script_safe_json(destination);
        format!(
            "<script>parent.postMessage({{type:'discuss:external-navigation',url:{destination_json}}},{parent_origin_json});</script>"
        )
    } else {
        String::new()
    };
    let html = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>External navigation blocked</title><body><h1>External navigation blocked</h1><p>This live review tried to leave its fixed upstream.</p>{choice}{notification}</body>"
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(html))
        .expect("valid external-navigation response")
}

fn rewrite_html(source: &str, api_origin: &str, upstream_origin: &str) -> String {
    let mut html = strip_csp_meta(source);
    inject_after_head(&mut html, SERVICE_WORKER_GUARD);
    let parent_origin = escape_html_attribute(api_origin);
    let upstream_origin = escape_html_attribute(upstream_origin);
    let inspector = format!(
        "<script src=\"{parent_origin}/assets/discuss-inspect.js?v=4\" data-discuss-parent-origin=\"{parent_origin}\" data-discuss-upstream-origin=\"{upstream_origin}\"></script>"
    );
    let lower = html.to_ascii_lowercase();
    if let Some(body_end) = lower.rfind("</body>") {
        html.insert_str(body_end, &inspector);
    } else {
        html.push_str(&inspector);
    }
    html
}

fn inject_after_head(html: &mut String, injection: &str) {
    if let Some((_, head_end)) = find_start_tag(html, "head", 0) {
        html.insert_str(head_end, injection);
    } else {
        html.insert_str(0, injection);
    }
}

fn strip_csp_meta(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    while let Some((start, end)) = find_start_tag(source, "meta", cursor) {
        let tag = source[start..end].to_ascii_lowercase();
        if tag.contains("http-equiv") && tag.contains("content-security-policy") {
            output.push_str(&source[cursor..start]);
        } else {
            output.push_str(&source[cursor..end]);
        }
        cursor = end;
    }
    output.push_str(&source[cursor..]);
    output
}

fn find_start_tag(source: &str, tag_name: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let tag = tag_name.as_bytes();
    let mut cursor = from;
    while cursor < bytes.len() {
        let relative = bytes[cursor..].iter().position(|byte| *byte == b'<')?;
        let start = cursor + relative;
        let name_start = start + 1;
        let name_end = name_start + tag.len();
        if name_end <= bytes.len()
            && bytes[name_start..name_end].eq_ignore_ascii_case(tag)
            && bytes
                .get(name_end)
                .is_some_and(|byte| byte.is_ascii_whitespace() || matches!(byte, b'>' | b'/'))
            && let Some(end) = find_tag_end(bytes, name_end)
        {
            return Some((start, end));
        }
        cursor = name_start;
    }
    None
}

fn find_tag_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, byte) in bytes[from..].iter().copied().enumerate() {
        match (quote, byte) {
            (None, b'\'' | b'"') => quote = Some(byte),
            (Some(open), close) if open == close => quote = None,
            (None, b'>') => return Some(from + offset + 1),
            _ => {}
        }
    }
    None
}

async fn proxy_websocket(
    proxy: LiveProxy,
    websocket: WebSocketUpgrade,
    request: Request<Body>,
) -> Response<Body> {
    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let target = proxy.websocket_url(path_and_query);
    let mut upstream_request = match target.as_str().into_client_request() {
        Ok(request) => request,
        Err(error) => {
            return proxy_error(
                StatusCode::BAD_GATEWAY,
                &format!("invalid live upstream WebSocket URL: {error}"),
            );
        }
    };
    for name in [
        header::COOKIE,
        header::USER_AGENT,
        header::SEC_WEBSOCKET_PROTOCOL,
    ] {
        if let Some(value) = request.headers().get(&name) {
            upstream_request.headers_mut().insert(name, value.clone());
        }
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        upstream_request.headers_mut().insert(
            header::ORIGIN,
            rewrite_request_header(&header::ORIGIN, origin, &proxy),
        );
    }
    let (upstream, response) = match connect_async(upstream_request).await {
        Ok(connection) => connection,
        Err(error) => {
            return proxy_error(
                StatusCode::BAD_GATEWAY,
                &format!("live upstream WebSocket failed: {error}"),
            );
        }
    };
    let selected_protocol = response
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let websocket = if let Some(protocol) = selected_protocol {
        websocket.protocols([protocol])
    } else {
        websocket
    };
    websocket.on_upgrade(move |downstream| bridge_websocket(downstream, upstream))
}

async fn bridge_websocket(
    downstream: WebSocket,
    upstream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
) {
    let (mut downstream_tx, mut downstream_rx) = downstream.split();
    let (mut upstream_tx, mut upstream_rx) = upstream.split();
    let downstream_to_upstream = async {
        while let Some(Ok(message)) = downstream_rx.next().await {
            let close = matches!(message, AxumMessage::Close(_));
            if upstream_tx.send(to_tungstenite(message)).await.is_err() || close {
                break;
            }
        }
    };
    let upstream_to_downstream = async {
        while let Some(Ok(message)) = upstream_rx.next().await {
            let close = matches!(message, TungsteniteMessage::Close(_));
            if let Some(message) = to_axum(message)
                && downstream_tx.send(message).await.is_err()
            {
                break;
            }
            if close {
                break;
            }
        }
    };
    tokio::select! {
        _ = downstream_to_upstream => {}
        _ = upstream_to_downstream => {}
    }
}

fn to_tungstenite(message: AxumMessage) -> TungsteniteMessage {
    match message {
        AxumMessage::Text(text) => TungsteniteMessage::Text(text.to_string().into()),
        AxumMessage::Binary(bytes) => TungsteniteMessage::Binary(bytes),
        AxumMessage::Ping(bytes) => TungsteniteMessage::Ping(bytes),
        AxumMessage::Pong(bytes) => TungsteniteMessage::Pong(bytes),
        AxumMessage::Close(frame) => TungsteniteMessage::Close(frame.map(|frame| {
            tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: frame.code.into(),
                reason: frame.reason.to_string().into(),
            }
        })),
    }
}

fn to_axum(message: TungsteniteMessage) -> Option<AxumMessage> {
    match message {
        TungsteniteMessage::Text(text) => Some(AxumMessage::Text(text.to_string().into())),
        TungsteniteMessage::Binary(bytes) => Some(AxumMessage::Binary(bytes)),
        TungsteniteMessage::Ping(bytes) => Some(AxumMessage::Ping(bytes)),
        TungsteniteMessage::Pong(bytes) => Some(AxumMessage::Pong(bytes)),
        TungsteniteMessage::Close(frame) => Some(AxumMessage::Close(frame.map(|frame| {
            axum::extract::ws::CloseFrame {
                code: frame.code.into(),
                reason: frame.reason.to_string().into(),
            }
        }))),
        TungsteniteMessage::Frame(_) => None,
    }
}

fn proxy_error(status: StatusCode, message: &str) -> Response<Body> {
    (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        message.to_string(),
    )
        .into_response()
}

fn script_safe_json(value: &str) -> String {
    serde_json::to_string(value)
        .expect("strings always serialize")
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn escape_html_attribute(value: &str) -> String {
    escape_html(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy() -> LiveProxy {
        LiveProxy::new(
            Url::parse("https://example.test/start").unwrap(),
            "http://127.0.0.1:49153".to_string(),
            "http://127.0.0.1:49152".to_string(),
        )
        .unwrap()
    }

    #[test]
    fn target_is_fixed_to_upstream_origin_and_preserves_request_path_query() {
        let target = proxy().target_url("/api/example?value=1");
        assert_eq!(target.as_str(), "https://example.test/api/example?value=1");
    }

    #[test]
    fn rewrites_same_origin_redirect_and_surfaces_external_redirect() {
        let proxy = proxy();
        let response_url = Url::parse("https://example.test/source").unwrap();
        match classify_redirect(
            &HeaderValue::from_static("https://example.test/account?tab=1#profile"),
            &response_url,
            &proxy,
        ) {
            Redirect::SameOrigin(value) => assert_eq!(
                value,
                HeaderValue::from_static("http://127.0.0.1:49153/account?tab=1#profile")
            ),
            _ => panic!("same-origin redirect should be rewritten"),
        }
        assert!(matches!(
            classify_redirect(
                &HeaderValue::from_static("https://other.test/"),
                &response_url,
                &proxy
            ),
            Redirect::External(destination) if destination == "https://other.test/"
        ));
        assert!(matches!(
            classify_redirect(
                &HeaderValue::from_static("/relative"),
                &response_url,
                &proxy
            ),
            Redirect::Relative
        ));
        assert!(matches!(
            classify_redirect(
                &HeaderValue::from_static(r"\\other.test/escape"),
                &response_url,
                &proxy
            ),
            Redirect::External(destination) if destination == "https://other.test/escape"
        ));
    }

    #[test]
    fn html_rewrite_strips_csp_meta_and_injects_guard_before_app_scripts() {
        let rewritten = rewrite_html(
            "<html><head><meta http-equiv='Content-Security-Policy' content=\"script-src none\"><script src='/app.js'></script></head><body>ok</body></html>",
            "http://127.0.0.1:49152",
            "https://example.test",
        );
        assert!(
            !rewritten
                .to_ascii_lowercase()
                .contains("content-security-policy")
        );
        assert!(
            rewritten.find("data-discuss-service-worker-guard").unwrap()
                < rewritten.find("/app.js").unwrap()
        );
        assert!(rewritten.contains("data-discuss-parent-origin=\"http://127.0.0.1:49152\""));
        assert!(rewritten.contains("data-discuss-upstream-origin=\"https://example.test\""));
        assert!(rewritten.contains("/assets/discuss-inspect.js?v=4"));

        let quoted = rewrite_html(
            "<head data-note=\"a > b\"><meta http-equiv=\"Content-Security-Policy\" content=\"script-src 'none'; report-uri='a>b'\"><script>app()</script></head>",
            "http://127.0.0.1:49152",
            "https://example.test",
        );
        assert!(
            !quoted
                .to_ascii_lowercase()
                .contains("content-security-policy")
        );
        assert!(
            quoted.find("data-discuss-service-worker-guard").unwrap()
                < quoted.find("app()").unwrap()
        );

        let headless = rewrite_html(
            "<script>app()</script><header>Title</header>",
            "http://127.0.0.1:49152",
            "https://example.test",
        );
        assert!(headless.starts_with(SERVICE_WORKER_GUARD));
    }

    #[test]
    fn html_decoding_honors_declared_non_utf8_charset() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=windows-1252"),
        );
        assert_eq!(
            decode_html(b"<p>caf\xe9</p>", &headers).unwrap(),
            "<p>caf\u{e9}</p>"
        );

        let meta_only =
            HeaderMap::from_iter([(header::CONTENT_TYPE, HeaderValue::from_static("text/html"))]);
        assert_eq!(
            decode_html(b"<meta charset=\"windows-1252\"><p>caf\xe9</p>", &meta_only).unwrap(),
            "<meta charset=\"windows-1252\"><p>caf\u{e9}</p>"
        );
    }

    #[test]
    fn connection_nominated_headers_are_removed() {
        let mut source = HeaderMap::new();
        source.insert(
            header::CONNECTION,
            HeaderValue::from_static("keep-alive, x-private"),
        );
        source.insert(
            HeaderName::from_static("x-private"),
            HeaderValue::from_static("secret"),
        );
        source.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let mut output = HeaderMap::new();
        copy_response_headers(&mut output, &source);
        assert!(!output.contains_key(header::CONNECTION));
        assert!(!output.contains_key("x-private"));
        assert_eq!(
            output.get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
    }

    #[test]
    fn proxy_origin_headers_are_rewritten_to_upstream_origin() {
        let proxy = proxy();
        let origin = rewrite_request_header(
            &header::ORIGIN,
            &HeaderValue::from_static("http://127.0.0.1:49153"),
            &proxy,
        );
        let referer = rewrite_request_header(
            &header::REFERER,
            &HeaderValue::from_static("http://127.0.0.1:49153/path?q=1"),
            &proxy,
        );
        assert_eq!(origin, "https://example.test");
        assert_eq!(referer, "https://example.test/path?q=1");
    }
}
