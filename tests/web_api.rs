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
        (
            r#"{"target":"loopback","items":[{"message_id":"m1"}],"set":{"MSH-10":"x"}}"#,
            StatusCode::BAD_REQUEST,
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
