use crate::mllp::MllpStats;
use crate::store::{MessageStore, StoreEvent};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rust_embed::Embed;
use serde::Deserialize;
use std::sync::atomic::Ordering;

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

#[derive(Embed)]
#[folder = "static/"]
struct StaticAssets;

#[derive(Clone)]
pub struct AppState {
    pub store: MessageStore,
    pub stats: MllpStats,
    pub mllp_port: u16,
    pub max_connections: usize,
}

async fn add_security_headers(req: axum::extract::Request, next: middleware::Next) -> Response {
    let mut res = next.run(req).await;
    res.headers_mut().insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(
            // No 'unsafe-inline' for scripts — blocks injected event handlers.
            // 'unsafe-inline' for styles is required for inline style= attributes.
            // Google Fonts domains remain until issue #142 (self-host fonts) is resolved.
            "default-src 'self'; \
             style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
             font-src https://fonts.gstatic.com; \
             connect-src 'self' ws: wss:",
        ),
    );
    res
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        // API routes
        .route("/api/messages", get(list_messages))
        .route("/api/messages/:id", get(get_message))
        .route("/api/search", get(search_messages))
        .route("/api/stats", get(get_stats))
        .route("/api/messages/:id/tags", axum::routing::post(add_tag))
        .route(
            "/api/messages/:id/tags/:tag",
            axum::routing::delete(remove_tag),
        )
        .route(
            "/api/messages/:id/bookmark",
            axum::routing::post(toggle_bookmark),
        )
        .route("/api/clear", axum::routing::post(clear_messages))
        .route("/api/export", get(export_raw_hl7))
        // WebSocket
        .route("/ws", get(ws_handler))
        // Static files (SPA)
        .fallback(get(static_handler))
        .with_state(state)
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
async fn export_raw_hl7(State(state): State<AppState>) -> impl IntoResponse {
    const MLLP_START: u8 = 0x0B;
    const MLLP_END_1: u8 = 0x1C;
    const MLLP_END_2: u8 = 0x0D;

    let raws = state.store.list_all_raw().await;
    let total: usize = raws.iter().map(|r| r.len() + 3).sum();
    let mut body = Vec::with_capacity(total);
    for raw in raws {
        body.push(MLLP_START);
        body.extend_from_slice(raw.as_bytes());
        body.push(MLLP_END_1);
        body.push(MLLP_END_2);
    }

    (
        [
            (axum::http::header::CONTENT_TYPE, "application/octet-stream"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"harux-export.hl7\"",
            ),
        ],
        body,
    )
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
        .send(Message::Text(payload.to_string()))
        .await
        .is_ok()
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    let mut rx = state.store.subscribe();

    // Send current stats on connect
    let count = state.store.count().await;
    let _ = socket
        .send(Message::Text(
            serde_json::json!({"type": "init", "total": count}).to_string(),
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
                    Ok(StoreEvent::Cleared) => {
                        if !send_ws_event(&mut socket, "cleared", None::<&()>).await {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        let _ = socket.send(Message::Text(
                            serde_json::json!({"type": "lagged", "missed": n}).to_string()
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
            (
                [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
                content.data.to_vec(),
            )
                .into_response()
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
