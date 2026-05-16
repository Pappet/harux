use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use harux::config::StoreConfig;
use harux::hl7::types::Hl7Message;
use harux::mllp::MllpStats;
use harux::store::MessageStore;
use harux::web::{create_router, AppState};
use tower::ServiceExt;

fn make_state() -> AppState {
    AppState {
        store: MessageStore::new(StoreConfig {
            max_messages: 1000,
            max_memory_mb: 10,
        }),
        stats: MllpStats::new(),
        mllp_port: 2575,
        max_connections: 10,
    }
}

fn simple_raw() -> String {
    "MSH|^~\\&|TEST|FAC|RECV|RFAC|20240101000000||ADT^A01|MSG001|P|2.5\r".to_string()
}

async fn collect_body(body: Body) -> axum::body::Bytes {
    axum::body::to_bytes(body, usize::MAX).await.unwrap()
}

// ── GET /api/messages ─────────────────────────────────────────────────────────

#[tokio::test]
async fn list_messages_empty_returns_empty_array() {
    let app = create_router(make_state());
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/messages")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = collect_body(res.into_body()).await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json, serde_json::json!([]));
}

#[tokio::test]
async fn list_messages_returns_one_summary() {
    let state = make_state();
    let mut msg = Hl7Message::new_empty(simple_raw(), "127.0.0.1".to_string());
    msg.id = "list-test".to_string();
    state.store.insert(msg).await;

    let app = create_router(state);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/messages")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = collect_body(res.into_body()).await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 1);
    assert_eq!(json[0]["id"], "list-test");
}

// ── GET /api/messages/{id} ────────────────────────────────────────────────────

#[tokio::test]
async fn get_message_known_id_returns_200() {
    let state = make_state();
    let mut msg = Hl7Message::new_empty(simple_raw(), "127.0.0.1".to_string());
    msg.id = "known".to_string();
    state.store.insert(msg).await;

    let app = create_router(state);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/messages/known")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = collect_body(res.into_body()).await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["id"], "known");
}

#[tokio::test]
async fn get_message_unknown_id_returns_404() {
    let app = create_router(make_state());
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/messages/no-such-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ── GET /api/search ───────────────────────────────────────────────────────────

#[tokio::test]
async fn search_returns_matching_message() {
    let state = make_state();
    let mut msg = Hl7Message::new_empty(simple_raw(), "127.0.0.1".to_string());
    msg.id = "srch-1".to_string();
    msg.message_type = "ADT^A01".to_string();
    state.store.insert(msg).await;

    let app = create_router(state);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/search?q=ADT%5EA01")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = collect_body(res.into_body()).await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn search_returns_empty_for_no_match() {
    let state = make_state();
    let mut msg = Hl7Message::new_empty(simple_raw(), "127.0.0.1".to_string());
    msg.id = "srch-2".to_string();
    msg.message_type = "ADT^A01".to_string();
    state.store.insert(msg).await;

    let app = create_router(state);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/search?q=NOMATCH_XYZ")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = collect_body(res.into_body()).await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 0);
}

// ── GET /api/stats ────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_stats_returns_expected_fields() {
    let app = create_router(make_state());
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = collect_body(res.into_body()).await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("total_messages").is_some());
    assert!(json.get("received").is_some());
    assert!(json.get("parsed_ok").is_some());
    assert!(json.get("parse_errors").is_some());
    assert!(json.get("mllp_port").is_some());
    assert_eq!(json["mllp_port"], 2575);
}

// ── POST /api/clear ───────────────────────────────────────────────────────────

#[tokio::test]
async fn clear_empties_the_store() {
    let state = make_state();
    let mut msg = Hl7Message::new_empty(simple_raw(), "127.0.0.1".to_string());
    msg.id = "to-clear".to_string();
    state.store.insert(msg).await;
    assert_eq!(state.store.count().await, 1);

    let app = create_router(state.clone());
    let res = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/clear")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(state.store.count().await, 0);
}

// ── POST /api/messages/{id}/tags ──────────────────────────────────────────────

#[tokio::test]
async fn add_tag_returns_ok() {
    let state = make_state();
    let mut msg = Hl7Message::new_empty(simple_raw(), "127.0.0.1".to_string());
    msg.id = "tag-test".to_string();
    state.store.insert(msg).await;

    let payload = serde_json::to_vec(&serde_json::json!({"tag": "urgent"})).unwrap();
    let app = create_router(state);
    let res = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/messages/tag-test/tags")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn add_empty_tag_returns_bad_request() {
    let state = make_state();
    let mut msg = Hl7Message::new_empty(simple_raw(), "127.0.0.1".to_string());
    msg.id = "empty-tag".to_string();
    state.store.insert(msg).await;

    let payload = serde_json::to_vec(&serde_json::json!({"tag": ""})).unwrap();
    let app = create_router(state);
    let res = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/messages/empty-tag/tags")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ── DELETE /api/messages/{id}/tags/{tag} ──────────────────────────────────────

#[tokio::test]
async fn remove_tag_round_trip() {
    let state = make_state();
    let mut msg = Hl7Message::new_empty(simple_raw(), "127.0.0.1".to_string());
    msg.id = "rmtag".to_string();
    state.store.insert(msg).await;
    state.store.add_tag("rmtag", "urgent".to_string()).await;

    let app = create_router(state);
    let res = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/messages/rmtag/tags/urgent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn remove_tag_unknown_message_returns_404() {
    let app = create_router(make_state());
    let res = app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/api/messages/no-such/tags/urgent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ── POST /api/messages/{id}/bookmark ─────────────────────────────────────────

#[tokio::test]
async fn bookmark_toggle_returns_bookmarked_true() {
    let state = make_state();
    let mut msg = Hl7Message::new_empty(simple_raw(), "127.0.0.1".to_string());
    msg.id = "bm-on".to_string();
    state.store.insert(msg).await;

    let app = create_router(state);
    let res = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/messages/bm-on/bookmark")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = collect_body(res.into_body()).await;
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["bookmarked"], true);
}

#[tokio::test]
async fn bookmark_toggle_unknown_id_returns_404() {
    let app = create_router(make_state());
    let res = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/messages/no-such/bookmark")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ── GET /api/export ───────────────────────────────────────────────────────────

#[tokio::test]
async fn export_starts_with_mllp_start_byte() {
    let state = make_state();
    let mut msg = Hl7Message::new_empty(simple_raw(), "127.0.0.1".to_string());
    msg.id = "export-1".to_string();
    state.store.insert(msg).await;

    let app = create_router(state);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/export")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = collect_body(res.into_body()).await;
    assert!(!body.is_empty());
    assert_eq!(body[0], 0x0B, "expected MLLP VT start byte");
    assert_eq!(body[body.len() - 2], 0x1C, "expected MLLP FS end byte");
    assert_eq!(body[body.len() - 1], 0x0D, "expected MLLP CR end byte");
}

#[tokio::test]
async fn export_empty_store_returns_empty_body() {
    let app = create_router(make_state());
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/export")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = collect_body(res.into_body()).await;
    assert!(body.is_empty());
}

// ── GET /ws ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn ws_endpoint_is_registered() {
    // oneshot cannot perform a real WebSocket upgrade (no underlying TCP connection).
    // We verify the route is registered: the handler runs and returns a WS-specific
    // response (not 404 "route missing" or 405 "method not allowed").
    let app = create_router(make_state());
    let res = app
        .oneshot(
            Request::builder()
                .uri("/ws")
                .header("connection", "upgrade")
                .header("upgrade", "websocket")
                .header("sec-websocket-version", "13")
                .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(res.status(), StatusCode::NOT_FOUND);
    assert_ne!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
}
