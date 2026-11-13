//! Sending over real TCP (#272): the MLLP client against fake receivers on
//! `127.0.0.1:0` (wire), a resend to Harux's own listener (round trip), and
//! the bytes that leave for UTF-8, Latin-1 and an irreproducible payload.

use harux::config::{Config, MllpConfig, StoreConfig};
use harux::hl7::types::{DeliveryState, Direction, Hl7MessageSummary, WireForm};
use harux::mllp::{self, MllpStats};
use harux::mllp_client::{self, ExchangeError, Timeouts, MAX_RESPONSE_BYTES};
use harux::send::{
    ExplicitTarget, ItemRequest, SendCounters, SendError, SendRequest, SendSettings, Sender,
    TargetRequest,
};
use harux::store::MessageStore;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

const VT: u8 = 0x0B;
const FS: u8 = 0x1C;
const CR: u8 = 0x0D;
const CLIENT: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7));

fn timeouts() -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(2),
        write: Duration::from_secs(2),
        response: Duration::from_millis(500),
    }
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut f = vec![VT];
    f.extend_from_slice(payload);
    f.extend_from_slice(&[FS, CR]);
    f
}

fn ack(control_id: &str) -> Vec<u8> {
    frame(
        format!("MSH|^~\\&|R|RF|H|HF|20260101||ACK^A01|A1|P|2.5\rMSA|AA|{control_id}\r").as_bytes(),
    )
}

/// Read one MLLP frame's payload; `None` on EOF.
async fn read_frame(s: &mut TcpStream) -> Option<Vec<u8>> {
    let mut acc = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        if let Some(end) = acc.windows(2).position(|w| w == [FS, CR]) {
            let start = acc.iter().position(|&b| b == VT).unwrap();
            return Some(acc[start + 1..end].to_vec());
        }
        let n = s.read(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        acc.extend_from_slice(&buf[..n]);
    }
}

/// A receiver that runs `handle` for every connection and reports each
/// frame it read through the channel `handle` is given.
async fn fake<F, Fut>(handle: F) -> (SocketAddr, mpsc::UnboundedReceiver<Vec<u8>>)
where
    F: Fn(TcpStream, mpsc::UnboundedSender<Vec<u8>>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            tokio::spawn(handle(s, tx.clone()));
        }
    });
    (addr, rx)
}

/// A receiver that answers every frame with `AA` for its control ID.
async fn acking() -> (SocketAddr, mpsc::UnboundedReceiver<Vec<u8>>) {
    fake(|mut s, tx| async move {
        while let Some(payload) = read_frame(&mut s).await {
            let text = String::from_utf8_lossy(&payload).into_owned();
            let control = text.split('|').nth(9).unwrap_or("").to_string();
            let _ = tx.send(payload);
            let _ = s.write_all(&ack(&control)).await;
        }
    })
    .await
}

async fn connect(addr: SocketAddr) -> TcpStream {
    mllp_client::connect("127.0.0.1", addr.port(), Duration::from_secs(2))
        .await
        .unwrap()
}

async fn exchange(addr: SocketAddr) -> Result<Option<Vec<u8>>, ExchangeError> {
    let mut s = connect(addr).await;
    mllp_client::exchange(
        &mut s,
        b"MSH|^~\\&|A|B|||1||ADT^A01|C1|P|2.5\r",
        true,
        timeouts(),
    )
    .await
}

// ── Wire ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn response_in_one_read_and_in_many() {
    let (addr, _) = acking().await;
    let got = exchange(addr).await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&got).contains("MSA|AA|C1"));

    let (addr, _) = fake(|mut s, _| async move {
        read_frame(&mut s).await;
        for b in ack("C1") {
            s.write_all(&[b]).await.unwrap();
            s.flush().await.unwrap();
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    })
    .await;
    let got = exchange(addr).await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&got).ends_with("MSA|AA|C1\r"));
}

#[tokio::test]
async fn delayed_response_within_the_timeout() {
    let (addr, _) = fake(|mut s, _| async move {
        read_frame(&mut s).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = s.write_all(&ack("C1")).await;
    })
    .await;
    assert!(exchange(addr).await.unwrap().is_some());
}

#[tokio::test]
async fn no_response_times_out_and_close_is_reported() {
    let (addr, _) = fake(|mut s, _| async move {
        read_frame(&mut s).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    })
    .await;
    let e = exchange(addr).await.unwrap_err();
    assert_eq!(e.reason(), "timeout", "{e:?}");

    let (addr, _) = fake(|mut s, _| async move {
        read_frame(&mut s).await;
        let _ = s.write_all(&[VT, b'M', b'S']).await;
    })
    .await;
    assert!(matches!(exchange(addr).await, Err(ExchangeError::Closed)));
}

#[tokio::test]
async fn refused_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let e = mllp_client::connect("127.0.0.1", port, Duration::from_secs(2))
        .await
        .unwrap_err();
    assert!(matches!(e, ExchangeError::Refused), "{e:?}");
}

#[tokio::test]
async fn unframed_and_oversized_responses_are_refused() {
    let (addr, _) = fake(|mut s, _| async move {
        read_frame(&mut s).await;
        let _ = s.write_all(b"HTTP/1.1 400 Bad Request\r\n").await;
        tokio::time::sleep(Duration::from_secs(2)).await;
    })
    .await;
    // At once, not after the response timeout.
    let started = std::time::Instant::now();
    assert!(matches!(
        exchange(addr).await,
        Err(ExchangeError::NotFramed)
    ));
    assert!(started.elapsed() < Duration::from_millis(400));

    let (addr, _) = fake(|mut s, _| async move {
        read_frame(&mut s).await;
        let mut big = vec![VT];
        big.extend(std::iter::repeat_n(b'x', MAX_RESPONSE_BYTES + 10));
        let _ = s.write_all(&big).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
    })
    .await;
    assert!(matches!(exchange(addr).await, Err(ExchangeError::TooLarge)));
}

#[tokio::test]
async fn two_messages_on_one_connection() {
    let (addr, mut frames) = acking().await;
    let mut s = connect(addr).await;
    for id in ["C1", "C2"] {
        let msg = format!("MSH|^~\\&|A|B|||1||ADT^A01|{id}|P|2.5\r");
        let got = mllp_client::exchange(&mut s, msg.as_bytes(), true, timeouts())
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&got).contains(&format!("MSA|AA|{id}")));
        assert_eq!(frames.recv().await.unwrap(), msg.as_bytes());
    }
}

#[tokio::test]
async fn no_wait_without_await_response() {
    let (addr, mut frames) = fake(|mut s, tx| async move {
        if let Some(f) = read_frame(&mut s).await {
            let _ = tx.send(f);
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    })
    .await;
    let mut s = connect(addr).await;
    let started = std::time::Instant::now();
    let got = mllp_client::exchange(
        &mut s,
        b"MSH|^~\\&|A|B|||1||ACK|C1|P|2.5\r",
        false,
        timeouts(),
    )
    .await
    .unwrap();
    assert!(got.is_none());
    assert!(started.elapsed() < Duration::from_millis(400));
    assert!(frames.recv().await.is_some());
}

// ── Sender ────────────────────────────────────────────────────────────────

fn settings(edit: impl FnOnce(&mut Config)) -> SendSettings {
    let mut config = Config::default();
    config.send.enabled = true;
    edit(&mut config);
    let mut s = SendSettings::from_config(&config).unwrap();
    s.timeouts = timeouts();
    s
}

fn store() -> MessageStore {
    MessageStore::new(StoreConfig::default())
}

fn to(addr: SocketAddr, name: &str) -> TargetRequest {
    TargetRequest::Explicit(ExplicitTarget {
        name: Some(name.into()),
        host: addr.ip().to_string(),
        port: addr.port(),
        tls: false,
        verify: true,
    })
}

fn request(target: TargetRequest, id: &str) -> SendRequest {
    SendRequest {
        target,
        items: vec![ItemRequest {
            message_id: id.into(),
        }],
        await_response: true,
        allow_invalid: false,
    }
}

/// Store `bytes` as a received message, the way the listener does.
async fn received(store: &MessageStore, bytes: &[u8]) -> String {
    let msg = mllp::ingest(bytes, "10.1.1.1:5000");
    let id = msg.id.clone();
    store.insert(msg).await;
    id
}

async fn summaries(store: &MessageStore) -> Vec<Hl7MessageSummary> {
    let mut v = store.list_summaries(0, 100).await;
    v.reverse(); // oldest first
    v
}

/// Unchanged means byte for byte (#230): UTF-8, Latin-1 (re-encoded from
/// `raw`) and a payload no encoding reproduces (kept as received).
#[tokio::test]
async fn unchanged_resend_writes_the_received_bytes() {
    let (addr, mut frames) = acking().await;
    let store = store();
    let sender = Sender::new(settings(|_| {}), store.clone());
    let mut forms = Vec::new();
    for bytes in [
        "MSH|^~\\&|A|B|||20260101||ADT^A01|U1|P|2.5\rPID|||1||Müller^Jörg\r".as_bytes().to_vec(),
        b"MSH|^~\\&|A|B|||20260101||ADT^A01|L1|P|2.5||||||8859/1\rPID|||1||M\xfcller^J\xf6rg\r"
            .to_vec(),
        // Invalid for the declared charset: no encoding reproduces it.
        b"MSH|^~\\&|A|B|||20260101||ADT^A01|O1|P|2.5||||||BIG-5\rPID|||1||\x81\x20\r".to_vec(),
        // Above 64 KiB: written in three parts, from the stored message.
        format!(
            "MSH|^~\\&|A|B|||20260101||MDM^T02|M1|P|2.5\rOBX|1|ED|PDF||^application^pdf^Base64^{}\r",
            "QUJD".repeat(25_000)
        )
        .into_bytes(),
    ] {
        let id = received(&store, &bytes).await;
        let wire = store.get_by_id(&id).await.unwrap().message.wire.clone();
        let result = sender
            .send(request(to(addr, "fake"), &id), CLIENT)
            .await
            .unwrap();
        assert_eq!(result.delivery.state, DeliveryState::Acked);
        assert_eq!(frames.recv().await.unwrap(), bytes, "bytes on the wire");
        let sent = store.get_by_id(&result.id).await.unwrap().message;
        assert_eq!(sent.wire_bytes().as_ref(), &bytes[..], "stored sent bytes");
        forms.push(match wire {
            WireForm::Utf8 => "utf8",
            WireForm::Encoded(_) => "encoded",
            WireForm::Original(_) => "original",
        });
    }
    assert_eq!(forms, ["utf8", "encoded", "original", "utf8"]);
}

/// Round trip through Harux's own listener: the sent message is stored
/// before the received one, the response is `AA` and repeats the control ID.
#[tokio::test]
async fn round_trip_to_harux_itself() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let store = store();
    let (_shutdown, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(mllp::serve(
        listener,
        store.clone(),
        MllpStats::new(),
        rx,
        MllpConfig::default(),
    ));
    let sender = Sender::new(
        settings(|c| {
            c.server.mllp_bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
            c.server.mllp_port = addr.port();
        }),
        store.clone(),
    );
    let original = received(
        &store,
        b"MSH|^~\\&|A|B|||20260101||ADT^A01|RT1|P|2.5\rPID|||1\r",
    )
    .await;
    let result = sender
        .send(
            request(TargetRequest::Named("loopback".into()), &original),
            CLIENT,
        )
        .await
        .unwrap();
    assert_eq!(result.delivery.state, DeliveryState::Acked);
    assert_eq!(result.delivery.ack_code.as_deref(), Some("AA"));
    assert_eq!(result.delivery.control_id_matches, Some(true));
    assert!(result.delivery.response.unwrap().contains("MSA|AA|RT1"));

    let list = summaries(&store).await;
    let order: Vec<_> = list.iter().map(|s| s.direction).collect();
    assert_eq!(order, [Direction::In, Direction::Out, Direction::In]);
    assert_eq!(list[1].id, result.id);
    assert_eq!(list[1].target.as_deref(), Some("This Harux"));
    assert_eq!(list[1].origin_id.as_deref(), Some(original.as_str()));
    assert_eq!(list[2].message_control_id, "RT1");

    let detail = store.get_by_id(&result.id).await.unwrap();
    let out = detail.message.outbound.as_ref().unwrap();
    assert_eq!(out.requested_by, "10.0.0.7");
    assert_eq!(detail.message.source_addr, addr.to_string());
    assert_eq!(
        sender.counters(),
        SendCounters {
            sent: 1,
            ..SendCounters::default()
        }
    );
}

/// A send that fails is a row too, stored before the connect (rule 1).
#[tokio::test]
async fn a_refused_send_is_stored_with_its_reason() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let store = store();
    let sender = Sender::new(settings(|_| {}), store.clone());
    let id = received(&store, b"MSH|^~\\&|A|B|||20260101||ADT^A01|F1|P|2.5\r").await;
    let result = sender
        .send(request(to(addr, "gone"), &id), CLIENT)
        .await
        .unwrap();
    assert_eq!(result.delivery.state, DeliveryState::Failed);
    assert_eq!(result.delivery.error.as_deref(), Some("refused"));
    let stored = summaries(&store).await;
    assert_eq!(stored.len(), 2);
    assert_eq!(
        stored[1].delivery.as_ref().unwrap().error.as_deref(),
        Some("refused")
    );
    assert_eq!(
        sender.counters(),
        SendCounters {
            failed: 1,
            ..SendCounters::default()
        }
    );
}

#[tokio::test]
async fn an_ack_is_written_without_waiting() {
    let (addr, mut frames) = fake(|mut s, tx| async move {
        if let Some(f) = read_frame(&mut s).await {
            let _ = tx.send(f);
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    })
    .await;
    let store = store();
    let sender = Sender::new(settings(|_| {}), store.clone());
    let id = received(
        &store,
        b"MSH|^~\\&|A|B|||20260101||ACK^A01|K1|P|2.5\rMSA|AA|X\r",
    )
    .await;
    let result = sender
        .send(request(to(addr, "t"), &id), CLIENT)
        .await
        .unwrap();
    assert_eq!(result.delivery.state, DeliveryState::Sent);
    assert!(frames.recv().await.is_some());
}

#[tokio::test]
async fn refusals_before_anything_is_stored() {
    let (addr, _) = acking().await;
    let store = store();
    let good = received(&store, b"MSH|^~\\&|A|B|||20260101||ADT^A01|G1|P|2.5\r").await;
    let broken = received(&store, b"NOT HL7").await;

    let off = Sender::new(
        SendSettings::from_config(&Config::default()).unwrap(),
        store.clone(),
    );
    assert_eq!(
        off.send(request(to(addr, "t"), &good), CLIENT)
            .await
            .unwrap_err(),
        SendError::Disabled
    );

    let limited = Sender::new(
        settings(|c| c.send.allowed_destinations = vec!["orchestra.local:6000-6999".into()]),
        store.clone(),
    );
    assert!(matches!(
        limited.send(request(to(addr, "t"), &good), CLIENT).await,
        Err(SendError::Forbidden(_))
    ));
    // Within the list: the name is compared as written, nothing resolved.
    let allowed = Sender::new(
        settings(|c| {
            c.send.allowed_destinations = vec![format!("127.0.0.1:{}-{}", addr.port(), addr.port())]
        }),
        store.clone(),
    );
    assert!(allowed
        .send(request(to(addr, "t"), &good), CLIENT)
        .await
        .is_ok());
    let by_name = TargetRequest::Explicit(ExplicitTarget {
        name: None,
        host: "localhost".into(),
        port: addr.port(),
        tls: false,
        verify: true,
    });
    assert!(matches!(
        allowed.send(request(by_name, &good), CLIENT).await,
        Err(SendError::Forbidden(_))
    ));

    let sender = Sender::new(settings(|_| {}), store.clone());
    let before = store.count().await;
    assert!(matches!(
        sender.send(request(to(addr, "t"), "nope"), CLIENT).await,
        Err(SendError::NotFound(_))
    ));
    assert!(matches!(
        sender.send(request(to(addr, "t"), &broken), CLIENT).await,
        Err(SendError::Unprocessable(_))
    ));
    // An unverified TLS connection needs send.tls.allow_unverified (#273).
    let mut unverified = request(to(addr, "t"), &good);
    if let TargetRequest::Explicit(t) = &mut unverified.target {
        t.tls = true;
        t.verify = false;
    }
    assert!(matches!(
        sender.send(unverified, CLIENT).await,
        Err(SendError::Forbidden(_))
    ));
    assert_eq!(store.count().await, before, "nothing stored for a refusal");

    // A broken message goes out as received after the second click.
    let mut anyway = request(to(addr, "t"), &broken);
    anyway.allow_invalid = true;
    let result = sender.send(anyway, CLIENT).await.unwrap();
    let sent = store.get_by_id(&result.id).await.unwrap().message;
    assert_eq!(sent.raw, "NOT HL7");
    assert!(sent.parse_error.is_some());
}

/// A cut-off entry (#223) cannot be resent.
#[tokio::test]
async fn a_truncated_entry_is_refused() {
    let srv = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = srv.local_addr().unwrap();
    let store = store();
    let (_shutdown, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(mllp::serve(
        srv,
        store.clone(),
        MllpStats::new(),
        rx,
        MllpConfig {
            max_message_size_mb: 1,
            ..MllpConfig::default()
        },
    ));
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut big = b"MSH|^~\\&|A|B|||20260101||MDM^T02|BIG|P|2.5\rOBX|1|ED|X||".to_vec();
    big.extend(std::iter::repeat_n(b'Q', 1024 * 1024 + 10));
    s.write_all(&frame(&big)).await.unwrap();
    read_frame(&mut s).await.unwrap();
    let entry = summaries(&store).await.pop().unwrap();
    assert!(entry.truncated);

    let sender = Sender::new(settings(|_| {}), store.clone());
    let mut req = request(to(addr, "t"), &entry.id);
    req.allow_invalid = true;
    assert!(matches!(
        sender.send(req, CLIENT).await,
        Err(SendError::Unprocessable(_))
    ));
}

/// Shutdown ends a pending send with an outcome, and a message cleared from
/// the store while it is under way is still sent.
#[tokio::test]
async fn shutdown_and_clear_during_a_send() {
    let (addr, mut frames) = fake(|mut s, tx| async move {
        if let Some(f) = read_frame(&mut s).await {
            let _ = tx.send(f);
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    })
    .await;
    let store = store();
    let mut settings = settings(|_| {});
    settings.timeouts.response = Duration::from_secs(5);
    let sender = Sender::new(settings, store.clone());
    let id = received(&store, b"MSH|^~\\&|A|B|||20260101||ADT^A01|S1|P|2.5\r").await;

    let task = tokio::spawn({
        let sender = sender.clone();
        async move { sender.send(request(to(addr, "t"), &id), CLIENT).await }
    });
    // The frame arrived: the send is waiting for its response.
    frames.recv().await.unwrap();
    assert_eq!(sender.active_runs(), 1);
    store.clear().await;
    sender.shutdown();
    let result = task.await.unwrap().unwrap();
    assert_eq!(result.delivery.state, DeliveryState::Failed);
    assert_eq!(result.delivery.error.as_deref(), Some("shutdown"));
    assert!(sender.wait_for_runs(Duration::from_secs(1)).await);
    assert!(
        store.get_by_id(&result.id).await.is_none(),
        "cleared meanwhile"
    );
}

#[tokio::test]
async fn too_many_runs_at_once() {
    let (addr, mut frames) = fake(|mut s, tx| async move {
        if let Some(f) = read_frame(&mut s).await {
            let _ = tx.send(f);
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    })
    .await;
    let store = store();
    let mut settings = settings(|c| c.send.max_concurrent_runs = 1);
    settings.timeouts.response = Duration::from_secs(5);
    let sender = Sender::new(settings, store.clone());
    let id = received(&store, b"MSH|^~\\&|A|B|||20260101||ADT^A01|B1|P|2.5\r").await;
    let first = tokio::spawn({
        let (sender, id) = (sender.clone(), id.clone());
        async move { sender.send(request(to(addr, "t"), &id), CLIENT).await }
    });
    frames.recv().await.unwrap();
    assert_eq!(
        sender
            .send(request(to(addr, "t"), &id), CLIENT)
            .await
            .unwrap_err(),
        SendError::Busy
    );
    sender.shutdown();
    first.await.unwrap().unwrap();
}

/// A NACK is an answer, not a delivery: `AE` / `AR` are counted as rejected,
/// not as sent (#284 review).
#[tokio::test]
async fn a_nack_is_counted_as_rejected() {
    let (addr, _) = fake(|mut s, _| async move {
        while read_frame(&mut s).await.is_some() {
            let _ = s
                .write_all(&frame(
                    b"MSH|^~\\&|R|RF|||20260101||ACK|A|P|2.5\rMSA|AR|N1\r",
                ))
                .await;
        }
    })
    .await;
    let store = store();
    let sender = Sender::new(settings(|_| {}), store.clone());
    let id = received(&store, b"MSH|^~\\&|A|B|||20260101||ADT^A01|N1|P|2.5\r").await;
    let result = sender
        .send(request(to(addr, "t"), &id), CLIENT)
        .await
        .unwrap();
    assert_eq!(result.delivery.state, DeliveryState::Acked);
    assert_eq!(result.delivery.ack_code.as_deref(), Some("AR"));
    assert_eq!(
        sender.counters(),
        SendCounters {
            rejected: 1,
            ..SendCounters::default()
        }
    );
    sender.reset_counters();
    assert_eq!(sender.counters(), SendCounters::default());
}

// ── TLS (#273) ────────────────────────────────────────────────────────────
//
// A TLS receiver in the test itself, with the static certificates of
// tests/tls/ (see generate.sh there): a trusted certificate, an unknown
// issuer, a wrong name, the unverified switch, a receiver that asks for a
// client certificate, and the two ways plain and TLS can be mixed up.

mod tls {
    use super::*;
    use harux::config::SendTlsConfig;
    use harux::tls::TlsContext;
    use std::sync::Arc;
    use tokio_rustls::rustls::pki_types::pem::PemObject;
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use tokio_rustls::rustls::server::WebPkiClientVerifier;
    use tokio_rustls::rustls::{crypto, RootCertStore, ServerConfig};
    use tokio_rustls::TlsAcceptor;

    fn file(name: &str) -> String {
        format!("{}/tests/tls/{name}", env!("CARGO_MANIFEST_DIR"))
    }

    /// A TLS receiver with `cert`, answering every frame with `AA`;
    /// `client_ca` makes it ask for (and require) a client certificate.
    async fn tls_receiver(
        cert: &str,
        client_ca: Option<&str>,
    ) -> (SocketAddr, mpsc::UnboundedReceiver<Vec<u8>>) {
        let provider = Arc::new(crypto::ring::default_provider());
        let chain: Vec<_> = CertificateDer::pem_file_iter(file(&format!("{cert}.pem")))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let key = PrivateKeyDer::from_pem_file(file(&format!("{cert}.key"))).unwrap();
        let builder = ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .unwrap();
        let builder = match client_ca {
            Some(ca) => {
                let mut roots = RootCertStore::empty();
                for c in CertificateDer::pem_file_iter(file(ca)).unwrap() {
                    roots.add(c.unwrap()).unwrap();
                }
                let verifier =
                    WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                        .build()
                        .unwrap();
                builder.with_client_cert_verifier(verifier)
            }
            None => builder.with_no_client_auth(),
        };
        let acceptor = TlsAcceptor::from(Arc::new(builder.with_single_cert(chain, key).unwrap()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((s, _)) = listener.accept().await {
                let (acceptor, tx) = (acceptor.clone(), tx.clone());
                tokio::spawn(async move {
                    let Ok(mut s) = acceptor.accept(s).await else {
                        return;
                    };
                    let mut acc = Vec::new();
                    let mut buf = [0u8; 4096];
                    while !acc.ends_with(&[FS, CR]) {
                        match s.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => acc.extend_from_slice(&buf[..n]),
                        }
                    }
                    let _ = tx.send(acc[1..acc.len() - 2].to_vec());
                    let _ = s.write_all(&ack("T1")).await;
                    let _ = s.flush().await;
                    let _ = s.shutdown().await;
                });
            }
        });
        (addr, rx)
    }

    fn context(ca: bool, client: bool, allow_unverified: bool) -> TlsContext {
        TlsContext::load(
            &SendTlsConfig {
                ca_file: ca.then(|| file("ca.pem")),
                allow_unverified,
                client_cert_file: client.then(|| file("client.pem")),
                client_key_file: client.then(|| file("client.key")),
            },
            false, // the OS store must not decide these tests
        )
        .unwrap()
    }

    fn sender(store: &MessageStore, tls: TlsContext) -> Sender {
        let mut s = settings(|_| {});
        s.tls = Some(Arc::new(tls));
        Sender::new(s, store.clone())
    }

    fn tls_to(addr: SocketAddr, host: &str, verify: bool) -> TargetRequest {
        TargetRequest::Explicit(ExplicitTarget {
            name: Some("tls target".into()),
            host: host.into(),
            port: addr.port(),
            tls: true,
            verify,
        })
    }

    const MSG: &[u8] = b"MSH|^~\\&|A|B|||20260101||ADT^A01|T1|P|2.5\rPID|||1||M\xc3\xbcller\r";

    async fn send(
        sender: &Sender,
        store: &MessageStore,
        target: TargetRequest,
    ) -> harux::hl7::types::Delivery {
        let id = received(store, MSG).await;
        sender
            .send(request(target, &id), CLIENT)
            .await
            .unwrap()
            .delivery
    }

    /// The certificate comes from a CA in `ca_file`: the message goes out
    /// encrypted, byte for byte, and the sent message records TLS.
    #[tokio::test]
    async fn trusted_certificate_from_ca_file() {
        let (addr, mut frames) = tls_receiver("server", None).await;
        let store = store();
        let sender = sender(&store, context(true, false, false));
        for host in ["localhost", "127.0.0.1"] {
            let d = send(&sender, &store, tls_to(addr, host, true)).await;
            assert_eq!(d.state, DeliveryState::Acked, "{host}: {d:?}");
            assert_eq!(d.ack_code.as_deref(), Some("AA"));
            assert_eq!(frames.recv().await.unwrap(), MSG, "same bytes inside TLS");
        }
        let sent = summaries(&store).await.pop().unwrap();
        assert_eq!(sent.transport, Some(harux::hl7::types::Transport::Tls));
    }

    /// An unknown issuer is refused with a reason that names it.
    #[tokio::test]
    async fn unknown_issuer_is_named() {
        let (addr, _) = tls_receiver("untrusted", None).await;
        let store = store();
        let d = send(
            &sender(&store, context(true, false, false)),
            &store,
            tls_to(addr, "localhost", true),
        )
        .await;
        assert_eq!(d.state, DeliveryState::Failed);
        assert_eq!(d.error.as_deref(), Some("untrusted certificate"));
        let detail = d.error_detail.unwrap();
        assert!(
            detail.contains("CN=Unknown Test CA, O=Elsewhere")
                && detail.contains("send.tls.ca_file"),
            "{detail}"
        );
    }

    #[tokio::test]
    async fn wrong_name_is_reported() {
        let (addr, _) = tls_receiver("wrongname", None).await;
        let store = store();
        let d = send(
            &sender(&store, context(true, false, false)),
            &store,
            tls_to(addr, "localhost", true),
        )
        .await;
        assert_eq!(d.error.as_deref(), Some("wrong name"), "{d:?}");
        assert!(d.error_detail.unwrap().contains("elsewhere.example"));
    }

    /// `verify: false` only with `send.tls.allow_unverified`; then the
    /// untrusted target is reached and the row says it was unverified.
    #[tokio::test]
    async fn unverified_needs_the_switch() {
        let (addr, mut frames) = tls_receiver("untrusted", None).await;
        let store = store();
        let strict = sender(&store, context(true, false, false));
        let id = received(&store, MSG).await;
        assert!(matches!(
            strict
                .send(request(tls_to(addr, "localhost", false), &id), CLIENT)
                .await,
            Err(SendError::Forbidden(_))
        ));

        let lenient = sender(&store, context(true, false, true));
        let d = send(&lenient, &store, tls_to(addr, "localhost", false)).await;
        assert_eq!(d.state, DeliveryState::Acked, "{d:?}");
        assert!(frames.recv().await.is_some());
        let sent = summaries(&store).await.pop().unwrap();
        assert_eq!(
            sent.transport,
            Some(harux::hl7::types::Transport::TlsUnverified)
        );
    }

    /// A target that asks for a client certificate: refused without one,
    /// served with the configured one.
    #[tokio::test]
    async fn client_certificate_when_asked() {
        let (addr, mut frames) = tls_receiver("server", Some("ca.pem")).await;
        let store = store();
        let d = send(
            &sender(&store, context(true, false, false)),
            &store,
            tls_to(addr, "localhost", true),
        )
        .await;
        assert_eq!(d.error.as_deref(), Some("certificate required"), "{d:?}");
        assert!(d
            .error_detail
            .unwrap()
            .contains("send.tls.client_cert_file"));

        let d = send(
            &sender(&store, context(true, true, false)),
            &store,
            tls_to(addr, "localhost", true),
        )
        .await;
        assert_eq!(d.state, DeliveryState::Acked, "{d:?}");
        assert_eq!(frames.recv().await.unwrap(), MSG);
    }

    /// TLS to a plain MLLP port: no TLS answer (within the connect
    /// timeout). Plain to a TLS port: the target expects TLS.
    #[tokio::test]
    async fn plain_and_tls_mixed_up() {
        let (plain, _) = acking().await;
        let store = store();
        let mut s = settings(|_| {});
        s.tls = Some(Arc::new(context(true, false, false)));
        s.timeouts.connect = Duration::from_millis(300);
        let quick = Sender::new(s, store.clone());
        let d = send(&quick, &store, tls_to(plain, "127.0.0.1", true)).await;
        assert_eq!(d.error.as_deref(), Some("no TLS answer"), "{d:?}");
        assert!(d.error_detail.unwrap().contains("plain MLLP port"));

        let (tls_port, _) = tls_receiver("server", None).await;
        let d = send(&quick, &store, to(tls_port, "plain")).await;
        assert_eq!(d.error.as_deref(), Some("expects TLS"), "{d:?}");
    }
}
