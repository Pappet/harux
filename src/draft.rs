//! Drafts on the server (#276, Milestone 4 package M4-5).
//!
//! A draft lives in the browser, but everything that needs to understand HL7
//! is done here. Nothing in this module stores anything.
//!
//! - [`draft_of`]: a stored message as draft text. Every field value above
//!   [`DRAFT_VALUE_MAX`] characters is replaced by a **reference**
//!   `{{from:OBX[1]-5}}`, so an MDM's Base64 attachment never travels
//!   through the browser and the editor stays fast (#215).
//! - [`preview`]: the draft's text parsed like a received frame, with the
//!   validation computed from sample values for the placeholders, and the
//!   position of every field in UTF-16 units (as a `<textarea>` counts).
//! - [`apply_set`]: a changed field written back into the text — the
//!   override code of #275 (`hl7::compose`), the one place that writes a
//!   field.
//! - [`build_payload`]: the bytes a send of the draft writes: placeholders
//!   and references filled in, line ends `CR`, the draft's encoding (MSH-18,
//!   else UTF-8, or the one the request names), no MLLP frame byte, at most
//!   `mllp.max_message_size_mb`.
//! - [`decode_file`]: a file's bytes decoded like a received frame
//!   (`decode_payload`, the first frame if the file is MLLP-framed).

use crate::hl7::charset;
use crate::hl7::compose::{self, FieldPath, PlaceholderContext};
use crate::hl7::types::{Delimiters, Hl7Message, WireForm};
use crate::send::SendError;
use encoding_rs::{Encoding, UTF_8};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Field values longer than this many characters become references in a
/// draft. The same number as `DISPLAY_VALUE_MAX` in `static/util.js`: a value
/// the inspector would shorten is one the editor does not hold.
pub const DRAFT_VALUE_MAX: usize = 2000;

/// Start of a reference in draft text: `{{from:OBX[1]-5}}`.
const REFERENCE_OPEN: &str = "{{from:";

/// Address used as the "peer" of a parsed draft.
const DRAFT_PEER: &str = "draft";

// --- A stored message as a draft ---

/// `GET /api/messages/{id}/draft`.
#[derive(Debug, Serialize)]
pub struct DraftText {
    /// The stored message the draft is made from: send it as `based_on`.
    pub based_on: String,
    /// The text, one segment per line (`LF`), long values as references.
    pub raw: String,
    /// MSH-18 as declared, if any.
    pub charset: Option<String>,
    /// The encoding the message arrived in (`windows-1252`, `UTF-8`, …):
    /// send it as the item's `charset` to keep it.
    pub encoding: &'static str,
    /// The references in `raw`, with the size of the value each stands for.
    pub references: Vec<Reference>,
}

#[derive(Debug, Serialize)]
pub struct Reference {
    /// `OBX[1]-5`.
    pub path: String,
    /// The text that stands in the draft: `{{from:OBX[1]-5}}`.
    pub placeholder: String,
    /// Size of the value in characters and in UTF-8 bytes.
    pub chars: usize,
    pub bytes: usize,
}

/// `msg` as draft text. An entry that holds only the beginning of its frame
/// (#223) cannot be edited. A message that did not parse is its text as it
/// is: without a structure there are no fields to refer to.
pub fn draft_of(msg: &Hl7Message) -> Result<DraftText, SendError> {
    if msg.truncated {
        return Err(SendError::Unprocessable(
            "This entry holds only the beginning of a frame above the size limit and \
             cannot be edited"
                .into(),
        ));
    }
    let mut text = msg.raw.clone();
    let mut references = Vec::new();
    if msg.parse_error.is_none() {
        let d = msg.delimiters;
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for seg in &msg.segments {
            let occurrence = seen.entry(seg.name.as_str()).or_default();
            *occurrence += 1;
            for f in &seg.fields {
                // Bytes ≥ characters: the cheap test first.
                if f.value.len() <= DRAFT_VALUE_MAX
                    || (seg.name == "MSH" && f.index <= 2)
                    || f.value.chars().count() <= DRAFT_VALUE_MAX
                {
                    continue;
                }
                // Only a path that reads back as this very field: the
                // parser takes segment names `FieldPath` refuses (`~OBX`,
                // `obx`, nine letters) and fields above 1,000; their
                // values stay in the text.
                let Ok(path) =
                    FieldPath::parse(&format!("{}[{}]-{}", seg.name, occurrence, f.index))
                else {
                    continue;
                };
                if path.segment != seg.name {
                    continue;
                }
                let placeholder = format!("{REFERENCE_OPEN}{path}}}}}");
                // A reference can't be written when the message's field
                // separator is one of its characters (`-`, `:`, …); the
                // value then stays in the text.
                if let Ok(changed) = compose::set_field(&text, d, &path, &placeholder) {
                    text = changed;
                    references.push(Reference {
                        path: path.to_string(),
                        placeholder,
                        chars: f.value.chars().count(),
                        bytes: f.value.len(),
                    });
                }
            }
        }
    }
    Ok(DraftText {
        based_on: msg.id.clone(),
        raw: lines_lf(&text),
        charset: msg.charset.clone(),
        encoding: stored_encoding(msg).name(),
        references,
    })
}

/// The encoding `msg` arrived in — the one a draft of it keeps. A pure-ASCII
/// message carries no evidence of its encoding, so its MSH-18 decides
/// (as for a resend with changes, #275).
fn stored_encoding(msg: &Hl7Message) -> &'static Encoding {
    let declared = || {
        msg.charset
            .as_deref()
            .and_then(charset::encoding_for)
            .unwrap_or(UTF_8)
    };
    match &msg.wire {
        WireForm::Encoded(e) => e,
        WireForm::Utf8 if !msg.raw.is_ascii() => UTF_8,
        WireForm::Utf8 | WireForm::Original(_) => declared(),
    }
}

/// `text` with every line end (`CR LF`, `CR`, `LF`) as `LF` and no line end
/// at the end — the form of a `<textarea>`.
fn lines_lf(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end_matches('\n')
        .to_owned()
}

/// `text` with every line end as `CR` and a final `CR`: the segment
/// terminator of HL7 (plan 5.4).
fn lines_cr(text: &str) -> String {
    let mut out = text.replace("\r\n", "\r").replace('\n', "\r");
    if !out.ends_with('\r') {
        out.push('\r');
    }
    out
}

// --- Writing fields ---

/// Write the overrides of `set` into the draft text `raw` (#275's
/// `compose::set_field`). Values are written as they are: placeholders stay
/// visible in the text and are filled in when the draft is sent. Returns the
/// text and the fields that were set.
pub fn apply_set(
    raw: &str,
    set: &BTreeMap<String, String>,
) -> Result<(String, Vec<String>), SendError> {
    if set.is_empty() {
        return Ok((raw.to_owned(), Vec::new()));
    }
    let d = delimiters_of(raw).ok_or_else(|| {
        SendError::Unprocessable(
            "Fields can only be set in a draft that starts with a valid MSH segment".into(),
        )
    })?;
    let mut text = raw.to_owned();
    let mut changes = Vec::with_capacity(set.len());
    for (path, value) in set {
        let path = FieldPath::parse(path).map_err(SendError::from)?;
        text = compose::set_field(&text, d, &path, value).map_err(SendError::from)?;
        changes.push(path.to_string());
    }
    Ok((text, changes))
}

/// The delimiters of a text's MSH segment, as the parser reads them.
fn delimiters_of(text: &str) -> Option<Delimiters> {
    let text = text.trim_start();
    if !text.starts_with("MSH") {
        return None;
    }
    crate::hl7::parser::parse_delimiters(text.as_bytes()).ok()
}

// --- References ---

/// Whether `text` holds a reference.
pub fn has_references(text: &str) -> bool {
    text.contains(REFERENCE_OPEN)
}

/// `text` with each reference replaced by the value of that field in
/// `based_on`, the message the draft was made from. The value is the
/// field's text as stored (escapes and all), so an attachment nobody
/// touched goes out as it came in.
pub fn fill_references(text: &str, based_on: Option<&Hl7Message>) -> Result<String, SendError> {
    if !has_references(text) {
        return Ok(text.to_owned());
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(REFERENCE_OPEN) {
        out.push_str(&rest[..open]);
        let after = &rest[open + REFERENCE_OPEN.len()..];
        let Some(close) = after.find("}}") else {
            // Not closed: not a reference, sent as written.
            out.push_str(&rest[open..]);
            return Ok(out);
        };
        let path = FieldPath::parse(&after[..close]).map_err(SendError::from)?;
        let original = based_on.ok_or_else(|| {
            SendError::Unprocessable(format!(
                "The draft refers to {path} of the message it was made from, but names \
                 no such message (based_on)"
            ))
        })?;
        out.push_str(reference_value(original, &path).ok_or_else(|| {
            SendError::Unprocessable(format!(
                "The draft refers to {path}, which the message it was made from does not have"
            ))
        })?);
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn reference_value<'a>(msg: &'a Hl7Message, path: &FieldPath) -> Option<&'a str> {
    msg.segments
        .iter()
        .filter(|s| s.name == path.segment)
        .nth(path.occurrence - 1)?
        .fields
        .iter()
        .find(|f| f.index == path.field)
        .map(|f| f.value.as_str())
}

// --- Building the bytes ---

/// The bytes a send of the draft `text` writes (plan 5.4, third row):
/// placeholders filled in (`n` is the message's position in its run), then
/// references from `based_on`, line ends normalized to `CR` with a final
/// `CR`, encoded with `charset` — `None` or `"auto"`: MSH-18, else UTF-8.
/// Refused: a character the encoding cannot write (named), an MLLP frame
/// byte, more than `max_bytes`.
pub fn build_payload(
    text: &str,
    charset: Option<&str>,
    based_on: Option<&Hl7Message>,
    n: usize,
    max_bytes: usize,
) -> Result<Vec<u8>, SendError> {
    let ctx = PlaceholderContext {
        n,
        now: chrono::Local::now(),
    };
    // Placeholders first: a reference's value is the stored text and is
    // never read for placeholders.
    let text = compose::fill_placeholders(text, &ctx);
    let text = lines_cr(&fill_references(&text, based_on)?);
    let encoding = draft_encoding(&text, charset)?;
    let bytes = charset::encode_strict(&text, encoding).map_err(SendError::Unprocessable)?;
    // One frame on the wire is one stored message (rule 1).
    if let Some(b) = bytes.iter().find(|&&b| b == 0x0B || b == 0x1C) {
        return Err(SendError::Unprocessable(format!(
            "The draft contains the MLLP {} byte (0x{b:02X}); a message Harux builds cannot carry it",
            if *b == 0x0B { "start" } else { "end" }
        )));
    }
    if bytes.len() > max_bytes {
        return Err(SendError::TooLarge(format!(
            "The draft ({} KB) is larger than mllp.max_message_size_mb",
            bytes.len() / 1024
        )));
    }
    Ok(bytes)
}

/// The encoding of a draft: the one `charset` names, or, for `None` /
/// `"auto"`, the one its MSH-18 declares — UTF-8 without one. An MSH-18
/// Harux cannot write is refused rather than guessed: the receiver reads the
/// bytes by what it says.
fn draft_encoding(text: &str, charset: Option<&str>) -> Result<&'static Encoding, SendError> {
    match charset
        .map(str::trim)
        .filter(|c| !c.is_empty() && !c.eq_ignore_ascii_case("auto"))
    {
        Some(label) => charset::encoding_for(label).ok_or_else(|| {
            SendError::BadRequest(format!("{label:?} is not an encoding Harux can write"))
        }),
        None => match charset::declared_charset(text.as_bytes()) {
            None => Ok(UTF_8),
            Some(cs) => charset::encoding_for(&cs).ok_or_else(|| {
                SendError::Unprocessable(format!(
                    "MSH-18 {cs:?} is not a charset Harux can write; name the encoding \
                     (charset) to send it anyway"
                ))
            }),
        },
    }
}

/// `bytes` in an MLLP frame (`VT … FS CR`), like the export.
pub fn framed(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 3);
    out.push(crate::mllp::MLLP_START);
    out.extend_from_slice(bytes);
    out.push(crate::mllp::MLLP_END_1);
    out.push(crate::mllp::MLLP_END_2);
    out
}

// --- Preview ---

/// `POST /api/parse` with a JSON body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParseRequest {
    /// The draft's text.
    pub raw: String,
    /// Fields to write into the text first (the Segments tab's inputs).
    #[serde(default)]
    pub set: BTreeMap<String, String>,
    /// The stored message the draft was made from.
    #[serde(default)]
    pub based_on: Option<String>,
    /// Fill the references in before parsing (*Diff vs pinned*: an
    /// attachment nobody touched compares as equal). Needs `based_on`.
    #[serde(default)]
    pub fill_references: bool,
}

/// The answer of `POST /api/parse`.
#[derive(Debug, Serialize)]
pub struct Preview {
    /// The text after the overrides (and with the references filled in, if
    /// asked) — the text `message` and `positions` describe.
    pub raw: String,
    /// The fields `set` wrote.
    pub changes: Vec<String>,
    /// What `GET /api/messages/{id}` returns for a stored message: segments
    /// with descriptions and datatypes, type information, the validation
    /// warnings or the parse error.
    pub message: Hl7Message,
    /// Where each segment and field is in `raw`, in UTF-16 units.
    pub positions: Vec<SegmentPosition>,
}

/// A segment line of the text and its fields: `[field, start, end]`, the
/// end exclusive, MSH-1 being the separator itself. Empty fields are listed
/// too (`start == end`), so a cursor between two separators finds its field.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SegmentPosition {
    pub name: String,
    pub start: usize,
    pub end: usize,
    pub fields: Vec<[usize; 3]>,
}

/// The preview of the draft `raw`: `set` written in, the references filled
/// in from `based_on` when `fill`, then parsed. The fields come from the
/// text as written, so an input shows `{{now}}`; the validation warnings
/// come from the same text with sample values for the placeholders, so
/// `{{now}}` is not reported as an invalid timestamp — the validator knows
/// nothing about placeholders.
pub fn preview(
    raw: &str,
    set: &BTreeMap<String, String>,
    based_on: Option<&Hl7Message>,
    fill: bool,
) -> Result<Preview, SendError> {
    let (mut text, changes) = apply_set(raw, set)?;
    if fill {
        text = fill_references(&text, based_on)?;
    }
    let mut message = crate::mllp::parse_text(&text, DRAFT_PEER);
    if message.parse_error.is_none() {
        let ctx = PlaceholderContext {
            n: 1,
            now: chrono::Local::now(),
        };
        let sample = compose::fill_placeholders(&text, &ctx);
        if sample != text {
            let checked = crate::mllp::parse_text(&sample, DRAFT_PEER);
            if checked.parse_error.is_none() {
                message.validation_warnings = checked.validation_warnings;
            }
        }
    }
    let positions = positions(&text, message.delimiters);
    Ok(Preview {
        raw: text,
        changes,
        message,
        positions,
    })
}

/// The segment lines of `text` (as the parser finds them) and their fields,
/// in UTF-16 units.
pub fn positions(text: &str, d: Delimiters) -> Vec<SegmentPosition> {
    // Byte offsets come in increasing order: convert them in one pass.
    let mut cursor = (0usize, 0usize); // (byte, utf16)
    let mut utf16 = |byte: usize| {
        cursor.1 += text[cursor.0..byte].encode_utf16().count();
        cursor.0 = byte;
        cursor.1
    };
    let mut out = Vec::new();
    for line in compose::segment_lines(text) {
        let s = &text[line.clone()];
        let mut parts = Vec::new();
        let mut part_start = 0;
        for (i, c) in s.char_indices() {
            if c == d.field {
                parts.push(part_start..i);
                part_start = i + c.len_utf8();
            }
        }
        parts.push(part_start..s.len());
        let name = s[parts[0].clone()].to_owned();
        let is_msh = name == "MSH";
        let start = utf16(line.start);
        let mut fields = Vec::with_capacity(parts.len());
        for (k, part) in parts.iter().enumerate().skip(1) {
            if is_msh && k == 1 {
                // MSH-1 is the separator between the name and MSH-2.
                let sep = line.start + part.start - d.field.len_utf8();
                let a = utf16(sep);
                fields.push([1, a, utf16(line.start + part.start)]);
            }
            let field = if is_msh { k + 1 } else { k };
            let a = utf16(line.start + part.start);
            fields.push([field, a, utf16(line.start + part.end)]);
        }
        let end = utf16(line.end);
        out.push(SegmentPosition {
            name,
            start,
            end,
            fields,
        });
    }
    out
}

// --- A file ---

/// `POST /api/parse` with a file's bytes.
#[derive(Debug, Serialize)]
pub struct DecodedFile {
    /// The text, one segment per line (`LF`).
    pub raw: String,
    /// MSH-18 as declared, if any.
    pub charset: Option<String>,
    /// The encoding the bytes were decoded with: the draft's `charset`.
    pub encoding: &'static str,
    /// Why the declared charset was not used, if it was not
    /// (`CHARSET_FALLBACK`).
    pub warning: Option<String>,
    /// Whether the file was MLLP-framed; only its first frame is read.
    pub framed: bool,
}

/// Decode a file's bytes like a received frame: the first frame if the file
/// is MLLP-framed, then `decode_payload` (MSH-18, with its fallbacks). A
/// browser reads text as UTF-8 only, so a Latin-1 file must reach the server
/// as bytes.
pub fn decode_file(bytes: &[u8]) -> DecodedFile {
    let trimmed = bytes.trim_ascii_start();
    let (payload, framed) = if trimmed.first() == Some(&crate::mllp::MLLP_START) {
        match crate::mllp::extract_mllp_frame(trimmed, 0) {
            Some((frame, _)) => (frame, true),
            // A start byte and no end: everything after it.
            None => (&trimmed[1..], true),
        }
    } else {
        (bytes, false)
    };
    let decoded = charset::decode_payload(payload);
    DecodedFile {
        raw: lines_lf(&decoded.text),
        charset: decoded.declared,
        encoding: decoded.encoding.name(),
        warning: decoded.warning.map(|w| w.message),
        framed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ingest(bytes: &[u8]) -> Hl7Message {
        crate::mllp::ingest(bytes, "10.0.0.1:5000")
    }

    fn mdm(attachment_len: usize) -> Hl7Message {
        let b64 = "QUJD".repeat(attachment_len / 4);
        ingest(
            format!(
                "MSH|^~\\&|A|B|||20240101||MDM^T02|C1|P|2.5\r\nPID|||1||Muster\rTXA|1|CN\rOBX|1|ED|PDF||^application^pdf^Base64^{b64}\rOBX|2|ST|X||short\r"
            )
            .as_bytes(),
        )
    }

    #[test]
    fn long_values_become_references() {
        let msg = mdm(4000);
        let d = draft_of(&msg).unwrap();
        assert_eq!(d.based_on, msg.id);
        assert_eq!(
            d.raw,
            "MSH|^~\\&|A|B|||20240101||MDM^T02|C1|P|2.5\nPID|||1||Muster\nTXA|1|CN\nOBX|1|ED|PDF||{{from:OBX-5}}\nOBX|2|ST|X||short"
        );
        assert_eq!(d.references.len(), 1);
        assert_eq!(d.references[0].path, "OBX-5");
        assert_eq!(
            d.references[0].bytes,
            4000 + "^application^pdf^Base64^".len()
        );
        assert_eq!(d.encoding, "UTF-8");

        // Filled in again, the text is the original's, line ends aside.
        let filled = fill_references(&d.raw, Some(&msg)).unwrap();
        assert_eq!(lines_cr(&filled), msg.raw.replace("\r\n", "\r"));
    }

    #[test]
    fn a_value_at_the_limit_stays_and_the_second_occurrence_is_named() {
        let at = "x".repeat(DRAFT_VALUE_MAX);
        let over = "ü".repeat(DRAFT_VALUE_MAX + 1); // bytes ≠ characters
        let msg = ingest(
            format!("MSH|^~\\&|A|B|||20240101||ORU^R01|C1|P|2.5\rOBX|1|ST|X||{at}\rOBX|2|ST|X||{over}\r")
                .as_bytes(),
        );
        let d = draft_of(&msg).unwrap();
        assert!(d.raw.contains(&at));
        assert!(d.raw.ends_with("OBX|2|ST|X||{{from:OBX[2]-5}}"));
        assert_eq!(d.references[0].chars, DRAFT_VALUE_MAX + 1);
        assert_eq!(d.references[0].bytes, 2 * (DRAFT_VALUE_MAX + 1));
    }

    /// Found by the fuzz test: a segment name `FieldPath` does not take
    /// gets no reference — its value stays in the text.
    #[test]
    fn odd_segment_names_keep_their_values() {
        let long = "x".repeat(DRAFT_VALUE_MAX + 1);
        let msg = ingest(
            format!("MSH|^~\\&|A|B|||20240101||ORU^R01|C1|P|2.5\r~OBX|1|ST|X||{long}\robx|1|ST|X||{long}\rOBXOBX|1|ST|X||{long}\rOBX|1|ST|X||{long}\r").as_bytes(),
        );
        let d = draft_of(&msg).unwrap();
        assert_eq!(d.references.len(), 1);
        assert_eq!(d.references[0].path, "OBX-5");
        assert_eq!(d.raw.matches(&long).count(), 3);
        let filled = fill_references(&d.raw, Some(&msg)).unwrap();
        assert_eq!(lines_cr(&filled), msg.raw);
    }

    #[test]
    fn a_draft_keeps_the_encoding_the_message_arrived_in() {
        let latin1 =
            ingest(b"MSH|^~\\&|A|B|||20240101||ADT^A01|C1|P|2.5||||||8859/1\rPID|||1||M\xfcller\r");
        let d = draft_of(&latin1).unwrap();
        assert_eq!(d.encoding, "windows-1252");
        assert_eq!(d.charset.as_deref(), Some("8859/1"));
        assert!(d.raw.ends_with("PID|||1||Müller"));
        // Sent with "auto", MSH-18 decides: the same bytes.
        let bytes = build_payload(&d.raw, None, None, 1, 1 << 20).unwrap();
        assert_eq!(bytes, latin1.wire_bytes().as_ref());

        // ASCII declared 8859/1: MSH-18 decides.
        let ascii = ingest(b"MSH|^~\\&|A|B|||20240101||ADT^A01|C1|P|2.5||||||8859/1\rPID|||1||M\r");
        assert_eq!(draft_of(&ascii).unwrap().encoding, "windows-1252");
        // Mislabelled (UTF-8 bytes, 8859/1 declared): kept as UTF-8.
        let mislabelled = ingest(
            "MSH|^~\\&|A|B|||20240101||ADT^A01|C1|P|2.5||||||8859/1\rPID|||1||Müller\r".as_bytes(),
        );
        assert_eq!(draft_of(&mislabelled).unwrap().encoding, "UTF-8");
    }

    #[test]
    fn a_truncated_entry_cannot_be_edited_and_a_broken_one_is_its_text() {
        let mut msg = mdm(8);
        msg.truncated = true;
        assert!(matches!(draft_of(&msg), Err(SendError::Unprocessable(_))));
        let broken = ingest(b"XYZ|not hl7\r\nsecond line\r");
        let d = draft_of(&broken).unwrap();
        assert_eq!(d.raw, "XYZ|not hl7\nsecond line");
        assert!(d.references.is_empty());
    }

    #[test]
    fn references_are_refused_without_their_message() {
        let msg = mdm(8);
        let e = fill_references("OBX|1|ED|X||{{from:OBX-5}}", None).unwrap_err();
        assert!(matches!(e, SendError::Unprocessable(ref m) if m.contains("based_on")));
        let e = fill_references("{{from:OBX[9]-5}}", Some(&msg)).unwrap_err();
        assert!(matches!(e, SendError::Unprocessable(ref m) if m.contains("OBX[9]-5")));
        let e = fill_references("{{from:nonsense}}", Some(&msg)).unwrap_err();
        assert!(matches!(e, SendError::BadRequest(_)));
        // Not closed: sent as written.
        assert_eq!(
            fill_references("a {{from:OBX-5", Some(&msg)).unwrap(),
            "a {{from:OBX-5"
        );
    }

    #[test]
    fn building_fills_placeholders_but_not_inside_references() {
        let msg = ingest(b"MSH|^~\\&|A|B|||20240101||ORU^R01|C1|P|2.5\rOBX|1|ST|X||{{now}}\r");
        let draft = "MSH|^~\\&|A|B|||{{today}}||ORU^R01|{{id}}|P|2.5\nOBX|1|ST|X||{{from:OBX-5}}";
        let bytes = build_payload(draft, None, Some(&msg), 1, 1 << 20).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("{{today}}") && !text.contains("{{id}}"));
        assert!(text.ends_with("\rOBX|1|ST|X||{{now}}\r"), "{text:?}");
        assert_eq!(
            text.matches('\r').count(),
            2,
            "line ends are CR, one at the end"
        );
    }

    #[test]
    fn building_refuses_what_cannot_be_written() {
        let latin1 = "MSH|^~\\&|A|B|||20240101||ADT^A01|C1|P|2.5||||||8859/1\nPID|||1||Ωmega";
        let e = build_payload(latin1, None, None, 1, 1 << 20).unwrap_err();
        assert!(matches!(e, SendError::Unprocessable(ref m) if m.contains("U+03A9")));
        // Named explicitly, UTF-8 goes.
        assert!(build_payload(latin1, Some("UTF-8"), None, 1, 1 << 20).is_ok());
        let e = build_payload("MSH|^~\\&|A\u{1c}", None, None, 1, 1 << 20).unwrap_err();
        assert!(matches!(e, SendError::Unprocessable(ref m) if m.contains("0x1C")));
        let e = build_payload("MSH|^~\\&|A", Some("klingon"), None, 1, 1 << 20).unwrap_err();
        assert!(matches!(e, SendError::BadRequest(_)));
        let utf16 = "MSH|^~\\&|A|B|||20240101||ADT^A01|C1|P|2.5||||||UNICODE UTF-16";
        let e = build_payload(utf16, Some("auto"), None, 1, 1 << 20).unwrap_err();
        assert!(matches!(e, SendError::Unprocessable(ref m) if m.contains("UNICODE UTF-16")));
        let e = build_payload("MSH|^~\\&|AAAA", None, None, 1, 10).unwrap_err();
        assert!(matches!(e, SendError::TooLarge(_)));
    }

    #[test]
    fn set_writes_the_raw_value_and_keeps_placeholders() {
        let set = BTreeMap::from([
            ("MSH-7".to_string(), "{{now}}".to_string()),
            ("PID-5".to_string(), "Doe^Jane".to_string()),
        ]);
        let (text, changes) = apply_set("MSH|^~\\&|A\nPID|||1||Muster", &set).unwrap();
        assert_eq!(text, "MSH|^~\\&|A||||{{now}}\nPID|||1||Doe^Jane");
        assert_eq!(changes, ["MSH-7", "PID-5"]);
        let bad = BTreeMap::from([("PID-5".to_string(), "a|b".to_string())]);
        assert!(matches!(
            apply_set("MSH|^~\\&|A\nPID|", &bad),
            Err(SendError::Unprocessable(_))
        ));
        assert!(matches!(
            apply_set("not hl7", &set),
            Err(SendError::Unprocessable(_))
        ));
    }

    #[test]
    fn positions_count_utf16_and_shift_msh() {
        let d = Delimiters::default();
        let text = "MSH|^~\\&|A\n\nPID|||😀||Müller";
        let p = positions(text, d);
        assert_eq!(p.len(), 2, "the blank line is no segment");
        assert_eq!(p[0].name, "MSH");
        assert_eq!(p[0].fields[0], [1, 3, 4], "MSH-1 is the separator");
        assert_eq!(p[0].fields[1], [2, 4, 8]);
        assert_eq!(p[0].fields[2], [3, 9, 10]);
        let pid = &p[1];
        assert_eq!((pid.start, pid.end), (12, 12 + 16));
        // PID-3 is the emoji: two UTF-16 units.
        assert_eq!(pid.fields[2], [3, 12 + 6, 12 + 8]);
        assert_eq!(pid.fields[4], [5, 12 + 10, 12 + 16]);
        assert_eq!(
            pid.fields[0],
            [1, 12 + 4, 12 + 4],
            "empty fields are listed"
        );
        let u16: Vec<u16> = text.encode_utf16().collect();
        assert_eq!(String::from_utf16(&u16[22..28]).unwrap(), "Müller");
    }

    #[test]
    fn the_preview_validates_with_sample_values() {
        let raw = "MSH|^~\\&|A|B|||{{now}}||ADT^A01|{{id}}|P|2.5\nEVN|A01|{{now}}\nPID|||1||Muster";
        let p = preview(raw, &BTreeMap::new(), None, false).unwrap();
        assert!(p.message.parse_error.is_none());
        let msh7 = &p.message.segments[0]
            .fields
            .iter()
            .find(|f| f.index == 7)
            .unwrap()
            .value;
        assert_eq!(msh7, "{{now}}", "the fields show the text as written");
        assert!(
            !p.message
                .validation_warnings
                .iter()
                .any(|w| w.field == Some(7) || w.message.contains("{{")),
            "{:?}",
            p.message.validation_warnings
        );
        // A real error is still reported.
        let raw = "MSH|^~\\&|A|B|||nonsense||ADT^A01|C1|P|2.5\nEVN|A01|20240101\nPID|||1||Muster";
        let p = preview(raw, &BTreeMap::new(), None, false).unwrap();
        assert!(!p.message.validation_warnings.is_empty());
        // Text that is not HL7 is a parse error, not a refusal.
        let p = preview("hello", &BTreeMap::new(), None, false).unwrap();
        assert!(p.message.parse_error.is_some());
    }

    #[test]
    fn files_are_decoded_like_frames() {
        let f = decode_file(b"\x0bMSH|^~\\&|A|B|||20240101||ADT^A01|C1|P|2.5||||||8859/1\rPID|||1||M\xfcller\r\x1c\r\x0bMSH|second\x1c\r");
        assert!(f.framed);
        assert_eq!(f.encoding, "windows-1252");
        assert!(f.raw.ends_with("PID|||1||Müller"));
        assert!(!f.raw.contains("second"));
        assert!(f.warning.is_none());
        let f = decode_file("MSH|^~\\&|A\r\nPID|||1||Jörg\r\n".as_bytes());
        assert!(!f.framed);
        assert_eq!(
            (f.raw.as_str(), f.encoding),
            ("MSH|^~\\&|A\nPID|||1||Jörg", "UTF-8")
        );
        let f = decode_file(b"MSH|^~\\&|A\rPID|||1||M\xfcller\r");
        assert!(f.warning.is_some(), "undeclared Latin-1 is named");
    }
}
