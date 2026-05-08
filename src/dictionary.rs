#![allow(dead_code)]
#![allow(unused_variables)]

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
pub struct FieldDef {
    pub seq: usize,
    pub desc: String,
    pub datatype: String,
}

#[derive(Debug, Deserialize)]
pub struct SegmentDef {
    pub desc: String,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, Deserialize)]
pub struct VersionDef {
    pub version: String,
    pub segments: HashMap<String, SegmentDef>,
}

static DICTIONARY_V251: OnceLock<VersionDef> = OnceLock::new();

pub fn get_v251() -> &'static VersionDef {
    DICTIONARY_V251.get_or_init(|| {
        let json_str = include_str!("assets/hl7/v2.5.1.json");
        serde_json::from_str(json_str).expect("Failed to parse embedded v2.5.1 dictionary")
    })
}

pub fn get_field_description(version: &str, segment: &str, field_seq: usize) -> Option<String> {
    // Currently fallback to v2.5.1 for all versions, could be extended later
    let dict = get_v251();

    if let Some(seg_def) = dict.segments.get(segment) {
        // Try O(1) fast-path first assuming perfect sequence, fallback to O(N) search.
        let field_def = if field_seq > 0 {
            seg_def.fields.get(field_seq - 1)
        } else {
            None
        };

        let field_def = field_def
            .filter(|f| f.seq == field_seq)
            .or_else(|| seg_def.fields.iter().find(|f| f.seq == field_seq));

        if let Some(field_def) = field_def {
            return Some(field_def.desc.clone());
        }
    }
    None
}

/// Return the description for a segment (e.g. "MSH" → "Message Header").
pub fn get_segment_description(name: &str) -> Option<String> {
    get_v251().segments.get(name).map(|s| s.desc.clone())
}

pub fn inject_descriptions(segments: &mut [crate::hl7::types::Hl7Segment], version: &str) {
    let dict = get_v251();
    for segment in segments.iter_mut() {
        let seg_name = segment.name.clone();
        if let Some(seg_def) = dict.segments.get(&seg_name) {
            segment.description = Some(seg_def.desc.clone());
            for field in segment.fields.iter_mut() {
                let field_def = if field.index > 0 {
                    seg_def.fields.get(field.index - 1)
                } else {
                    None
                };

                let field_def = field_def
                    .filter(|f| f.seq == field.index)
                    .or_else(|| seg_def.fields.iter().find(|f| f.seq == field.index));

                if let Some(field_def) = field_def {
                    field.description = Some(field_def.desc.clone());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_lookup_pid5() {
        let desc = get_field_description("2.5.1", "PID", 5);
        assert_eq!(desc, Some("Patient Name".to_string()));
    }

    #[test]
    fn test_fallback_version() {
        let desc = get_field_description("2.2", "PID", 5);
        assert_eq!(desc, Some("Patient Name".to_string()));
    }

    #[test]
    fn test_invalid_segment() {
        let desc = get_field_description("2.5.1", "ZZZ", 1);
        assert_eq!(desc, None);
    }
}
