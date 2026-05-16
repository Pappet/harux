/// HL7 v2.x message type registry.
///
/// Maps "TYPE^EVENT" strings to human-readable descriptions and the segments
/// that are typically present in that message type.  The data lives in
/// `src/assets/hl7/message_types.json` and is embedded at compile time via
/// `include_str!`.  Editing the JSON file is sufficient to add or change entries
/// — no Rust code needs to change.
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Deserialize)]
pub struct MessageTypeInfo {
    pub description: String,
    pub typical_segments: Vec<String>,
}

static REGISTRY: OnceLock<HashMap<String, MessageTypeInfo>> = OnceLock::new();

pub fn get_message_type_info(message_type: &str) -> Option<&'static MessageTypeInfo> {
    REGISTRY.get_or_init(load_registry).get(message_type)
}

fn load_registry() -> HashMap<String, MessageTypeInfo> {
    let json = include_str!("../assets/hl7/message_types.json");
    serde_json::from_str(json).expect("failed to parse embedded message_types.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_type() {
        let info = get_message_type_info("ADT^A01").unwrap();
        assert_eq!(info.description, "Admit / Visit Notification");
        assert!(info.typical_segments.iter().any(|s| s == "PID"));
    }

    #[test]
    fn test_unknown_type() {
        assert!(get_message_type_info("ZZZ^Z99").is_none());
    }

    #[test]
    fn test_oru_r01() {
        let info = get_message_type_info("ORU^R01").unwrap();
        assert_eq!(info.description, "Unsolicited Observation Result");
        assert!(info.typical_segments.iter().any(|s| s == "OBX"));
    }
}
