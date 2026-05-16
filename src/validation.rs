/// HL7 v2.x message validation.
///
/// Checks required segments and fields per message type and returns a list
/// of human-readable warnings.  Rules are intentionally pragmatic: they cover
/// the most critical constraints from the HL7 v2.5.1 specification for the
/// message types most commonly seen in hospital integration engines.
///
/// The validator is non-blocking — every message is stored regardless of
/// warnings.  Warnings are surfaced in the UI so developers can spot missing
/// fields at a glance without consulting the spec.
use crate::hl7::types::{Hl7Message, Hl7Segment};
use serde::{Deserialize, Serialize};

// ─── Public types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationWarning {
    /// Short machine-readable code (e.g. `"MISSING_SEGMENT"`)
    pub code: String,
    /// Human-readable message shown in the UI
    pub message: String,
    /// Segment the warning relates to (e.g. `"PID"`)
    pub segment: String,
    /// Field index within the segment, if applicable (1-based, HL7 standard)
    pub field: Option<usize>,
}

// ─── Constants ────────────────────────────────────────────────────────────────

const PV1_REQUIRED_ADT_EVENTS: &[&str] = &[
    "ADT^A01", "ADT^A02", "ADT^A03", "ADT^A04", "ADT^A05", "ADT^A06", "ADT^A07", "ADT^A08",
    "ADT^A09", "ADT^A10", "ADT^A11", "ADT^A12", "ADT^A13",
];

// ─── Entry point ──────────────────────────────────────────────────────────────

/// Validate a parsed HL7 message and return all warnings found.
/// Returns an empty `Vec` if the message is fully valid (or unknown type).
pub fn validate_message(msg: &Hl7Message) -> Vec<ValidationWarning> {
    let mut warnings = Vec::new();

    // Always validate MSH required fields first
    validate_msh(msg, &mut warnings);

    // Message-type–specific rules
    match msg.message_type.as_str() {
        t if t.starts_with("ADT^") => validate_adt(msg, &mut warnings),
        "ORU^R01" => validate_oru_r01(msg, &mut warnings),
        "ORM^O01" => validate_orm_o01(msg, &mut warnings),
        "OML^O21" => validate_oml_o21(msg, &mut warnings),
        t if t.starts_with("SIU^") => validate_siu(msg, &mut warnings),
        t if t.starts_with("MDM^") => validate_mdm(msg, &mut warnings),
        _ => {} // No rules for unknown types — do not emit spurious warnings
    }

    validate_data_types(msg, &mut warnings);

    warnings
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn find_segment<'a>(msg: &'a Hl7Message, name: &str) -> Option<&'a Hl7Segment> {
    msg.segments.iter().find(|s| s.name == name)
}

fn has_segment(msg: &Hl7Message, name: &str) -> bool {
    msg.segments.iter().any(|s| s.name == name)
}

/// Return the field value for a segment (1-based HL7 field index).
fn field_value(seg: &Hl7Segment, index: usize) -> Option<&str> {
    seg.fields
        .iter()
        .find(|f| f.index == index)
        .map(|f| f.value.as_str())
        .filter(|v| !v.is_empty())
}

fn warn_missing_segment(warnings: &mut Vec<ValidationWarning>, seg: &str, context: &str) {
    warnings.push(ValidationWarning {
        code: "MISSING_SEGMENT".into(),
        message: format!("{seg} segment is required for {context}"),
        segment: seg.into(),
        field: None,
    });
}

fn warn_missing_field(
    warnings: &mut Vec<ValidationWarning>,
    seg: &str,
    field: usize,
    field_name: &str,
    context: &str,
) {
    warnings.push(ValidationWarning {
        code: "MISSING_FIELD".into(),
        message: format!("{seg}-{field} ({field_name}) is required for {context}"),
        segment: seg.into(),
        field: Some(field),
    });
}

// ─── MSH — universal required fields ─────────────────────────────────────────

fn validate_msh(msg: &Hl7Message, warnings: &mut Vec<ValidationWarning>) {
    let Some(msh) = find_segment(msg, "MSH") else {
        // Without MSH the parser would have failed; guard anyway
        warn_missing_segment(warnings, "MSH", "all HL7 messages");
        return;
    };

    // MSH-3: Sending Application
    if field_value(msh, 3).is_none() {
        warn_missing_field(
            warnings,
            "MSH",
            3,
            "Sending Application",
            "all HL7 messages",
        );
    }
    // MSH-4: Sending Facility
    if field_value(msh, 4).is_none() {
        warn_missing_field(warnings, "MSH", 4, "Sending Facility", "all HL7 messages");
    }
    // MSH-9: Message Type
    if field_value(msh, 9).is_none() {
        warn_missing_field(warnings, "MSH", 9, "Message Type", "all HL7 messages");
    }
    // MSH-10: Message Control ID
    if field_value(msh, 10).is_none() {
        warn_missing_field(
            warnings,
            "MSH",
            10,
            "Message Control ID",
            "all HL7 messages",
        );
    }
    // MSH-11: Processing ID (P/T/D)
    if field_value(msh, 11).is_none() {
        warn_missing_field(warnings, "MSH", 11, "Processing ID", "all HL7 messages");
    }
    // MSH-12: Version ID
    if field_value(msh, 12).is_none() {
        warn_missing_field(warnings, "MSH", 12, "Version ID", "all HL7 messages");
    }
}

// ─── ADT ──────────────────────────────────────────────────────────────────────

fn validate_adt(msg: &Hl7Message, warnings: &mut Vec<ValidationWarning>) {
    let ctx = &msg.message_type;

    // EVN is required for all ADT messages except A19 (query)
    if msg.message_type != "ADT^A19" && !has_segment(msg, "EVN") {
        warn_missing_segment(warnings, "EVN", ctx);
    }

    // PID is required for all ADT messages
    let Some(pid) = find_segment(msg, "PID") else {
        warn_missing_segment(warnings, "PID", ctx);
        return; // Can't validate PID fields without the segment
    };

    // PID-3: Patient Identifier List
    if field_value(pid, 3).is_none() {
        warn_missing_field(warnings, "PID", 3, "Patient Identifier List", ctx);
    }
    // PID-5: Patient Name
    if field_value(pid, 5).is_none() {
        warn_missing_field(warnings, "PID", 5, "Patient Name", ctx);
    }
    // PID-7: Date/Time of Birth (recommended)
    // PID-8: Administrative Sex
    if field_value(pid, 8).is_none() {
        warn_missing_field(warnings, "PID", 8, "Administrative Sex", ctx);
    }

    // PV1 is required for most ADT events (transfer/admit/discharge)
    let pv1_required = PV1_REQUIRED_ADT_EVENTS.contains(&msg.message_type.as_str());
    if pv1_required {
        if let Some(pv1) = find_segment(msg, "PV1") {
            // PV1-2: Patient Class (I/O/E/P/R/B/C/N/U)
            if field_value(pv1, 2).is_none() {
                warn_missing_field(warnings, "PV1", 2, "Patient Class", ctx);
            }
        } else {
            warn_missing_segment(warnings, "PV1", ctx);
        }
    }
}

// ─── ORU^R01 ─────────────────────────────────────────────────────────────────

fn validate_oru_r01(msg: &Hl7Message, warnings: &mut Vec<ValidationWarning>) {
    let ctx = "ORU^R01";

    // OBR is required
    let Some(obr) = find_segment(msg, "OBR") else {
        warn_missing_segment(warnings, "OBR", ctx);
        return;
    };
    // OBR-1: Set ID
    // OBR-3: Filler Order Number (required in R01)
    if field_value(obr, 3).is_none() {
        warn_missing_field(warnings, "OBR", 3, "Filler Order Number", ctx);
    }
    // OBR-4: Universal Service Identifier
    if field_value(obr, 4).is_none() {
        warn_missing_field(warnings, "OBR", 4, "Universal Service Identifier", ctx);
    }

    // OBX is required (at least one observation)
    if !has_segment(msg, "OBX") {
        warn_missing_segment(warnings, "OBX", ctx);
    }
    // Validate each OBX
    for seg in msg.segments.iter().filter(|s| s.name == "OBX") {
        // OBX-2: Value Type (NM, ST, CWE, etc.)
        if field_value(seg, 2).is_none() {
            warn_missing_field(warnings, "OBX", 2, "Value Type", ctx);
        }
        // OBX-3: Observation Identifier
        if field_value(seg, 3).is_none() {
            warn_missing_field(warnings, "OBX", 3, "Observation Identifier", ctx);
        }
        // OBX-11: Observation Result Status
        if field_value(seg, 11).is_none() {
            warn_missing_field(warnings, "OBX", 11, "Observation Result Status", ctx);
        }
    }
}

// ─── ORM^O01 ─────────────────────────────────────────────────────────────────

fn validate_orm_o01(msg: &Hl7Message, warnings: &mut Vec<ValidationWarning>) {
    let ctx = "ORM^O01";

    if !has_segment(msg, "PID") {
        warn_missing_segment(warnings, "PID", ctx);
    }

    let Some(orc) = find_segment(msg, "ORC") else {
        warn_missing_segment(warnings, "ORC", ctx);
        return;
    };
    // ORC-1: Order Control
    if field_value(orc, 1).is_none() {
        warn_missing_field(warnings, "ORC", 1, "Order Control", ctx);
    }

    if !has_segment(msg, "OBR") {
        warn_missing_segment(warnings, "OBR", ctx);
    }
}

// ─── OML^O21 ─────────────────────────────────────────────────────────────────

fn validate_oml_o21(msg: &Hl7Message, warnings: &mut Vec<ValidationWarning>) {
    let ctx = "OML^O21";

    if !has_segment(msg, "PID") {
        warn_missing_segment(warnings, "PID", ctx);
    }
    if !has_segment(msg, "ORC") {
        warn_missing_segment(warnings, "ORC", ctx);
    }
    if !has_segment(msg, "OBR") {
        warn_missing_segment(warnings, "OBR", ctx);
    }
}

// ─── SIU ─────────────────────────────────────────────────────────────────────

fn validate_siu(msg: &Hl7Message, warnings: &mut Vec<ValidationWarning>) {
    let ctx = &msg.message_type;

    let Some(sch) = find_segment(msg, "SCH") else {
        warn_missing_segment(warnings, "SCH", ctx);
        return;
    };
    // SCH-1 or SCH-2 must identify the appointment
    if field_value(sch, 1).is_none() && field_value(sch, 2).is_none() {
        warnings.push(ValidationWarning {
            code: "MISSING_FIELD".into(),
            message: format!(
                "SCH-1 (Placer Appointment ID) or SCH-2 (Filler Appointment ID) is required for {ctx}"
            ),
            segment: "SCH".into(),
            field: Some(1),
        });
    }
    // SCH-7: Appointment Reason
    // SCH-25: Filler Status Code
}

// ─── MDM ─────────────────────────────────────────────────────────────────────

fn validate_mdm(msg: &Hl7Message, warnings: &mut Vec<ValidationWarning>) {
    let ctx = &msg.message_type;

    if !has_segment(msg, "EVN") {
        warn_missing_segment(warnings, "EVN", ctx);
    }
    if !has_segment(msg, "PID") {
        warn_missing_segment(warnings, "PID", ctx);
    }
    if !has_segment(msg, "PV1") {
        warn_missing_segment(warnings, "PV1", ctx);
    }

    let Some(txa) = find_segment(msg, "TXA") else {
        warn_missing_segment(warnings, "TXA", ctx);
        return;
    };
    // TXA-2: Document Type
    if field_value(txa, 2).is_none() {
        warn_missing_field(warnings, "TXA", 2, "Document Type", ctx);
    }
    // TXA-9: Originator Code/Name
    // TXA-17: Document Completion Status
    if field_value(txa, 17).is_none() {
        warn_missing_field(warnings, "TXA", 17, "Document Completion Status", ctx);
    }
}

// ─── Data type validation ────────────────────────────────────────────────────

/// Validate field values against the data types declared in the HL7 dictionary.
///
/// Covers the four primitive types with unambiguous, checkable formats:
/// - **NM** (Numeric) — digits, optional sign, optional single decimal point
/// - **DT** (Date) — YYYY[MM[DD]] — all digits, valid month/day ranges
/// - **TS** (Timestamp) — YYYY[MM[DD[HH[MM[SS[.S+]]]]]][+/-HHMM]
/// - **SI** (Sequence ID) — non-negative integer
///
/// Composite types (XPN, CX, CE, …) and free-text types (ST, TX, FT, ID, IS)
/// are intentionally skipped — they are too format-ambiguous or table-dependent
/// to validate without value sets.
///
/// Only the first component of each field is checked. Multi-component values
/// (e.g. `CODE^Text^System`) are typical for composite types and are skipped
/// at the `check_datatype` level.
fn validate_data_types(msg: &Hl7Message, warnings: &mut Vec<ValidationWarning>) {
    let dict = crate::dictionary::get_v251();
    for seg in &msg.segments {
        let Some(seg_def) = dict.segments.get(&seg.name) else {
            continue;
        };
        for field in &seg.fields {
            if field.value.is_empty() {
                continue; // empty values are caught by MISSING_FIELD rules
            }
            let field_def = if field.index > 0 {
                seg_def.fields.get(field.index - 1)
            } else {
                None
            };

            let Some(field_def) = field_def
                .filter(|f| f.seq == field.index)
                .or_else(|| seg_def.fields.iter().find(|f| f.seq == field.index))
            else {
                continue;
            };
            // Use only the first component — composite values include sub-component
            // separators (^) that are not part of the primitive value itself.
            let primary = field
                .components
                .first()
                .map(String::as_str)
                .unwrap_or(field.value.as_str());
            if primary.is_empty() {
                continue;
            }
            if let Some(err) = check_datatype(&field_def.datatype, primary) {
                warnings.push(ValidationWarning {
                    code: "INVALID_DATATYPE".into(),
                    message: format!("{}-{} ({}): {}", seg.name, field.index, field_def.desc, err),
                    segment: seg.name.clone(),
                    field: Some(field.index),
                });
            }
        }
    }
}

fn check_datatype(datatype: &str, value: &str) -> Option<String> {
    match datatype {
        "NM" => validate_nm(value).map(str::to_string),
        "DT" => validate_dt(value),
        "TS" => validate_ts(value),
        "SI" => validate_si(value).map(str::to_string),
        _ => None, // composite or table-dependent types are not validated
    }
}

/// NM — Numeric: optional sign, digits, optional single decimal point.
fn validate_nm(value: &str) -> Option<&'static str> {
    let v = value
        .strip_prefix(|c| matches!(c, '+' | '-'))
        .unwrap_or(value);
    if v.is_empty() {
        return Some("expected a number (NM) but value contains only a sign character");
    }
    let mut dot_seen = false;
    for c in v.chars() {
        if c == '.' {
            if dot_seen {
                return Some("expected a number (NM) but value has multiple decimal points");
            }
            dot_seen = true;
        } else if !c.is_ascii_digit() {
            return Some("expected a number (NM) but value contains non-numeric characters");
        }
    }
    None
}

/// DT — Date: YYYY, YYYYMM, or YYYYMMDD — all digits, valid month/day ranges.
fn validate_dt(value: &str) -> Option<String> {
    if !value.chars().all(|c| c.is_ascii_digit()) {
        return Some("expected a date (DT) in YYYY[MM[DD]] format".to_string());
    }
    match value.len() {
        4 => None,
        6 => {
            let month: u32 = value[4..6].parse().unwrap_or(0);
            if !(1..=12).contains(&month) {
                Some(format!(
                    "expected a date (DT) but month '{}' is out of range 01–12",
                    &value[4..6]
                ))
            } else {
                None
            }
        }
        8 => {
            let month: u32 = value[4..6].parse().unwrap_or(0);
            if !(1..=12).contains(&month) {
                return Some(format!(
                    "expected a date (DT) but month '{}' is out of range 01–12",
                    &value[4..6]
                ));
            }
            let day: u32 = value[6..8].parse().unwrap_or(0);
            if !(1..=31).contains(&day) {
                return Some(format!(
                    "expected a date (DT) but day '{}' is out of range 01–31",
                    &value[6..8]
                ));
            }
            None
        }
        _ => Some(
            "expected a date (DT): length must be 4 (YYYY), 6 (YYYYMM), or 8 (YYYYMMDD)"
                .to_string(),
        ),
    }
}

/// TS — Timestamp: YYYY[MM[DD[HH[MM[SS[.S+]]]]]][+/-HHMM].
/// Strips fractional seconds and timezone suffix, then validates date portion.
fn validate_ts(value: &str) -> Option<String> {
    // Strip fractional seconds (everything after the first '.')
    let without_frac = match value.find('.') {
        Some(pos) => &value[..pos],
        None => value,
    };
    // Strip timezone offset (+HHMM / -HHMM) — only search after position 8
    // so a leading '-' in an unexpected value doesn't confuse the parser.
    let core = if without_frac.len() > 8 {
        match without_frac[8..].find(['+', '-']) {
            Some(offset) => &without_frac[..8 + offset],
            None => without_frac,
        }
    } else {
        without_frac
    };
    if core.len() < 4 || !core.chars().all(|c| c.is_ascii_digit()) {
        return Some("expected a timestamp (TS) in YYYY[MM[DD[HH[MM[SS]]]]] format".to_string());
    }
    // Validate however much of the date is present
    let date_len = core.len().min(8);
    if date_len >= 6 {
        validate_dt(&core[..date_len])
    } else {
        None // YYYY only — valid
    }
}

/// SI — Sequence ID: non-negative integer (digits only).
fn validate_si(value: &str) -> Option<&'static str> {
    if value.chars().all(|c| c.is_ascii_digit()) {
        None
    } else {
        Some("expected a sequence ID (SI) but value contains non-digit characters")
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hl7::parser::parse_message;

    const VALID_ADT_A01: &str =
        "MSH|^~\\&|SEND_APP|SEND_FAC|REC_APP|REC_FAC|20240101120000||ADT^A01|MSG001|P|2.5\r\
         EVN||20240101120000\r\
         PID|||12345^^^HOSP||Smith^John^^||19800515|M\r\
         PV1||I|WARD1^ROOM1^BED1";

    const MISSING_PID3: &str =
        "MSH|^~\\&|SEND_APP|SEND_FAC|REC_APP|REC_FAC|20240101120000||ADT^A01|MSG001|P|2.5\r\
         EVN||20240101120000\r\
         PID|||||Smith^John^^||19800515|M\r\
         PV1||I|WARD^ROOM^BED";

    const NO_PV1: &str =
        "MSH|^~\\&|SEND_APP|SEND_FAC|REC_APP|REC_FAC|20240101120000||ADT^A01|MSG001|P|2.5\r\
         EVN||20240101120000\r\
         PID|||12345^^^HOSP||Smith^John^^||19800515|M";

    #[test]
    fn test_valid_adt_has_no_warnings() {
        let msg = parse_message(VALID_ADT_A01, "127.0.0.1:9999").unwrap();
        let warnings = validate_message(&msg);
        assert!(
            warnings.is_empty(),
            "Expected no warnings, got: {:?}",
            warnings
        );
    }

    #[test]
    fn test_missing_pid3_triggers_warning() {
        let msg = parse_message(MISSING_PID3, "127.0.0.1:9999").unwrap();
        let warnings = validate_message(&msg);
        assert!(
            warnings
                .iter()
                .any(|w| w.segment == "PID" && w.field == Some(3)),
            "Expected PID-3 warning, got: {:?}",
            warnings
        );
    }

    #[test]
    fn test_missing_pv1_triggers_warning() {
        let msg = parse_message(NO_PV1, "127.0.0.1:9999").unwrap();
        let warnings = validate_message(&msg);
        assert!(
            warnings
                .iter()
                .any(|w| w.segment == "PV1" && w.code == "MISSING_SEGMENT"),
            "Expected PV1 MISSING_SEGMENT warning, got: {:?}",
            warnings
        );
    }

    #[test]
    fn test_unknown_type_no_warnings() {
        let raw = "MSH|^~\\&|APP|FAC|R|R|20240101||ZZZ^Z01|MSG001|P|2.5";
        let msg = parse_message(raw, "127.0.0.1:9999").unwrap();
        // Only MSH field warnings possible; no message-type rules for ZZZ
        let warnings = validate_message(&msg);
        // Should not have segment-level warnings (only possibly MSH field ones)
        assert!(
            warnings.iter().all(|w| w.segment == "MSH"),
            "Unknown type should produce only MSH warnings, got: {:?}",
            warnings
        );
    }

    // ── Data type unit tests ────────────────────────────────────────────────

    #[test]
    fn nm_valid_integers_and_decimals() {
        assert!(validate_nm("0").is_none());
        assert!(validate_nm("123").is_none());
        assert!(validate_nm("-45").is_none());
        assert!(validate_nm("+0.5").is_none());
        assert!(validate_nm("3.14").is_none());
    }

    #[test]
    fn nm_rejects_non_numeric() {
        assert!(validate_nm("abc").is_some());
        assert!(validate_nm("12.3.4").is_some());
        assert!(validate_nm("12 mg").is_some());
        assert!(validate_nm("+").is_some()); // sign only
    }

    #[test]
    fn dt_valid_dates() {
        assert!(validate_dt("2026").is_none());
        assert!(validate_dt("202603").is_none());
        assert!(validate_dt("20260308").is_none());
    }

    #[test]
    fn dt_rejects_bad_dates() {
        assert!(validate_dt("2026/03/08").is_some()); // slashes
        assert!(validate_dt("20261301").is_some()); // month 13
        assert!(validate_dt("20260332").is_some()); // day 32
        assert!(validate_dt("20260").is_some()); // odd length
    }

    #[test]
    fn ts_valid_timestamps() {
        assert!(validate_ts("20260308").is_none());
        assert!(validate_ts("20260308143000").is_none());
        assert!(validate_ts("20260308143000.000").is_none());
        assert!(validate_ts("20260308143000+0500").is_none());
        assert!(validate_ts("2026").is_none());
    }

    #[test]
    fn ts_rejects_bad_timestamps() {
        assert!(validate_ts("notadate").is_some());
        assert!(validate_ts("20261301").is_some()); // bad month
        assert!(validate_ts("abc").is_some());
    }

    #[test]
    fn si_valid() {
        assert!(validate_si("0").is_none());
        assert!(validate_si("1").is_none());
        assert!(validate_si("42").is_none());
    }

    #[test]
    fn si_rejects_non_integer() {
        assert!(validate_si("abc").is_some());
        assert!(validate_si("-1").is_some());
        assert!(validate_si("1.5").is_some());
    }

    #[test]
    fn valid_adt_produces_no_datatype_warnings() {
        // VALID_ADT_A01 has correct types — no INVALID_DATATYPE warnings expected
        let msg = parse_message(VALID_ADT_A01, "127.0.0.1:9999").unwrap();
        let warnings = validate_message(&msg);
        assert!(
            warnings.iter().all(|w| w.code != "INVALID_DATATYPE"),
            "Valid ADT should have no datatype warnings, got: {:?}",
            warnings
        );
    }

    #[test]
    fn invalid_ts_field_triggers_datatype_warning() {
        // MSH-7 is TS; "notadate" should trigger INVALID_DATATYPE
        let raw = "MSH|^~\\&|APP|FAC|R|R|notadate||ADT^A01|MSG001|P|2.5\r\
             EVN||20240101\r\
             PID|||12345^^^HOSP||Smith^John^^||19800515|M\r\
             PV1||I";
        let msg = parse_message(raw, "127.0.0.1:9999").unwrap();
        let warnings = validate_message(&msg);
        assert!(
            warnings
                .iter()
                .any(|w| w.code == "INVALID_DATATYPE" && w.segment == "MSH" && w.field == Some(7)),
            "Expected INVALID_DATATYPE for MSH-7, got: {:?}",
            warnings
        );
    }
}
