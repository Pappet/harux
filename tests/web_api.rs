use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use harux::config::{Config, StoreConfig};
use harux::hl7::types::Hl7Message;
use harux::mllp::MllpStats;
use harux::send::{SendSettings, Sender};
use harux::store::MessageStore;
use harux::web::{create_router, AppState};
use tower::ServiceExt;

fn make_state() -> AppState {
    state_with(Config::default())
}

/// The state for `config` (its `[send]` section matters; the store limits
/// are the tests' own).
fn state_with(config: Config) -> AppState {
    let store = MessageStore::new(StoreConfig {
        max_messages: 1000,
        max_memory_mb: 10,
        ..StoreConfig::default()
    });
    AppState {
        sender: Sender::new(SendSettings::from_config(&config).unwrap(), store.clone()),
        store,
        stats: MllpStats::new(),
        mllp_port: 2575,
        max_connections: 10,
        allowed_hosts: std::sync::Arc::new(vec!["harux.klinik.local".to_string()]),
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

/// Locks the detail JSON contract (#180, #185): segments carry no `raw`
/// copy; plain fields are `{index, value, description}`; fields with any
/// structure (repetition/component/sub-component separator or escape) add
/// `repetitions` — `[rep][comp][sub]`, escape-decoded, computed at
/// serialization; MSH-1/2 are never structured; the message carries its
/// `delimiters`.
#[tokio::test]
async fn get_message_json_shape_has_no_duplicate_payload() {
    let state = make_state();
    let raw = "MSH|^~\\&|APP|FAC|RCV|RFAC|20240101||ADT^A01|C1|P|2.5\rPID|||12345^^^HOSP&1.2&ISO~678||Smith^John||19800515|M||||O\\T\\Brien";
    let mut msg = harux::hl7::parser::parse_message(raw, "127.0.0.1:1").unwrap();
    msg.id = "shape".to_string();
    state.store.insert(msg).await;

    let app = create_router(state);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/messages/shape")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(&collect_body(res.into_body()).await).unwrap();

    let segments = json["segments"].as_array().unwrap();
    for seg in segments {
        let mut keys: Vec<&str> = seg
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["description", "fields", "name"], "segment keys");
    }
    let pid = segments.iter().find(|s| s["name"] == "PID").unwrap();
    let field = |i: u64| {
        pid["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["index"] == i)
            .unwrap()
    };
    // PID-7 (plain): value only.
    assert_eq!(field(7)["value"], "19800515");
    assert!(field(7).get("repetitions").is_none());
    // PID-5: one repetition, two components.
    assert_eq!(
        field(5)["repetitions"],
        serde_json::json!([[["Smith"], ["John"]]])
    );
    assert_eq!(field(5)["description"], "Patient Name");
    // PID-3: two repetitions, sub-components; `value` stays raw.
    assert_eq!(field(3)["value"], "12345^^^HOSP&1.2&ISO~678");
    assert_eq!(
        field(3)["repetitions"],
        serde_json::json!([[["12345"], [""], [""], ["HOSP", "1.2", "ISO"]], [["678"]]])
    );
    // PID-12: escape only — decoded in the structure, raw in `value`.
    assert_eq!(field(12)["value"], "O\\T\\Brien");
    assert_eq!(field(12)["repetitions"], serde_json::json!([[["O&Brien"]]]));
    // MSH-1/MSH-2 are the delimiters themselves, never structured.
    let msh = &segments[0]["fields"];
    assert_eq!(msh[1]["value"], "^~\\&");
    assert!(msh[0].get("repetitions").is_none() && msh[1].get("repetitions").is_none());
    // The old key is gone everywhere.
    assert!(!json.to_string().contains("\"components\""));
    assert_eq!(json["delimiters"], "|^~\\&");
    assert_eq!(json["patient_id"], "12345");
    // The message raw is still there for the UI to derive segment lines.
    assert_eq!(json["raw"], raw);
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

/// #195: the streamed export must be byte-identical to "VT raw FS CR" per
/// message, oldest first — across the coalescing path (many small messages
/// crossing the 64 KiB chunk boundary) and the zero-copy path (payloads
/// >= 64 KiB) — and announce the exact Content-Length.
#[tokio::test]
async fn export_streams_exact_mllp_framing_for_small_and_large_messages() {
    let state = make_state();
    let mut raws: Vec<String> = Vec::new();
    // 300 small (~1 KiB) messages, two large ones (100 KiB / 64 KiB exactly)
    // interleaved, so both chunking paths and their boundaries are exercised.
    for i in 0..300 {
        raws.push(format!("{}{}", simple_raw(), "x".repeat(1000 + i)));
        if i == 120 {
            raws.push(format!("{}{}", simple_raw(), "L".repeat(100 * 1024)));
        }
        if i == 250 {
            let pad = 64 * 1024 - simple_raw().len();
            raws.push(format!("{}{}", simple_raw(), "M".repeat(pad)));
        }
    }
    for (i, raw) in raws.iter().enumerate() {
        let mut msg = Hl7Message::new_empty(raw.clone(), "127.0.0.1".to_string());
        msg.id = format!("exp-{i}");
        state.store.insert(msg).await;
    }

    let mut expected = Vec::new();
    for raw in &raws {
        expected.push(0x0B);
        expected.extend_from_slice(raw.as_bytes());
        expected.extend_from_slice(&[0x1C, 0x0D]);
    }

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
    let content_length: usize = res.headers()[header::CONTENT_LENGTH]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    let body = collect_body(res.into_body()).await;
    assert_eq!(content_length, expected.len());
    assert_eq!(body.len(), expected.len());
    assert!(
        body[..] == expected[..],
        "export bytes differ from expected framing"
    );
}

/// The export works on a snapshot of payload `Arc`s: messages removed from
/// the store after the response was created (eviction, Clear) must still be
/// streamed in full.
#[tokio::test]
async fn export_survives_store_clear_while_streaming() {
    let state = make_state();
    let store = state.store.clone();
    let raws: Vec<String> = (0..50)
        .map(|i| format!("{}{}", simple_raw(), "y".repeat(2000 * (i % 50 + 1))))
        .collect();
    for (i, raw) in raws.iter().enumerate() {
        let mut msg = Hl7Message::new_empty(raw.clone(), "127.0.0.1".to_string());
        msg.id = format!("ev-{i}");
        store.insert(msg).await;
    }

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
    // The handler has taken its snapshot; empty the store before reading.
    store.clear().await;
    assert_eq!(store.count().await, 0);

    let body = collect_body(res.into_body()).await;
    let expected: usize = raws.iter().map(|r| r.len() + 3).sum();
    assert_eq!(body.len(), expected);
    assert_eq!(body[0], 0x0B);
    assert_eq!(&body[body.len() - 2..], &[0x1C, 0x0D]);
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

// ── Cross-site guard (#227) ───────────────────────────────────────────────────

fn ws_upgrade(origin: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .uri("/ws")
        .header("host", "10.1.2.3:8080")
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
    if let Some(o) = origin {
        b = b.header("origin", o);
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn ws_with_foreign_origin_is_refused() {
    let res = create_router(make_state())
        .oneshot(ws_upgrade(Some("https://evil.example")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn ws_same_origin_or_without_origin_passes_the_guard() {
    for origin in [
        Some("http://10.1.2.3:8080"),
        Some("https://harux.klinik.local"),
        None,
    ] {
        let res = create_router(make_state())
            .oneshot(ws_upgrade(origin))
            .await
            .unwrap();
        assert_ne!(res.status(), StatusCode::FORBIDDEN, "{origin:?}");
        assert_ne!(res.status(), StatusCode::NOT_FOUND, "{origin:?}");
    }
}

async fn clear_with(state: AppState, origin: Option<&str>) -> StatusCode {
    let mut b = Request::builder()
        .method(Method::POST)
        .uri("/api/clear")
        .header("host", "10.1.2.3:8080")
        .header("content-type", "text/plain");
    if let Some(o) = origin {
        b = b.header("origin", o);
    }
    create_router(state)
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn cross_origin_clear_is_refused_and_store_kept() {
    let state = make_state();
    state
        .store
        .insert(Hl7Message::new_empty(simple_raw(), "127.0.0.1".into()))
        .await;
    for origin in ["https://evil.example", "null", "http://10.1.2.3:9999"] {
        assert_eq!(
            clear_with(state.clone(), Some(origin)).await,
            StatusCode::FORBIDDEN,
            "{origin}"
        );
    }
    assert_eq!(state.store.count().await, 1, "store must not be cleared");

    assert_eq!(
        clear_with(state.clone(), Some("http://10.1.2.3:8080")).await,
        StatusCode::OK
    );
    assert_eq!(
        clear_with(state.clone(), None).await,
        StatusCode::OK,
        "curl/scripts"
    );
}

#[tokio::test]
async fn unknown_host_is_refused() {
    for (host, expected) in [
        ("attacker-controlled.example", StatusCode::FORBIDDEN),
        ("attacker-controlled.example:8080", StatusCode::FORBIDDEN),
        ("10.1.2.3:8080", StatusCode::OK),
        ("localhost:8080", StatusCode::OK),
        ("harux.klinik.local:8080", StatusCode::OK),
    ] {
        let res = create_router(make_state())
            .oneshot(
                Request::builder()
                    .uri("/api/messages")
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), expected, "{host}");
    }
}

#[tokio::test]
async fn security_headers_are_set() {
    let res = create_router(make_state())
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "127.0.0.1:8080")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let csp = res.headers()["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
}

/// #231: `include_bookmarks=true` adds bookmarks older than the page.
#[tokio::test]
async fn list_messages_can_include_older_bookmarks() {
    let state = make_state();
    for i in 0..5 {
        let mut msg = Hl7Message::new_empty(simple_raw(), "127.0.0.1".to_string());
        msg.id = format!("m{i}");
        state.store.insert(msg).await;
    }
    state.store.toggle_bookmark("m0").await;
    let ids = |uri: &'static str| {
        let app = create_router(state.clone());
        async move {
            let res = app
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let json: serde_json::Value =
                serde_json::from_slice(&collect_body(res.into_body()).await).unwrap();
            json.as_array()
                .unwrap()
                .iter()
                .map(|m| m["id"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(ids("/api/messages?limit=2").await, ["m4", "m3"]);
    for flag in ["true", "1", "yes", "TRUE"] {
        let uri: &'static str =
            Box::leak(format!("/api/messages?limit=2&include_bookmarks={flag}").into_boxed_str());
        assert_eq!(ids(uri).await, ["m4", "m3", "m0"], "{flag}");
    }
    assert_eq!(
        ids("/api/messages?limit=2&include_bookmarks=false").await,
        ["m4", "m3"]
    );
}

// ── #197 / #198: cursor paging and search like the UI filter ─────────────────

async fn get_json(state: &AppState, uri: &str) -> serde_json::Value {
    let res = create_router(state.clone())
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "{uri}");
    serde_json::from_slice(&collect_body(res.into_body()).await).unwrap()
}

fn ids(json: &serde_json::Value) -> Vec<String> {
    json.as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect()
}

/// Five messages m0 (oldest) … m4 (newest); m1 tagged, m3 with a missing
/// segment, m2 with a patient name that needs Unicode case folding.
async fn history_state() -> AppState {
    let state = make_state();
    for i in 0..5 {
        let mut msg = Hl7Message::new_empty(simple_raw(), "127.0.0.1".to_string());
        msg.id = format!("m{i}");
        msg.message_type = if i % 2 == 0 { "ADT^A01" } else { "ORU^R01" }.to_string();
        if i == 2 {
            msg.patient_name = Some("Müller, Jörg".to_string());
        }
        if i == 3 {
            msg.validation_warnings
                .push(harux::validation::ValidationWarning {
                    code: "MISSING_SEGMENT".into(),
                    message: "PV1 missing".into(),
                    segment: "PV1".into(),
                    field: None,
                });
        }
        if i == 4 {
            // A warning that is not a segment error: has:warnings, not has:errors.
            msg.validation_warnings
                .push(harux::validation::ValidationWarning {
                    code: "MISSING_FIELD".into(),
                    message: "PID-3 missing".into(),
                    segment: "PID".into(),
                    field: Some(3),
                });
        }
        state.store.insert(msg).await;
    }
    state.store.add_tag("m1", "Bug 1234".into()).await;
    state
}

#[tokio::test]
async fn messages_before_a_cursor_page_through_older_history() {
    let state = history_state().await;
    assert_eq!(
        ids(&get_json(&state, "/api/messages?limit=2").await),
        ["m4", "m3"]
    );
    // The next page continues below the oldest message of the last one —
    // unaffected by messages that arrive in between.
    let mut late = Hl7Message::new_empty(simple_raw(), "127.0.0.1".to_string());
    late.id = "late".into();
    state.store.insert(late).await;
    assert_eq!(
        ids(&get_json(&state, "/api/messages?before=m3&limit=2").await),
        ["m2", "m1"]
    );
    assert_eq!(
        ids(&get_json(&state, "/api/messages?before=m1&limit=2").await),
        ["m0"]
    );
    assert_eq!(
        ids(&get_json(&state, "/api/messages?before=m0&limit=2").await),
        Vec::<String>::new()
    );
    // An unknown (evicted) cursor gives nothing, not the newest page again.
    assert_eq!(
        ids(&get_json(&state, "/api/messages?before=gone&limit=2").await),
        Vec::<String>::new()
    );
    // `before` replaces `offset` / `include_bookmarks`: combining them is an
    // error, not a silently ignored parameter.
    for uri in [
        "/api/messages?before=m3&offset=1",
        "/api/messages?before=m3&include_bookmarks=true",
    ] {
        let res = create_router(state.clone())
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{uri}");
    }
}

#[tokio::test]
async fn search_matches_like_the_ui_filter() {
    let state = history_state().await;
    let q = |q: &str| format!("/api/search?q={q}");
    // Tags, as matchesSearch does.
    assert_eq!(ids(&get_json(&state, &q("bug%201234")).await), ["m1"]);
    // Unicode case folding: "MÜLLER" finds "Müller".
    assert_eq!(ids(&get_json(&state, &q("M%C3%9CLLER")).await), ["m2"]);
    // has:errors / has:warnings prefixes, with and without a term.
    assert_eq!(ids(&get_json(&state, &q("has:errors")).await), ["m3"]);
    assert_eq!(
        ids(&get_json(&state, &q("has:warnings")).await),
        ["m4", "m3"]
    );
    assert_eq!(
        ids(&get_json(&state, &q("has:warnings%20oru")).await),
        ["m3"]
    );
    assert_eq!(
        ids(&get_json(&state, &q("has:errors%20adt")).await),
        Vec::<String>::new()
    );
    // Newest first, and `before` continues below the last hit.
    assert_eq!(ids(&get_json(&state, &q("oru")).await), ["m3", "m1"]);
    assert_eq!(
        ids(&get_json(&state, &format!("{}&before=m3", q("oru"))).await),
        ["m1"]
    );
    assert_eq!(
        ids(&get_json(&state, &format!("{}&limit=1", q("adt"))).await),
        ["m4"]
    );
}

// ── Sending (#272) ────────────────────────────────────────────────────────────

/// A state with `send.enabled` (and whatever `edit` adds).
fn sending_state(edit: impl FnOnce(&mut Config)) -> AppState {
    let mut config = Config::default();
    config.send.enabled = true;
    edit(&mut config);
    state_with(config)
}

/// `POST /api/send` with `body`, from client 10.0.0.7 (the `ConnectInfo`
/// the real server provides), optionally with an `Origin`.
async fn post_send(state: &AppState, body: &str, origin: Option<&str>) -> (StatusCode, String) {
    let mut b = Request::builder()
        .method(Method::POST)
        .uri("/api/send")
        .header("host", "10.1.2.3:8080")
        .header("content-type", "application/json");
    if let Some(o) = origin {
        b = b.header("origin", o);
    }
    let client: std::net::SocketAddr = "10.0.0.7:50000".parse().unwrap();
    let res = create_router(state.clone())
        .layer(axum::extract::connect_info::MockConnectInfo(client))
        .oneshot(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let body = collect_body(res.into_body()).await;
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn stored_received(state: &AppState, id: &str) {
    let mut msg = harux::mllp::ingest(simple_raw().as_bytes(), "10.9.9.9:5000");
    msg.id = id.to_string();
    state.store.insert(msg).await;
}

/// A receiver on 127.0.0.1 that answers one frame with `AA`.
async fn ack_receiver() -> std::net::SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let mut buf = vec![0u8; 4096];
            let mut acc = Vec::new();
            while !acc.ends_with(&[0x1c, 0x0d]) {
                let Ok(n) = s.read(&mut buf).await else { break };
                if n == 0 {
                    break;
                }
                acc.extend_from_slice(&buf[..n]);
            }
            let _ = s
                .write_all(
                    b"\x0bMSH|^~\\&|R|RF|||20260101||ACK^A01|A|P|2.5\rMSA|AA|MSG001\r\x1c\x0d",
                )
                .await;
        }
    });
    addr
}

/// D11: without the switch every send route answers 403, and the stats say
/// sending is off.
#[tokio::test]
async fn send_routes_are_forbidden_while_sending_is_off() {
    let state = make_state();
    stored_received(&state, "m1").await;
    let (status, body) = post_send(
        &state,
        r#"{"target":"loopback","items":[{"message_id":"m1"}]}"#,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("send.enabled"), "{body}");
    let res = create_router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/send/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    // Runs (#280).
    for (method, uri) in [
        (Method::GET, "/api/send/runs/r1"),
        (Method::POST, "/api/send/runs/r1/cancel"),
    ] {
        let (status, _) = call(&state, method, uri).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
    let stats = get_json(&state, "/api/stats").await;
    assert_eq!(stats["sending_enabled"], false);
    assert_eq!(stats["sent"], 0);
    assert_eq!(state.store.count().await, 1, "nothing was stored");
}

#[tokio::test]
async fn send_config_describes_the_limits() {
    let state = sending_state(|c| {
        c.send.allowed_destinations = vec!["Orchestra.Local:6000-6999".into()];
    });
    let json = get_json(&state, "/api/send/config").await;
    assert_eq!(json["enabled"], true);
    assert_eq!(json["loopback"]["name"], "This Harux");
    assert_eq!(json["loopback"]["host"], "127.0.0.1");
    assert_eq!(json["loopback"]["port"], 2575);
    assert_eq!(
        json["allowed_destinations"],
        serde_json::json!(["orchestra.local:6000-6999"])
    );
    assert_eq!(json["max_run_store_percent"], 25);
    assert_eq!(
        json["tls"],
        serde_json::json!({"allow_unverified": false, "client_certificate": false})
    );
}

/// `curl -X POST /api/send` resends a stored message to a plain MLLP
/// target; the answer carries the sent message's ID and its delivery, and
/// the sent message records who asked for it.
#[tokio::test]
async fn send_a_stored_message_to_a_target() {
    let state = sending_state(|_| {});
    stored_received(&state, "m1").await;
    let addr = ack_receiver().await;
    let (status, body) = post_send(
        &state,
        &format!(
            r#"{{"target":{{"name":"fake receiver","host":"127.0.0.1","port":{}}},"items":[{{"message_id":"m1"}}]}}"#,
            addr.port()
        ),
        Some("http://10.1.2.3:8080"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["delivery"]["state"], "acked");
    assert_eq!(json["delivery"]["ack_code"], "AA");
    assert_eq!(json["delivery"]["control_id_matches"], true);
    let id = json["id"].as_str().unwrap();

    let detail = get_json(&state, &format!("/api/messages/{id}")).await;
    assert_eq!(detail["direction"], "out");
    assert_eq!(detail["source_addr"], addr.to_string());
    assert_eq!(detail["outbound"]["target"], "fake receiver");
    assert_eq!(detail["outbound"]["requested_by"], "10.0.0.7");
    assert_eq!(detail["outbound"]["origin"]["message_id"], "m1");
    let list = get_json(&state, "/api/messages").await;
    assert_eq!(list[0]["direction"], "out");
    assert_eq!(list[0]["target"], "fake receiver");
    assert_eq!(list[0]["delivery"]["ack_code"], "AA");
    // The spelling static/render.js reads (#285 review).
    assert_eq!(list[0]["transport"], "plain");
    assert_eq!(detail["outbound"]["transport"], "plain");
    assert!(list[0]["delivery"].get("response").is_none());
    assert_eq!(list[1]["direction"], "in");

    let stats = get_json(&state, "/api/stats").await;
    assert_eq!(stats["sending_enabled"], true);
    assert_eq!(stats["sent"], 1);
    assert_eq!(stats["send_rejected"], 0);
    assert_eq!(stats["send_failed"], 0);
    assert_eq!(stats["active_runs"], 0);
}

#[tokio::test]
async fn send_refusals_have_their_status_codes() {
    let state = sending_state(|c| {
        c.send.allowed_destinations = vec!["orchestra.local:6000".into()];
    });
    stored_received(&state, "m1").await;
    let cases = [
        ("not json", StatusCode::BAD_REQUEST),
        (
            r#"{"target":"loopback","items":[]}"#,
            StatusCode::BAD_REQUEST,
        ),
        // Runs (#280): the limits are checked before anything is stored.
        (
            r#"{"target":"loopback","items":[{"message_id":"m1"}],"repeat":0}"#,
            StatusCode::BAD_REQUEST,
        ),
        (
            r#"{"target":"loopback","items":[{"message_id":"m1"}],"repeat":1001}"#,
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            r#"{"target":"loopback","items":[{"message_id":"m1"}],"interval_ms":60001}"#,
            StatusCode::BAD_REQUEST,
        ),
        (
            r#"{"target":"loopback","items":[{"message_id":"m1"},{"message_id":"gone"}]}"#,
            StatusCode::NOT_FOUND,
        ),
        (
            r#"{"target":"loopback","items":[{"message_id":"m1"}],"loop":true}"#,
            StatusCode::BAD_REQUEST,
        ),
        // #275: overrides are checked before anything is stored.
        (
            r#"{"target":{"host":"orchestra.local","port":6000},"items":[{"message_id":"m1"}],"set":{"MSH-2":"x"}}"#,
            StatusCode::BAD_REQUEST,
        ),
        (
            r#"{"target":{"host":"orchestra.local","port":6000},"items":[{"message_id":"m1"}],"set":{"PID-5":"a|b"}}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            r#"{"target":{"host":"orchestra.local","port":6000},"items":[{"message_id":"m1"}],"set":{"MSH-10":"x\u001c"}}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            r#"{"target":{"host":"orchestra.local","port":6000},"items":[{"message_id":"m1"}],"set":{"PV1-2":"I"}}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            r#"{"target":{"host":"bad host","port":6000},"items":[{"message_id":"m1"}]}"#,
            StatusCode::BAD_REQUEST,
        ),
        (
            r#"{"target":{"host":"orchestra.local","port":6000,"tls":true,"verify":false},"items":[{"message_id":"m1"}]}"#,
            StatusCode::FORBIDDEN,
        ),
        (
            r#"{"target":{"host":"elsewhere.local","port":6000},"items":[{"message_id":"m1"}]}"#,
            StatusCode::FORBIDDEN,
        ),
        (
            r#"{"target":{"host":"orchestra.local","port":6001},"items":[{"message_id":"m1"}]}"#,
            StatusCode::FORBIDDEN,
        ),
        (
            r#"{"target":{"host":"orchestra.local","port":6000},"items":[{"message_id":"gone"}]}"#,
            StatusCode::NOT_FOUND,
        ),
    ];
    for (body, expect) in cases {
        let (status, text) = post_send(&state, body, None).await;
        assert_eq!(status, expect, "{body}: {text}");
    }
    assert_eq!(state.store.count().await, 1, "no refusal stores anything");
}

/// The origin guard covers the new state-changing route (#227).
#[tokio::test]
async fn cross_origin_send_is_refused() {
    let state = sending_state(|_| {});
    stored_received(&state, "m1").await;
    let (status, _) = post_send(
        &state,
        r#"{"target":"loopback","items":[{"message_id":"m1"}]}"#,
        Some("https://evil.example"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(state.store.count().await, 1);
}

/// The export keeps its meaning (received messages) unless asked otherwise.
#[tokio::test]
async fn export_by_direction() {
    let state = sending_state(|_| {});
    stored_received(&state, "m1").await;
    let addr = ack_receiver().await;
    let (status, _) = post_send(
        &state,
        &format!(
            r#"{{"target":{{"host":"127.0.0.1","port":{}}},"items":[{{"message_id":"m1"}}]}}"#,
            addr.port()
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let frames = |uri: &'static str| {
        let state = state.clone();
        async move {
            let res = create_router(state)
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let status = res.status();
            let body = collect_body(res.into_body()).await;
            (status, body.iter().filter(|&&b| b == 0x0b).count())
        }
    };
    assert_eq!(frames("/api/export").await, (StatusCode::OK, 1));
    assert_eq!(
        frames("/api/export?direction=in").await,
        (StatusCode::OK, 1)
    );
    assert_eq!(
        frames("/api/export?direction=out").await,
        (StatusCode::OK, 1)
    );
    assert_eq!(
        frames("/api/export?direction=all").await,
        (StatusCode::OK, 2)
    );
    assert_eq!(
        frames("/api/export?direction=sideways").await.0,
        StatusCode::BAD_REQUEST
    );
}

// ── Drafts (#276) ─────────────────────────────────────────────────────────────

/// A request to `uri` from client 10.0.0.7: `(status, body, content type)`.
/// `length: false` leaves `Content-Length` out (a body of unknown size).
async fn post_raw(
    state: &AppState,
    uri: &str,
    content_type: &str,
    body: Vec<u8>,
    length: bool,
) -> (StatusCode, Vec<u8>, String) {
    let mut b = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("host", "10.1.2.3:8080")
        .header("content-type", content_type);
    if length {
        b = b.header("content-length", body.len());
    }
    let client: std::net::SocketAddr = "10.0.0.7:50000".parse().unwrap();
    let res = create_router(state.clone())
        .layer(axum::extract::connect_info::MockConnectInfo(client))
        .oneshot(b.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    (status, collect_body(res.into_body()).await.to_vec(), ct)
}

async fn post_json(state: &AppState, uri: &str, body: serde_json::Value) -> (StatusCode, Vec<u8>) {
    let (status, body, _) = post_raw(
        state,
        uri,
        "application/json",
        body.to_string().into_bytes(),
        true,
    )
    .await;
    (status, body)
}

fn json_of(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(body)))
}

/// A received message from `bytes`, stored as `id`.
async fn store_bytes(state: &AppState, id: &str, bytes: &[u8]) -> std::sync::Arc<Hl7Message> {
    let mut msg = harux::mllp::ingest(bytes, "10.9.9.9:5000");
    msg.id = id.to_string();
    state.store.insert(msg).await;
    state.store.get_by_id(id).await.unwrap().message
}

fn mdm_bytes(attachment: &str) -> Vec<u8> {
    format!(
        "MSH|^~\\&|KIS|KLINIK|ORCH|RZ|20240101120000||MDM^T02|MDM1|P|2.5\rEVN|T02|20240101120000\rPID|||4711||Muster^Max\rPV1||I\rTXA|1|CN|TX|20240101120000\rOBX|1|ED|PDF^Report||^application^pdf^Base64^{attachment}||||||F\r"
    )
    .into_bytes()
}

#[tokio::test]
async fn draft_routes_are_forbidden_while_sending_is_off() {
    let state = make_state();
    stored_received(&state, "m1").await;
    let res = create_router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/messages/m1/draft")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    for uri in ["/api/parse", "/api/build"] {
        let (status, _) = post_json(&state, uri, serde_json::json!({"raw": simple_raw()})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
    }
}

/// The detail JSON names each field's datatype (#276), for stored messages
/// and previews alike.
#[tokio::test]
async fn the_detail_json_names_datatypes() {
    let state = sending_state(|_| {});
    store_bytes(
        &state,
        "m1",
        b"MSH|^~\\&|A|B|||20240101||ADT^A01|C1|P|2.5\rPID|||1||Muster^Max\rZZZ|x\r",
    )
    .await;
    let detail = get_json(&state, "/api/messages/m1").await;
    let field = |seg: usize, index: u64| {
        detail["segments"][seg]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["index"] == index)
            .unwrap()
            .clone()
    };
    assert_eq!(field(0, 7)["datatype"], "TS");
    assert_eq!(field(1, 5)["datatype"], "XPN");
    assert_eq!(
        field(2, 1)["datatype"],
        serde_json::Value::Null,
        "unknown segment"
    );
}

/// The roadmap's criterion with `curl` alone, for the MDM: a draft whose
/// attachment is a reference is made, previewed, changed with `set`,
/// built and sent — and the attachment arrives as it was stored.
#[tokio::test]
async fn an_mdm_draft_carries_its_attachment_as_a_reference() {
    let state = sending_state(|_| {});
    let attachment = "JVBERi0xLjQK".repeat(1000); // 12,000 characters
    let original = store_bytes(&state, "mdm", &mdm_bytes(&attachment)).await;

    let draft = get_json(&state, "/api/messages/mdm/draft").await;
    assert_eq!(draft["based_on"], "mdm");
    assert_eq!(draft["encoding"], "UTF-8");
    let raw = draft["raw"].as_str().unwrap().to_string();
    assert!(
        raw.contains("OBX|1|ED|PDF^Report||{{from:OBX-5}}||||||F"),
        "{raw}"
    );
    assert!(raw.len() < 1000, "the attachment stays on the server");
    assert_eq!(draft["references"][0]["path"], "OBX-5");
    assert_eq!(
        draft["references"][0]["bytes"],
        attachment.len() + "^application^pdf^Base64^".len()
    );

    // Preview: the text as written, positions, datatypes.
    let (status, body) = post_json(
        &state,
        "/api/parse",
        serde_json::json!({"raw": raw, "based_on": "mdm"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let p = json_of(&body);
    assert!(p["message"]["parse_error"].is_null());
    assert_eq!(p["message"]["message_type"], "MDM^T02");
    let obx = &p["message"]["segments"][5];
    assert_eq!(obx["fields"][4]["value"], "{{from:OBX-5}}");
    assert_eq!(obx["fields"][4]["datatype"], "VARIES");
    assert_eq!(p["positions"].as_array().unwrap().len(), 6);
    let pid5 = &p["positions"][2]["fields"][4];
    let units: Vec<u16> = raw.encode_utf16().collect();
    let (a, b) = (
        pid5[1].as_u64().unwrap() as usize,
        pid5[2].as_u64().unwrap() as usize,
    );
    assert_eq!(pid5[0], 5);
    assert_eq!(String::from_utf16(&units[a..b]).unwrap(), "Muster^Max");

    // Diff vs pinned: the references filled in compare as equal.
    let (status, body) = post_json(
        &state,
        "/api/parse",
        serde_json::json!({"raw": raw, "based_on": "mdm", "fill_references": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let filled = json_of(&body);
    assert_eq!(
        filled["message"]["segments"][5]["fields"][4]["value"],
        original.segments[5].fields[4].value
    );

    // Write a field back: the returned text is the new draft.
    let (status, body) = post_json(
        &state,
        "/api/parse",
        serde_json::json!({"raw": raw, "set": {"PID-5": "Doe^Jane", "MSH-10": "{{id}}"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let changed = json_of(&body);
    let changed_raw = changed["raw"].as_str().unwrap().to_string();
    assert!(changed_raw.contains("PID|||4711||Doe^Jane"));
    assert!(changed_raw.contains("||MDM^T02|{{id}}|P|"));
    assert_eq!(changed["changes"], serde_json::json!(["MSH-10", "PID-5"]));

    // Built unchanged: the bytes as stored.
    let (status, bytes, ct) = post_raw(
        &state,
        "/api/build",
        "application/json",
        serde_json::json!({"raw": raw, "based_on": "mdm"})
            .to_string()
            .into_bytes(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, "application/octet-stream");
    assert_eq!(bytes[0], 0x0b);
    assert!(bytes.ends_with(&[0x1c, 0x0d]));
    assert_eq!(&bytes[1..bytes.len() - 2], original.wire_bytes().as_ref());

    // Sent: the attachment arrives as stored, and the row says where the
    // draft came from.
    let addr = ack_receiver().await;
    let (status, body) = post_send(
        &state,
        &serde_json::json!({
            "target": {"host": "127.0.0.1", "port": addr.port()},
            "items": [{"raw": changed_raw, "based_on": "mdm"}],
        })
        .to_string(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = json_of(body.as_bytes())["id"].as_str().unwrap().to_string();
    let sent = get_json(&state, &format!("/api/messages/{id}")).await;
    assert_eq!(
        sent["outbound"]["origin"],
        serde_json::json!({"kind": "draft", "based_on": "mdm"})
    );
    let sent_raw = sent["raw"].as_str().unwrap();
    assert!(sent_raw.contains(&attachment));
    assert!(sent_raw.contains("PID|||4711||Doe^Jane\r"));
    assert!(
        !sent_raw.contains("{{"),
        "placeholders and references filled in"
    );
    let list = get_json(&state, "/api/messages").await;
    assert_eq!(list[0]["origin_id"], "mdm");

    // The original gone, the reference cannot be filled: refused, nothing
    // stored.
    state.store.clear().await;
    let (status, body) = post_send(
        &state,
        &serde_json::json!({
            "target": {"host": "127.0.0.1", "port": addr.port()},
            "items": [{"raw": raw, "based_on": "mdm"}],
        })
        .to_string(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("no longer stored"), "{body}");
    assert_eq!(state.store.count().await, 0);
}

/// The roadmap's criterion for the Latin-1 message: the draft keeps the
/// encoding, the build gives back the received bytes, and a file in
/// Latin-1 reaches the editor as text.
#[tokio::test]
async fn a_latin1_draft_keeps_its_bytes() {
    let state = sending_state(|_| {});
    let bytes = b"MSH|^~\\&|KIS|KLINIK|ORCH|RZ|20240101120000||ADT^A01|L1|P|2.5||||||8859/1\rEVN|A01|20240101120000\rPID|||1||M\xfcller^J\xf6rg\rPV1||I\r";
    store_bytes(&state, "l1", bytes).await;
    let draft = get_json(&state, "/api/messages/l1/draft").await;
    assert_eq!(draft["encoding"], "windows-1252");
    assert_eq!(draft["charset"], "8859/1");
    let raw = draft["raw"].as_str().unwrap();
    assert!(raw.contains("PID|||1||Müller^Jörg\nPV1"));

    let (status, built, _) = post_raw(
        &state,
        "/api/build",
        "application/json",
        serde_json::json!({"raw": raw, "charset": draft["encoding"]})
            .to_string()
            .into_bytes(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&built[1..built.len() - 2], bytes);

    // A character Latin-1 cannot write is refused, named.
    let (status, body, _) = post_raw(
        &state,
        "/api/build",
        "application/json",
        serde_json::json!({"raw": raw.replace("Müller", "Ωmega")})
            .to_string()
            .into_bytes(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(String::from_utf8_lossy(&body).contains("U+03A9"));

    // From a file: the bytes, MLLP-framed, decoded by the server.
    let mut file = vec![0x0b];
    file.extend_from_slice(bytes);
    file.extend_from_slice(&[0x1c, 0x0d]);
    let (status, body, _) =
        post_raw(&state, "/api/parse", "application/octet-stream", file, true).await;
    assert_eq!(status, StatusCode::OK);
    let decoded = json_of(&body);
    assert_eq!(decoded["encoding"], "windows-1252");
    assert_eq!(decoded["framed"], true);
    assert_eq!(decoded["raw"], raw);

    // Sent with the encoding the draft names.
    let addr = ack_receiver().await;
    let (status, body) = post_send(
        &state,
        &serde_json::json!({
            "target": {"host": "127.0.0.1", "port": addr.port()},
            "items": [{"raw": raw, "charset": "windows-1252"}],
        })
        .to_string(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let id = json_of(body.as_bytes())["id"].as_str().unwrap().to_string();
    let sent = state.store.get_by_id(&id).await.unwrap().message;
    assert_eq!(sent.wire_bytes().as_ref(), bytes);
    assert_eq!(
        get_json(&state, &format!("/api/messages/{id}")).await["outbound"]["origin"],
        serde_json::json!({"kind": "draft", "based_on": null})
    );
}

#[tokio::test]
async fn draft_refusals_have_their_status_codes() {
    let state = sending_state(|c| {
        c.send.allowed_destinations = vec!["orchestra.local:6000".into()];
    });
    let target = serde_json::json!({"host": "orchestra.local", "port": 6000});
    let send = |item: serde_json::Value| {
        serde_json::json!({"target": target, "items": [item]}).to_string()
    };
    let cases = [
        // Not HL7 without allow_invalid.
        (
            send(serde_json::json!({"raw": "hello"})),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        // Both or neither.
        (
            send(serde_json::json!({"raw": simple_raw(), "message_id": "m1"})),
            StatusCode::BAD_REQUEST,
        ),
        (send(serde_json::json!({})), StatusCode::BAD_REQUEST),
        // Draft fields on a stored message.
        (
            send(serde_json::json!({"message_id": "m1", "charset": "UTF-8"})),
            StatusCode::BAD_REQUEST,
        ),
        // A frame byte.
        (
            send(serde_json::json!({"raw": format!("{}\u{1c}", simple_raw())})),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        // An encoding Harux does not know.
        (
            send(serde_json::json!({"raw": simple_raw(), "charset": "klingon"})),
            StatusCode::BAD_REQUEST,
        ),
        // A reference without its message.
        (
            send(
                serde_json::json!({"raw": format!("{}OBX|1|ST|X||{{{{from:OBX-5}}}}", simple_raw())}),
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        // A reference to a message that is not stored.
        (
            send(
                serde_json::json!({"raw": format!("{}OBX|1|ST|X||{{{{from:OBX-5}}}}", simple_raw()), "based_on": "gone"}),
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        // An unknown item field.
        (
            send(serde_json::json!({"raw": simple_raw(), "repeat": 2})),
            StatusCode::BAD_REQUEST,
        ),
    ];
    for (body, expect) in cases {
        let (status, text) = post_send(&state, &body, None).await;
        assert_eq!(status, expect, "{body}: {text}");
    }
    assert_eq!(state.store.count().await, 0, "no refusal stores anything");

    // #289 review: a JSON body without its Content-Type is still a JSON
    // request — and a broken one is a 400, not a "file".
    let body = serde_json::json!({"raw": simple_raw()})
        .to_string()
        .into_bytes();
    for ct in ["text/plain", "application/octet-stream"] {
        let (status, body, _) = post_raw(&state, "/api/parse", ct, body.clone(), true).await;
        assert_eq!(status, StatusCode::OK, "{ct}");
        assert!(
            json_of(&body)["message"].is_object(),
            "{ct}: a preview, not a decoded file"
        );
    }
    let (status, _, _) = post_raw(
        &state,
        "/api/parse",
        "text/plain",
        b" {\"raw\": ".to_vec(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post_json(
        &state,
        "/api/parse",
        serde_json::json!({"raw": "x", "fill_references": true, "raw2": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unknown field");
    let (status, _) = post_json(
        &state,
        "/api/parse",
        serde_json::json!({"raw": "hello", "set": {"PID-5": "x"}}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "set needs an MSH");
    let res = create_router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/messages/gone/draft")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// A draft that does not parse goes out with `allow_invalid`, and is
/// stored with its parse error, like a broken received frame.
#[tokio::test]
async fn a_broken_draft_is_sent_with_allow_invalid() {
    let state = sending_state(|_| {});
    let addr = ack_receiver().await;
    let (status, body) = post_send(
        &state,
        &serde_json::json!({
            "target": {"host": "127.0.0.1", "port": addr.port()},
            "items": [{"raw": "MSH|broken"}],
            "allow_invalid": true,
        })
        .to_string(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let list = get_json(&state, "/api/messages").await;
    assert!(list[0]["parse_error"].is_string(), "{}", list[0]);
}

/// The body limit is twice `mllp.max_message_size_mb` plus 1 MB — above
/// axum's 2 MB default — and a large body needs a permit (`429` without).
#[tokio::test]
async fn message_routes_have_a_body_limit_and_permits() {
    let state = sending_state(|c| c.mllp.max_message_size_mb = 1);
    assert_eq!(harux::web::message_body_limit(1 << 20), 3 << 20);
    let preview = |len: usize| {
        let raw = format!("{}OBX|1|ST|X||{}", simple_raw(), "x".repeat(len));
        serde_json::json!({"raw": raw}).to_string().into_bytes()
    };
    let (status, _, _) = post_raw(
        &state,
        "/api/parse",
        "application/json",
        preview(2_500_000),
        true,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "above axum's default, below the limit"
    );
    let (status, _, _) = post_raw(
        &state,
        "/api/parse",
        "application/json",
        preview(3_200_000),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

    // The draft itself may not exceed the message limit.
    let (status, body) = post_json(
        &state,
        "/api/build",
        serde_json::json!({"raw": format!("{}OBX|1|ST|X||{}", simple_raw(), "x".repeat(1_100_000))}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        String::from_utf8_lossy(&body)
    );

    // Every permit taken: a large body, or one of unknown size, waits for none.
    let held: Vec<_> = (0..harux::send::LARGE_BODY_PERMITS)
        .map(|_| state.sender.large_body_permit().unwrap())
        .collect();
    let small = serde_json::json!({"raw": simple_raw()})
        .to_string()
        .into_bytes();
    let (status, _, _) = post_raw(
        &state,
        "/api/parse",
        "application/json",
        small.clone(),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "a small body needs no permit");
    let (status, _, _) = post_raw(
        &state,
        "/api/parse",
        "application/json",
        small.clone(),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "unknown size");
    let res = create_router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/parse")
                .header("host", "10.1.2.3:8080")
                .header("content-type", "application/json")
                .body(Body::from(small.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(res.headers()[header::RETRY_AFTER], "1", "#289 review");
    let (status, _, _) = post_raw(
        &state,
        "/api/parse",
        "application/json",
        preview(300_000),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    drop(held);
    let (status, _, _) = post_raw(
        &state,
        "/api/parse",
        "application/json",
        preview(300_000),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        state.sender.large_body_permit().is_some(),
        "the permit was given back"
    );
}

// ── Dictionary (#278) ─────────────────────────────────────────────────────────

#[tokio::test]
async fn the_dictionary_lists_a_segments_fields() {
    let state = make_state(); // reference data: not tied to send.enabled
    let pid = get_json(&state, "/api/dictionary/pid").await;
    assert_eq!(pid["name"], "PID");
    assert_eq!(pid["description"], "Patient Identification");
    let fields = pid["fields"].as_array().unwrap();
    assert!(fields.len() >= 30, "{}", fields.len());
    assert_eq!(fields[0]["seq"], 1);
    assert_eq!(
        fields[4],
        serde_json::json!({"seq": 5, "name": "Patient Name", "datatype": "XPN"})
    );
    let seqs: Vec<u64> = fields.iter().map(|f| f["seq"].as_u64().unwrap()).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "in order");
    let msh = get_json(&state, "/api/dictionary/MSH").await;
    assert_eq!(msh["fields"][6]["name"], "Date/Time Of Message");

    for (uri, status) in [
        ("/api/dictionary/ZZZ", StatusCode::NOT_FOUND),
        ("/api/dictionary/PIDX", StatusCode::BAD_REQUEST),
        ("/api/dictionary/P-D", StatusCode::BAD_REQUEST),
    ] {
        let res = create_router(state.clone())
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), status, "{uri}");
    }
}

// ── Templates (#279) ──────────────────────────────────────────────────────────

async fn get_status(state: &AppState, uri: &str) -> StatusCode {
    create_router(state.clone())
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn templates_are_listed_and_fetched_by_key_only() {
    assert_eq!(
        get_status(&make_state(), "/api/templates").await,
        StatusCode::FORBIDDEN,
        "part of sending"
    );

    let dir = std::env::temp_dir().join(format!("harux-web-templates-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Site-ADT.hl7"), "MSH|^~\\&|SITE|X|||{{now}}||ADT^A08|{{id}}|T|2.5\rEVN|A08\rPID|||1||Site^Patient\rPV1||I\r").unwrap();
    std::fs::write(dir.with_extension("secret.hl7"), "MSH|^~\\&|SECRET").unwrap();
    let state = sending_state(|c| c.send.templates_dir = Some(dir.to_string_lossy().into_owned()));

    let list = get_json(&state, "/api/templates").await;
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 9);
    assert_eq!(
        list[0],
        serde_json::json!({"id": "builtin:adt_a01", "name": "ADT^A01 Admit", "group": "Built in", "message_type": "ADT^A01"})
    );
    assert_eq!(
        list[8],
        serde_json::json!({"id": "site:Site-ADT.hl7", "name": "Site-ADT", "group": "Site", "message_type": "ADT^A08"})
    );

    let t = get_json(&state, "/api/templates/builtin:mdm_t02").await;
    assert_eq!(t["message_type"], "MDM^T02");
    assert!(t["raw"]
        .as_str()
        .unwrap()
        .starts_with("MSH|^~\\&|HARUX|TEST|RECEIVER|TEST|{{now}}||MDM^T02"));
    assert!(
        !t["raw"].as_str().unwrap().contains('\r'),
        "one segment per line (LF)"
    );
    let site = get_json(&state, "/api/templates/site:Site-ADT.hl7").await;
    assert_eq!(
        site["raw"],
        "MSH|^~\\&|SITE|X|||{{now}}||ADT^A08|{{id}}|T|2.5\nEVN|A08\nPID|||1||Site^Patient\nPV1||I"
    );

    // An ID is looked up, never joined into a path: nothing outside the
    // directory is ever returned, and every one of these is a 404
    // (`site:../x` has two segments: the /api fallback, #292 review).
    let secret = dir.with_extension("secret.hl7");
    let name = secret.file_name().unwrap().to_string_lossy();
    for (uri, status) in [
        (
            format!("/api/templates/site:..%2F{name}"),
            Some(StatusCode::NOT_FOUND),
        ),
        (
            format!("/api/templates/site:%2E%2E%2F{name}"),
            Some(StatusCode::NOT_FOUND),
        ),
        (
            "/api/templates/Site-ADT.hl7".to_string(),
            Some(StatusCode::NOT_FOUND),
        ),
        (
            "/api/templates/builtin:nope".to_string(),
            Some(StatusCode::NOT_FOUND),
        ),
        (
            format!("/api/templates/site:../{name}"),
            Some(StatusCode::NOT_FOUND),
        ),
    ] {
        let res = create_router(state.clone())
            .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        if let Some(expected) = status {
            assert_eq!(res.status(), expected, "{uri}");
        }
        let body = collect_body(res.into_body()).await;
        assert!(!String::from_utf8_lossy(&body).contains("SECRET"), "{uri}");
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(secret);
}

/// The roadmap's second criterion with the API: a template, filled out, is
/// sent, and the sent message names the template it came from.
#[tokio::test]
async fn a_template_is_filled_out_and_sent() {
    let state = sending_state(|_| {});
    let t = get_json(&state, "/api/templates/builtin:adt_a01").await;
    let (status, body) = post_json(
        &state,
        "/api/parse",
        serde_json::json!({"raw": t["raw"], "set": {"PID-5": "Doe^Jane", "PID-3": "999^^^TEST^MR"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let filled = json_of(&body);
    assert!(filled["message"]["validation_warnings"]
        .as_array()
        .unwrap()
        .is_empty());
    let addr = ack_receiver().await;
    let (status, body) = post_send(
        &state,
        &serde_json::json!({
            "target": {"host": "127.0.0.1", "port": addr.port()},
            "items": [{"raw": filled["raw"], "template": " ADT^A01 Admit\u{7}"}],
        })
        .to_string(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result = json_of(body.as_bytes());
    assert_eq!(
        result["delivery"]["ack_code"], "AA",
        "the response is there"
    );
    let sent = get_json(
        &state,
        &format!("/api/messages/{}", result["id"].as_str().unwrap()),
    )
    .await;
    assert_eq!(
        sent["outbound"]["origin"],
        serde_json::json!({"kind": "template", "name": "ADT^A01 Admit"})
    );
    assert_eq!(sent["patient_name"], "Doe, Jane");
    assert!(
        !sent["raw"].as_str().unwrap().contains("{{"),
        "placeholders filled in"
    );

    // `template` belongs to a draft.
    stored_received(&state, "m1").await;
    let (status, _) = post_send(
        &state,
        r#"{"target":"loopback","items":[{"message_id":"m1","template":"x"}]}"#,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// #292 review: an /api path no route takes is a 404 — never the SPA's
/// index.html, which a client would misread as an answer.
#[tokio::test]
async fn unknown_api_paths_are_404_not_the_spa() {
    let state = sending_state(|_| {});
    for uri in [
        "/api/templates/builtin:../../etc/passwd",
        "/api/templates/site:../x.hl7",
        "/api/nope",
        "/api/messages/a/b/c",
    ] {
        let res = create_router(state.clone())
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{uri}");
        let body = collect_body(res.into_body()).await;
        assert!(!String::from_utf8_lossy(&body).contains("<html"), "{uri}");
    }
    // A known route keeps its method check, and the SPA still serves pages.
    let res = create_router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/stats")
                .header("host", "10.1.2.3:8080")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(get_status(&state, "/").await, StatusCode::OK);
}

/// A request with a `Host` the guard allows; status and body as text.
async fn call(state: &AppState, method: Method, uri: &str) -> (StatusCode, String) {
    let res = create_router(state.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("host", "10.1.2.3:8080")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let body = collect_body(res.into_body()).await;
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// A run (#280): `202` with its status, progress from
/// `GET /api/send/runs/{id}`, cancel by `POST …/cancel`; unknown runs 404.
#[tokio::test]
async fn a_run_is_started_followed_and_cancelled() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // A receiver that answers AA to every frame.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut acc = Vec::new();
                let mut buf = [0u8; 4096];
                while let Ok(n) = s.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    acc.extend_from_slice(&buf[..n]);
                    while let Some(end) = acc.windows(2).position(|w| w == [0x1C, 0x0D]) {
                        acc.drain(..end + 2);
                        let ack =
                            b"\x0bMSH|^~\\&|R|F|H|H|20260101||ACK|A|P|2.5\rMSA|AA|MSG001\r\x1c\r";
                        let _ = s.write_all(ack).await;
                    }
                }
            });
        }
    });
    let state = sending_state(|_| {});
    stored_received(&state, "m1").await;
    let body = format!(
        r#"{{"target":{{"name":"t","host":"127.0.0.1","port":{port}}},"items":[{{"message_id":"m1"}}],"repeat":3}}"#
    );
    let (status, text) = post_send(&state, &body, None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{text}");
    let run: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(run["total"], 3);
    assert_eq!(run["target"], "t");
    assert_eq!(run["requested_by"], "10.0.0.7");
    let id = run["id"].as_str().unwrap().to_string();

    let mut last = serde_json::Value::Null;
    for _ in 0..300 {
        let (status, text) = call(&state, Method::GET, &format!("/api/send/runs/{id}")).await;
        assert_eq!(status, StatusCode::OK);
        last = serde_json::from_str(&text).unwrap();
        if last["state"] != "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(last["state"], "done", "{last}");
    assert_eq!(last["done"], 3);
    assert_eq!(last["codes"]["AA"], 3);
    assert!(
        last.get("latency_sum").is_none(),
        "internal counters stay inside"
    );
    // Three sent rows besides the received one.
    assert_eq!(state.store.count().await, 4);

    // Cancel of a finished run: its status, unchanged.
    let (status, text) = call(&state, Method::POST, &format!("/api/send/runs/{id}/cancel")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(text.contains(r#""state":"done""#), "{text}");
    for (method, uri) in [
        (Method::GET, "/api/send/runs/nope"),
        (Method::POST, "/api/send/runs/nope/cancel"),
    ] {
        assert_eq!(
            call(&state, method, uri).await.0,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
    // Cancel changes something: a GET is not enough (#227).
    assert_eq!(
        call(&state, Method::GET, &format!("/api/send/runs/{id}/cancel"))
            .await
            .0,
        StatusCode::METHOD_NOT_ALLOWED
    );
}

// ── POST /api/import (#281) ──────────────────────────────────────────────────

/// A request body that arrives in the given pieces, as a browser streams a
/// file.
struct Pieces(std::collections::VecDeque<Vec<u8>>);

impl futures_core::Stream for Pieces {
    type Item = Result<Vec<u8>, std::io::Error>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::task::Poll::Ready(self.0.pop_front().map(Ok))
    }
}

async fn post_import(state: &AppState, name: &str, pieces: Vec<Vec<u8>>) -> (StatusCode, String) {
    let res = create_router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/import?name={name}"))
                .header("host", "10.1.2.3:8080")
                .header("content-type", "application/octet-stream")
                .body(Body::from_stream(Pieces(pieces.into())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let body = collect_body(res.into_body()).await;
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn export_of(state: &AppState) -> Vec<u8> {
    let res = create_router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/export?direction=all")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    collect_body(res.into_body()).await.to_vec()
}

/// The milestone's criterion: what `/api/export` writes loads back — every
/// message as received from `import:<file>`, byte for byte (an export of the
/// import is the same file), in its order — and can be resent.
#[tokio::test]
async fn an_export_is_imported_byte_for_byte_and_resent() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let source = sending_state(|_| {});
    let frames: Vec<Vec<u8>> = vec![
        b"MSH|^~\\&|A|B|||20260101||ADT^A01|I1|P|2.5\rPID|||1||Muster^Max\r".to_vec(),
        // Latin-1, declared in MSH-18.
        b"MSH|^~\\&|A|B|||20260101||ADT^A08|I2|P|2.5||||||8859/1\rPID|||2||M\xfcller\r".to_vec(),
        b"NOT HL7 AT ALL".to_vec(),
        format!(
            "MSH|^~\\&|A|B|||20260101||MDM^T02|I4|P|2.5\rOBX|1|ED|PDF||^application^pdf^Base64^{}\r",
            "QUJD".repeat(40_000)
        )
        .into_bytes(),
    ];
    for f in &frames {
        source
            .store
            .insert(harux::mllp::ingest(f, "10.9.9.9:5000"))
            .await;
    }
    let file = export_of(&source).await;

    let state = sending_state(|_| {});
    // In awkward pieces: a frame end split between two of them.
    let pieces: Vec<Vec<u8>> = file.chunks(7_777).map(<[u8]>::to_vec).collect();
    let (status, text) = post_import(&state, "../exports/harux-export.hl7", pieces).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let summary: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(summary["imported"], 4);
    assert_eq!(summary["parse_errors"], 1);
    assert_eq!(summary["truncated"], 0);
    assert_eq!(summary["skipped_bytes"], 0);
    assert_eq!(summary["bytes"], file.len());
    assert_eq!(summary["source"], "import:harux-export.hl7");

    assert_eq!(
        export_of(&state).await,
        file,
        "the same bytes, in the same order"
    );
    let list = get_json(&state, "/api/messages?limit=10").await;
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["source_addr"] == "import:harux-export.hl7" && m["direction"] == "in"));
    assert_eq!(
        list[0]["message_control_id"], "I4",
        "newest last in the file"
    );

    // Resent: the Latin-1 message goes out as it arrived.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut acc = Vec::new();
        let mut buf = [0u8; 4096];
        while !acc.ends_with(&[0x1C, 0x0D]) {
            let n = s.read(&mut buf).await.unwrap();
            acc.extend_from_slice(&buf[..n]);
        }
        let _ = tx.send(acc[1..acc.len() - 2].to_vec());
        let _ = s
            .write_all(b"\x0bMSH|^~\\&|R|F|H|H|20260101||ACK|A|P|2.5\rMSA|AA|I2\r\x1c\r")
            .await;
    });
    let latin = list
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["message_control_id"] == "I2")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, text) = post_send(
        &state,
        &format!(
            r#"{{"target":{{"host":"127.0.0.1","port":{port}}},"items":[{{"message_id":"{latin}"}}]}}"#
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(rx.recv().await.unwrap(), frames[1], "byte for byte");
}

/// Refusals and edges: sending off (403), a file without a frame (422,
/// nothing stored), a file larger than the store (413), a file that ends
/// inside a frame, an over-limit frame kept cut off.
#[tokio::test]
async fn import_refusals_and_edges() {
    let off = make_state();
    let frame = b"\x0bMSH|^~\\&|A|B|||20260101||ADT^A01|E1|P|2.5\r\x1c\r".to_vec();
    assert_eq!(
        post_import(&off, "a.hl7", vec![frame.clone()]).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(off.store.count().await, 0);

    // An empty file is refused like an unframed one (#294 review).
    let empty_state = sending_state(|_| {});
    assert_eq!(
        post_import(&empty_state, "empty.hl7", vec![]).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let state = sending_state(|c| c.mllp.max_message_size_mb = 1);
    let (status, text) = post_import(
        &state,
        "msg.hl7",
        vec![b"MSH|^~\\&|A|B|||20260101||ADT^A01|U1|P|2.5\r".to_vec()],
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{text}");
    assert!(text.contains("From a file"), "{text}");
    assert_eq!(state.store.count().await, 0);

    // Ends inside a frame: the frames before it are loaded, the rest named.
    let mut cut = frame.clone();
    cut.extend_from_slice(b"\x0bMSH|^~\\&|A|B|||20260101||ADT^A01|E2");
    let (status, text) = post_import(&state, "cut.hl7", vec![cut]).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let summary: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(summary["imported"], 1);
    assert!(summary["skipped_bytes"].as_u64().unwrap() > 30);

    // An over-limit frame (1 MB here): stored cut off, the next one too.
    let mut big = vec![0x0B];
    big.extend_from_slice(b"MSH|^~\\&|A|B|||20260101||MDM^T02|BIG|P|2.5\rOBX|1|ED|X||");
    big.extend(std::iter::repeat_n(b'Q', 1024 * 1024 + 10));
    big.extend_from_slice(&[0x1C, 0x0D]);
    big.extend_from_slice(&frame);
    let pieces: Vec<Vec<u8>> = big.chunks(64 * 1024).map(<[u8]>::to_vec).collect();
    let (status, text) = post_import(&state, "big.hl7", pieces).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let summary: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        (summary["imported"].as_u64(), summary["truncated"].as_u64()),
        (Some(2), Some(1))
    );
    let list = get_json(&state, "/api/messages?limit=1&offset=1").await;
    assert_eq!(list[0]["message_control_id"], "BIG");
    assert_eq!(list[0]["truncated"], true);

    // Larger than the store (10 MB in these tests): announced → 413 at once;
    // streamed → 413 at the limit, nothing beyond it stored.
    let before = state.store.count().await;
    let res = create_router(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/import?name=huge.hl7")
                .header("host", "10.1.2.3:8080")
                .header("content-length", (11 * 1024 * 1024).to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let pieces: Vec<Vec<u8>> = (0..12).map(|_| vec![b' '; 1024 * 1024]).collect();
    let (status, text) = post_import(&state, "huge.hl7", pieces).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{text}");
    assert!(text.contains("store.max_memory_mb"), "{text}");
    assert_eq!(state.store.count().await, before);
}

/// #301: a preview grows with the draft — parse, validation, positions and
/// the answer's JSON — and runs off the async workers. On a one-thread
/// runtime a ticker keeps running while a large one is made; on the worker
/// it would stop for the whole preview (and with it the listener's ACKs).
#[tokio::test(flavor = "current_thread")]
async fn a_large_preview_runs_off_the_async_workers() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};
    let state = sending_state(|_| {});
    let mut raw = String::from("MSH|^~\\&|A|B|||20260101||ORU^R01|C1|P|2.5\nPID|||1||Doe^Jane\n");
    for i in 0..5000 {
        raw.push_str(&format!("OBX|{i}|ST|CODE^Name^L||value {i}|||N|||F\n"));
    }
    // Ticks of a 1 ms timer, and the longest time between two of them.
    let ticks = std::sync::Arc::new(AtomicU64::new(0));
    let longest_gap = std::sync::Arc::new(AtomicU64::new(0));
    let origin = Instant::now();
    let last_tick = std::sync::Arc::new(AtomicU64::new(0)); // µs since `origin`
    let ticker = tokio::spawn({
        let (ticks, longest_gap, last_tick) = (
            std::sync::Arc::clone(&ticks),
            std::sync::Arc::clone(&longest_gap),
            std::sync::Arc::clone(&last_tick),
        );
        async move {
            loop {
                tokio::time::sleep(Duration::from_millis(1)).await;
                let now = origin.elapsed().as_micros() as u64;
                let before = last_tick.swap(now, Ordering::Relaxed);
                longest_gap.fetch_max(now - before, Ordering::Relaxed);
                ticks.fetch_add(1, Ordering::Relaxed);
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(5)).await; // the ticker is running
    ticks.store(0, Ordering::Relaxed);
    longest_gap.store(0, Ordering::Relaxed);
    let started = Instant::now();
    let (status, body) = post_json(&state, "/api/parse", serde_json::json!({ "raw": raw })).await;
    let took = started.elapsed();
    // The time since the last tick counts too: a runtime that never ticked
    // during the request stopped for all of it.
    let tail = origin.elapsed().as_micros() as u64 - last_tick.load(Ordering::Relaxed);
    let gap = Duration::from_micros(longest_gap.load(Ordering::Relaxed).max(tail));
    let ticked = ticks.load(Ordering::Relaxed);
    ticker.abort();
    assert_eq!(status, StatusCode::OK);
    let json = json_of(&body);
    assert_eq!(json["message"]["segments"].as_array().unwrap().len(), 5002);
    assert!(
        took > Duration::from_millis(20),
        "the preview must take long enough to tell ({took:?})"
    );
    assert!(
        gap < took / 2 && ticked > 5,
        "the runtime stopped for {gap:?} of a {took:?} preview ({ticked} ticks): it ran on the async worker"
    );
}

/// A body that records whether anyone read it (#306).
struct Watched(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl futures_core::Stream for Watched {
    type Item = Result<Vec<u8>, std::io::Error>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        std::task::Poll::Ready(Some(Ok(vec![b'{'; 1024 * 1024])))
    }
}

/// #306: while sending is off, a route that takes a message answers `403`
/// before its body is read — no permit taken, no byte read — and every
/// route of sending is behind the same guard.
#[tokio::test]
async fn with_sending_off_a_large_body_is_refused_unread() {
    let state = make_state();
    for uri in [
        "/api/send",
        "/api/parse",
        "/api/build",
        "/api/import?name=a.hl7",
    ] {
        let read = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let res = create_router(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(uri)
                    .header("host", "10.1.2.3:8080")
                    .header("content-type", "application/json")
                    .header("content-length", (20 * 1024 * 1024).to_string())
                    .body(Body::from_stream(Watched(std::sync::Arc::clone(&read))))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{uri}");
        assert!(
            !read.load(std::sync::atomic::Ordering::SeqCst),
            "{uri}: the body was read"
        );
    }
    // Every permit is still free: none was taken for a refused request.
    let permits: Vec<_> = (0..harux::send::LARGE_BODY_PERMITS)
        .map(|_| state.sender.large_body_permit())
        .collect();
    assert!(permits.iter().all(Option::is_some), "a permit was kept");
    // The rest of sending answers 403 too; the dictionary is reference data.
    for (method, uri) in [
        (Method::GET, "/api/send/config"),
        (Method::GET, "/api/send/runs/r1"),
        (Method::POST, "/api/send/runs/r1/cancel"),
        (Method::GET, "/api/messages/m1/draft"),
        (Method::GET, "/api/templates"),
        (Method::GET, "/api/templates/t1"),
    ] {
        assert_eq!(
            call(&state, method, uri).await.0,
            StatusCode::FORBIDDEN,
            "{uri}"
        );
    }
    assert_eq!(
        call(&state, Method::GET, "/api/dictionary/PID").await.0,
        StatusCode::OK
    );
}
