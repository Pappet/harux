use super::types::*;
use std::borrow::Cow;

/// Parse a raw HL7 v2.x message string into a structured Hl7Message.
/// Handles standard and custom delimiters from MSH segment.
pub fn parse_message(raw: &str, source_addr: &str) -> Result<Hl7Message, String> {
    let (msg, delimiters) = parse_structure(raw, source_addr)?;
    let msg = enrich_msh(msg, delimiters);
    let msg = enrich_pid(msg, delimiters);
    let msg = inject_descriptions(msg);
    let msg = annotate_message_type(msg);
    let mut msg = msg;
    msg.validation_warnings = validate(&msg);

    Ok(msg)
}

fn parse_structure(raw: &str, source_addr: &str) -> Result<(Hl7Message, Delimiters), String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("Empty message".into());
    }

    // HL7 messages must start with MSH
    if !raw.starts_with("MSH") {
        // Quote by characters, not bytes: a byte cut could split a
        // multi-byte character and panic (#222).
        let start: String = raw.chars().take(20).collect();
        return Err(format!("Message does not start with MSH: {start:?}"));
    }

    // Extract delimiters from MSH-1 (field sep) and MSH-2 (encoding chars)
    let delimiters = parse_delimiters(raw.as_bytes())?;
    let mut msg = Hl7Message::new_empty(raw.to_string(), source_addr.to_string());
    msg.delimiters = delimiters;

    // Split into segments (HL7 uses \r as segment terminator, but be lenient)
    for seg_str in raw.split(['\r', '\n']).filter(|s| !s.trim().is_empty()) {
        let segment = parse_segment(seg_str, delimiters);
        msg.segments.push(segment);
    }

    Ok((msg, delimiters))
}

fn enrich_msh(mut msg: Hl7Message, delimiters: Delimiters) -> Hl7Message {
    // Extract key fields from MSH
    if let Some(msh) = msg.segments.first() {
        // Use HL7-standard field numbers (1-based): MSH-1=separator, MSH-2=encoding chars, etc.
        msg.sending_application = get_field_value(msh, 3);
        msg.sending_facility = get_field_value(msh, 4);
        msg.receiving_application = get_field_value(msh, 5);
        msg.receiving_facility = get_field_value(msh, 6);

        // MSH-9: Message Type (e.g. ADT^A01^ADT_A01)
        let msg_type_field = get_field_value(msh, 9);
        let mut type_components = msg_type_field.split(delimiters.component);
        if let Some(c0) = type_components.next() {
            if let Some(c1) = type_components.next() {
                msg.message_type = format!("{}^{}", c0, c1);
                msg.trigger_event = c1.to_string();
                // MSH-9.3: optional message structure identifier (e.g. "ADT_A01")
                if let Some(c2) = type_components.next() {
                    if !c2.is_empty() {
                        msg.message_structure = Some(c2.to_string());
                    }
                }
            } else {
                msg.message_type = c0.to_string();
            }
        }

        msg.message_control_id = get_field_value(msh, 10);
        msg.version = get_field_value(msh, 12);
    }
    msg
}

fn enrich_pid(mut msg: Hl7Message, d: Delimiters) -> Hl7Message {
    let Some(pid) = msg.segments.iter().find(|s| s.name == "PID") else {
        return msg;
    };
    let field = |index| pid.fields.iter().find(|f| f.index == index);

    // PID-3 (CX, repeating): the ID itself is CX.1 of the first repetition,
    // before `^^^authority` and before any `~second-id` (#185).
    let patient_id = field(3)
        .and_then(|f| f.leaf(d, 0, 0, 0))
        .filter(|id| !id.is_empty())
        .map(Cow::into_owned);

    // PID-5 (XPN, repeating): "Family, Given" from the first repetition.
    // XPN.1 is itself composite (FN: Surname&Prefix…), so take its first
    // sub-component. Escapes are decoded (`O\T\Brien` → `O&Brien`).
    let patient_name = field(5).filter(|f| !f.value.is_empty()).and_then(|f| {
        let family = f.leaf(d, 0, 0, 0).unwrap_or_default();
        let name = match f.leaf(d, 0, 1, 0) {
            Some(given) => format!("{family}, {given}"),
            None => family.into_owned(),
        };
        (!name.is_empty()).then_some(name)
    });

    msg.patient_id = patient_id;
    msg.patient_name = patient_name;
    msg
}

fn inject_descriptions(mut msg: Hl7Message) -> Hl7Message {
    // Second pass: inject field descriptions from the embedded dictionary
    crate::dictionary::inject_descriptions(&mut msg.segments);
    msg
}

fn annotate_message_type(mut msg: Hl7Message) -> Hl7Message {
    // Third pass: look up message type description and typical segments
    if let Some(info) = super::message_types::get_message_type_info(&msg.message_type) {
        msg.message_type_description = Some(info.description.to_string());
        msg.typical_segments = info
            .typical_segments
            .iter()
            .map(|s| s.to_string())
            .collect();
        msg.typical_segment_descriptions = info
            .typical_segments
            .iter()
            .filter_map(|s| {
                crate::dictionary::get_segment_description(s).map(|d| (s.to_string(), d))
            })
            .collect();
    }
    msg
}

fn validate(msg: &Hl7Message) -> Vec<crate::validation::ValidationWarning> {
    // Fourth pass: validate required segments and fields
    crate::validation::validate_message(msg)
}

/// Extract and validate the delimiters from an MSH header (`MSH|^~\&…`).
///
/// Works on bytes so the MLLP layer can read MSH-18 *before* it knows the
/// payload's charset (#188): delimiters are printable ASCII, which is the
/// same byte in every ASCII-compatible encoding and never part of a UTF-8
/// multi-byte sequence. The single validation for both the parser and the
/// charset lookup — keep it that way.
pub(crate) fn parse_delimiters(bytes: &[u8]) -> Result<Delimiters, String> {
    // MSH|^~\&  ->  field=|, component=^, repetition=~, escape=\, subcomponent=&
    if bytes.len() < 8 {
        return Err("MSH segment too short to extract delimiters".into());
    }

    let field_sep = bytes[3] as char;

    if !field_sep.is_ascii_graphic() {
        return Err(format!(
            "MSH-1 field separator 0x{:02X} is not printable ASCII",
            bytes[3]
        ));
    }

    // MSH-2 must contain exactly 4 printable ASCII chars, each distinct from
    // the field separator and from each other. Violating any rule produces
    // garbage field splits downstream (e.g. MSH||||| makes component == '|').
    let enc = &bytes[4..8];
    for (i, &b) in enc.iter().enumerate() {
        if !b.is_ascii_graphic() {
            return Err(format!(
                "MSH-2 encoding character {} (0x{:02X}) is not printable ASCII",
                i + 1,
                b
            ));
        }
        if b == bytes[3] {
            return Err(format!(
                "MSH-2 encoding character {} (0x{:02X}) duplicates the field separator",
                i + 1,
                b
            ));
        }
    }
    for i in 0..4 {
        for j in (i + 1)..4 {
            if enc[i] == enc[j] {
                return Err(format!(
                    "MSH-2 encoding characters are not distinct: positions {} and {} are both 0x{:02X}",
                    i + 1,
                    j + 1,
                    enc[i]
                ));
            }
        }
    }

    Ok(Delimiters {
        field: field_sep,
        component: enc[0] as char,
        repetition: enc[1] as char,
        escape: enc[2] as char,
        subcomponent: enc[3] as char,
    })
}

/// Position of field MSH-`n` (HL7 numbering, `n >= 2`) in
/// `msh.split(field_separator)`: element 0 is `"MSH"`, and MSH-1 *is* the
/// separator, so MSH-2 is element 1. This is the same shift `parse_segment`
/// applies; `test_msh_split_index_matches_parser` keeps the two in sync.
pub(crate) const fn msh_split_index(n: usize) -> usize {
    n - 1
}

fn parse_segment(raw: &str, delimiters: Delimiters) -> Hl7Segment {
    let sep = delimiters.field;
    let mut parts = raw.split(sep);
    let name = parts.next().unwrap_or("???").to_string();

    let mut fields = Vec::new();

    // For MSH, field indexing is special: MSH-1 is the separator itself
    // Only the raw value is stored; repetitions / components /
    // sub-components / escapes are derived on demand (#180, #185).
    for (i, part) in parts.enumerate() {
        fields.push(Hl7Field {
            index: i + 1,
            value: part.to_string(),
            description: None,
        });
    }

    // For MSH, align indices with the HL7 standard (MSH-1 = separator, MSH-2 = encoding chars, ...).
    // The split-based loop assigns index i starting at 1, which maps to HL7 MSH-2 onwards,
    // so shift everything up by 1 and insert the separator as MSH-1.
    if name == "MSH" {
        for field in fields.iter_mut() {
            field.index += 1;
        }
        fields.insert(
            0,
            Hl7Field {
                index: 1,
                value: sep.to_string(),
                description: None,
            },
        );
    }

    Hl7Segment {
        name,
        fields,
        description: None,
        delimiters,
    }
}

/// Get field value by HL7 field number (1-based standard numbering).
/// For MSH: index 1 = field separator, 2 = encoding chars, 3 = sending app, etc.
fn get_field_value(segment: &Hl7Segment, index: usize) -> String {
    segment
        .fields
        .iter()
        .find(|f| f.index == index)
        .map(|f| f.value.clone())
        .unwrap_or_default()
}

/// MSH-12 used when the original message declares no version, and for NACKs
/// of unparseable frames (no original version known).
const DEFAULT_ACK_VERSION: &str = "2.5";

/// Everything that differs between an ACK and a NACK. The MSH/MSA layout
/// itself lives in [`render_ack`] only, so the two can't drift apart (#187).
struct AckSpec<'a> {
    /// Encoding of the ACK. ACKs reuse the original message's delimiters, so
    /// values echoed from it (MSH-3/4 → MSH-5/6, MSH-10 → MSA-2, MSH-11)
    /// stay valid verbatim even for senders with non-default delimiters.
    delimiters: Delimiters,
    receiving_application: &'a str,
    receiving_facility: &'a str,
    /// MSH-9.2 of the original; empty → MSH-9 is plain `ACK`.
    trigger_event: &'a str,
    /// MSH-11 of the original (`P`/`T`/`D`); empty → `P`.
    processing_id: &'a str,
    /// MSH-12 of the original; empty → [`DEFAULT_ACK_VERSION`].
    version: &'a str,
    /// MSA-1: `AA`, `AE` or `AR`.
    ack_code: &'a str,
    /// MSA-2: MSH-10 of the message being acknowledged; empty → `UNKNOWN`
    /// (MSA-2 is required, same fallback as the NACK).
    acknowledged_control_id: &'a str,
    /// MSA-3 free text; escaped before it is written.
    text: Option<&'a str>,
}

/// Render an [`AckSpec`] as `MSH…\rMSA…`. Every ACK/NACK gets a fresh
/// 20-character MSH-10 control ID.
fn render_ack(spec: &AckSpec) -> String {
    let d = spec.delimiters;
    let f = d.field;
    let message_type = if spec.trigger_event.is_empty() {
        "ACK".to_string()
    } else {
        format!("ACK{}{}", d.component, spec.trigger_event)
    };
    let processing_id = non_empty_or(spec.processing_id, "P");
    let version = non_empty_or(spec.version, DEFAULT_ACK_VERSION);
    let acknowledged_control_id = non_empty_or(spec.acknowledged_control_id, "UNKNOWN");
    let control_id = &uuid::Uuid::new_v4().simple().to_string()[..20];
    let now = chrono::Utc::now().format("%Y%m%d%H%M%S");

    let mut ack = format!(
        "MSH{f}{}{}{}{}{f}Harux{f}Harux{f}{}{f}{}{f}{now}{f}{f}{message_type}{f}{control_id}{f}{processing_id}{f}{version}\rMSA{f}{}{f}{}",
        d.component,
        d.repetition,
        d.escape,
        d.subcomponent,
        spec.receiving_application,
        spec.receiving_facility,
        spec.ack_code,
        acknowledged_control_id,
    );
    if let Some(text) = spec.text {
        ack.push(f);
        ack.push_str(&super::escape::encode(text, d));
    }
    ack
}

fn non_empty_or<'a>(value: &'a str, default: &'a str) -> &'a str {
    if value.is_empty() {
        default
    } else {
        value
    }
}

/// Build an ACK for a successfully parsed message.
pub fn build_ack(original: &Hl7Message, ack_code: &str) -> String {
    let delimiters = original.delimiters;
    let processing_id = original
        .segments
        .first()
        .filter(|msh| msh.name == "MSH")
        .map(|msh| get_field_value(msh, 11))
        .unwrap_or_default();
    render_ack(&AckSpec {
        delimiters,
        receiving_application: &original.sending_application,
        receiving_facility: &original.sending_facility,
        trigger_event: &original.trigger_event,
        processing_id: &processing_id,
        version: &original.version,
        ack_code,
        acknowledged_control_id: &original.message_control_id,
        text: None,
    })
}

/// Build a NACK (`AE`, Application Error) for a frame that could not be
/// parsed. Nothing of the original is trustworthy, so it uses default
/// delimiters, MSH-9 `ACK` and MSA-2 `UNKNOWN`; `reason` goes into MSA-3.
pub fn build_nack(reason: &str) -> String {
    render_ack(&AckSpec {
        delimiters: Delimiters::default(),
        receiving_application: "",
        receiving_facility: "",
        trigger_event: "",
        processing_id: "",
        version: "",
        ack_code: "AE",
        acknowledged_control_id: "UNKNOWN",
        text: Some(reason),
    })
}

/// Build a reject (`AR`, Application Reject) for a frame Harux refuses
/// outright, e.g. one above `mllp.max_message_size_mb` (#223). Default
/// delimiters like the NACK; MSA-2 echoes `control_id` when it could be read
/// (empty → `UNKNOWN`), `reason` goes into MSA-3.
pub fn build_reject(control_id: &str, reason: &str) -> String {
    render_ack(&AckSpec {
        delimiters: Delimiters::default(),
        receiving_application: "",
        receiving_facility: "",
        trigger_event: "",
        processing_id: "",
        version: "",
        ack_code: "AR",
        acknowledged_control_id: control_id,
        text: Some(reason),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_ADT: &str = "MSH|^~\\&|SENDING_APP|SENDING_FAC|REC_APP|REC_FAC|20240101120000||ADT^A01^ADT_A01|MSG00001|P|2.5\rPID|||12345^^^HOSP||Smith^John^Peter||19800515|M\rPV1||I|WARD1^ROOM1^BED1";

    /// #185: structure is derived from `value` on demand — repetitions,
    /// components and sub-components, escape-decoded — and nothing but
    /// `value` is stored.
    #[test]
    fn test_field_structure_accessors() {
        let msg = parse_message(SAMPLE_ADT, "127.0.0.1:9999").unwrap();
        let d = msg.delimiters;
        assert_eq!(d, Delimiters::default());
        let pid = msg.segments.iter().find(|s| s.name == "PID").unwrap();
        let pid5 = pid.fields.iter().find(|f| f.index == 5).unwrap();
        assert!(pid5.is_structured(d));
        assert_eq!(pid5.leaf(d, 0, 0, 0).as_deref(), Some("Smith"));
        assert_eq!(pid5.leaf(d, 0, 2, 0).as_deref(), Some("Peter"));
        assert_eq!(pid5.leaf(d, 0, 3, 0), None);
        assert_eq!(pid5.leaf(d, 1, 0, 0), None, "no second repetition");

        let pid7 = pid.fields.iter().find(|f| f.index == 7).unwrap();
        assert!(!pid7.is_structured(d));
        assert_eq!(pid7.leaf(d, 0, 0, 0).as_deref(), Some("19800515"));
        assert_eq!(pid7.leaf(d, 0, 1, 0), None);
    }

    #[test]
    fn test_repetitions_subcomponents_and_escapes() {
        let raw = "MSH|^~\\&|A|B|C|D|20240101||ADT^A01|1|P|2.5\r\
                   PID|||111^^^HOSP&1.2.3&ISO~222^^^LAB||O\\T\\Brien&van^Hans~Alias^X";
        let msg = parse_message(raw, "t").unwrap();
        let d = msg.delimiters;
        let pid = &msg.segments[1];
        let pid3 = pid.fields.iter().find(|f| f.index == 3).unwrap();
        assert_eq!(pid3.repetitions(d).count(), 2);
        assert_eq!(pid3.leaf(d, 1, 0, 0).as_deref(), Some("222"));
        assert_eq!(pid3.leaf(d, 0, 3, 1).as_deref(), Some("1.2.3"));
        assert_eq!(
            pid3.structure(d),
            vec![
                vec![
                    vec!["111"],
                    vec![""],
                    vec![""],
                    vec!["HOSP", "1.2.3", "ISO"]
                ],
                vec![vec!["222"], vec![""], vec![""], vec!["LAB"]],
            ]
        );
        let pid5 = pid.fields.iter().find(|f| f.index == 5).unwrap();
        // `\T\` is a literal `&`, not a sub-component separator.
        assert_eq!(pid5.leaf(d, 0, 0, 0).as_deref(), Some("O&Brien"));
        assert_eq!(pid5.leaf(d, 0, 0, 1).as_deref(), Some("van"));
        assert!(matches!(pid5.leaf(d, 0, 0, 1), Some(Cow::Borrowed("van"))));

        // Summary fields come from the first repetition, decoded.
        assert_eq!(msg.patient_id.as_deref(), Some("111"));
        assert_eq!(msg.patient_name.as_deref(), Some("O&Brien, Hans"));
    }

    #[test]
    fn test_patient_summary_edge_cases() {
        let parse = |pid: &str| {
            let raw = format!("MSH|^~\\&|A|B|C|D|20240101||ADT^A01|1|P|2.5\r{pid}");
            parse_message(&raw, "t").unwrap()
        };
        // Family only, no given-name component: no dangling ", ".
        let m = parse("PID|||1||Smith");
        assert_eq!(m.patient_name.as_deref(), Some("Smith"));
        // Empty ID before the authority: no empty patient ID.
        let m = parse("PID|||^^^HOSP||^John");
        assert_eq!(m.patient_id, None);
        assert_eq!(m.patient_name.as_deref(), Some(", John"));
        // No PID-5 at all.
        let m = parse("PID|||1");
        assert_eq!(m.patient_name, None);
    }

    /// #180: a large single-component payload (Base64 OBX-5) is held once in
    /// `raw` and once as the field value — not additionally as a segment copy
    /// and as `components[0]`. Descriptions are borrowed from the dictionary.
    #[test]
    fn test_large_payload_is_not_duplicated() {
        let b64 = "QUJD".repeat(256 * 1024); // 1 MiB
        let raw = format!(
            "MSH|^~\\&|APP|FAC|RCV|RFAC|20240101||MDM^T02|C1|P|2.5\rPID|||1\rOBX|1|ED|DOC||{b64}"
        );
        let msg = parse_message(&raw, "127.0.0.1:9999").unwrap();
        let obx5 = msg.segments[2]
            .fields
            .iter()
            .find(|f| f.index == 5)
            .unwrap();
        assert!(!obx5.is_structured(msg.delimiters));
        let bytes = msg.estimated_bytes();
        assert!(
            bytes < raw.len() * 2 + 16 * 1024,
            "estimated {bytes} bytes for {} raw — payload held more than twice",
            raw.len()
        );
        assert!(matches!(
            obx5.description,
            Some(std::borrow::Cow::Borrowed(_))
        ));
    }

    /// #185: an MDM attachment as a real ED value (`^application^pdf^Base64^…`)
    /// is structured, but its components are derived from `value` — the
    /// payload is not copied into a component (it used to be, via
    /// `components`), and reading it is zero-copy.
    #[test]
    fn test_ed_payload_is_not_duplicated() {
        let b64 = "QUJD".repeat(256 * 1024); // 1 MiB
        let raw = format!(
            "MSH|^~\\&|APP|FAC|RCV|RFAC|20240101||MDM^T02|C1|P|2.5\rOBX|1|ED|DOC||^application^pdf^Base64^{b64}"
        );
        let msg = parse_message(&raw, "t").unwrap();
        let d = msg.delimiters;
        let obx5 = msg.segments[1]
            .fields
            .iter()
            .find(|f| f.index == 5)
            .unwrap();
        assert!(obx5.is_structured(d));
        assert!(matches!(obx5.leaf(d, 0, 4, 0), Some(Cow::Borrowed(p)) if p.len() == b64.len()));
        let bytes = msg.estimated_bytes();
        assert!(
            bytes < raw.len() * 2 + 16 * 1024,
            "estimated {bytes} bytes for {} raw — ED payload held more than twice",
            raw.len()
        );
    }

    /// Contract the UI's "copy segment" relies on now that segments carry no
    /// text of their own (#180): segment `i` is the `i`-th non-blank line of
    /// `raw` when split on `\r` / `\n` (see `segmentLines` in static/app.js).
    #[test]
    fn test_segments_match_nonblank_raw_lines() {
        let raw = "MSH|^~\\&|A|B|C|D|20240101||ADT^A01|1|P|2.5\r\nPID|||1\n\nPV1||I\r";
        let msg = parse_message(raw, "127.0.0.1:9999").unwrap();
        let lines: Vec<&str> = msg
            .raw
            .split(['\r', '\n'])
            .filter(|s| !s.trim().is_empty())
            .collect();
        assert_eq!(lines.len(), msg.segments.len());
        for (line, seg) in lines.iter().zip(&msg.segments) {
            // Rebuild the line from the parsed segment: it must match the raw
            // line exactly, not just share a prefix. MSH skips the synthetic
            // MSH-1 (the separator itself).
            let skip = usize::from(seg.name == "MSH");
            let rebuilt = std::iter::once(seg.name.as_str())
                .chain(seg.fields.iter().skip(skip).map(|f| f.value.as_str()))
                .collect::<Vec<_>>()
                .join("|");
            assert_eq!(
                *line, rebuilt,
                "segment {} does not match its raw line",
                seg.name
            );
        }
    }

    /// Regression guard for the MSH field-index shift (#189).
    ///
    /// `parse_segment` inserts a synthetic MSH-1 (`"|"`) so MSH fields use
    /// HL7 numbering. That only lines up with the embedded dictionary because
    /// `v2.5.1.json` numbers MSH from seq 1 = Field Separator. If either side
    /// changes (parser shift or dictionary schema, e.g. for #49), every MSH
    /// field description, `enrich_msh` extraction and datatype check shifts
    /// by one *silently* — this test makes it fail loudly instead.
    #[test]
    fn test_msh_shift_dictionary_alignment() {
        let msg = parse_message(SAMPLE_ADT, "127.0.0.1:9999").unwrap();
        let msh = &msg.segments[0];
        assert_eq!(msh.name, "MSH");

        let field = |i: usize| {
            msh.fields
                .iter()
                .find(|f| f.index == i)
                .unwrap_or_else(|| panic!("MSH-{i} missing"))
        };
        // (index, expected value, expected dictionary description)
        let expected = [
            (1, "|", "Field Separator"),
            (2, "^~\\&", "Encoding Characters"),
            (3, "SENDING_APP", "Sending Application"),
            (4, "SENDING_FAC", "Sending Facility"),
            (5, "REC_APP", "Receiving Application"),
            (6, "REC_FAC", "Receiving Facility"),
            (7, "20240101120000", "Date/Time Of Message"),
            (9, "ADT^A01^ADT_A01", "Message Type"),
            (10, "MSG00001", "Message Control ID"),
            (11, "P", "Processing ID"),
            (12, "2.5", "Version ID"),
        ];
        for (i, value, desc) in expected {
            let f = field(i);
            assert_eq!(f.value, value, "MSH-{i} value");
            assert_eq!(f.description.as_deref(), Some(desc), "MSH-{i} description");
        }

        // The header fields extracted by enrich_msh must come from the same
        // HL7-numbered positions.
        assert_eq!(msg.sending_application, field(3).value);
        assert_eq!(msg.sending_facility, field(4).value);
        assert_eq!(msg.receiving_application, field(5).value);
        assert_eq!(msg.receiving_facility, field(6).value);
        assert_eq!(msg.message_control_id, field(10).value);
        assert_eq!(msg.version, field(12).value);
        assert_eq!(msg.message_structure.as_deref(), Some("ADT_A01"));
    }

    #[test]
    fn test_parse_adt_a01() {
        let msg = parse_message(SAMPLE_ADT, "127.0.0.1:9999").unwrap();
        assert_eq!(msg.message_type, "ADT^A01");
        assert_eq!(msg.trigger_event, "A01");
        assert_eq!(msg.sending_application, "SENDING_APP");
        assert_eq!(msg.sending_facility, "SENDING_FAC");
        assert_eq!(msg.message_control_id, "MSG00001");
        assert_eq!(msg.version, "2.5");
        assert_eq!(msg.patient_id, Some("12345".into()));
        assert_eq!(msg.patient_name, Some("Smith, John".into()));
        assert_eq!(msg.segments.len(), 3);
    }

    /// Field `index` (HL7 numbering) of the first `segment` in `msg`.
    fn field(msg: &Hl7Message, segment: &str, index: usize) -> String {
        let seg = msg.segments.iter().find(|s| s.name == segment).unwrap();
        get_field_value(seg, index)
    }

    #[test]
    fn test_build_ack() {
        let msg = parse_message(SAMPLE_ADT, "127.0.0.1:9999").unwrap();
        let ack = build_ack(&msg, "AA");
        assert!(ack.starts_with("MSH|^~\\&|Harux|Harux|SENDING_APP|SENDING_FAC|"));

        // The ACK is itself valid HL7: parse it back and check every field.
        let parsed = parse_message(&ack, "test").unwrap();
        assert_eq!(parsed.segments.len(), 2);
        assert_eq!(parsed.message_type, "ACK^A01");
        assert_eq!(parsed.receiving_application, "SENDING_APP");
        assert_eq!(parsed.receiving_facility, "SENDING_FAC");
        assert_eq!(parsed.message_control_id.len(), 20);
        assert_eq!(field(&parsed, "MSH", 11), "P");
        assert_eq!(parsed.version, "2.5");
        assert_eq!(field(&parsed, "MSA", 1), "AA");
        assert_eq!(field(&parsed, "MSA", 2), "MSG00001");
        assert_eq!(parsed.segments[1].fields.len(), 2, "no MSA-3 on an ACK");
    }

    /// #187: the NACK shares the ACK's MSH layout — it now carries its own
    /// MSH-10 control ID instead of an empty one.
    #[test]
    fn test_build_nack() {
        let nack = build_nack("Message parse error");
        let parsed = parse_message(&nack, "test").unwrap();
        assert_eq!(parsed.message_type, "ACK");
        assert_eq!(parsed.sending_application, "Harux");
        assert_eq!(parsed.receiving_application, "");
        assert_eq!(parsed.message_control_id.len(), 20);
        assert_eq!(field(&parsed, "MSH", 11), "P");
        assert_eq!(parsed.version, "2.5");
        assert_eq!(field(&parsed, "MSA", 1), "AE");
        assert_eq!(field(&parsed, "MSA", 2), "UNKNOWN");
        assert_eq!(field(&parsed, "MSA", 3), "Message parse error");
    }

    #[test]
    fn test_ack_control_ids_are_unique() {
        let msg = parse_message(SAMPLE_ADT, "test").unwrap();
        let a = parse_message(&build_ack(&msg, "AA"), "test").unwrap();
        let b = parse_message(&build_ack(&msg, "AA"), "test").unwrap();
        let n = parse_message(&build_nack("x"), "test").unwrap();
        assert_ne!(a.message_control_id, b.message_control_id);
        assert_ne!(a.message_control_id, n.message_control_id);
    }

    /// The ACK is written in the sender's encoding, so echoed values that
    /// contain the sender's delimiters stay intact.
    #[test]
    fn test_ack_uses_original_delimiters() {
        let raw = "MSH#*~\\&#APP*NS*ISO#FAC#REC#RFAC#20240101##ADT*A01#CTRL1#T#2.5.1\rPID###42";
        let msg = parse_message(raw, "test").unwrap();
        let ack = build_ack(&msg, "AA");
        assert!(
            ack.starts_with("MSH#*~\\&#Harux#Harux#APP*NS*ISO#FAC#"),
            "{ack}"
        );
        assert!(!ack.contains('|'), "{ack}");

        let parsed = parse_message(&ack, "test").unwrap();
        assert_eq!(field(&parsed, "MSH", 9), "ACK*A01");
        assert_eq!(parsed.receiving_application, "APP*NS*ISO");
        assert_eq!(field(&parsed, "MSH", 11), "T", "processing ID is echoed");
        assert_eq!(parsed.version, "2.5.1");
        assert_eq!(field(&parsed, "MSA", 2), "CTRL1");
    }

    #[test]
    fn test_ack_defaults_for_missing_original_fields() {
        // MSH-9 without trigger, no MSH-11, no MSH-12.
        let msg = parse_message("MSH|^~\\&|APP|FAC|||20240101||ADT|C1", "test").unwrap();
        let parsed = parse_message(&build_ack(&msg, "AA"), "test").unwrap();
        assert_eq!(parsed.message_type, "ACK", "no dangling ACK^");
        assert_eq!(field(&parsed, "MSH", 11), "P");
        assert_eq!(parsed.version, "2.5");
        assert_eq!(field(&parsed, "MSA", 2), "C1");

        // No MSH-10 at all → MSA-2 falls back like the NACK's instead of staying empty.
        let msg = parse_message("MSH|^~\\&|APP|FAC|||20240101||ADT^A01", "test").unwrap();
        let parsed = parse_message(&build_ack(&msg, "AA"), "test").unwrap();
        assert_eq!(field(&parsed, "MSA", 2), "UNKNOWN");
    }

    #[test]
    fn test_nack_text_is_escaped() {
        let nack = build_nack("a|b^c&d~e\\f\r\ng");
        assert!(
            nack.ends_with("MSA|AE|UNKNOWN|a\\F\\b\\S\\c\\T\\d\\R\\e\\E\\f  g"),
            "{nack:?}"
        );
        assert_eq!(nack.matches('\r').count(), 1, "only the MSH/MSA separator");
    }

    #[test]
    fn test_msh_field_indexing_quirk() {
        let msg = parse_message(SAMPLE_ADT, "127.0.0.1:9999").unwrap();
        let msh = msg.segments.first().unwrap();
        assert_eq!(msh.name, "MSH");
        assert_eq!(get_field_value(msh, 1), "|");
        assert_eq!(get_field_value(msh, 2), "^~\\&");
        assert_eq!(get_field_value(msh, 3), "SENDING_APP");
        assert_eq!(get_field_value(msh, 4), "SENDING_FAC");
        assert_eq!(get_field_value(msh, 9), "ADT^A01^ADT_A01");
    }

    /// #188: `msh_split_index` (used by the byte-level MSH-18 lookup in
    /// `charset.rs`) must agree with the parser's MSH shift for every field.
    #[test]
    fn test_msh_split_index_matches_parser() {
        let fields: Vec<String> = (3..=21).map(|n| format!("F{n}")).collect();
        let raw = format!("MSH|^~\\&|{}", fields.join("|"));
        let msg = parse_message(&raw, "test").unwrap();
        let msh = &msg.segments[0];
        let parts: Vec<&str> = raw.split('|').collect();
        for n in 2..=21 {
            assert_eq!(
                parts[msh_split_index(n)],
                get_field_value(msh, n),
                "MSH-{n}"
            );
        }
    }

    #[test]
    fn test_parse_empty_message() {
        let res = parse_message("", "127.0.0.1:9999");
        assert_eq!(res.unwrap_err(), "Empty message");
    }

    #[test]
    fn test_parse_invalid_msh_prefix() {
        let res = parse_message("NOT MSH", "127.0.0.1:9999");
        let err = res.unwrap_err();
        assert!(err.starts_with("Message does not start with MSH"));
    }

    /// #222: the error text quotes the start of the payload by characters;
    /// a byte cut inside `ä` (bytes 19..21) used to panic.
    #[test]
    fn test_non_msh_error_quotes_by_chars() {
        let err = parse_message("Sehr geehrte Damen ä und Herren", "t").unwrap_err();
        assert!(err.contains("Sehr geehrte Damen ä"), "{err}");
    }

    #[test]
    fn test_parse_short_msh_segment() {
        let res = parse_message("MSH|", "127.0.0.1:9999");
        assert_eq!(
            res.unwrap_err(),
            "MSH segment too short to extract delimiters"
        );
    }

    #[test]
    fn test_parse_structure() {
        let (msg, _) = parse_structure(SAMPLE_ADT, "127.0.0.1:9999").unwrap();
        assert_eq!(msg.segments.len(), 3);
        assert_eq!(msg.segments[0].name, "MSH");
        assert_eq!(msg.segments[1].name, "PID");
        assert_eq!(msg.segments[2].name, "PV1");
        assert_eq!(msg.message_type, ""); // Not enriched yet
    }

    #[test]
    fn test_enrich_msh() {
        let (msg, delimiters) = parse_structure(SAMPLE_ADT, "127.0.0.1:9999").unwrap();
        let msg = enrich_msh(msg, delimiters);
        assert_eq!(msg.message_type, "ADT^A01");
        assert_eq!(msg.trigger_event, "A01");
        assert_eq!(msg.sending_application, "SENDING_APP");
        assert_eq!(msg.sending_facility, "SENDING_FAC");
        assert_eq!(msg.message_control_id, "MSG00001");
        assert_eq!(msg.version, "2.5");
    }

    #[test]
    fn test_enrich_pid() {
        let (msg, delimiters) = parse_structure(SAMPLE_ADT, "127.0.0.1:9999").unwrap();
        let msg = enrich_pid(msg, delimiters);
        assert_eq!(msg.patient_id, Some("12345".into()));
        assert_eq!(msg.patient_name, Some("Smith, John".into()));
    }

    #[test]
    fn test_inject_descriptions() {
        let (msg, _) = parse_structure(SAMPLE_ADT, "127.0.0.1:9999").unwrap();
        let mut msg = msg;
        msg.version = "2.5".to_string(); // Need version for description lookup
        let msg = inject_descriptions(msg);
        // We know MSH-9 should get a description injected if dictionary works
        let msh = &msg.segments[0];
        let field9 = msh.fields.iter().find(|f| f.index == 9).unwrap();
        assert!(field9.description.is_some());
    }

    #[test]
    fn test_annotate_message_type() {
        let (msg, delimiters) = parse_structure(SAMPLE_ADT, "127.0.0.1:9999").unwrap();
        let msg = enrich_msh(msg, delimiters);
        let msg = annotate_message_type(msg);
        assert!(msg.message_type_description.is_some());
        assert!(!msg.typical_segments.is_empty());
    }

    #[test]
    fn test_validate() {
        let valid_adt = "MSH|^~\\&|APP|FAC|APP|FAC|2024||ADT^A01|MSG001|P|2.5\rEVN||2024\rPID|||12345||Smith^John||1980|M\rPV1||I";
        let (msg, delimiters) = parse_structure(valid_adt, "127.0.0.1:9999").unwrap();
        let msg = enrich_msh(msg, delimiters);
        let msg = enrich_pid(msg, delimiters);
        let warnings = validate(&msg);
        assert!(
            warnings.is_empty(),
            "Expected no warnings for valid ADT, got: {:?}",
            warnings
        );
    }

    #[test]
    fn test_msh2_field_sep_collision() {
        // MSH||||| — component separator == field separator '|' → parse error
        let res = parse_message("MSH|||||FOO\r", "127.0.0.1:9999");
        let err = res.unwrap_err();
        assert!(
            err.contains("duplicates the field separator"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_msh2_non_printable() {
        // Encoding char is a non-printable byte (0x01)
        let raw = "MSH|\x01~\\&|APP|FAC|||20240101||ADT^A01|1|P|2.5\r";
        let res = parse_message(raw, "127.0.0.1:9999");
        let err = res.unwrap_err();
        assert!(
            err.contains("not printable ASCII"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_msh2_duplicate_encoding_chars() {
        // Repetition char same as component char (^^~\&)
        let raw = "MSH|^^~\\&|APP|FAC|||20240101||ADT^A01|1|P|2.5\r";
        let res = parse_message(raw, "127.0.0.1:9999");
        let err = res.unwrap_err();
        assert!(err.contains("not distinct"), "unexpected error: {err}");
    }

    #[test]
    fn test_msh2_non_default_valid() {
        // Non-standard but valid delimiters should parse correctly
        let raw = "MSH|$@!#|APP|FAC|||20240101||ADT^A01|1|P|2.5\rPID|||12345\r";
        let msg = parse_message(raw, "127.0.0.1:9999").unwrap();
        assert_eq!(msg.message_type, "ADT^A01");
    }
}
