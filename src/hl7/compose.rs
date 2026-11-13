//! Writing fields into a message's text (#275) — the one place that does
//! (plan rule 4). A resend "with a new control ID" is the override
//! `MSH-10 = {{id}}`; the draft editor (M4-5) writes a changed field back
//! with the same code.
//!
//! An override replaces exactly one field's text. Segments are found the way
//! the parser finds them (split at `CR` and `LF`, blank lines skipped,
//! leading whitespace of the message ignored), fields by the message's own
//! field separator, and the MSH shift goes through [`msh_split_index`] — so
//! every other byte, the line ends and the final terminator stay as they
//! were.

use super::parser::msh_split_index;
use super::types::Delimiters;

/// Highest field number an override may name.
pub const MAX_FIELD: usize = 1000;

/// Why an override was refused. The text names the field and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeError {
    /// The path is not `SEG-n` / `SEG[k]-n` with a field Harux may write.
    BadPath(String),
    /// The message has no such segment (or not that many of them).
    MissingSegment(String),
    /// The value would leave its field.
    BadValue(String),
}

impl std::fmt::Display for ComposeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComposeError::BadPath(m)
            | ComposeError::MissingSegment(m)
            | ComposeError::BadValue(m) => f.write_str(m),
        }
    }
}

/// `MSH-10`, `OBX[2]-5`: a segment, its occurrence (from 1) and a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldPath {
    pub segment: String,
    pub occurrence: usize,
    pub field: usize,
}

impl FieldPath {
    pub fn parse(path: &str) -> Result<Self, ComposeError> {
        let bad = |why: &str| ComposeError::BadPath(format!("{path:?}: {why}"));
        let (seg, field) = path
            .trim()
            .rsplit_once('-')
            .ok_or_else(|| bad("write a field as SEG-n or SEG[k]-n, e.g. MSH-10 or OBX[2]-5"))?;
        let (segment, occurrence) = match seg.split_once('[') {
            Some((name, rest)) => {
                let k = rest
                    .strip_suffix(']')
                    .and_then(|k| k.parse::<usize>().ok())
                    .filter(|&k| k >= 1)
                    .ok_or_else(|| bad("the occurrence in [ ] must be 1 or more"))?;
                (name, k)
            }
            None => (seg, 1),
        };
        let segment = segment.to_ascii_uppercase();
        if segment.len() != 3 || !segment.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(bad("a segment name has three letters or digits"));
        }
        let field: usize = field
            .parse()
            .map_err(|_| bad("the field number is not a number"))?;
        let lowest = if segment == "MSH" { 3 } else { 1 };
        if field < lowest || field > MAX_FIELD {
            return Err(bad(&if segment == "MSH" {
                format!(
                    "MSH-1 and MSH-2 are the delimiters themselves; a field from {lowest} to {MAX_FIELD} can be set"
                )
            } else {
                format!("a field from {lowest} to {MAX_FIELD} can be set")
            }));
        }
        Ok(Self {
            segment,
            occurrence,
            field,
        })
    }
}

impl std::fmt::Display for FieldPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.occurrence == 1 {
            write!(f, "{}-{}", self.segment, self.field)
        } else {
            write!(f, "{}[{}]-{}", self.segment, self.occurrence, self.field)
        }
    }
}

/// A field's value may hold components, repetitions, sub-components and
/// escapes — it is the field's raw text — but nothing that ends the field,
/// the segment or the MLLP frame.
fn check_value(path: &FieldPath, value: &str, d: Delimiters) -> Result<(), ComposeError> {
    let bad = value
        .chars()
        .find(|&c| c == d.field || c == '\r' || c == '\n' || c == '\u{0B}' || c == '\u{1C}');
    match bad {
        None => Ok(()),
        Some(c) => Err(ComposeError::BadValue(format!(
            "{path}: the value contains {} — it would leave its field",
            match c {
                '\r' => "a carriage return (CR)".to_string(),
                '\n' => "a line feed (LF)".to_string(),
                '\u{0B}' => "the MLLP start byte (0x0B)".to_string(),
                '\u{1C}' => "the MLLP end byte (0x1C)".to_string(),
                c => format!("the field separator {c:?}"),
            }
        ))),
    }
}

/// The byte ranges of the message's segment lines, as the parser sees
/// them: leading whitespace of the message skipped, split at `CR`/`LF`,
/// blank lines left out.
pub fn segment_lines(raw: &str) -> Vec<std::ops::Range<usize>> {
    let start = raw.len() - raw.trim_start().len();
    let mut lines = Vec::new();
    let mut line_start = start;
    for (i, b) in raw.bytes().enumerate().skip(start) {
        if b == b'\r' || b == b'\n' {
            if !raw[line_start..i].trim().is_empty() {
                lines.push(line_start..i);
            }
            line_start = i + 1;
        }
    }
    if !raw[line_start..].trim().is_empty() {
        lines.push(line_start..raw.len());
    }
    lines
}

/// Write `value` into the field `path` of `raw`. A missing field is created
/// by adding field separators; a missing segment is an error.
pub fn set_field(
    raw: &str,
    d: Delimiters,
    path: &FieldPath,
    value: &str,
) -> Result<String, ComposeError> {
    check_value(path, value, d)?;
    let line = segment_lines(raw)
        .into_iter()
        .filter(|r| raw[r.clone()].split(d.field).next() == Some(path.segment.as_str()))
        .nth(path.occurrence - 1)
        .ok_or_else(|| {
            ComposeError::MissingSegment(format!(
                "{path}: the message has no {}{}",
                path.segment,
                if path.occurrence > 1 {
                    format!(" number {}", path.occurrence)
                } else {
                    String::new()
                }
            ))
        })?;
    // Index of the field among the line's separator-split parts: part 0 is
    // the segment name; for MSH the parser's shift applies.
    let index = if path.segment == "MSH" {
        msh_split_index(path.field)
    } else {
        path.field
    };
    // The byte range of every part of the line.
    let text = &raw[line.clone()];
    let mut parts = Vec::new();
    let mut part_start = 0;
    for (i, c) in text.char_indices() {
        if c == d.field {
            parts.push(part_start..i);
            part_start = i + c.len_utf8();
        }
    }
    parts.push(part_start..text.len());

    let mut out = String::with_capacity(raw.len() + value.len());
    match parts.get(index) {
        Some(part) => {
            out.push_str(&raw[..line.start + part.start]);
            out.push_str(value);
            out.push_str(&raw[line.start + part.end..]);
        }
        None => {
            // Missing: append separators up to the field, at the line's end.
            out.push_str(&raw[..line.end]);
            for _ in 0..(index - (parts.len() - 1)) {
                out.push(d.field);
            }
            out.push_str(value);
            out.push_str(&raw[line.end..]);
        }
    }
    Ok(out)
}

/// The segment names in order, as the parser would read them — to check
/// that overrides kept the list of segments.
pub fn segment_names(raw: &str, d: Delimiters) -> Vec<&str> {
    segment_lines(raw)
        .into_iter()
        .map(|r| raw[r].split(d.field).next().unwrap_or(""))
        .collect()
}

// --- Placeholders ---

/// What placeholders are filled in with.
#[derive(Debug, Clone)]
pub struct PlaceholderContext {
    /// Position of the message in its run, from 1 (`{{n}}`).
    pub n: usize,
    /// The moment of the send, in local time (`{{now}}`, `{{today}}`).
    pub now: chrono::DateTime<chrono::Local>,
}

/// Fill in the placeholders of an override value. Only these are
/// recognized: `{{now}}`, `{{today}}`, `{{id}}`, `{{n}}`, `{{rand:N}}`
/// (N = 1–32 digits) and `{{open}}` (a literal `{{`). Anything else in
/// double braces is left as written.
pub fn fill_placeholders(value: &str, ctx: &PlaceholderContext) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            out.push_str(&rest[open..]);
            return out;
        };
        let name = &after[..close];
        match placeholder(name, ctx) {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[open..open + 2 + close + 2]),
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

/// The value of the placeholder `{{name}}`, `None` if `name` is not one.
/// `{{open}}` is a literal `{{` (#276): the escape a draft uses for text
/// that only looks like a placeholder.
pub fn placeholder(name: &str, ctx: &PlaceholderContext) -> Option<String> {
    match name {
        "open" => Some("{{".into()),
        "now" => Some(ctx.now.format("%Y%m%d%H%M%S").to_string()),
        "today" => Some(ctx.now.format("%Y%m%d").to_string()),
        "id" => Some(super::parser::new_control_id()),
        "n" => Some(ctx.n.to_string()),
        _ => {
            let digits: usize = name.strip_prefix("rand:")?.parse().ok()?;
            (1..=32).contains(&digits).then(|| random_digits(digits))
        }
    }
}

/// `n` random decimal digits, from v4 UUIDs (`uuid` is a dependency, `rand`
/// is not). Each UUID gives 122 random bits; 30 digits per UUID keeps the
/// modulo bias below 1e-6.
fn random_digits(n: usize) -> String {
    let mut out = String::with_capacity(n);
    while out.len() < n {
        let r = uuid::Uuid::new_v4().as_u128() >> 6; // the 122 random bits
        let chunk = format!("{:030}", r % 10u128.pow(30));
        out.push_str(&chunk[..(n - out.len()).min(30)]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: Delimiters = Delimiters {
        field: '|',
        component: '^',
        repetition: '~',
        escape: '\\',
        subcomponent: '&',
    };
    const MSG: &str = "MSH|^~\\&|A|B|C|D|20240101||ADT^A01|C1|P|2.5\r\nPID|||1||Muster^Max\rOBX|1|ST|X||one\rOBX|2|ST|X||two\r";

    fn set(raw: &str, path: &str, value: &str) -> Result<String, ComposeError> {
        set_field(raw, D, &FieldPath::parse(path).unwrap(), value)
    }

    #[test]
    fn paths() {
        let p = FieldPath::parse("obx[2]-5").unwrap();
        assert_eq!((p.segment.as_str(), p.occurrence, p.field), ("OBX", 2, 5));
        assert_eq!(p.to_string(), "OBX[2]-5");
        assert_eq!(FieldPath::parse("MSH-10").unwrap().to_string(), "MSH-10");
        for bad in [
            "MSH-1", "MSH-2", "PID-0", "PID-1001", "PID", "PIDX-1", "OBX[0]-5", "OBX[x]-5", "P-1",
            "PID-a",
        ] {
            assert!(
                matches!(FieldPath::parse(bad), Err(ComposeError::BadPath(_))),
                "{bad}"
            );
        }
        assert!(FieldPath::parse("PID-1").is_ok());
        assert!(FieldPath::parse("ZZ1-1000").is_ok());
    }

    /// Only the one field changes: the line ends (CR LF, CR) and the final
    /// terminator stay; the MSH shift is the parser's.
    #[test]
    fn one_field_changes_and_nothing_else() {
        let out = set(MSG, "MSH-10", "NEW").unwrap();
        assert_eq!(out, MSG.replace("|C1|", "|NEW|"));
        let msg = crate::hl7::parser::parse_message(&out, "t").unwrap();
        assert_eq!(msg.message_control_id, "NEW");

        let out = set(MSG, "MSH-7", "20261113120000").unwrap();
        assert!(out.starts_with("MSH|^~\\&|A|B|C|D|20261113120000||ADT^A01|C1|"));

        let out = set(MSG, "OBX[2]-5", "zwei^b~c\\T\\").unwrap();
        assert_eq!(out, MSG.replace("||two\r", "||zwei^b~c\\T\\\r"));
        assert_eq!(
            set(MSG, "OBX-5", "eins").unwrap(),
            MSG.replace("||one\r", "||eins\r")
        );
    }

    #[test]
    fn missing_fields_are_created_missing_segments_refused() {
        let out = set(MSG, "PID-8", "M").unwrap();
        assert!(out.contains("\r\nPID|||1||Muster^Max|||M\r"), "{out:?}");
        let out = set(MSG, "MSH-18", "UNICODE UTF-8").unwrap();
        assert!(
            out.starts_with("MSH|^~\\&|A|B|C|D|20240101||ADT^A01|C1|P|2.5||||||UNICODE UTF-8\r\n")
        );
        assert_eq!(
            crate::hl7::charset::declared_charset(out.as_bytes()).as_deref(),
            Some("UNICODE UTF-8")
        );
        assert!(matches!(
            set(MSG, "PV1-2", "I"),
            Err(ComposeError::MissingSegment(_))
        ));
        assert!(matches!(
            set(MSG, "OBX[3]-5", "x"),
            Err(ComposeError::MissingSegment(_))
        ));
    }

    #[test]
    fn values_that_leave_their_field_are_refused() {
        for bad in ["a|b", "a\rb", "a\nb", "a\u{0B}b", "a\u{1C}\rb"] {
            let e = set(MSG, "PID-5", bad).unwrap_err();
            assert!(matches!(e, ComposeError::BadValue(_)), "{bad:?}: {e}");
        }
        let e = set(MSG, "PID-5", "x\u{1C}").unwrap_err();
        assert!(e.to_string().contains("MLLP end byte"), "{e}");
    }

    #[test]
    fn custom_delimiters_and_leading_whitespace() {
        let d = Delimiters {
            field: '#',
            component: '$',
            repetition: '*',
            escape: '!',
            subcomponent: '@',
        };
        let raw = "\r\n  MSH#$*!@#A#B#C#D#2024##ADT$A01#OLD#P#2.5\rPID###1\r";
        let out = set_field(raw, d, &FieldPath::parse("MSH-10").unwrap(), "NEW").unwrap();
        assert_eq!(out, raw.replace("#OLD#", "#NEW#"));
        assert_eq!(segment_names(&out, d), ["MSH", "PID"]);
    }

    #[test]
    fn placeholders() {
        let ctx = PlaceholderContext {
            n: 3,
            now: chrono::Local::now(),
        };
        let now = fill_placeholders("{{now}}", &ctx);
        assert_eq!(now.len(), 14);
        assert!(now.bytes().all(|b| b.is_ascii_digit()));
        assert_eq!(fill_placeholders("{{today}}", &ctx), now[..8]);
        let id = fill_placeholders("{{id}}", &ctx);
        assert_eq!(id.len(), 20);
        assert_ne!(id, fill_placeholders("{{id}}", &ctx), "a new one each time");
        assert_eq!(fill_placeholders("RUN-{{n}}", &ctx), "RUN-3");
        let r = fill_placeholders("{{rand:6}}", &ctx);
        assert!(r.len() == 6 && r.bytes().all(|b| b.is_ascii_digit()), "{r}");
        assert_eq!(fill_placeholders("{{rand:45}}", &ctx), "{{rand:45}}");
        assert_eq!(fill_placeholders("{{rand:0}}", &ctx), "{{rand:0}}");
        assert_eq!(
            fill_placeholders("{{from:OBX-5}} {{x}} {{", &ctx),
            "{{from:OBX-5}} {{x}} {{"
        );
        assert_eq!(fill_placeholders("{{n}}{{n}}", &ctx), "33");
        assert_eq!(random_digits(70).len(), 70);
    }
}
