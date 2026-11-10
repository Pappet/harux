//! HL7 v2 escape sequences, in both directions (#185).
//!
//! Inside a field, the escape character (`\` by default) wraps a code:
//! `\F\` `\S\` `\T\` `\R\` `\E\` stand for the field, component,
//! sub-component, repetition and escape characters, and `\.br\` etc. are
//! formatting commands. [`encode`] writes free text into a field (used for the
//! ACK's MSA-3), and [`decode`] turns a field's *leaf* (one sub-component,
//! already split on the raw delimiters) back into display text.
//!
//! Decoding is only for display and analysis. `Hl7Field::value` and
//! `Hl7Message::raw` always stay exactly as received.

use super::types::Delimiters;
use std::borrow::Cow;

/// Escape free text so it can't break a field's structure. Line breaks
/// would end the segment, so they become spaces.
pub fn encode(text: &str, d: Delimiters) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let code = if c == d.escape {
            'E'
        } else if c == d.field {
            'F'
        } else if c == d.component {
            'S'
        } else if c == d.subcomponent {
            'T'
        } else if c == d.repetition {
            'R'
        } else if c == '\r' || c == '\n' {
            out.push(' ');
            continue;
        } else {
            out.push(c);
            continue;
        };
        out.push(d.escape);
        out.push(code);
        out.push(d.escape);
    }
    out
}

/// Decode the escape sequences in one leaf value.
///
/// | Sequence | Result |
/// |---|---|
/// | `\F\` `\S\` `\T\` `\R\` `\E\` | the message's own delimiter / escape character |
/// | `\.br\`, `\.sp\`, `\.sp n\`, `\.ce\` | line break (`\.sp n\`: `n` of them, at most 10) |
/// | `\.sk n\` | `n` spaces (at most 100) |
/// | `\H\` `\N\` `\.fi\` `\.nf\` `\.in n\` `\.ti n\` | dropped (highlighting / layout only) |
/// | anything else (`\Xhh\`, `\Zxx\`, `\C…\`, `\M…\`, unknown, unterminated) | kept verbatim |
///
/// `\Xhh\` stays verbatim on purpose: its bytes are in the message charset,
/// so decoding them correctly needs more than the escape table.
///
/// Returns `Cow::Borrowed` when there is nothing to decode, which is the
/// common case and allocation-free.
pub fn decode(text: &str, d: Delimiters) -> Cow<'_, str> {
    if !text.contains(d.escape) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(d.escape) {
        out.push_str(&rest[..start]);
        let after = &rest[start + d.escape.len_utf8()..];
        let Some(len) = after.find(d.escape) else {
            // Unterminated: keep the escape character and the rest as is.
            out.push_str(&rest[start..]);
            return Cow::Owned(out);
        };
        let code = &after[..len];
        if !decode_sequence(code, d, &mut out) {
            // Unknown or not decodable: keep the whole sequence verbatim.
            out.push(d.escape);
            out.push_str(code);
            out.push(d.escape);
        }
        rest = &after[len + d.escape.len_utf8()..];
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Append the decoded form of one sequence's `code` (the text between the
/// escape characters). Returns `false` if it should be kept verbatim.
fn decode_sequence(code: &str, d: Delimiters, out: &mut String) -> bool {
    match code {
        "F" => out.push(d.field),
        "S" => out.push(d.component),
        "T" => out.push(d.subcomponent),
        "R" => out.push(d.repetition),
        "E" => out.push(d.escape),
        "H" | "N" => {}
        ".br" | ".ce" => out.push('\n'),
        ".fi" | ".nf" => {}
        _ => {
            let Some((cmd, arg)) = formatting_command(code) else {
                return false;
            };
            match cmd {
                "sp" => (0..arg.unwrap_or(1).clamp(1, 10)).for_each(|_| out.push('\n')),
                "sk" => (0..arg.unwrap_or(1).min(100)).for_each(|_| out.push(' ')),
                "in" | "ti" => {}
                _ => return false,
            }
        }
    }
    true
}

/// Split `.sp 2`, `.sk3`, `.in -4` into the command and its numeric argument.
fn formatting_command(code: &str) -> Option<(&str, Option<u32>)> {
    let body = code.strip_prefix('.')?;
    let cmd = body.get(..2)?;
    let arg = body[2..].trim();
    if arg.is_empty() {
        return Some((cmd, None));
    }
    // `.in`/`.ti` take a signed indent; only the magnitude matters to us.
    let magnitude = arg.strip_prefix(['+', '-']).unwrap_or(arg);
    magnitude.parse().ok().map(|n| (cmd, Some(n)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d() -> Delimiters {
        Delimiters::default()
    }

    #[test]
    fn delimiter_escapes() {
        assert_eq!(
            decode("A\\F\\B\\S\\C\\T\\D\\R\\E\\E\\F", d()),
            "A|B^C&D~E\\F"
        );
    }

    #[test]
    fn plain_text_is_borrowed() {
        assert!(matches!(decode("Smith", d()), Cow::Borrowed("Smith")));
    }

    #[test]
    fn formatting_commands() {
        assert_eq!(decode("line1\\.br\\line2", d()), "line1\nline2");
        assert_eq!(decode("a\\.sp\\b", d()), "a\nb");
        assert_eq!(decode("a\\.sp 2\\b", d()), "a\n\nb");
        assert_eq!(decode("a\\.sk 3\\b", d()), "a   b");
        assert_eq!(decode("a\\.ce\\b", d()), "a\nb");
        assert_eq!(decode("\\H\\bold\\N\\", d()), "bold");
        assert_eq!(decode("\\.in +4\\a\\.ti -2\\b\\.fi\\\\.nf\\", d()), "ab");
        // Absurd counts are capped instead of blowing up the output.
        assert_eq!(decode("\\.sp 999999\\", d()).len(), 10);
        assert_eq!(decode("\\.sk 999999\\", d()).len(), 100);
    }

    #[test]
    fn unknown_and_unterminated_sequences_stay_verbatim() {
        assert_eq!(decode("\\X0D\\", d()), "\\X0D\\");
        assert_eq!(decode("\\Zfoo\\", d()), "\\Zfoo\\");
        assert_eq!(decode("\\.xx\\", d()), "\\.xx\\");
        assert_eq!(decode("\\.sp abc\\", d()), "\\.sp abc\\");
        assert_eq!(decode("a\\F", d()), "a\\F");
        assert_eq!(decode("a\\\\b", d()), "a\\\\b", "empty sequence");
    }

    /// The sender's own escape character and delimiters are used.
    #[test]
    fn custom_delimiters() {
        let d = Delimiters {
            field: '*',
            component: '$',
            repetition: '!',
            escape: '#',
            subcomponent: '%',
        };
        assert_eq!(decode("John#E#Doe#S#x", d), "John#Doe$x");
        assert_eq!(decode("a\\F\\b", d), "a\\F\\b", "`\\` is just text here");
    }

    #[test]
    fn encode_then_decode_round_trips() {
        let text = "a|b^c&d~e\\f";
        let encoded = encode(text, d());
        assert_eq!(encoded, "a\\F\\b\\S\\c\\T\\d\\R\\e\\E\\f");
        assert_eq!(decode(&encoded, d()), text);
        assert_eq!(encode("x\r\ny", d()), "x  y");
    }
}
