//! Keeps the store's byte accounting honest (#226): `estimated_bytes()` must
//! never be below what a parsed message really holds on the heap, and not
//! far above it.
//!
//! This test binary installs its own counting `#[global_allocator]`
//! (per thread, so the test harness can't interfere). For every live
//! allocation it adds what `heap_bytes` — the allocator model the store
//! uses — says that allocation costs. For a corpus of messages, filled in
//! the way the MLLP path stores them (charset, ACK, a kept original payload), the test then requires
//! `Hl7Message::estimated_bytes()` to be
//!
//! - **never below** that modeled heap: if someone adds a heap-owning field
//!   to `Hl7Message` and doesn't count it in `estimated_bytes`, this fails;
//! - **at most 5 % above** it, so the estimate stays an estimate of reality.
//!
//! At the time of writing the two are equal to the byte. Both sides use the
//! same model on the same `Layout` sizes, so the test means the same on every
//! platform; how well the model fits a given allocator is a separate
//! question (see `heap_bytes`).

use harux::hl7::parser::{build_ack, parse_message};
use harux::hl7::types::{
    heap_bytes, Delivery, DeliveryState, MessageMeta, Origin, Outbound, WireForm,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static HELD: Cell<isize> = const { Cell::new(0) };
    static ALLOCS: Cell<isize> = const { Cell::new(0) };
    /// Live allocations run through the same model the store uses.
    static MODELED: Cell<isize> = const { Cell::new(0) };
}

fn add(bytes: isize, allocs: isize, modeled: isize) {
    // `try_with`: the thread-local may already be gone while a thread exits.
    let _ = HELD.try_with(|h| h.set(h.get() + bytes));
    let _ = ALLOCS.try_with(|a| a.set(a.get() + allocs));
    let _ = MODELED.try_with(|m| m.set(m.get() + modeled));
}

fn model(size: usize) -> isize {
    heap_bytes(size) as isize
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        add(l.size() as isize, 1, model(l.size()));
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        add(l.size() as isize, 1, model(l.size()));
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        add(-(l.size() as isize), -1, -model(l.size()));
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        add(
            new as isize - l.size() as isize,
            0,
            model(new) - model(l.size()),
        );
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn snapshot() -> (isize, isize, isize) {
    (
        HELD.with(Cell::get),
        ALLOCS.with(Cell::get),
        MODELED.with(Cell::get),
    )
}

fn corpus() -> Vec<(String, String)> {
    let mut v = Vec::new();
    for dir in [
        "tests/messages/valid",
        "tests/messages/errors",
        "tests/messages/unknown_types",
    ] {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        for f in files {
            v.push((
                f.display().to_string(),
                String::from_utf8_lossy(&std::fs::read(&f).unwrap()).into_owned(),
            ));
        }
    }
    v.push((
        "realistic ADT".into(),
        "MSH|^~\\&|KIS|KLINIKUM|ORCHESTRA|RZ|20261110120000||ADT^A01^ADT_A01|0000000001|P|2.5|||AL|NE||8859/1\r\
         EVN|A01|20261110120000|||SCHMIDTA^Schmidt^Anna\r\
         PID|1||4000001^^^KLINIKUM^PI~9000001^^^GKV^SS||Mustermann^Max^^^Dr.||19650412|M|||Musterstr. 12^^Musterstadt^^12345^DE^H||^PRN^PH^^^^030123456|||M|EV|||||||||||||N\r\
         PV1|1|I|CHI1^204^1^CH^^N|E|||12345^Arzt^Albert^^^Dr.|||CHI||||||||7000001^^^KLINIKUM^VN|||||||||||||||||||||||||20261110113000\r\
         IN1|1|GKV|104212059|AOK Nordost|Behlertstr. 33A^^Potsdam^^14467^DE||||||||||||||||||||||||||||||||||M1234567\r\
         DG1|1||K35.8^Akute Appendizitis^ICD10GM2026|||AD\r"
            .into(),
    ));
    v.push((
        "many empty fields".into(),
        format!(
            "MSH|^~\\&|A|B|C|D|20240101||ZZZ^Z01|P|P|2.5\rZZZ{}",
            "|".repeat(20_000)
        ),
    ));
    v.push((
        "MDM with 256 KB ED attachment".into(),
        format!(
            "MSH|^~\\&|D|K|R|Z|20240101||MDM^T02|M|P|2.5\rEVN||2024\rPID|||1||A^B\rPV1||I\r\
             TXA|1|DS\rOBX|1|ED|PDF||^application^pdf^Base64^{}||||||F\r",
            "QUJD".repeat(64 * 1024)
        ),
    ));
    v
}

#[test]
fn estimated_bytes_tracks_the_real_heap() {
    let corpus = corpus();
    // Warm up the lazily built static dictionary and registry, which are
    // shared and not part of any message.
    for (_, raw) in &corpus {
        let _ = parse_message(raw, "127.0.0.1:5000");
    }

    let mut checked = 0;
    let mut report = Vec::new();
    for (name, raw) in &corpus {
        let (h0, a0, m0) = snapshot();
        let parsed = parse_message(raw, "127.0.0.1:5000").map(|mut msg| {
            // What the MLLP path adds before the message is stored.
            msg.charset = Some("8859/1".to_string());
            msg.ack_response = Some(build_ack(&msg, "AA"));
            msg.ack_code = Some("AA".to_string());
            // A payload no encoding reproduces keeps its bytes (#230).
            msg.wire = WireForm::Original(raw.as_bytes().into());
            msg
        });
        let (h1, a1, m1) = snapshot();
        let Ok(msg) = parsed else { continue };
        // The `Hl7Message` struct itself lives on this stack frame here; in
        // the store it sits in an `Arc`, which the store accounts separately.
        let requested = (h1 - h0) as usize;
        let allocs = (a1 - a0) as usize;
        let modeled = (m1 - m0) as usize;
        let estimated = msg.estimated_bytes();
        report.push(format!(
            "{name}: requested {requested} B in {allocs} allocs, modeled {modeled} B, estimated {estimated} B"
        ));
        assert!(
            estimated >= modeled,
            "{name}: estimated_bytes() = {estimated} B is below the modeled heap of the \
             message ({modeled} B; {requested} B requested in {allocs} allocations). \
             A heap-owning field of Hl7Message is probably not counted in estimated_bytes()."
        );
        assert!(
            estimated <= modeled + modeled / 20,
            "{name}: estimated_bytes() = {estimated} B is more than 5 % above the modeled \
             heap ({modeled} B)"
        );
        checked += 1;
        drop(msg);
    }
    assert!(
        checked >= 15,
        "only {checked} messages parsed:\n{}",
        report.join("\n")
    );
    println!("{}", report.join("\n"));
}

/// #272: what a sent message adds — the `Outbound` facts on the payload and
/// the `Delivery` with its response in the metadata — is accounted too.
#[test]
fn sent_message_and_its_metadata_are_accounted() {
    let mut msg = parse_message(
        "MSH|^~\\&|A|B|C|D|20240101||ADT^A01|C1|P|2.5\rPID|||1||Muster^Max\r",
        "10.9.9.9:6661",
    )
    .unwrap();
    let payload_before = msg.estimated_bytes();

    let (_, a0, m0) = snapshot();
    msg.outbound = Some(Box::new(Outbound {
        target: "orchestra-test-adt".to_string(),
        tls: false,
        origin: Origin::Resend {
            message_id: "0b9e8f9c-4d2e-4f3a-9c1b-2a7d5e6f8a90".to_string(),
        },
        requested_by: "10.0.0.7".to_string(),
        run: "6c1f0e2d-3b4a-4c5d-8e9f-0a1b2c3d4e5f".to_string(),
    }));
    let meta = MessageMeta {
        tags: vec!["Bug 1234".to_string()],
        bookmarked: true,
        delivery: Some(Box::new(Delivery {
            state: DeliveryState::Acked,
            ack_code: Some("AA".to_string()),
            response: Some("MSH|^~\\&|R|RF|||20260101||ACK^A01|A1|P|2.5\rMSA|AA|C1\r".to_string()),
            response_size: Some(52),
            latency_ms: Some(42),
            control_id_matches: Some(true),
            ..Delivery::pending()
        })),
    };
    let (_, a1, m1) = snapshot();
    let modeled = (m1 - m0) as usize;
    let estimated = msg.estimated_bytes() - payload_before + meta.estimated_bytes();
    assert!(
        estimated >= modeled && estimated <= modeled + modeled / 20,
        "estimated {estimated} B, modeled {modeled} B in {} allocations",
        a1 - a0
    );
}
