use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// A parsed HL7 v2.x message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hl7Message {
    pub id: String,
    pub raw: String,
    pub received_at: DateTime<Utc>,
    pub source_addr: String,
    pub message_type: String,  // e.g. "ADT^A01"
    pub trigger_event: String, // e.g. "A01"
    pub message_control_id: String,
    pub sending_application: String,
    pub sending_facility: String,
    pub receiving_application: String,
    pub receiving_facility: String,
    pub version: String,
    pub segments: Vec<Hl7Segment>,
    pub patient_name: Option<String>,
    pub patient_id: Option<String>,
    pub parse_error: Option<String>,
    pub ack_response: Option<String>,
    pub ack_code: Option<String>,
    pub tags: Vec<String>,
    pub bookmarked: bool,
    /// Validation warnings produced by the rule engine (empty = valid)
    pub validation_warnings: Vec<crate::validation::ValidationWarning>,
    /// Human-readable description of the message type (e.g. "Admit / Visit Notification")
    pub message_type_description: Option<String>,
    /// Segments typically present in this message type (from the HL7 spec)
    pub typical_segments: Vec<String>,
    /// Description for each typical segment name, from the embedded dictionary
    pub typical_segment_descriptions: HashMap<String, String>,
    pub charset: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hl7Segment {
    pub name: String,
    pub fields: Vec<Hl7Field>,
    pub raw: String,
    /// Human-readable description from the HL7 dictionary (e.g. "Patient Identification")
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hl7Field {
    pub index: usize,
    pub value: String,
    pub components: Vec<String>,
    pub description: Option<String>,
}

/// Separators / encoding characters from MSH-1 and MSH-2
#[derive(Debug, Clone, Copy)]
pub struct Delimiters {
    pub field: char,
    pub component: char,
    // Parsed from MSH-2 but not yet applied in field splitting (future: repetition/escape/sub-component parsing)
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
        }
    }
}

/// Summary for the message list (lightweight, no raw/segments)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hl7MessageSummary {
    pub id: String,
    pub received_at: DateTime<Utc>,
    pub source_addr: String,
    pub message_type: String,
    pub trigger_event: String,
    pub message_control_id: String,
    pub sending_facility: String,
    pub patient_name: Option<String>,
    pub patient_id: Option<String>,
    pub segment_count: usize,
    pub parse_error: Option<String>,
    pub ack_response: Option<String>,
    pub ack_code: Option<String>,
    pub tags: Vec<String>,
    pub bookmarked: bool,
    /// Number of validation warnings (for the list-view warning badge)
    pub validation_warning_count: usize,
    /// True when at least one warning is a MISSING_SEGMENT error (badge turns red)
    pub has_segment_errors: bool,
    pub message_type_description: Option<String>,
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
