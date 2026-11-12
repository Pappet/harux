//! #243: deterministic mutation fuzz of the ingest path — every byte string
//! that can arrive on the MLLP port goes through charset decoding, parsing
//! with validation, the ACK, the list summary, size accounting and JSON
//! serialization without a panic.
//!
//! The corpus is `tests/messages/**`; each input gets 1–4 mutations
//! (insert a token — multi-byte characters, invalid UTF-8, delimiters,
//! escapes, CR/LF, digits, `MSH` —, delete a short range, overwrite a byte,
//! duplicate a range). A fixed-seed xorshift generator keeps it
//! reproducible and needs no crate. `ingest` catches panics itself (#235),
//! so the pieces are called directly here, where a panic is a failure.
//!
//! Besides "no panic", each input is checked for:
//! - the ACK parses, and its MSA-2 echoes MSH-10 (`UNKNOWN` when empty);
//! - each segment rebuilds its non-blank line of `raw` exactly;
//! - the bytes the export gives back (`wire_bytes`) are the bytes received.
//!
//! `HARUX_FUZZ_ITERATIONS` raises the count for a longer local run, and
//! `HARUX_FUZZ_SEED` picks another sequence.

use harux::hl7::charset::decode_payload;
use harux::hl7::parser::{build_ack, build_nack, parse_message, AckError};
use harux::hl7::types::{Hl7Message, Hl7MessageSummary, MessageMeta};
use harux::mllp::ingest;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// xorshift64* — small, fast, deterministic.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const TOKENS: &[&[u8]] = &[
    "ä".as_bytes(),
    "€".as_bytes(),
    "😀".as_bytes(),
    "\u{0301}".as_bytes(),
    b"\xff",
    b"\xc3",
    b"\xe2\x82",
    b"\x80",
    b"|",
    b"^",
    b"~",
    b"\\",
    b"&",
    b"\\F\\",
    b"\\X0D\\",
    b"\\.br\\",
    b"\\",
    b"\r",
    b"\n",
    b"\r\n",
    b"0",
    b"9",
    b"99",
    b".",
    b"+",
    b"-",
    b"MSH",
    b"MSH|^~\\&|",
    b"\x00",
    b" ",
];

fn mutate(rng: &mut Rng, input: &mut Vec<u8>) {
    for _ in 0..1 + rng.below(4) {
        let pos = rng.below(input.len() + 1);
        match rng.below(4) {
            0 => {
                let token = TOKENS[rng.below(TOKENS.len())];
                input.splice(pos..pos, token.iter().copied());
            }
            1 if !input.is_empty() => {
                let start = pos.min(input.len() - 1);
                let end = (start + 1 + rng.below(8)).min(input.len());
                input.drain(start..end);
            }
            2 if !input.is_empty() => {
                let at = pos.min(input.len() - 1);
                input[at] = rng.next() as u8;
            }
            _ if !input.is_empty() => {
                let start = pos.min(input.len() - 1);
                let end = (start + 1 + rng.below(16)).min(input.len());
                let copy = input[start..end].to_vec();
                let at = rng.below(input.len() + 1);
                input.splice(at..at, copy);
            }
            _ => input.extend_from_slice(b"MSH"),
        }
    }
}

fn corpus() -> Vec<Vec<u8>> {
    let mut files = Vec::new();
    for dir in [
        "tests/messages/valid",
        "tests/messages/errors",
        "tests/messages/unknown_types",
    ] {
        for entry in std::fs::read_dir(dir).unwrap() {
            files.push(entry.unwrap().path());
        }
    }
    files.sort();
    files
        .iter()
        .map(|f| {
            // The fixtures use LF for readability; MLLP senders use CR.
            let text = std::fs::read(f).unwrap();
            text.iter()
                .map(|&b| if b == b'\n' { b'\r' } else { b })
                .collect()
        })
        .collect()
}

fn field<'a>(msg: &'a Hl7Message, segment: &str, index: usize) -> Option<&'a str> {
    msg.segments
        .iter()
        .find(|s| s.name == segment)?
        .fields
        .iter()
        .find(|f| f.index == index)
        .map(|f| f.value.as_str())
}

/// Everything the ingest path does with one frame, plus the properties.
/// `Err` describes a broken property; a panic propagates.
fn check(payload: &[u8]) -> Result<(), String> {
    let decoded = decode_payload(payload);

    // What is stored for every frame, parsed or not: `ingest` builds the
    // parse-error entry for a failed frame (#235). Size accounting, the list
    // summary, JSON and the export's bytes must work for both.
    let stored = ingest(payload, "127.0.0.1:1");
    let _ = stored.estimated_bytes();
    let _ = Hl7MessageSummary::new(&stored, &MessageMeta::default());
    serde_json::to_string(&stored).map_err(|e| format!("JSON of the stored entry: {e}"))?;
    if stored.wire_bytes().as_ref() != payload {
        return Err("wire_bytes() differs from the received payload".into());
    }

    let msg = match parse_message(&decoded.text, "127.0.0.1:1") {
        Ok(msg) => msg,
        Err(reason) => {
            if stored.parse_error.is_none() {
                return Err(format!(
                    "ingest stored a parsed message, parse_message failed: {reason}"
                ));
            }
            let nack = build_nack(AckError::SegmentSequence, &reason);
            parse_message(&nack, "127.0.0.1:1").map_err(|e| format!("NACK unparseable: {e}"))?;
            return Ok(());
        }
    };

    let ack = build_ack(&msg, "AA");
    let parsed_ack =
        parse_message(&ack, "127.0.0.1:1").map_err(|e| format!("ACK unparseable: {e}"))?;
    let expected = match msg.message_control_id.as_str() {
        "" => "UNKNOWN",
        id => id,
    };
    if field(&parsed_ack, "MSA", 2) != Some(expected) {
        return Err(format!(
            "MSA-2 {:?} does not echo MSH-10 {expected:?}",
            field(&parsed_ack, "MSA", 2)
        ));
    }

    // The parser skips leading whitespace before MSH; after that, every
    // non-blank line is one segment, character for character.
    let lines: Vec<&str> = msg
        .raw
        .trim_start()
        .split(['\r', '\n'])
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.len() != msg.segments.len() {
        return Err(format!(
            "{} non-blank lines but {} segments",
            lines.len(),
            msg.segments.len()
        ));
    }
    let sep = msg.delimiters.field.to_string();
    for (line, seg) in lines.iter().zip(&msg.segments) {
        let skip = usize::from(seg.name == "MSH");
        let rebuilt = std::iter::once(seg.name.as_str())
            .chain(seg.fields.iter().skip(skip).map(|f| f.value.as_str()))
            .collect::<Vec<_>>()
            .join(&sep);
        if *line != rebuilt {
            return Err(format!(
                "segment {:?} rebuilds {rebuilt:?}, not {line:?}",
                seg.name
            ));
        }
    }

    if let Some(e) = &stored.parse_error {
        return Err(format!(
            "parse_message succeeded, ingest stored a parse error: {e}"
        ));
    }
    Ok(())
}

#[test]
fn mutated_frames_never_panic_and_keep_their_invariants() {
    let iterations: usize = std::env::var("HARUX_FUZZ_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);
    let seed: u64 = std::env::var("HARUX_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x4841_5255_5832_3433); // "HARUX243"
    let corpus = corpus();
    assert!(corpus.len() >= 20, "corpus too small: {}", corpus.len());

    // Silence the default hook so a panic doesn't print a backtrace per input;
    // the failure message below carries the input.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut rng = Rng(seed | 1);
    let mut failures = Vec::new();
    for i in 0..iterations {
        let mut input = corpus[i % corpus.len()].clone();
        mutate(&mut rng, &mut input);
        let outcome = catch_unwind(AssertUnwindSafe(|| check(&input)));
        let problem = match outcome {
            Ok(Ok(())) => continue,
            Ok(Err(broken)) => broken,
            Err(p) => format!(
                "panic: {}",
                p.downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_default()
            ),
        };
        failures.push(format!(
            "#{i}: {problem}\n    input: {:?}",
            String::from_utf8_lossy(&input)
        ));
        if failures.len() >= 5 {
            break;
        }
    }
    std::panic::set_hook(hook);
    assert!(
        failures.is_empty(),
        "seed {seed:#x}, {iterations} inputs:\n{}",
        failures.join("\n")
    );
}
