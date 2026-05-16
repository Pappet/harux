use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// A parsed HL7 v2.x message as stored in the in-memory store and serialised to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hl7Message {
    /// Stable UUID assigned at receive time; used as the primary key for API and WebSocket events.
    pub id: String,
    /// The raw HL7 text as received over MLLP, including all segment separators (`\r`).
    pub raw: String,
    /// Wall-clock time the message arrived at the MLLP listener.
    pub received_at: DateTime<Utc>,
    /// IP address (and optional port) of the sending TCP peer, e.g. `"192.168.1.10:52341"`.
    pub source_addr: String,
    /// Composite message type from MSH-9, e.g. `"ADT^A01"` or `"ORU^R01"`.
    pub message_type: String,
    /// Trigger event component of MSH-9.2, e.g. `"A01"`.
    pub trigger_event: String,
    /// MSH-10: uniquely identifies this message exchange within the sending application.
    pub message_control_id: String,
    /// MSH-3: name of the system sending the message.
    pub sending_application: String,
    /// MSH-4: organisational entity sending the message (e.g. hospital name).
    pub sending_facility: String,
    /// MSH-5: name of the system that should receive the message.
    pub receiving_application: String,
    /// MSH-6: organisational entity receiving the message.
    pub receiving_facility: String,
    /// MSH-12: HL7 version string, e.g. `"2.5.1"`.
    pub version: String,
    /// All segments parsed from the raw message, in order.
    pub segments: Vec<Hl7Segment>,
    /// Extracted from PID-5 as `"Family, Given"` for display; `None` if PID is absent.
    pub patient_name: Option<String>,
    /// First component of PID-3 (patient identifier); `None` if PID is absent.
    pub patient_id: Option<String>,
    /// Non-`None` when the parser failed to fully parse the message; contains the error text.
    pub parse_error: Option<String>,
    /// The ACK/NACK HL7 text sent back to the sender (omitted for ACK-type messages).
    pub ack_response: Option<String>,
    /// The MSA-1 acknowledgement code sent back: `"AA"` (accepted) or `"AE"` (error).
    pub ack_code: Option<String>,
    /// User-defined tag labels attached via the API (mutable after storage).
    pub tags: Vec<String>,
    /// Whether the message is pinned; bookmarked messages survive eviction.
    pub bookmarked: bool,
    /// Validation warnings produced by the rule engine (empty = valid)
    pub validation_warnings: Vec<crate::validation::ValidationWarning>,
    /// Human-readable description of the message type (e.g. "Admit / Visit Notification")
    pub message_type_description: Option<String>,
    /// Segments typically present in this message type (from the HL7 spec)
    pub typical_segments: Vec<String>,
    /// Description for each typical segment name, from the embedded dictionary
    pub typical_segment_descriptions: HashMap<String, String>,
    /// Character set declared in MSH-18, e.g. `"UTF-8"` or `"8859/1"`; `None` if absent.
    pub charset: Option<String>,
    /// MSH-9.3 message structure identifier, e.g. `"ADT_A01"` or `"ORU_R01"`.
    /// Used by Milestone 3 validation to select the correct structure definition.
    /// `None` when MSH-9 has fewer than three components (common in HL7 v2.3 senders).
    pub message_structure: Option<String>,
}

/// One segment within an HL7 message (e.g. `MSH`, `PID`, `OBR`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hl7Field {
    /// 1-based field position within the segment (matches the HL7 standard numbering, e.g. PID-3).
    /// MSH is special: field 1 is synthetically `"|"`, so MSH-3 is Sending Application.
    pub index: usize,
    /// The raw field value string, possibly containing component separators (`^`).
    pub value: String,
    /// Pre-split components from `value`, split on the `^` delimiter.
    pub components: Vec<String>,
    /// Human-readable field name from the embedded HL7 dictionary, if available.
    pub description: Option<String>,
}

/// One field within an HL7 segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hl7Segment {
    /// Segment identifier, e.g. `"MSH"`, `"PID"`, `"OBX"`.
    pub name: String,
    /// All fields in this segment, in order.
    pub fields: Vec<Hl7Field>,
    /// The original segment text (one line of the raw message).
    pub raw: String,
    /// Human-readable description from the HL7 dictionary (e.g. "Patient Identification")
    pub description: Option<String>,
}

/// Separators / encoding characters extracted from MSH-1 (field separator) and MSH-2.
#[derive(Debug, Clone, Copy)]
pub struct Delimiters {
    /// MSH-1: separates fields within a segment (standard: `|`).
    pub field: char,
    /// MSH-2 first char: separates components within a field (standard: `^`).
    pub component: char,
    // Parsed from MSH-2 but not yet applied in field splitting (future use)
    #[allow(dead_code)]
    pub repetition: char,
    #[allow(dead_code)]
    pub escape: char,
    #[allow(dead_code)]
    pub subcomponent: char,
}

impl Default for Delimiters {
    fn default() -> Self {
        Self {
            field: '|',
            component: '^',
            repetition: '~',
            escape: '\\',
            subcomponent: '&',
        }
    }
}

impl Hl7Message {
    /// Estimate the heap bytes consumed by this message (raw + all parsed strings).
    /// Used by the store to track memory more accurately than `raw.len()` alone.
    pub fn estimated_bytes(&self) -> usize {
        let mut total = self.raw.len()
            + self.id.len()
            + self.source_addr.len()
            + self.message_type.len()
            + self.trigger_event.len()
            + self.message_control_id.len()
            + self.sending_application.len()
            + self.sending_facility.len()
            + self.receiving_application.len()
            + self.receiving_facility.len()
            + self.version.len();

        total += self.patient_name.as_deref().map_or(0, str::len);
        total += self.patient_id.as_deref().map_or(0, str::len);
        total += self.parse_error.as_deref().map_or(0, str::len);
        total += self.ack_response.as_deref().map_or(0, str::len);
        total += self.ack_code.as_deref().map_or(0, str::len);
        total += self.message_type_description.as_deref().map_or(0, str::len);
        total += self.charset.as_deref().map_or(0, str::len);

        total += self.tags.iter().map(String::len).sum::<usize>();
        total += self.typical_segments.iter().map(String::len).sum::<usize>();
        total += self
            .typical_segment_descriptions
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>();
        total += self
            .validation_warnings
            .iter()
            .map(|w| w.code.len() + w.message.len() + w.segment.len())
            .sum::<usize>();

        for seg in &self.segments {
            total += seg.name.len() + seg.raw.len();
            total += seg.description.as_deref().map_or(0, str::len);
            for field in &seg.fields {
                total += field.value.len();
                total += field.description.as_deref().map_or(0, str::len);
                total += field.components.iter().map(String::len).sum::<usize>();
            }
        }
        total
    }

    pub fn new_empty(raw: String, source_addr: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            raw,
            received_at: Utc::now(),
            source_addr,
            message_type: String::new(),
            trigger_event: String::new(),
            message_control_id: String::new(),
            sending_application: String::new(),
            sending_facility: String::new(),
            receiving_application: String::new(),
            receiving_facility: String::new(),
            version: String::new(),
            segments: Vec::new(),
            patient_name: None,
            patient_id: None,
            parse_error: None,
            ack_response: None,
            ack_code: None,
            tags: Vec::new(),
            bookmarked: false,
            validation_warnings: Vec::new(),
            message_type_description: None,
            typical_segments: Vec::new(),
            typical_segment_descriptions: HashMap::new(),
            charset: None,
            message_structure: None,
        }
    }
}

/// Lightweight projection of `Hl7Message` sent to the frontend message list.
/// Omits `raw` and `segments` to keep the WebSocket payload small.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hl7MessageSummary {
    /// Matches `Hl7Message::id` — used to fetch the full message via `GET /api/messages/{id}`.
    pub id: String,
    /// Wall-clock receive time; used for display and sorting (newest first).
    pub received_at: DateTime<Utc>,
    /// TCP peer address of the sender.
    pub source_addr: String,
    /// Composite message type, e.g. `"ADT^A01"`.
    pub message_type: String,
    /// Trigger event component, e.g. `"A01"`.
    pub trigger_event: String,
    /// MSH-10 message control ID.
    pub message_control_id: String,
    /// MSH-4 sending facility name.
    pub sending_facility: String,
    /// Formatted patient name (`"Family, Given"`), or `None` if PID is absent.
    pub patient_name: Option<String>,
    /// First PID-3 component, or `None` if PID is absent.
    pub patient_id: Option<String>,
    /// Total number of segments parsed from the message.
    pub segment_count: usize,
    /// Parse error text, or `None` if the message parsed cleanly.
    pub parse_error: Option<String>,
    /// ACK/NACK text sent back to the sender (omitted for ACK-type messages).
    pub ack_response: Option<String>,
    /// MSA-1 code: `"AA"` or `"AE"`.
    pub ack_code: Option<String>,
    /// User-defined tag labels.
    pub tags: Vec<String>,
    /// Whether this message is bookmarked (survives eviction).
    pub bookmarked: bool,
    /// Number of validation warnings (for the list-view warning badge)
    pub validation_warning_count: usize,
    /// True when at least one warning is a MISSING_SEGMENT error (badge turns red)
    pub has_segment_errors: bool,
    /// Human-readable message type description, e.g. `"Admit / Visit Notification"`.
    pub message_type_description: Option<String>,
    /// Character set from MSH-18, e.g. `"UTF-8"`.
    pub charset: Option<String>,
}

impl From<&Hl7Message> for Hl7MessageSummary {
    fn from(msg: &Hl7Message) -> Self {
        Self {
            id: msg.id.clone(),
            received_at: msg.received_at,
            source_addr: msg.source_addr.clone(),
            message_type: msg.message_type.clone(),
            trigger_event: msg.trigger_event.clone(),
            message_control_id: msg.message_control_id.clone(),
            sending_facility: msg.sending_facility.clone(),
            patient_name: msg.patient_name.clone(),
            patient_id: msg.patient_id.clone(),
            segment_count: msg.segments.len(),
            parse_error: msg.parse_error.clone(),
            ack_response: msg.ack_response.clone(),
            ack_code: msg.ack_code.clone(),
            tags: msg.tags.clone(),
            bookmarked: msg.bookmarked,
            validation_warning_count: msg.validation_warnings.len(),
            has_segment_errors: msg
                .validation_warnings
                .iter()
                .any(|w| w.code == "MISSING_SEGMENT"),
            message_type_description: msg.message_type_description.clone(),
            charset: msg.charset.clone(),
        }
    }
}
