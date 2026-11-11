//! MLLP over real TCP connections (#245): the listener (`mllp::serve`) runs
//! on an ephemeral port and is driven with `tokio::net::TcpStream`, so
//! framing across reads, ACK/NACK, the size limit, the connection limit,
//! timeouts and shutdown draining are tested end to end.

use harux::config::{MllpConfig, StoreConfig};
use harux::hl7::types::Hl7MessageSummary;
use harux::mllp::{self, MllpStats};
use harux::store::MessageStore;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

const VT: u8 = 0x0B;
const FS: u8 = 0x1C;
const CR: u8 = 0x0D;

struct Server {
    addr: SocketAddr,
    store: MessageStore,
    stats: MllpStats,
    shutdown: watch::Sender<bool>,
}

fn config() -> MllpConfig {
    MllpConfig {
        read_timeout_secs: 30,
        write_timeout_secs: 5,
        ..MllpConfig::default()
    }
}

async fn start(config: MllpConfig) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let store = MessageStore::new(StoreConfig::default());
    let stats = MllpStats::new();
    let (shutdown, rx) = watch::channel(false);
    tokio::spawn(mllp::serve(
        listener,
        store.clone(),
        stats.clone(),
        rx,
        config,
    ));
    Server {
        addr,
        store,
        stats,
        shutdown,
    }
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut f = vec![VT];
    f.extend_from_slice(payload);
    f.extend_from_slice(&[FS, CR]);
    f
}

fn adt(control_id: &str) -> String {
    format!(
        "MSH|^~\\&|APP|FAC|RCV|RFAC|20240101120000||ADT^A01|{control_id}|P|2.5\r\
         EVN||20240101120000\r\
         PID|||4711^^^HOSP||Smith^John||19800515|M\r\
         PV1||I|W1\r"
    )
}

/// Read exactly one MLLP reply (byte by byte, so a second reply that arrived
/// in the same segment stays unread). `None` on EOF, error or 5 s timeout.
async fn read_reply(s: &mut TcpStream) -> Option<String> {
    let read = async {
        let mut buf = Vec::new();
        loop {
            buf.push(s.read_u8().await.ok()?);
            if buf.ends_with(&[FS, CR]) {
                assert_eq!(buf[0], VT, "reply is MLLP-framed");
                return Some(String::from_utf8_lossy(&buf[1..buf.len() - 2]).into_owned());
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .ok()
        .flatten()
}

/// True if the peer closes the connection within `limit`.
async fn closed_within(s: &mut TcpStream, limit: Duration) -> bool {
    let mut b = [0u8; 64];
    matches!(
        tokio::time::timeout(limit, s.read(&mut b)).await,
        Ok(Ok(0)) | Ok(Err(_))
    )
}

async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..500 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for: {what}");
}

async fn summaries(store: &MessageStore) -> Vec<Hl7MessageSummary> {
    let mut v = store.list_summaries(0, usize::MAX).await;
    v.reverse(); // oldest first
    v
}

fn msa(reply: &str) -> &str {
    reply
        .split('\r')
        .find(|seg| seg.starts_with("MSA"))
        .expect("reply has an MSA segment")
}

#[tokio::test]
async fn ack_after_store_and_stats() {
    let srv = start(config()).await;
    let mut s = TcpStream::connect(srv.addr).await.unwrap();
    s.write_all(&frame(adt("C1").as_bytes())).await.unwrap();
    let reply = read_reply(&mut s).await.expect("ACK");
    assert!(msa(&reply).starts_with("MSA|AA|C1"), "{reply}");
    // Stored before the ACK went out.
    assert_eq!(srv.store.count().await, 1);
    assert_eq!(srv.stats.received.load(Ordering::SeqCst), 1);
    assert_eq!(srv.stats.parsed_ok.load(Ordering::SeqCst), 1);
    assert_eq!(srv.stats.active_connections.load(Ordering::SeqCst), 1);
    drop(s);
    let stats = srv.stats.clone();
    eventually("connection un-counted", || {
        stats.active_connections.load(Ordering::SeqCst) == 0
    })
    .await;
}

#[tokio::test]
async fn frames_split_across_reads_and_batched_in_one_write() {
    let srv = start(config()).await;
    let mut s = TcpStream::connect(srv.addr).await.unwrap();
    let f = frame(adt("SPLIT").as_bytes());
    let (body, end) = f.split_at(f.len() - 2);
    let (a, b) = body.split_at(body.len() / 2);
    for part in [a, b, &end[..1], &end[1..]] {
        s.write_all(part).await.unwrap();
        s.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let reply = read_reply(&mut s).await.expect("ACK");
    assert!(msa(&reply).starts_with("MSA|AA|SPLIT"), "{reply}");

    let mut two = frame(adt("B1").as_bytes());
    two.extend(frame(adt("B2").as_bytes()));
    s.write_all(&two).await.unwrap();
    let r1 = read_reply(&mut s).await.expect("first ACK");
    assert!(msa(&r1).starts_with("MSA|AA|B1"), "{r1}");
    let r2 = read_reply(&mut s).await.expect("second ACK");
    assert!(msa(&r2).starts_with("MSA|AA|B2"), "{r2}");
    assert_eq!(srv.store.count().await, 3);
}

#[tokio::test]
async fn unparseable_frame_gets_nack_and_is_stored() {
    let srv = start(config()).await;
    let mut s = TcpStream::connect(srv.addr).await.unwrap();
    s.write_all(&frame(b"NOT HL7 AT ALL")).await.unwrap();
    let reply = read_reply(&mut s).await.expect("NACK");
    assert!(msa(&reply).starts_with("MSA|AE|UNKNOWN"), "{reply}");
    let stored = summaries(&srv.store).await;
    assert_eq!(stored.len(), 1);
    assert!(stored[0].parse_error.is_some());
    assert_eq!(stored[0].ack_code.as_deref(), Some("AE"));
    assert_eq!(srv.stats.parse_errors.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn incoming_ack_is_stored_but_not_answered() {
    let srv = start(config()).await;
    let mut s = TcpStream::connect(srv.addr).await.unwrap();
    let ack = "MSH|^~\\&|RCV|RFAC|APP|FAC|20240101120000||ACK^A01|A1|P|2.5\rMSA|AA|C1\r";
    s.write_all(&frame(ack.as_bytes())).await.unwrap();
    // The next reply on the connection belongs to the ADT, not to the ACK.
    s.write_all(&frame(adt("AFTER").as_bytes())).await.unwrap();
    let reply = read_reply(&mut s).await.expect("ACK for the ADT");
    assert!(msa(&reply).starts_with("MSA|AA|AFTER"), "{reply}");
    let stored = summaries(&srv.store).await;
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].message_type, "ACK^A01");
    assert_eq!(stored[0].ack_code, None);
}

/// #222: multi-byte characters at the old fixed byte offsets.
#[tokio::test]
async fn multibyte_inputs_are_answered_and_stored() {
    let srv = start(config()).await;
    let mut s = TcpStream::connect(srv.addr).await.unwrap();

    s.write_all(&frame("Sehr geehrte Damen ä und Herren".as_bytes()))
        .await
        .unwrap();
    let reply = read_reply(&mut s).await.expect("NACK for non-HL7 text");
    assert!(msa(&reply).starts_with("MSA|AE"), "{reply}");

    let poison = "MSH|^~\\&|APP|FAC|RCV|RFAC|20240101120000||ADT^A01|POISON1|P|2.5\r\
                  EVN||20240101120000\r\
                  PID|||4711^^^HOSP||Müller^Jörg||1980-05ä|M\r\
                  PV1||I|WARD1\r";
    s.write_all(&frame(poison.as_bytes())).await.unwrap();
    let reply = read_reply(&mut s)
        .await
        .expect("ACK on the same connection");
    assert!(msa(&reply).starts_with("MSA|AA|POISON1"), "{reply}");

    let stored = summaries(&srv.store).await;
    assert_eq!(stored.len(), 2);
    let detail = srv.store.get_by_id(&stored[1].id).await.unwrap();
    assert!(detail
        .message
        .validation_warnings
        .iter()
        .any(|w| w.code == "INVALID_DATATYPE" && w.segment == "PID" && w.field == Some(7)));
    assert_eq!(srv.stats.parse_errors.load(Ordering::SeqCst), 1);
    assert_eq!(srv.stats.parsed_ok.load(Ordering::SeqCst), 1);
}

fn big_mdm(control_id: &str, base64_bytes: usize) -> Vec<u8> {
    let mut m = format!(
        "MSH|^~\\&|DMS|KLINIK|RCV|RZ|20240101120000||MDM^T02|{control_id}|P|2.5\r\
         EVN||20240101\rPID|||1^^^H||A^B\rPV1||I\rTXA|1|DS|AP\r\
         OBX|1|ED|PDF||^application^pdf^Base64^"
    )
    .into_bytes();
    m.extend(std::iter::repeat_n(b'Q', base64_bytes));
    m.extend_from_slice(b"||||||F\r");
    m
}

/// #223: an over-limit frame is answered with AR, stored as a visible entry
/// without its payload, and the connection keeps working.
#[tokio::test]
async fn oversize_frame_is_rejected_visibly_and_connection_survives() {
    let srv = start(MllpConfig {
        max_message_size_mb: 1,
        ..config()
    })
    .await;
    let mut s = TcpStream::connect(srv.addr).await.unwrap();

    // Far above the limit: rejected while it is still arriving.
    let mut data = frame(&big_mdm("BIG1", 3 * 1024 * 1024));
    // Just above the limit: may complete within the read that crosses it.
    data.extend(frame(&big_mdm("BIG2", 1024 * 1024)));
    data.extend(frame(adt("SMALL").as_bytes()));
    s.write_all(&data).await.unwrap();

    for expected in ["MSA|AR|BIG1", "MSA|AR|BIG2", "MSA|AA|SMALL"] {
        let reply = read_reply(&mut s).await.expect(expected);
        assert!(msa(&reply).starts_with(expected), "{reply}");
    }

    let stored = summaries(&srv.store).await;
    assert_eq!(stored.len(), 3);
    for big in &stored[..2] {
        assert_eq!(big.message_type, "MDM^T02");
        assert_eq!(big.ack_code.as_deref(), Some("AR"));
        assert!(big
            .parse_error
            .as_deref()
            .unwrap()
            .contains("max_message_size_mb (1 MB)"));
        let detail = srv.store.get_by_id(&big.id).await.unwrap();
        assert!(detail.message.raw.len() <= 4096);
    }
    assert_eq!(srv.stats.received.load(Ordering::SeqCst), 3);
    assert_eq!(srv.stats.parse_errors.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn connection_limit_rejects_and_counts() {
    let srv = start(MllpConfig {
        max_connections: 1,
        ..config()
    })
    .await;
    let mut a = TcpStream::connect(srv.addr).await.unwrap();
    a.write_all(&frame(adt("A").as_bytes())).await.unwrap();
    assert!(read_reply(&mut a).await.is_some(), "A is served");

    let mut b = TcpStream::connect(srv.addr).await.unwrap();
    assert!(closed_within(&mut b, Duration::from_secs(5)).await);
    assert_eq!(srv.stats.rejected_connections.load(Ordering::SeqCst), 1);
    assert_eq!(srv.stats.active_connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn idle_connection_closed_after_read_timeout() {
    let srv = start(MllpConfig {
        read_timeout_secs: 1,
        ..config()
    })
    .await;
    let mut s = TcpStream::connect(srv.addr).await.unwrap();
    assert!(closed_within(&mut s, Duration::from_secs(4)).await);
    let stats = srv.stats.clone();
    eventually("connection un-counted", || {
        stats.active_connections.load(Ordering::SeqCst) == 0
    })
    .await;
}

/// #241: after an earlier connection has come and gone, shutdown still waits
/// for a frame in flight and ACKs it.
#[tokio::test]
async fn shutdown_drains_in_flight_frame_after_earlier_connection() {
    let srv = start(config()).await;

    // Connection A: complete exchange, then closed — leaves a stale permit.
    let mut a = TcpStream::connect(srv.addr).await.unwrap();
    a.write_all(&frame(adt("A").as_bytes())).await.unwrap();
    assert!(read_reply(&mut a).await.is_some());
    drop(a);
    let stats = srv.stats.clone();
    eventually("A un-counted", || {
        stats.active_connections.load(Ordering::SeqCst) == 0
    })
    .await;

    // Connection B: frame in flight.
    let mut b = TcpStream::connect(srv.addr).await.unwrap();
    let f = frame(adt("INFLIGHT").as_bytes());
    let (head, tail) = f.split_at(40);
    b.write_all(head).await.unwrap();
    eventually("B counted", || {
        stats.active_connections.load(Ordering::SeqCst) == 1
    })
    .await;

    srv.shutdown.send(true).unwrap();
    let drain_stats = srv.stats.clone();
    let drain =
        tokio::spawn(
            async move { mllp::wait_for_drain(&drain_stats, Duration::from_secs(10)).await },
        );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!drain.is_finished(), "drain returned while B is mid-frame");

    b.write_all(tail).await.unwrap();
    let reply = read_reply(&mut b).await.expect("in-flight frame is ACKed");
    assert!(msa(&reply).starts_with("MSA|AA|INFLIGHT"), "{reply}");
    assert!(closed_within(&mut b, Duration::from_secs(5)).await);
    assert!(drain.await.unwrap(), "drain completes once B is closed");

    // The listener is gone too.
    assert!(TcpStream::connect(srv.addr).await.is_err());
}

#[tokio::test]
async fn latin1_payload_decoded_per_msh18() {
    let srv = start(config()).await;
    let mut s = TcpStream::connect(srv.addr).await.unwrap();
    let mut payload = b"MSH|^~\\&|KIS|KLINIK|RCV|RZ|20240101120000||ADT^A01|L1|P|2.5||||||8859/1\r\
          EVN||20240101120000\rPID|||4711^^^HOSP||M"
        .to_vec();
    payload.extend_from_slice(b"\xFCller^J\xF6rg||19800515|M\rPV1||I|W1\r");
    s.write_all(&frame(&payload)).await.unwrap();
    let reply = read_reply(&mut s).await.expect("ACK");
    assert!(msa(&reply).starts_with("MSA|AA|L1"), "{reply}");
    let stored = summaries(&srv.store).await;
    assert_eq!(stored[0].patient_name.as_deref(), Some("Müller, Jörg"));
    assert_eq!(stored[0].charset.as_deref(), Some("8859/1"));
    let detail = srv.store.get_by_id(&stored[0].id).await.unwrap();
    assert!(detail
        .message
        .validation_warnings
        .iter()
        .all(|w| w.code != "CHARSET_FALLBACK"));
}
