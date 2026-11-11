use crate::hl7::types::Hl7Message;
use crate::mllp::{MllpStats, MLLP_END_1, MLLP_END_2, MLLP_START};
use crate::store::{MessageStore, StoreEvent};
use axum::body::{Body, Bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rust_embed::Embed;
use serde::Deserialize;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::task::{Context, Poll};

#[derive(serde::Serialize)]
struct StatsResponse {
    total_messages: usize,
    received: u64,
    parsed_ok: u64,
    parse_errors: u64,
    active_connections: u64,
    rejected_connections: u64,
    max_connections: usize,
    mllp_port: u16,
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    #[test]
    fn host_rules() {
        let allowed = vec!["harux.klinik.local".to_string()];
        for ok in [
            "127.0.0.1:8080",
            "10.1.2.3",
            "[::1]:8080",
            "localhost:8080",
            "LOCALHOST",
            "Harux.Klinik.Local:8080",
        ] {
            assert!(host_allowed(ok, &allowed), "{ok}");
        }
        for bad in [
            "evil.example",
            "evil.example:8080",
            "harux.klinik.local.evil.example",
            "localhost.evil.example",
        ] {
            assert!(!host_allowed(bad, &allowed), "{bad}");
        }
    }

    #[test]
    fn origin_rules() {
        let allowed = vec!["harux.klinik.local".to_string()];
        let host = Some("10.1.2.3:8080");
        assert!(origin_allowed("http://10.1.2.3:8080", host, &allowed));
        assert!(origin_allowed("https://harux.klinik.local", host, &allowed));
        assert!(
            !origin_allowed("http://10.1.2.3:9999", host, &allowed),
            "other port"
        );
        assert!(!origin_allowed("https://evil.example", host, &allowed));
        assert!(!origin_allowed("null", host, &allowed));
        assert!(!origin_allowed("http://10.1.2.3:8080/x", host, &allowed));
        assert!(!origin_allowed("http://10.1.2.3:8080", None, &[]));
    }
}

#[derive(Embed)]
#[folder = "static/"]
struct StaticAssets;

#[derive(Clone)]
pub struct AppState {
    pub store: MessageStore,
    pub stats: MllpStats,
    pub mllp_port: u16,
    pub max_connections: usize,
    /// `server.allowed_hosts`: names accepted in `Host` besides IP literals
    /// and `localhost` (#227).
    pub allowed_hosts: Arc<Vec<String>>,
}

async fn add_security_headers(req: axum::extract::Request, next: middleware::Next) -> Response {
    let mut res = next.run(req).await;
    res.headers_mut().insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(
            // No 'unsafe-inline' for scripts or styles: all styles live in style.css;
            // initial display:none states use ID-scoped CSS rules, not style= attributes;
            // dynamic colors (source bar, source chips) are set via element.style in JS,
            // which is script execution — not governed by style-src.
            // Fonts are self-hosted under /fonts/ — no external font-src needed.
            // frame-ancestors: no other page may embed the UI (clickjacking).
            "default-src 'self'; \
             style-src 'self'; \
             font-src 'self'; \
             connect-src 'self' ws: wss:; \
             frame-ancestors 'none'",
        ),
    );
    res.headers_mut().insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    res
}

/// Refuse browser requests that another site makes on the user's behalf
/// (#227). Harux has no authentication, so "only reachable from the trusted
/// network" must also hold for pages a user in that network has open:
///
/// - **Host:** must be an IP literal, `localhost` or a name from
///   `server.allowed_hosts`. Blocks DNS rebinding, where a foreign name
///   resolves to the Harux server and the page reads the API as same-origin.
/// - **Origin:** on `/ws` and on every method that changes state, an
///   `Origin` header must name this server. Browsers don't apply the
///   same-origin policy to WebSockets, and a body-less `POST /api/clear` is
///   a "simple" request that needs no CORS preflight. Requests without
///   `Origin` (curl, scripts) stay allowed, so the API remains scriptable.
async fn guard_cross_site(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| req.uri().authority().map(|a| a.as_str().to_owned()));
    if let Some(host) = &host {
        if !host_allowed(host, &state.allowed_hosts) {
            return (
                StatusCode::FORBIDDEN,
                "Host not allowed. Open Harux by IP address, localhost, or a name \
                 listed in server.allowed_hosts.",
            )
                .into_response();
        }
    }

    let changes_state = !matches!(
        *req.method(),
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    );
    if changes_state || req.uri().path() == "/ws" {
        if let Some(origin) = req.headers().get(axum::http::header::ORIGIN) {
            let same = origin
                .to_str()
                .ok()
                .is_some_and(|o| origin_allowed(o, host.as_deref(), &state.allowed_hosts));
            if !same {
                return (StatusCode::FORBIDDEN, "Cross-origin request refused").into_response();
            }
        }
    }
    next.run(req).await
}

/// The host part of a `Host` header value (`name[:port]`, `[v6]:port`).
fn host_part(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    match authority.rsplit_once(':') {
        Some((h, port)) if port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => authority,
    }
}

fn host_allowed(authority: &str, allowed: &[String]) -> bool {
    let host = host_part(authority.trim());
    host.parse::<std::net::IpAddr>().is_ok()
        || host.eq_ignore_ascii_case("localhost")
        || allowed.iter().any(|a| a.eq_ignore_ascii_case(host))
}

/// `Origin` is `scheme://host[:port]`. It is accepted when its authority is
/// the request's own `Host` (the UI talking to its own server), or when its
/// host is an allowed name (a reverse proxy that rewrites `Host`).
/// `null` and anything unparsable are refused.
fn origin_allowed(origin: &str, host: Option<&str>, allowed: &[String]) -> bool {
    let Some(authority) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    if authority.is_empty() || authority.contains('/') {
        return false;
    }
    host.is_some_and(|h| h.trim().eq_ignore_ascii_case(authority))
        || allowed
            .iter()
            .any(|a| a.eq_ignore_ascii_case(host_part(authority)))
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        // API routes
        .route("/api/messages", get(list_messages))
        .route("/api/messages/{id}", get(get_message))
        .route("/api/search", get(search_messages))
        .route("/api/stats", get(get_stats))
        .route("/api/messages/{id}/tags", axum::routing::post(add_tag))
        .route(
            "/api/messages/{id}/tags/{tag}",
            axum::routing::delete(remove_tag),
        )
        .route(
            "/api/messages/{id}/bookmark",
            axum::routing::post(toggle_bookmark),
        )
        .route("/api/clear", axum::routing::post(clear_messages))
        .route("/api/export", get(export_raw_hl7))
        // WebSocket
        .route("/ws", get(ws_handler))
        // Static files (SPA)
        .fallback(get(static_handler))
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(state, guard_cross_site))
        .layer(middleware::from_fn(add_security_headers))
}

// --- API Handlers ---

#[derive(Deserialize)]
struct ListParams {
    offset: Option<usize>,
    limit: Option<usize>,
}

async fn list_messages(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(100).min(1000);
    let summaries = state.store.list_summaries(offset, limit).await;
    Json(summaries)
}

async fn get_message(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    // `Arc<Hl7Message>: Serialize` via serde's blanket impl — no deep clone,
    // and no intermediate `serde_json::Value` allocation.
    match state.store.get_by_id(&id).await {
        Some(msg) => Json(msg).into_response(),
        None => (StatusCode::NOT_FOUND, "Message not found").into_response(),
    }
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    limit: Option<usize>,
}

async fn search_messages(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(100).min(1000);
    let results = state.store.search(&params.q, limit).await;
    Json(results)
}

async fn get_stats(State(state): State<AppState>) -> impl IntoResponse {
    Json(StatsResponse {
        total_messages: state.store.count().await,
        received: state.stats.received.load(Ordering::Relaxed),
        parsed_ok: state.stats.parsed_ok.load(Ordering::Relaxed),
        parse_errors: state.stats.parse_errors.load(Ordering::Relaxed),
        active_connections: state.stats.active_connections.load(Ordering::Relaxed),
        rejected_connections: state.stats.rejected_connections.load(Ordering::Relaxed),
        max_connections: state.max_connections,
        mllp_port: state.mllp_port,
    })
}

async fn clear_messages(State(state): State<AppState>) -> impl IntoResponse {
    state.store.clear().await;
    state.stats.reset_message_counters();
    Json(serde_json::json!({"status": "cleared"}))
}

/// Stream every stored message as a single MLLP-framed payload.
///
/// Each message is wrapped in the original MLLP envelope (VT … FS CR), so the
/// resulting `.hl7` file can be replayed straight into an MLLP listener (e.g.
/// `nc host port < harux-export.hl7`) and the framing also serves as an
/// unambiguous message separator when opening the file in tools that
/// understand HL7.
///
/// The body is **streamed** (#195): the store lock is held only to snapshot
/// the payload `Arc`s, and frames are produced as the client reads. Large
/// payloads go out zero-copy straight from the stored message; small ones are
/// coalesced into ~64 KiB chunks. Peak extra memory is one chunk instead of
/// two full copies of the store.
async fn export_raw_hl7(State(state): State<AppState>) -> impl IntoResponse {
    let messages = state.store.payload_snapshot().await;
    let content_length: usize = messages.iter().map(|m| m.raw.len() + 3).sum();

    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/octet-stream".to_string(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"harux-export.hl7\"".to_string(),
            ),
            (
                axum::http::header::CONTENT_LENGTH,
                content_length.to_string(),
            ),
        ],
        Body::from_stream(MllpExportStream::new(messages)),
    )
}

/// Chunks of at most this size are built by copying small payloads; a payload
/// at least this large is sent zero-copy as its own chunk.
const EXPORT_COALESCE_BYTES: usize = 64 * 1024;

static MLLP_START_BYTE: [u8; 1] = [MLLP_START];
static MLLP_END_FRAME: [u8; 2] = [MLLP_END_1, MLLP_END_2];

/// Lets `Bytes::from_owner` borrow a stored message's raw payload without
/// copying it; the `Arc` keeps the message alive until the chunk is written.
struct RawPayload(Arc<Hl7Message>);

impl AsRef<[u8]> for RawPayload {
    fn as_ref(&self) -> &[u8] {
        self.0.raw.as_bytes()
    }
}

/// Yields the MLLP-framed export (`VT raw FS CR` per message, oldest first)
/// chunk by chunk. Consumed `Arc`s are dropped as the iterator advances.
struct MllpExportStream {
    messages: std::vec::IntoIter<Arc<Hl7Message>>,
    ready: VecDeque<Bytes>,
    buf: Vec<u8>,
}

impl MllpExportStream {
    fn new(messages: Vec<Arc<Hl7Message>>) -> Self {
        Self {
            messages: messages.into_iter(),
            ready: VecDeque::new(),
            buf: Vec::new(),
        }
    }

    fn flush_buf(&mut self) {
        if !self.buf.is_empty() {
            self.ready
                .push_back(Bytes::from(std::mem::take(&mut self.buf)));
        }
    }
}

impl futures_core::Stream for MllpExportStream {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Purely CPU-bound and never pending: each call emits the next chunk.
        // Fine for a store bounded by `store.max_memory_mb`; if exports ever
        // span far larger data, yield `Pending` periodically (or move framing
        // to `spawn_blocking`) so one export can't monopolize a worker thread.
        loop {
            if let Some(chunk) = self.ready.pop_front() {
                return Poll::Ready(Some(Ok(chunk)));
            }
            let Some(msg) = self.messages.next() else {
                self.flush_buf();
                return Poll::Ready(self.ready.pop_front().map(Ok));
            };
            if msg.raw.len() >= EXPORT_COALESCE_BYTES {
                self.flush_buf();
                self.ready.push_back(Bytes::from_static(&MLLP_START_BYTE));
                self.ready.push_back(Bytes::from_owner(RawPayload(msg)));
                self.ready.push_back(Bytes::from_static(&MLLP_END_FRAME));
            } else {
                if self.buf.capacity() == 0 {
                    self.buf.reserve(EXPORT_COALESCE_BYTES + 3);
                }
                self.buf.push(MLLP_START);
                self.buf.extend_from_slice(msg.raw.as_bytes());
                self.buf.extend_from_slice(&MLLP_END_FRAME);
                if self.buf.len() >= EXPORT_COALESCE_BYTES {
                    self.flush_buf();
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct AddTagPayload {
    tag: String,
}

async fn add_tag(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<AddTagPayload>,
) -> impl IntoResponse {
    let tag = payload.tag.trim().to_string();
    if tag.is_empty() {
        return (StatusCode::BAD_REQUEST, "Tag cannot be empty").into_response();
    }
    if state.store.add_tag(&id, tag).await {
        (StatusCode::OK, "Tag added").into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            "Message not found or tag already exists",
        )
            .into_response()
    }
}

async fn remove_tag(
    State(state): State<AppState>,
    Path((id, tag)): Path<(String, String)>,
) -> impl IntoResponse {
    if state.store.remove_tag(&id, &tag).await {
        (StatusCode::OK, "Tag removed").into_response()
    } else {
        (StatusCode::NOT_FOUND, "Message or tag not found").into_response()
    }
}

async fn toggle_bookmark(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.store.toggle_bookmark(&id).await {
        Some(bookmarked) => Json(serde_json::json!({"bookmarked": bookmarked})).into_response(),
        None => (StatusCode::NOT_FOUND, "Message not found").into_response(),
    }
}

// --- WebSocket ---

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn send_ws_event(
    socket: &mut WebSocket,
    event_type: &str,
    data: Option<&impl serde::Serialize>,
) -> bool {
    let payload = match data {
        Some(d) => serde_json::json!({"type": event_type, "data": d}),
        None => serde_json::json!({"type": event_type}),
    };
    socket
        .send(Message::Text(payload.to_string().into()))
        .await
        .is_ok()
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    let mut rx = state.store.subscribe();

    // Send current stats on connect
    let count = state.store.count().await;
    let _ = socket
        .send(Message::Text(
            serde_json::json!({"type": "init", "total": count})
                .to_string()
                .into(),
        ))
        .await;

    // Forward broadcast messages to WebSocket client
    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(StoreEvent::NewMessage(summary)) => {
                        if !send_ws_event(&mut socket, "new_message", Some(&*summary)).await {
                            break;
                        }
                    }
                    Ok(StoreEvent::TagsUpdated(summary)) => {
                        if !send_ws_event(&mut socket, "tags_updated", Some(&*summary)).await {
                            break;
                        }
                    }
                    Ok(StoreEvent::BookmarkToggled(summary)) => {
                        if !send_ws_event(&mut socket, "bookmark_toggled", Some(&*summary)).await {
                            break;
                        }
                    }
                    Ok(StoreEvent::Evicted(ids)) => {
                        if !send_ws_event(&mut socket, "evicted", Some(&*ids)).await {
                            break;
                        }
                    }
                    Ok(StoreEvent::Cleared) => {
                        if !send_ws_event(&mut socket, "cleared", None::<&()>).await {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        let _ = socket.send(Message::Text(
                            serde_json::json!({"type": "lagged", "missed": n}).to_string().into()
                        )).await;
                    }
                    Err(_) => break,
                }
            }
            // Also handle incoming WebSocket messages (ping/pong, close)
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {} // ignore other client messages for now
                }
            }
        }
    }
}

// --- Static File Serving ---

async fn static_handler(uri: axum::http::Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match StaticAssets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let mut res = (
                [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
                content.data.to_vec(),
            )
                .into_response();

            // Fonts are immutable for the lifetime of a binary build — cache aggressively.
            if path.starts_with("fonts/") {
                use std::fmt::Write as _;
                let hash = content.metadata.sha256_hash();
                let mut etag = String::with_capacity(66);
                etag.push('"');
                for b in &hash {
                    let _ = write!(etag, "{:02x}", b);
                }
                etag.push('"');
                let headers = res.headers_mut();
                headers.insert(
                    axum::http::header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=31536000, immutable"),
                );
                if let Ok(v) = HeaderValue::from_str(&etag) {
                    headers.insert(axum::http::header::ETAG, v);
                }
            }
            res
        }
        None => {
            // SPA fallback: serve index.html for unknown routes
            match StaticAssets::get("index.html") {
                Some(content) => {
                    Html(String::from_utf8_lossy(&content.data).to_string()).into_response()
                }
                None => (StatusCode::NOT_FOUND, "Not found").into_response(),
            }
        }
    }
}
