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
            ..StoreConfig::default()
        }),
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
