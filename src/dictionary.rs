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

impl SegmentDef {
    /// Look up a field by its 1-based HL7 sequence number.
    /// Uses an O(1) index fast-path first; falls back to a linear scan when the
    /// dictionary fields are not perfectly sequential (sparse or non-standard segments).
    pub fn field_by_seq(&self, seq: usize) -> Option<&FieldDef> {
        self.fields
            .get(seq.saturating_sub(1))
            .filter(|f| f.seq == seq)
            .or_else(|| self.fields.iter().find(|f| f.seq == seq))
    }
}

#[derive(Debug, Deserialize)]
pub struct VersionDef {
    /// Version string from the JSON file (e.g. `"2.5.1"`); reserved for future version-aware lookup.
    #[allow(dead_code)]
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

/// Look up a field description by segment name and 1-based field index.
/// Currently always uses the v2.5.1 dictionary; version-aware lookup is tracked in issue #49.
#[allow(dead_code)]
pub fn get_field_description(segment: &str, field_seq: usize) -> Option<String> {
    // Currently fallback to v2.5.1 for all versions, could be extended later
    let dict = get_v251();

    dict.segments
        .get(segment)
        .and_then(|seg| seg.field_by_seq(field_seq))
        .map(|f| f.desc.clone())
}

/// Return the description for a segment (e.g. "MSH" → "Message Header").
pub fn get_segment_description(name: &str) -> Option<String> {
    get_v251().segments.get(name).map(|s| s.desc.clone())
}

pub fn inject_descriptions(segments: &mut [crate::hl7::types::Hl7Segment]) {
    let dict = get_v251();
    for segment in segments.iter_mut() {
        let seg_name = segment.name.clone();
        if let Some(seg_def) = dict.segments.get(&seg_name) {
            segment.description = Some(seg_def.desc.clone());
            for field in segment.fields.iter_mut() {
                if let Some(field_def) = seg_def.field_by_seq(field.index) {
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
        let desc = get_field_description("PID", 5);
        assert_eq!(desc, Some("Patient Name".to_string()));
    }

    #[test]
    fn test_fallback_version() {
        let desc = get_field_description("PID", 5);
        assert_eq!(desc, Some("Patient Name".to_string()));
    }

    #[test]
    fn test_invalid_segment() {
        let desc = get_field_description("ZZZ", 1);
        assert_eq!(desc, None);
    }
}
