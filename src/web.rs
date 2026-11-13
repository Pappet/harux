use crate::hl7::types::{Direction, Hl7Message, WireForm};
use crate::mllp::{MllpStats, MLLP_END_1, MLLP_END_2, MLLP_START};
use crate::store::{MessageStore, StoreEvent};
use axum::body::{Body, Bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rust_embed::Embed;
use serde::Deserialize;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::task::{Context, Poll};

#[derive(serde::Serialize)]
struct StatsResponse {
    total_messages: usize,
    /// Bytes the store accounts for its messages (#226) …
    store_bytes: usize,
    /// … and the limit they are evicted against (`store.max_memory_mb`).
    store_max_bytes: usize,
    received: u64,
    parsed_ok: u64,
    parse_errors: u64,
    active_connections: u64,
    rejected_connections: u64,
    max_connections: usize,
    mllp_port: u16,
    /// Sending (#272): whether it is switched on, messages sent and failed
    /// since start or *Clear*, runs under way.
    sending_enabled: bool,
    /// Accepted (`AA`/`CA`) or written without awaiting a response.
    sent: u64,
    /// Answered with another code (`AE`, `AR`, …).
    send_rejected: u64,
    /// No usable answer (refused, timeout, no MSA, …).
    send_failed: u64,
    active_runs: usize,
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
    fn trailing_root_dot_is_ignored() {
        let allowed = vec!["harux.klinik.local".to_string()];
        assert!(host_allowed("harux.klinik.local.:8080", &allowed));
        assert!(host_allowed("localhost.:8080", &allowed));
        assert!(host_allowed("x.local", &["x.local.".to_string()]));
        assert!(origin_allowed(
            "http://harux.klinik.local.:8080",
            Some("10.0.0.1"),
            &allowed
        ));
    }

    #[test]
    fn allowed_hosts_are_validated() {
        let cfg = [
            "ok.local",
            "harux.klinik.local:8080",
            "http://x.local",
            "",
            "a b",
        ]
        .map(String::from);
        let problems = allowed_hosts_problems(&cfg);
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(problems[0].contains("harux.klinik.local:8080"));
        assert!(allowed_hosts_problems(&["harux.klinik.local".to_string()]).is_empty());
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
    /// Sending stored messages (#272); answers `403` while `send.enabled`
    /// is off.
    pub sender: crate::send::Sender,
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

/// A host name without one trailing dot: `harux.klinik.local.` (the fully
/// qualified form browsers and Windows accept) equals `harux.klinik.local`.
fn without_root_dot(host: &str) -> &str {
    host.strip_suffix('.').unwrap_or(host)
}

fn name_listed(host: &str, allowed: &[String]) -> bool {
    let host = without_root_dot(host);
    allowed
        .iter()
        .any(|a| without_root_dot(a.trim()).eq_ignore_ascii_case(host))
}

fn host_allowed(authority: &str, allowed: &[String]) -> bool {
    let host = host_part(authority.trim());
    host.parse::<std::net::IpAddr>().is_ok()
        || without_root_dot(host).eq_ignore_ascii_case("localhost")
        || name_listed(host, allowed)
}

/// Problems with `server.allowed_hosts` entries that would make them never
/// match, logged once at startup — otherwise the only symptom is a 403.
pub fn allowed_hosts_problems(allowed: &[String]) -> Vec<String> {
    allowed
        .iter()
        .filter_map(|h| {
            let h = h.trim();
            if h.is_empty() {
                Some("an empty entry".to_string())
            } else if h.contains(['/', ':']) || h.contains("://") {
                Some(format!(
                    "{h:?} — list host names only, without scheme, port or path"
                ))
            } else if h.contains(char::is_whitespace) {
                Some(format!("{h:?} — contains whitespace"))
            } else {
                None
            }
        })
        .collect()
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
        || name_listed(host_part(authority), allowed)
}

/// Every route that changes state must use a non-GET method:
/// `guard_cross_site` checks `Origin` only on non-GET requests (and `/ws`),
/// so a mutating `GET` would be callable from any website (#227).
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
        // Sending (#272) — `403` while `send.enabled` is off.
        .route("/api/send/config", get(send_config))
        .route("/api/send/runs/{id}", get(run_status))
        .route(
            "/api/send/runs/{id}/cancel",
            axum::routing::post(cancel_run),
        )
        .route("/api/messages/{id}/draft", get(message_draft))
        .route("/api/dictionary/{segment}", get(dictionary_segment))
        .route("/api/templates", get(list_templates))
        .route("/api/templates/{id}", get(get_template))
        // Routes with a message in the body (#276): a body limit above
        // axum's 2 MB default, and a permit before a large body is read.
        .route(
            "/api/send",
            large_body(&state, axum::routing::post(send_message)),
        )
        .route(
            "/api/parse",
            large_body(&state, axum::routing::post(parse_draft)),
        )
        .route(
            "/api/build",
            large_body(&state, axum::routing::post(build_draft)),
        )
        // Import (#281): streamed, so no body limit of axum's — the handler
        // counts — but the permit, held for the whole import.
        .route(
            "/api/import",
            axum::routing::post(import_file).layer::<_, Infallible>(
                middleware::from_fn_with_state(state.clone(), take_large_body_permit),
            ),
        )
        // An /api path no route takes is a 404, never the SPA (#292
        // review): a client must not read index.html as an API answer.
        .route("/api/{*rest}", axum::routing::any(api_not_found))
        // WebSocket
        .route("/ws", get(ws_handler))
        // Static files (SPA)
        .fallback(get(static_handler))
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(state, guard_cross_site))
        .layer(middleware::from_fn(add_security_headers))
}

/// The body limit of the routes that carry a message (#276): twice
/// `mllp.max_message_size_mb` plus 1 MB. A message inside a JSON string
/// grows by escaping (every `CR` becomes two characters, other control
/// characters six), so a limit equal to the message cap would refuse
/// messages the listener accepts. The size that counts is checked on the
/// decoded message.
pub fn message_body_limit(max_message_bytes: usize) -> usize {
    max_message_bytes
        .saturating_mul(2)
        .saturating_add(1024 * 1024)
}

/// `route` with the message body limit and the large-body permit (#276).
fn large_body(
    state: &AppState,
    route: axum::routing::MethodRouter<AppState>,
) -> axum::routing::MethodRouter<AppState> {
    route
        .layer::<_, Infallible>(middleware::from_fn_with_state(
            state.clone(),
            take_large_body_permit,
        ))
        .layer::<_, Infallible>(axum::extract::DefaultBodyLimit::max(message_body_limit(
            state.sender.max_message_bytes(),
        )))
}

/// Before a large body is read (above `send::LARGE_BODY_BYTES`, or of
/// unknown size), take a permit; none free → `429`. axum reads a body into
/// memory before the handler runs, so many large requests at once would
/// cost memory no size limit sees. The permit is held until the response.
async fn take_large_body_permit(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    let length = req
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if length.is_some_and(|l| l < crate::send::LARGE_BODY_BYTES) {
        return next.run(req).await;
    }
    match state.sender.large_body_permit() {
        Some(permit) => {
            let response = next.run(req).await;
            drop(permit);
            response
        }
        None => (
            StatusCode::TOO_MANY_REQUESTS,
            [(
                axum::http::header::RETRY_AFTER,
                HeaderValue::from_static("1"),
            )],
            "Too many large requests at once; try again shortly",
        )
            .into_response(),
    }
}

// --- API Handlers ---

#[derive(Deserialize)]
struct ListParams {
    offset: Option<usize>,
    limit: Option<usize>,
    /// Page through older history (#198): messages older than this ID,
    /// newest first. Replaces `offset`, which shifts as messages arrive.
    before: Option<String>,
    /// Also return every bookmarked message older than the page (#231).
    /// `true`, `1` and `yes` turn it on, so hand-written requests work too.
    include_bookmarks: Option<String>,
}

async fn list_messages(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(100).min(1000);
    if let Some(before) = params.before.as_deref() {
        // `before` replaces `offset` and pages plain history; combining them
        // would silently ignore one, so say so instead (#268 review).
        if params.offset.is_some() || params.include_bookmarks.is_some() {
            return (
                StatusCode::BAD_REQUEST,
                "`before` cannot be combined with `offset` or `include_bookmarks`",
            )
                .into_response();
        }
        return Json(state.store.older_than(Some(before), None, limit).await).into_response();
    }
    let include_bookmarks = matches!(
        params
            .include_bookmarks
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("true" | "1" | "yes")
    );
    let summaries = if include_bookmarks {
        state
            .store
            .list_summaries_with_bookmarks(offset, limit)
            .await
    } else {
        state.store.list_summaries(offset, limit).await
    };
    Json(summaries).into_response()
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
    /// Only messages older than this ID (#197): the UI searches the part of
    /// the store it hasn't loaded, continuing from its last hit.
    before: Option<String>,
}

async fn search_messages(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(100).min(1000);
    let query = crate::store::SearchQuery::parse(&params.q);
    let results = state
        .store
        .older_than(params.before.as_deref(), Some(&query), limit)
        .await;
    Json(results)
}

async fn get_stats(State(state): State<AppState>) -> impl IntoResponse {
    let (store_bytes, store_max_bytes) = state.store.memory_usage().await;
    let send = state.sender.counters();
    Json(StatsResponse {
        total_messages: state.store.count().await,
        store_bytes,
        store_max_bytes,
        received: state.stats.received.load(Ordering::Relaxed),
        parsed_ok: state.stats.parsed_ok.load(Ordering::Relaxed),
        parse_errors: state.stats.parse_errors.load(Ordering::Relaxed),
        active_connections: state.stats.active_connections.load(Ordering::Relaxed),
        rejected_connections: state.stats.rejected_connections.load(Ordering::Relaxed),
        max_connections: state.max_connections,
        mllp_port: state.mllp_port,
        sending_enabled: state.sender.enabled(),
        sent: send.sent,
        send_rejected: send.rejected,
        send_failed: send.failed,
        active_runs: state.sender.active_runs(),
    })
}

async fn clear_messages(State(state): State<AppState>) -> impl IntoResponse {
    state.store.clear().await;
    state.stats.reset_message_counters();
    state.sender.reset_counters();
    Json(serde_json::json!({"status": "cleared"}))
}

// --- Sending (#272) ---

fn sending_off() -> Response {
    (
        StatusCode::FORBIDDEN,
        crate::send::SendError::Disabled.message(),
    )
        .into_response()
}

async fn send_config(State(state): State<AppState>) -> Response {
    if !state.sender.enabled() {
        return sending_off();
    }
    Json(state.sender.config_response()).into_response()
}

/// `POST /api/send`: send stored messages or drafts to the target the
/// request names. One message: `200` with the sent message's ID and outcome.
/// Several (a run, #280): `202` with the run's status; its progress from
/// `GET /api/send/runs/{id}`. The requesting address comes from the
/// connection (`ConnectInfo`) and is recorded on every sent message; behind
/// a reverse proxy it is the proxy's.
async fn send_message(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    body: Bytes,
) -> Response {
    if !state.sender.enabled() {
        return sending_off();
    }
    let request: crate::send::SendRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Malformed send request: {e}"),
            )
                .into_response()
        }
    };
    match state.sender.start(request, peer.ip()).await {
        Ok(crate::send::SendOutcome::One(result)) => Json(result).into_response(),
        Ok(crate::send::SendOutcome::Run(status)) => {
            (StatusCode::ACCEPTED, Json(status)).into_response()
        }
        Err(e) => refused(e),
    }
}

/// `GET /api/send/runs/{id}` (#280): a run's progress and result, while the
/// server keeps it (every run under way, the newest finished ones).
async fn run_status(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !state.sender.enabled() {
        return sending_off();
    }
    match state.sender.run_status(&id) {
        Some(status) => Json(status).into_response(),
        None => (StatusCode::NOT_FOUND, "No such run").into_response(),
    }
}

/// `POST /api/send/runs/{id}/cancel` (#280): stop a run before its next
/// message. Answers the run's status; a finished run stays as it was.
async fn cancel_run(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !state.sender.enabled() {
        return sending_off();
    }
    match state.sender.cancel_run(&id) {
        Some(status) => Json(status).into_response(),
        None => (StatusCode::NOT_FOUND, "No such run").into_response(),
    }
}

/// A refusal as a response: its status and its reason as text.
fn refused(e: crate::send::SendError) -> Response {
    let status = StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, e.message()).into_response()
}

// --- Drafts (#276) ---

/// `GET /api/messages/{id}/draft`: a stored message as draft text, long
/// values as references (`draft::draft_of`). Part of sending: `403` while it
/// is off.
async fn message_draft(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !state.sender.enabled() {
        return sending_off();
    }
    let Some(msg) = state.sender.stored(&id).await else {
        return (StatusCode::NOT_FOUND, "Message not found").into_response();
    };
    // Linear in the message's size (#289 review), but up to
    // `mllp.max_message_size_mb` of text: off the async workers.
    match tokio::task::spawn_blocking(move || crate::draft::draft_of(&msg)).await {
        Ok(Ok(draft)) => Json(draft).into_response(),
        Ok(Err(e)) => refused(e),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Internal error while making the draft: {e}"),
        )
            .into_response(),
    }
}

/// `GET /api/dictionary/{segment}` (#278): a segment's fields from the
/// embedded v2.5.1 dictionary — number, name, datatype — for the draft
/// editor's *show empty fields*. Reference data only, so not tied to
/// `send.enabled`. `400` for a name that is not three letters or digits,
/// `404` for one the dictionary does not know.
async fn dictionary_segment(Path(segment): Path<String>) -> Response {
    let name = segment.trim().to_ascii_uppercase();
    if name.len() != 3 || !name.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return (
            StatusCode::BAD_REQUEST,
            "A segment name has three letters or digits",
        )
            .into_response();
    }
    let Some(def) = crate::dictionary::get_v251().segments.get(&name) else {
        return (
            StatusCode::NOT_FOUND,
            format!("{name} is not in the v2.5.1 dictionary"),
        )
            .into_response();
    };
    let mut fields: Vec<_> = def.fields.iter().filter(|f| f.seq >= 1).collect();
    fields.sort_by_key(|f| f.seq);
    Json(serde_json::json!({
        "name": name,
        "description": def.desc,
        "fields": fields
            .iter()
            .map(|f| serde_json::json!({"seq": f.seq, "name": f.desc, "datatype": f.datatype}))
            .collect::<Vec<_>>(),
    }))
    .into_response()
}

async fn api_not_found() -> Response {
    (StatusCode::NOT_FOUND, "No such API route").into_response()
}

// --- Templates (#279) ---

/// `GET /api/templates`: the built-in set and the site's `.hl7` files
/// (`send.templates_dir`, read now), with name, group and message type.
/// Part of sending: `403` while it is off.
async fn list_templates(State(state): State<AppState>) -> Response {
    if !state.sender.enabled() {
        return sending_off();
    }
    let dir = state.sender.templates_dir().map(|d| d.to_path_buf());
    match tokio::task::spawn_blocking(move || crate::templates::list(dir.as_deref())).await {
        Ok((templates, problems)) => {
            for p in &problems {
                tracing::warn!("Templates: {p}");
            }
            Json(templates).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Internal error: {e}"),
        )
            .into_response(),
    }
}

/// `GET /api/templates/{id}`: a template as draft text. The ID is looked up
/// in the listing, never joined into a path; anything else is `404`.
async fn get_template(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if !state.sender.enabled() {
        return sending_off();
    }
    let dir = state.sender.templates_dir().map(|d| d.to_path_buf());
    match tokio::task::spawn_blocking(move || crate::templates::get(dir.as_deref(), &id)).await {
        Ok(Some(t)) => Json(t).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "No such template").into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Internal error: {e}"),
        )
            .into_response(),
    }
}

fn is_json(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"))
        })
}

/// `POST /api/parse`. With `Content-Type: application/json`, or a body that
/// starts with `{`: the preview of a draft (`draft::ParseRequest` →
/// `draft::Preview`). With any other body: a file's bytes, decoded like a
/// received frame (`draft::DecodedFile`).
async fn parse_draft(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Response {
    if !state.sender.enabled() {
        return sending_off();
    }
    // A file is HL7 (`MSH…`) or MLLP-framed; a body that starts with `{`
    // is a JSON request whatever its `Content-Type` says (#289 review), and
    // answered `400` if it does not parse.
    if !is_json(&headers) && !body.trim_ascii_start().starts_with(b"{") {
        return Json(crate::draft::decode_file(&body)).into_response();
    }
    let request: crate::draft::ParseRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Malformed parse request: {e}"),
            )
                .into_response()
        }
    };
    drop(body);
    let based_on = if request.fill_references {
        match state
            .sender
            .based_on(request.based_on.as_deref(), &request.raw)
            .await
        {
            Ok(m) => m,
            Err(e) => return refused(e),
        }
    } else {
        None
    };
    match crate::draft::preview(
        &request.raw,
        &request.set,
        based_on.as_deref(),
        request.fill_references,
    ) {
        Ok(preview) => Json(preview).into_response(),
        Err(e) => refused(e),
    }
}

/// `POST /api/build`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildRequest {
    raw: String,
    #[serde(default)]
    charset: Option<String>,
    #[serde(default)]
    based_on: Option<String>,
    #[serde(default)]
    set: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct ImportQuery {
    /// The file's name, a label for `import:<name>`.
    #[serde(default)]
    name: Option<String>,
}

/// `POST /api/import?name=<file name>` (#281): load an MLLP-framed `.hl7`
/// file — what `/api/export` writes — into the store, every frame as
/// received from `import:<name>`. The body is the file's bytes, read as a
/// stream and split frame by frame by the listener's own splitter; nothing of
/// it is held beyond the frame in work. At most `store.max_memory_mb` (`413`;
/// frames before the limit stay stored). A file without a single frame is
/// refused (`422`): one unframed message is opened as a draft instead.
/// Part of sending: `403` while it is off.
async fn import_file(
    State(state): State<AppState>,
    Query(query): Query<ImportQuery>,
    request: axum::extract::Request,
) -> Response {
    use futures_core::Stream;
    if !state.sender.enabled() {
        return sending_off();
    }
    let (_, max_bytes) = state.store.memory_usage().await;
    let length = request
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let too_large = || {
        format!(
            "The file is larger than store.max_memory_mb ({} MB), what the store can hold",
            max_bytes / (1024 * 1024)
        )
    };
    if length.is_some_and(|l| l > max_bytes as u64) {
        return (StatusCode::PAYLOAD_TOO_LARGE, too_large()).into_response();
    }
    let mut importer = crate::mllp::Importer::new(
        query.name.as_deref().unwrap_or_default(),
        state.sender.max_message_bytes() / (1024 * 1024),
    );
    let mut body = request.into_body().into_data_stream();
    loop {
        let next = std::future::poll_fn(|cx| Pin::new(&mut body).poll_next(cx)).await;
        let chunk = match next {
            None => break,
            Some(Ok(chunk)) => chunk,
            Some(Err(e)) => {
                let s = importer.finish();
                tracing::warn!("Import into {} broke off: {e}", s.source);
                return (
                    StatusCode::BAD_REQUEST,
                    format!("The upload broke off after {} messages: {e}", s.imported),
                )
                    .into_response();
            }
        };
        // Checked before the piece is stored: nothing beyond the limit is.
        if importer.bytes() + chunk.len() as u64 > max_bytes as u64 {
            let s = importer.finish();
            tracing::warn!(
                "Import into {} stopped at store.max_memory_mb after {} messages",
                s.source,
                s.imported
            );
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("{}; stopped after {} messages", too_large(), s.imported),
            )
                .into_response();
        }
        importer.push(&chunk, &state.store).await;
    }
    let summary = importer.finish();
    if summary.imported == 0 && summary.bytes > 0 {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "The file holds no MLLP frame (what Export writes). A single message is opened as a \
             draft instead: New message → From a file",
        )
            .into_response();
    }
    tracing::info!(
        "Imported {} messages ({} bytes) as {}: {} did not parse, {} cut off, {} bytes outside frames not loaded",
        summary.imported,
        summary.bytes,
        summary.source,
        summary.parse_errors,
        summary.truncated,
        summary.skipped_bytes
    );
    Json(summary).into_response()
}

/// `POST /api/build`: *Save as file* — the bytes `POST /api/send` would
/// write for this draft, MLLP-framed like the export, as a download. A
/// browser writes text as UTF-8 only, so a Latin-1 draft is built here.
async fn build_draft(State(state): State<AppState>, body: Bytes) -> Response {
    if !state.sender.enabled() {
        return sending_off();
    }
    let request: BuildRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Malformed build request: {e}"),
            )
                .into_response()
        }
    };
    drop(body);
    let built = async {
        let (text, _) = crate::draft::apply_set(&request.raw, &request.set)?;
        let based_on = state
            .sender
            .based_on(request.based_on.as_deref(), &text)
            .await?;
        crate::draft::build_payload(
            &text,
            request.charset.as_deref(),
            based_on.as_deref(),
            1,
            state.sender.max_message_bytes(),
        )
    }
    .await;
    match built {
        Ok(bytes) => (
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/octet-stream"),
                ),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    HeaderValue::from_static("attachment; filename=\"harux-draft.hl7\""),
                ),
            ],
            crate::draft::framed(&bytes),
        )
            .into_response(),
        Err(e) => refused(e),
    }
}

#[derive(Deserialize)]
struct ExportParams {
    /// `in` (default: received messages, as before #272), `out` or `all`.
    direction: Option<String>,
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
///
/// `?direction=` (#272): `in` (default) exports received messages only, so
/// the export keeps its meaning once Harux sends; `out` the sent ones, `all`
/// both.
async fn export_raw_hl7(
    State(state): State<AppState>,
    Query(params): Query<ExportParams>,
) -> Response {
    let direction = match params.direction.as_deref().unwrap_or("in") {
        "in" => Some(Direction::In),
        "out" => Some(Direction::Out),
        "all" => None,
        other => {
            return (
                StatusCode::BAD_REQUEST,
                format!("direction {other:?}: use in, out or all"),
            )
                .into_response()
        }
    };
    let messages = state.store.payload_snapshot(direction).await;
    let content_length: usize = messages.iter().map(|m| m.wire_len() + 3).sum();

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
        .into_response()
}

/// Chunks of at most this size are built by copying small payloads; a payload
/// at least this large is sent zero-copy as its own chunk.
const EXPORT_COALESCE_BYTES: usize = 64 * 1024;

static MLLP_START_BYTE: [u8; 1] = [MLLP_START];
static MLLP_END_FRAME: [u8; 2] = [MLLP_END_1, MLLP_END_2];

/// Lets `Bytes::from_owner` borrow a stored message's received bytes
/// without copying them; the `Arc` keeps the message alive until the chunk
/// is written. Only for messages whose bytes are stored as such (UTF-8, or
/// a kept original) — re-encoded ones are built per chunk (#230).
struct RawPayload(Arc<Hl7Message>);

impl AsRef<[u8]> for RawPayload {
    fn as_ref(&self) -> &[u8] {
        debug_assert!(
            !matches!(self.0.wire, WireForm::Encoded(_)),
            "RawPayload used for a re-encoded message: its raw text is not its bytes"
        );
        match &self.0.wire {
            WireForm::Original(bytes) => bytes,
            _ => self.0.raw.as_bytes(),
        }
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
            // The bytes the message arrived as (#230), not its decoded text.
            if msg.wire_len() >= EXPORT_COALESCE_BYTES {
                self.flush_buf();
                self.ready.push_back(Bytes::from_static(&MLLP_START_BYTE));
                let payload = match msg.wire {
                    WireForm::Encoded(_) => Bytes::from(msg.wire_bytes().into_owned()),
                    _ => Bytes::from_owner(RawPayload(msg)),
                };
                self.ready.push_back(payload);
                self.ready.push_back(Bytes::from_static(&MLLP_END_FRAME));
            } else {
                if self.buf.capacity() == 0 {
                    self.buf.reserve(EXPORT_COALESCE_BYTES + 3);
                }
                self.buf.push(MLLP_START);
                self.buf.extend_from_slice(&msg.wire_bytes());
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

/// A WebSocket event frame: `{"type": …, "data": …}`, `data` omitted when
/// there is none.
#[derive(serde::Serialize)]
struct WsFrame<'a, T: serde::Serialize> {
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a T>,
}

fn frame_json<T: serde::Serialize>(kind: &str, data: Option<&T>) -> String {
    // Summaries and ID lists always serialize. If one ever doesn't, don't
    // panic in the WebSocket task: send `lagged`, which makes the UI reload
    // the list from the REST API and so recover the event's effect.
    serde_json::to_string(&WsFrame { kind, data }).unwrap_or_else(|e| {
        tracing::error!("Could not serialize the {kind} event: {e}");
        r#"{"type":"lagged","missed":1}"#.to_string()
    })
}

/// The wire format of a store event. Rendered once per event for all
/// subscribers through [`crate::store::EventFrame::cached_json`] (#196),
/// directly into a string instead of via a `serde_json::Value` tree.
pub fn ws_event_json(event: &StoreEvent) -> String {
    match event {
        StoreEvent::NewMessage(s) => frame_json("new_message", Some(&**s)),
        StoreEvent::TagsUpdated(s) => frame_json("tags_updated", Some(&**s)),
        StoreEvent::BookmarkToggled(s) => frame_json("bookmark_toggled", Some(&**s)),
        StoreEvent::DeliveryUpdated(s) => frame_json("delivery_updated", Some(&**s)),
        StoreEvent::Evicted(ids) => frame_json("evicted", Some(&**ids)),
        StoreEvent::Cleared => frame_json::<()>("cleared", None),
    }
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
                    Ok(frame) => {
                        // Rendered by the first subscriber, reused by the
                        // rest (#196); only the bytes are copied per socket.
                        let json = frame.cached_json(ws_event_json);
                        if socket.send(Message::Text(json.into())).await.is_err() {
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

#[cfg(test)]
mod ws_tests {
    use super::*;
    use crate::config::StoreConfig;
    use crate::hl7::parser::parse_message;

    /// #196: the frames keep the shape the UI reads (`static/ws.js`):
    /// `type` plus `data` (a summary, or the evicted IDs), no `data` for
    /// `cleared`.
    #[tokio::test]
    async fn ws_event_json_keeps_the_wire_format() {
        let store = MessageStore::new(StoreConfig::default());
        let mut rx = store.subscribe();
        let msg = parse_message(
            "MSH|^~\\&|A|FAC|R|RF|20240101||ADT^A01|C1|P|2.5\rPID|||1||Muster^Max",
            "127.0.0.1:1",
        )
        .unwrap();
        let id = msg.id.clone();
        store.insert(msg).await;
        store.add_tag(&id, "Bug 1".into()).await;
        store.toggle_bookmark(&id).await;
        store.clear().await;

        let mut frames = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            let json: serde_json::Value =
                serde_json::from_str(frame.cached_json(ws_event_json)).unwrap();
            frames.push(json);
        }
        let types: Vec<_> = frames.iter().map(|f| f["type"].as_str().unwrap()).collect();
        assert_eq!(
            types,
            ["new_message", "tags_updated", "bookmark_toggled", "cleared"]
        );
        assert_eq!(frames[0]["data"]["id"], id.as_str());
        assert_eq!(frames[0]["data"]["message_type"], "ADT^A01");
        assert_eq!(frames[1]["data"]["tags"], serde_json::json!(["Bug 1"]));
        assert_eq!(frames[2]["data"]["bookmarked"], true);
        assert!(frames[3].get("data").is_none(), "cleared has no data");

        let evicted = ws_event_json(&StoreEvent::Evicted(Arc::new(vec!["a".into(), "b".into()])));
        assert_eq!(evicted, r#"{"type":"evicted","data":["a","b"]}"#);
    }
}
