//! Byte → text decoding of an incoming MLLP payload, driven by MSH-18 (#188).
//!
//! The MLLP layer has to pick a charset *before* it has text to parse, so it
//! reads MSH-18 from the raw bytes. That lookup uses the parser's own
//! [`parse_delimiters`] and [`msh_split_index`], so an MSH header the parser
//! rejects never gets a charset applied, and "which element is MSH-18"
//! has a single definition.
//!
//! The declared charset isn't trusted blindly. When it doesn't fit the bytes
//! (a sender declaring `8859/1` but sending UTF-8 is the classic case), the
//! payload is decoded with the encoding that does fit. The result carries a
//! `CHARSET_FALLBACK` warning, so the mismatch is visible in the UI instead of
//! turning into silent mojibake.

use super::parser::{msh_split_index, parse_delimiters};
use crate::validation::ValidationWarning;
use encoding_rs::{Encoding, UTF_8, WINDOWS_1252};

/// Result of [`decode_payload`].
#[derive(Debug)]
pub struct DecodedPayload {
    /// The payload as text.
    pub text: String,
    /// MSH-18 as declared (first repetition, trimmed). `None` if absent, empty,
    /// or the MSH header is invalid.
    pub declared: Option<String>,
    /// Set when the bytes did not fit the declared (or default) charset and
    /// another encoding was used.
    pub warning: Option<ValidationWarning>,
    /// The encoding `text` was decoded with (`UTF_8` for ASCII and UTF-8
    /// payloads). Encoding `text` with it gives back the received bytes
    /// unless the decode replaced invalid bytes (#230).
    pub encoding: &'static Encoding,
}

/// Decode an MLLP payload, honouring its MSH-18 charset where it fits.
///
/// - **ASCII-only** payloads decode identically in every supported charset:
///   no lookup and no warning.
/// - **Declared and fitting:** decoded with the declared charset.
/// - **Declared single-byte charset (e.g. `8859/1`), but the bytes are valid
///   UTF-8:** decoded as UTF-8 with a warning. Non-ASCII Latin-1 text that is
///   also valid UTF-8 is practically non-existent, so this is the sender
///   mislabelling.
/// - **UTF-8 declared or nothing declared, but the bytes aren't valid UTF-8:**
///   decoded as Windows-1252 (a superset of Latin-1, the usual undeclared
///   charset) with a warning, instead of `U+FFFD` replacement characters.
/// - **Unsupported or unknown MSH-18:** UTF-8 if valid, else Windows-1252,
///   with a warning.
pub fn decode_payload(bytes: &[u8]) -> DecodedPayload {
    let declared = declared_charset(bytes);

    if bytes.is_ascii() {
        return DecodedPayload {
            // ASCII is valid UTF-8: no validation pass, no replacement.
            text: String::from_utf8_lossy(bytes).into_owned(),
            declared,
            warning: None,
            encoding: UTF_8,
        };
    }

    let as_utf8 = std::str::from_utf8(bytes).ok();
    let encoding = declared.as_deref().map(encoding_for);
    let utf8_expected = match encoding {
        None => true,                // nothing declared
        Some(Some(e)) => e == UTF_8, // UTF-8 declared
        Some(None) => false,         // unsupported, handled below
    };

    let (text, used, problem) = match (declared.as_deref().unwrap_or_default(), encoding) {
        _ if utf8_expected => match as_utf8 {
            Some(text) => (text.to_owned(), UTF_8, None),
            None => (
                windows_1252(bytes),
                WINDOWS_1252,
                Some(match declared.as_deref() {
                    None => "No MSH-18 charset is declared and the payload is not valid UTF-8; decoded as Windows-1252 (Latin-1)".to_owned(),
                    Some(cs) => format!("MSH-18 declares {cs:?}, but the payload is not valid UTF-8; decoded as Windows-1252 (Latin-1)"),
                }),
            ),
        },
        // A supported non-UTF-8 charset is declared.
        (cs, Some(Some(encoding))) => match as_utf8 {
            Some(text) if encoding.is_single_byte() => (
                text.to_owned(),
                UTF_8,
                Some(format!(
                    "MSH-18 declares {cs:?}, but the payload is valid UTF-8; decoded as UTF-8"
                )),
            ),
            _ => {
                let (text, had_errors) = encoding.decode_without_bom_handling(bytes);
                let problem = had_errors.then(|| {
                    format!("The payload contains bytes that are invalid in the declared MSH-18 charset {cs:?}; they were replaced")
                });
                (text.into_owned(), encoding, problem)
            }
        },
        // Declared, but not a charset we can decode with.
        (cs, _) => match as_utf8 {
            Some(text) => (
                text.to_owned(),
                UTF_8,
                Some(format!(
                    "MSH-18 charset {cs:?} is not supported; the payload is valid UTF-8 and was decoded as UTF-8"
                )),
            ),
            None => (
                windows_1252(bytes),
                WINDOWS_1252,
                Some(format!(
                    "MSH-18 charset {cs:?} is not supported; decoded as Windows-1252 (Latin-1)"
                )),
            ),
        },
    };

    DecodedPayload {
        text,
        declared,
        encoding: used,
        warning: problem.map(|message| ValidationWarning {
            code: "CHARSET_FALLBACK".to_owned(),
            message,
            segment: "MSH".to_owned(),
            field: Some(18),
        }),
    }
}

/// MSH-18 read from the raw bytes, using the same delimiter validation and
/// field numbering as the parser. Returns the first repetition, trimmed, or
/// `None` if MSH-18 is absent or empty, or the header is invalid.
pub fn declared_charset(bytes: &[u8]) -> Option<String> {
    // Same leniency as the parser: leading whitespace is ignored, and both
    // `\r` and `\n` end a segment.
    let bytes = bytes.trim_ascii_start();
    let end = bytes
        .iter()
        .position(|&b| b == b'\r' || b == b'\n')
        .unwrap_or(bytes.len());
    let header = &bytes[..end];
    if !header.starts_with(b"MSH") {
        return None;
    }
    let delimiters = parse_delimiters(header).ok()?;

    let msh18 = header
        .split(|&b| b == delimiters.field as u8)
        .nth(msh_split_index(18))?;
    // MSH-18 repeats; the first repetition is the message's charset.
    let first = msh18
        .split(|&b| b == delimiters.repetition as u8)
        .next()
        .unwrap_or_default();
    // Charset names are ASCII; anything else is not a usable label.
    let charset = std::str::from_utf8(first).ok()?.trim();
    (!charset.is_empty()).then(|| charset.to_owned())
}

/// Map an MSH-18 value to an ASCII-compatible encoding. Returns `None` for
/// charsets we can't decode an MLLP payload with: unknown labels, and UTF-16
/// or UTF-32, whose bytes couldn't have been framed and read as an ASCII MSH
/// header in the first place.
pub fn encoding_for(charset: &str) -> Option<&'static Encoding> {
    let upper = charset.trim().to_ascii_uppercase();
    // HL7 table 0211 names that aren't WHATWG labels.
    let label: &str = match upper.as_str() {
        "UNICODE UTF-8" => "utf-8",
        "ISO IR6" => "us-ascii",
        "ISO IR100" => "iso-8859-1",
        "GB 18030-2000" => "gb18030",
        "BIG-5" => "big5",
        "KS X 1001" => "euc-kr",
        "UNICODE" | "UNICODE UTF-16" | "UNICODE UTF-32" => return None,
        // `8859/1` → `iso-8859-1`; other values are tried as labels directly.
        other => {
            return match other.strip_prefix("8859/") {
                Some(part) => Encoding::for_label(format!("iso-8859-{part}").as_bytes()),
                None => Encoding::for_label(other.replace('/', "-").as_bytes()),
            }
            .filter(|e| e.is_ascii_compatible());
        }
    };
    Encoding::for_label(label.as_bytes()).filter(|e| e.is_ascii_compatible())
}

/// The bytes a changed message is sent as (#275): its text in the encoding
/// the original arrived in. A pure-ASCII original carries no evidence of its
/// encoding — it is stored as `WireForm::Utf8` whatever MSH-18 says — so for
/// it the charset MSH-18 declares is used, else UTF-8: an override that
/// brings an `ü` into a message declared `8859/1` must leave as Latin-1.
/// When the change gives MSH-18 another charset (`declared_now` differs
/// from `declared`, the original's), the bytes follow the new declaration
/// instead: no MSH-18 → UTF-8, a charset Harux cannot write is an error
/// naming it (#287 review) — the receiver reads the bytes by what MSH-18
/// says. A message whose bytes no encoding reproduces (`WireForm::Original`)
/// can only be resent unchanged. A character the encoding cannot write is an
/// error that names it — never a silent `?` or `&#252;`.
pub fn encode_for_send(
    text: &str,
    wire: &super::types::WireForm,
    original_is_ascii: bool,
    declared: Option<&str>,
    declared_now: Option<&str>,
) -> Result<Vec<u8>, String> {
    use super::types::WireForm;
    let encoding = match wire {
        WireForm::Original(_) => {
            return Err(
                "This message's bytes are not valid in its charset (they were kept as \
                 received); it can only be resent unchanged"
                    .into(),
            )
        }
        _ if declared_now != declared => match declared_now {
            None => UTF_8,
            Some(cs) => encoding_for(cs).ok_or_else(|| {
                format!("MSH-18 {cs:?} is not a charset Harux can write the message in")
            })?,
        },
        WireForm::Encoded(e) => *e,
        WireForm::Utf8 if original_is_ascii => declared.and_then(encoding_for).unwrap_or(UTF_8),
        WireForm::Utf8 => UTF_8,
    };
    encode_strict(text, encoding)
}

/// `text` in `encoding`. A character the encoding cannot write is an error
/// that names it — never a silent `?` or `&#252;` (#275, #276).
pub fn encode_strict(text: &str, encoding: &'static Encoding) -> Result<Vec<u8>, String> {
    if encoding == UTF_8 {
        return Ok(text.as_bytes().to_vec());
    }
    let (bytes, _, unmappable) = encoding.encode(text);
    if unmappable {
        let bad = text
            .chars()
            .find(|c| encoding.encode(c.encode_utf8(&mut [0; 4])).2)
            .unwrap_or('?');
        return Err(format!(
            "{bad:?} (U+{:04X}) cannot be written in {}, the message's encoding",
            bad as u32,
            encoding.name()
        ));
    }
    Ok(bytes.into_owned())
}

fn windows_1252(bytes: &[u8]) -> String {
    // Windows-1252 maps every byte, so this never fails.
    WINDOWS_1252
        .decode_without_bom_handling(bytes)
        .0
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MSH header `MSH{sep}{enc}` with every field up to MSH-17 empty and
    /// MSH-18 = `charset` — no hand-counted separators.
    fn header(sep: char, enc: &str, charset: &str) -> Vec<u8> {
        let mut h = format!("MSH{sep}{enc}");
        for _ in 3..=18 {
            h.push(sep);
        }
        h.push_str(charset);
        h.into_bytes()
    }

    /// Realistic `MSH|^~\&` message with MSH-18 = `charset` (empty → no
    /// MSH-18), then a PID whose last field is `body`.
    fn frame(charset: &str, body: &[u8]) -> Vec<u8> {
        let mut bytes = b"MSH|^~\\&|APP|FAC|REC|RFAC|20240101||ADT^A01|C1|P|2.5||||||".to_vec();
        bytes.extend_from_slice(charset.as_bytes());
        bytes.extend_from_slice(b"\rPID|||1||");
        bytes.extend_from_slice(body);
        bytes
    }

    fn code(d: &DecodedPayload) -> Option<&str> {
        d.warning.as_ref().map(|w| w.code.as_str())
    }

    #[test]
    fn ascii_payload_needs_no_decoding() {
        let d = decode_payload(&frame("8859/1", b"Smith"));
        assert!(d.text.ends_with("PID|||1||Smith"));
        assert_eq!(d.declared.as_deref(), Some("8859/1"));
        assert_eq!(code(&d), None);

        // Even an unsupported label is harmless when every byte is ASCII.
        let d = decode_payload(&frame("UNICODE UTF-16", b"Smith"));
        assert_eq!(code(&d), None);
    }

    #[test]
    fn declared_latin1_is_honoured() {
        let d = decode_payload(&frame("8859/1", b"M\xFCller"));
        assert!(d.text.ends_with("M\u{FC}ller"), "{}", d.text);
        assert_eq!(code(&d), None);

        let d = decode_payload(&frame("ISO IR100", b"M\xFCller"));
        assert!(d.text.ends_with("M\u{FC}ller"));
        assert_eq!(code(&d), None);
    }

    #[test]
    fn declared_utf8_is_honoured() {
        for cs in ["UNICODE UTF-8", "UTF-8", ""] {
            let d = decode_payload(&frame(cs, "Müller".as_bytes()));
            assert!(d.text.ends_with("Müller"), "{cs}: {}", d.text);
            assert_eq!(code(&d), None, "{cs}");
        }
    }

    /// The classic mislabel: Latin-1 declared, UTF-8 sent. Before #188 this
    /// produced `MÃ¼ller` without any hint.
    #[test]
    fn latin1_declared_but_utf8_sent_falls_back_to_utf8() {
        let d = decode_payload(&frame("8859/1", "Müller".as_bytes()));
        assert!(d.text.ends_with("Müller"), "{}", d.text);
        assert_eq!(d.declared.as_deref(), Some("8859/1"));
        let w = d.warning.unwrap();
        assert_eq!(w.code, "CHARSET_FALLBACK");
        assert_eq!((w.segment.as_str(), w.field), ("MSH", Some(18)));
        assert!(w.message.contains("8859/1"), "{}", w.message);
    }

    /// UTF-8 declared or nothing declared, but Latin-1 bytes. Before #188
    /// this produced `M\u{FFFD}ller`.
    #[test]
    fn invalid_utf8_falls_back_to_windows_1252() {
        for cs in ["UNICODE UTF-8", ""] {
            let d = decode_payload(&frame(cs, b"M\xFCller"));
            assert!(d.text.ends_with("M\u{FC}ller"), "{cs}: {}", d.text);
            assert_eq!(code(&d), Some("CHARSET_FALLBACK"), "{cs}");
        }
    }

    #[test]
    fn unsupported_charset_falls_back_with_warning() {
        let d = decode_payload(&frame("UNICODE UTF-16", "Müller".as_bytes()));
        assert!(d.text.ends_with("Müller"));
        assert_eq!(code(&d), Some("CHARSET_FALLBACK"));

        let d = decode_payload(&frame("NOT-A-CHARSET", b"M\xFCller"));
        assert!(d.text.ends_with("M\u{FC}ller"));
        assert_eq!(code(&d), Some("CHARSET_FALLBACK"));
    }

    #[test]
    fn multibyte_charset_with_invalid_bytes_warns() {
        // 0x81 0x20 is not a valid Big5 sequence.
        let d = decode_payload(&frame("BIG-5", b"\x81\x20"));
        assert_eq!(code(&d), Some("CHARSET_FALLBACK"));
        assert!(d.text.contains('\u{FFFD}'));
    }

    /// #188 divergence: the parser rejects an MSH-2 that repeats a delimiter,
    /// so no charset may be read from such a header either.
    #[test]
    fn invalid_msh2_yields_no_charset() {
        let mut bytes = header('|', "^^\\&", "8859/1");
        bytes.extend_from_slice(b"\rPID|||\xFC");
        assert_eq!(declared_charset(&bytes), None);
        assert!(super::super::parser::parse_message("MSH|^^\\&|A", "t").is_err());
        let d = decode_payload(&bytes);
        assert!(d.text.ends_with('\u{FC}'), "still readable: {}", d.text);
        assert_eq!(code(&d), Some("CHARSET_FALLBACK"));
    }

    /// The old byte scanner only stopped at `\r`, so with `\n` endings
    /// MSH-18 read as `8859/1\nPID…`. The parser accepts both.
    #[test]
    fn header_ends_at_cr_or_lf() {
        let mut bytes = header('|', "^~\\&", "8859/1");
        assert_eq!(declared_charset(&bytes).as_deref(), Some("8859/1"));
        bytes.extend_from_slice(b"\nPID|||1");
        assert_eq!(declared_charset(&bytes).as_deref(), Some("8859/1"));
    }

    #[test]
    fn leading_whitespace_is_ignored() {
        let mut bytes = b"\r\n MSH".to_vec();
        bytes.extend_from_slice(&header('|', "^~\\&", "8859/1")[3..]);
        bytes.extend_from_slice(b"\rPID");
        assert_eq!(declared_charset(&bytes).as_deref(), Some("8859/1"));
    }

    #[test]
    fn first_repetition_of_msh18_is_used() {
        let d = decode_payload(&frame("8859/1~UNICODE UTF-8", b"M\xFCller"));
        assert_eq!(d.declared.as_deref(), Some("8859/1"));
        assert!(d.text.ends_with("M\u{FC}ller"));
        assert_eq!(code(&d), None);
    }

    /// Non-default delimiters: MSH-18 is found with the message's own
    /// separator.
    #[test]
    fn custom_delimiters() {
        let mut bytes = header('#', "*~\\&", "8859/1");
        bytes.extend_from_slice(b"\rPID###\xFC");
        assert_eq!(declared_charset(&bytes).as_deref(), Some("8859/1"));
        // One element short: what used to be read as MSH-18 is MSH-17.
        let short = header('#', "*~\\&", "8859/1");
        let short = [&short[..8], &short[9..]].concat();
        assert_eq!(declared_charset(&short), None);
    }

    #[test]
    fn missing_or_empty_msh18() {
        assert_eq!(declared_charset(b"MSH|^~\\&|APP|FAC"), None);
        assert_eq!(declared_charset(&frame("", b"")), None);
        assert_eq!(declared_charset(&frame("  ", b"")), None);
        assert_eq!(declared_charset(b"PID|||1"), None);
        assert_eq!(declared_charset(b""), None);
    }

    /// `declared_charset` must agree with what the parser sees in MSH-18.
    #[test]
    fn declared_charset_matches_parsed_msh18() {
        for cs in ["8859/1", "UNICODE UTF-8", "8859/15~UNICODE UTF-8", ""] {
            let bytes = frame(cs, b"Smith");
            let msg =
                super::super::parser::parse_message(std::str::from_utf8(&bytes).unwrap(), "test")
                    .unwrap();
            let parsed = msg.segments[0]
                .fields
                .iter()
                .find(|f| f.index == 18)
                .map(|f| f.value.split('~').next().unwrap().trim().to_owned())
                .filter(|v| !v.is_empty());
            assert_eq!(declared_charset(&bytes), parsed, "{cs:?}");
        }
    }
}
