use super::types::*;

/// Parse a raw HL7 v2.x message string into a structured Hl7Message.
/// Handles standard and custom delimiters from MSH segment.
pub fn parse_message(raw: &str, source_addr: &str) -> Result<Hl7Message, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("Empty message".into());
    }

    // HL7 messages must start with MSH
    if !raw.starts_with("MSH") {
        return Err(format!(
            "Message does not start with MSH: {:?}",
            &raw[..raw.len().min(20)]
        ));
    }

    // Extract delimiters from MSH-1 (field sep) and MSH-2 (encoding chars)
    let delimiters = parse_delimiters(raw)?;
    let mut msg = Hl7Message::new_empty(raw.to_string(), source_addr.to_string());

    // Split into segments (HL7 uses \r as segment terminator, but be lenient)
    for seg_str in raw.split(['\r', '\n']).filter(|s| !s.trim().is_empty()) {
        let segment = parse_segment(seg_str, delimiters);
        msg.segments.push(segment);
    }

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
            } else {
                msg.message_type = c0.to_string();
            }
        }

        msg.message_control_id = get_field_value(msh, 10);
        msg.version = get_field_value(msh, 12);
    }

    // Extract patient info from PID segment
    if let Some(pid) = msg.segments.iter().find(|s| s.name == "PID") {
        // PID-3: Patient ID
        let pid3 = get_field_value(pid, 3);
        if !pid3.is_empty() {
            // Take first component (ID itself, before ^^^authority)
            msg.patient_id = Some(
                pid3.split(delimiters.component)
                    .next()
                    .unwrap_or(&pid3)
                    .to_string(),
            );
        }

        // PID-5: Patient Name (Family^Given^Middle^Suffix^Prefix)
        let pid5 = get_field_value(pid, 5);
        if !pid5.is_empty() {
            let name_parts: Vec<&str> = pid5.split(delimiters.component).collect();
            let name = match name_parts.len() {
                0 => String::new(),
                1 => name_parts[0].to_string(),
                _ => format!("{}, {}", name_parts[0], name_parts[1]),
            };
            if !name.is_empty() {
                msg.patient_name = Some(name);
            }
        }
    }

    // Second pass: inject field descriptions from the embedded dictionary
    let version = if msg.version.is_empty() {
        "2.5.1"
    } else {
        &msg.version
    };
    crate::dictionary::inject_descriptions(&mut msg.segments, version);

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

    // Fourth pass: validate required segments and fields
    msg.validation_warnings = crate::validation::validate_message(&msg);

    Ok(msg)
}

fn parse_delimiters(raw: &str) -> Result<Delimiters, String> {
    // MSH|^~\&  ->  field=|, component=^, repetition=~, escape=\, subcomponent=&
    if raw.len() < 8 {
        return Err("MSH segment too short to extract delimiters".into());
    }

    let bytes = raw.as_bytes();
    let field_sep = bytes[3] as char;

    // MSH-2 is positions 4..8 (the 4 encoding characters)
    let mut delims = Delimiters {
        field: field_sep,
        ..Default::default()
    };

    if bytes.len() > 4 {
        delims.component = bytes[4] as char;
    }
    if bytes.len() > 5 {
        delims.repetition = bytes[5] as char;
    }
    if bytes.len() > 6 {
        delims.escape = bytes[6] as char;
    }
    if bytes.len() > 7 {
        delims.subcomponent = bytes[7] as char;
    }

    Ok(delims)
}

fn parse_segment(raw: &str, delimiters: Delimiters) -> Hl7Segment {
    let sep = delimiters.field;
    let mut parts = raw.split(sep);
    let name = parts.next().unwrap_or("???").to_string();

    let mut fields = Vec::new();

    // For MSH, field indexing is special: MSH-1 is the separator itself
    for (i, part) in parts.enumerate() {
        let components: Vec<String> = part
            .split(delimiters.component)
            .map(|c| c.to_string())
            .collect();

        fields.push(Hl7Field {
            index: i + 1,
            value: part.to_string(),
            components,
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
                components: vec![sep.to_string()],
                description: None,
            },
        );
    }

    Hl7Segment {
        name,
        fields,
        raw: raw.to_string(),
        description: None,
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

/// Build an ACK message for a received HL7 message
pub fn build_ack(original: &Hl7Message, ack_code: &str) -> String {
    let now = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
    let msh = format!(
        "MSH|^~\\&|Harux|Harux|{}|{}|{}||ACK^{}|{}|P|{}",
        original.sending_application,
        original.sending_facility,
        now,
        original.trigger_event,
        &uuid::Uuid::new_v4().to_string().replace('-', "")[..20],
        original.version,
    );
    let msa = format!("MSA|{}|{}", ack_code, original.message_control_id,);
    format!("{}\r{}", msh, msa)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_ADT: &str = "MSH|^~\\&|SENDING_APP|SENDING_FAC|REC_APP|REC_FAC|20240101120000||ADT^A01^ADT_A01|MSG00001|P|2.5\rPID|||12345^^^HOSP||Smith^John^Peter||19800515|M\rPV1||I|WARD1^ROOM1^BED1";

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

    #[test]
    fn test_build_ack() {
        let msg = parse_message(SAMPLE_ADT, "127.0.0.1:9999").unwrap();
        let ack = build_ack(&msg, "AA");
        assert!(ack.starts_with("MSH|^~\\&|Harux"));
        assert!(ack.contains("MSA|AA|MSG00001"));
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

    #[test]
    fn test_parse_short_msh_segment() {
        let res = parse_message("MSH|", "127.0.0.1:9999");
        assert_eq!(
            res.unwrap_err(),
            "MSH segment too short to extract delimiters"
        );
    }
}
