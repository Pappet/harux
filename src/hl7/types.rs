use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use uuid::Uuid;

/// A parsed HL7 v2.x message as stored in the in-memory store and serialised to the frontend.
///
/// Serialize-only: the detail JSON computes each field's `repetitions` on the
/// fly (see [`Hl7Segment`]'s `Serialize`), so there is nothing to read back.
#[derive(Debug, Clone, Serialize)]
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
    /// The message's delimiters from MSH-1/MSH-2, serialized as the 5-char
    /// string `"|^~\\&"` so the UI can draw the sender's own separators.
    /// Default delimiters for messages that failed to parse.
    pub delimiters: Delimiters,
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
    /// Validation warnings produced by the rule engine (empty = valid)
    pub validation_warnings: Vec<crate::validation::ValidationWarning>,
    /// Human-readable description of the message type (e.g. "Admit / Visit Notification")
    pub message_type_description: Option<String>,
    /// Segments typically present in this message type (from the HL7 spec),
    /// borrowed from the static message-type registry.
    pub typical_segments: &'static [String],
    /// Description for each typical segment name, borrowed from the embedded
    /// dictionary.
    pub typical_segment_descriptions: HashMap<&'static str, &'static str>,
    /// Character set declared in MSH-18, e.g. `"UTF-8"` or `"8859/1"`; `None` if absent.
    pub charset: Option<String>,
    /// How to give back the exact bytes that were received (#230). `raw` is
    /// text; for a non-UTF-8 payload these are not its bytes. Not part of the
    /// JSON.
    #[serde(skip)]
    pub wire: WireForm,
    /// MSH-9.3 message structure identifier, e.g. `"ADT_A01"` or `"ORU_R01"`.
    /// **Informational only**: exposed in the detail JSON (the UI's JSON tab)
    /// for the reader. Validation does not use it and is not meant to —
    /// `validate_message` dispatches on `message_type` (type and trigger
    /// event), because MSH-9.3 is optional and often missing or wrong in real
    /// traffic (#186). Kept because removing it would change the API.
    /// `None` when MSH-9 has fewer than three components (common in HL7 v2.3 senders).
    pub message_structure: Option<String>,
    /// Set for a message Harux sent (#272): the facts of the send that never
    /// change. `None` for a received message. For a sent message
    /// `source_addr` is the destination (`host:port`) and `received_at` the
    /// moment the send began — "the peer" and "when it entered the list".
    pub outbound: Option<Box<Outbound>>,
    /// Set for an over-limit frame (#223), which holds only the beginning of
    /// what was received. Such an entry cannot be resent (#272).
    pub truncated: bool,
}

/// Whether a stored message was received or sent (#272).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    In,
    Out,
}

/// What a sent message records about its send, fixed at insert time (#272).
/// What comes back later (the response) is [`Delivery`], in the metadata.
#[derive(Debug, Clone, Serialize)]
pub struct Outbound {
    /// The name the user gave the target — a label, shown in the list.
    pub target: String,
    /// How the message travelled: plain, TLS, or TLS without a certificate
    /// check (#273). Shown in the row.
    pub transport: Transport,
    /// Where the message came from.
    pub origin: Origin,
    /// The address of the client that asked for the send.
    pub requested_by: String,
    /// The run the send belongs to.
    pub run: String,
}

/// The connection a message was sent over (#273).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Plain,
    /// Encrypted, the target's certificate checked.
    Tls,
    /// Encrypted, the certificate not checked (`send.tls.allow_unverified`).
    TlsUnverified,
}

/// Where a sent message came from (#272).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    /// A stored message, sent again — unchanged, or with the fields in
    /// `changes` overridden (#275), e.g. `["MSH-10", "MSH-7"]`.
    Resend {
        message_id: String,
        changes: Vec<String>,
    },
    /// A draft (#276): text written or changed in the editor. `based_on` is
    /// the stored message it was made from, if any — where its references
    /// (`{{from:OBX[1]-5}}`) were filled in from.
    Draft { based_on: Option<String> },
    /// A draft made from a template (#279), by its name.
    Template { name: String },
}

impl Origin {
    /// The stored message this one was made from, if any.
    pub fn message_id(&self) -> Option<&str> {
        match self {
            Origin::Resend { message_id, .. } => Some(message_id),
            Origin::Draft { based_on } => based_on.as_deref(),
            Origin::Template { .. } => None,
        }
    }
}

/// What became of a sent message (#272).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryState {
    /// Stored, the exchange is under way.
    Pending,
    /// The destination answered with an MSA segment; `ack_code` says how.
    Acked,
    /// No usable answer: a transport failure, or a response without MSA.
    Failed,
    /// Written, and no response was awaited (an ACK-type message).
    Sent,
}

/// The outcome of a send, kept in the entry's mutable metadata next to tags
/// and bookmark (#272) — the payload stays immutable (#192).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub state: DeliveryState,
    /// MSA-1 of the response (`AA`, `AE`, `AR`, `CA`, …).
    pub ack_code: Option<String>,
    /// The response as text, cut at [`MAX_STORED_RESPONSE_BYTES`]. Never
    /// part of a summary: the destination chooses its size.
    pub response: Option<String>,
    /// Size of the response payload as received, in bytes.
    pub response_size: Option<usize>,
    /// `response` holds only the beginning of a larger response.
    pub response_truncated: bool,
    /// Why the response could not be read as an HL7 acknowledgement.
    pub response_error: Option<String>,
    /// Short reason of a failure: `refused`, `timeout`, `closed`, …
    pub error: Option<String>,
    /// The failure in a sentence.
    pub error_detail: Option<String>,
    /// From the start of the connect to the end of the response.
    pub latency_ms: Option<u64>,
    /// Whether MSA-2 repeats the control ID that was sent.
    pub control_id_matches: Option<bool>,
}

/// Most of a response that is stored (#272). A destination writes it, so its
/// size is not Harux's to choose.
pub const MAX_STORED_RESPONSE_BYTES: usize = 64 * 1024;

impl Delivery {
    /// The state a sent message is inserted with.
    pub fn pending() -> Self {
        Self {
            state: DeliveryState::Pending,
            ack_code: None,
            response: None,
            response_size: None,
            response_truncated: false,
            response_error: None,
            error: None,
            error_detail: None,
            latency_ms: None,
            control_id_matches: None,
        }
    }

    /// A failure with its short reason and its sentence.
    pub fn failed(error: &str, detail: impl Into<String>, latency_ms: Option<u64>) -> Self {
        Self {
            state: DeliveryState::Failed,
            error: Some(error.to_string()),
            error_detail: Some(detail.into()),
            latency_ms,
            ..Self::pending()
        }
    }

    fn estimated_bytes(&self) -> usize {
        opt_string(&self.ack_code)
            + opt_string(&self.response)
            + opt_string(&self.response_error)
            + opt_string(&self.error)
            + opt_string(&self.error_detail)
    }
}

/// The part of a [`Delivery`] a list row needs: no response text (#272).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverySummary {
    pub state: DeliveryState,
    pub ack_code: Option<String>,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

/// How the bytes a message arrived as relate to its `raw` text (#230).
#[derive(Debug, Clone, Default)]
pub enum WireForm {
    /// The payload was UTF-8 (or ASCII): the bytes are `raw` itself.
    #[default]
    Utf8,
    /// The payload was in this single- or multi-byte charset, and encoding
    /// `raw` with it gives back exactly the received bytes — so they are not
    /// stored a second time.
    Encoded(&'static encoding_rs::Encoding),
    /// The received bytes, kept because no encoding reproduces them from
    /// `raw` (the decode replaced invalid bytes).
    Original(Box<[u8]>),
}

/// One field within an HL7 segment.
#[derive(Debug, Clone)]
pub struct Hl7Field {
    /// 1-based field position within the segment (matches the HL7 standard numbering, e.g. PID-3).
    /// MSH is special: field 1 is synthetically `"|"`, so MSH-3 is Sending Application.
    pub index: usize,
    /// The raw field value exactly as received: repetitions, components,
    /// sub-components and escape sequences included. This is the field's
    /// **only** stored text (#180): its structure is derived on demand by the
    /// accessors below (and, for the detail JSON, at serialization time —
    /// #185), so a multi-MB ED payload (`^application^pdf^Base64^…`) is never
    /// held twice.
    pub value: String,
    /// Human-readable field name, borrowed from the embedded HL7 dictionary
    /// (`Cow::Borrowed`, no per-field allocation — #180).
    pub description: Option<Cow<'static, str>>,
}

/// A field's decoded structure: repetitions → components → sub-components,
/// each leaf escape-decoded (`Cow::Borrowed` unless it contained an escape).
pub type FieldStructure<'a> = Vec<Vec<Vec<Cow<'a, str>>>>;

impl Hl7Field {
    /// The raw repetitions of this field (always at least one, possibly empty).
    pub fn repetitions(&self, d: Delimiters) -> impl Iterator<Item = &str> {
        self.value.split(d.repetition)
    }

    /// Leaf `rep`/`comp`/`sub` (all 0-based), escape-decoded. `None` if the
    /// field has no such position. `leaf(d, 0, 0, 0)` of a plain value is the
    /// whole value.
    pub fn leaf(&self, d: Delimiters, rep: usize, comp: usize, sub: usize) -> Option<Cow<'_, str>> {
        let raw = self
            .repetitions(d)
            .nth(rep)?
            .split(d.component)
            .nth(comp)?
            .split(d.subcomponent)
            .nth(sub)?;
        Some(super::escape::decode(raw, d))
    }

    /// Whether the field has any structure worth showing: a repetition,
    /// component or sub-component separator, or an escape sequence.
    /// Plain values (and Base64 payloads) don't.
    pub fn is_structured(&self, d: Delimiters) -> bool {
        self.value
            .contains([d.repetition, d.component, d.subcomponent, d.escape])
    }

    /// The full decoded structure (see [`FieldStructure`]).
    pub fn structure(&self, d: Delimiters) -> FieldStructure<'_> {
        self.repetitions(d)
            .map(|rep| {
                rep.split(d.component)
                    .map(|comp| {
                        comp.split(d.subcomponent)
                            .map(|leaf| super::escape::decode(leaf, d))
                            .collect()
                    })
                    .collect()
            })
            .collect()
    }
}

/// One segment within an HL7 message (e.g. `MSH`, `PID`, `OBR`).
#[derive(Debug, Clone)]
pub struct Hl7Segment {
    /// Segment identifier, e.g. `"MSH"`, `"PID"`, `"OBX"`.
    pub name: String,
    /// All fields in this segment, in order. The segment's original text is
    /// not stored separately (#180) — it is a line of `Hl7Message::raw`
    /// (split on `\r` / `\n`, skipping blank lines, as the parser does).
    pub fields: Vec<Hl7Field>,
    /// Human-readable description from the HL7 dictionary (e.g. "Patient
    /// Identification"), borrowed from the embedded dictionary.
    pub description: Option<Cow<'static, str>>,
    /// The message's delimiters (a copy per segment, 20 bytes), so the
    /// segment can serialize its fields' structure on its own.
    pub delimiters: Delimiters,
}

/// Detail-JSON shape of a segment: `{name, fields, description}`, where each
/// field is `{index, value, description, datatype}` plus `repetitions`
/// (a [`FieldStructure`]) when [`Hl7Field::is_structured`]. Computed here,
/// at serialization time, instead of being stored (#185).
impl Serialize for Hl7Segment {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        // Datatypes from the dictionary (#276): the draft editor shows them.
        let def = crate::dictionary::get_v251().segments.get(&self.name);
        let fields: Vec<FieldJson<'_>> = self
            .fields
            .iter()
            .map(|f| {
                // MSH-1/MSH-2 *are* the delimiters, not structured content.
                let is_msh_header = self.name == "MSH" && f.index <= 2;
                FieldJson {
                    index: f.index,
                    value: &f.value,
                    repetitions: (!is_msh_header && f.is_structured(self.delimiters))
                        .then(|| f.structure(self.delimiters)),
                    description: f.description.as_deref(),
                    datatype: def
                        .and_then(|d| d.field_by_seq(f.index))
                        .map(|d| d.datatype.as_str()),
                }
            })
            .collect();
        let mut st = serializer.serialize_struct("Hl7Segment", 3)?;
        st.serialize_field("name", &self.name)?;
        st.serialize_field("fields", &fields)?;
        st.serialize_field("description", &self.description)?;
        st.end()
    }
}

#[derive(Serialize)]
struct FieldJson<'a> {
    index: usize,
    value: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    repetitions: Option<FieldStructure<'a>>,
    description: Option<&'a str>,
    /// HL7 datatype from the dictionary (`XPN`, `TS`, …), `null` for a
    /// field the dictionary does not know.
    datatype: Option<&'a str>,
}

/// Approximate heap footprint of one allocation of `size` bytes, including
/// the allocator's bookkeeping. No allocation → 0. The basis of all store
/// accounting (#226).
///
/// The model is **glibc malloc** (Linux): an 8-byte header, 16-byte
/// granularity and a 32-byte minimum chunk; `tests/alloc_accounting.rs`
/// checks it there. Other allocators differ — the Windows heap, the primary
/// deployment, has no 32-byte minimum, so short strings are over-counted
/// there. That errs on the safe side: the store evicts somewhat earlier, it
/// never holds more than `store.max_memory_mb`.
pub fn heap_bytes(size: usize) -> usize {
    if size == 0 {
        0
    } else {
        ((size + 8 + 15) & !15).max(32)
    }
}

/// Heap held by a `String`: its capacity, not its length.
fn owned_string(s: &String) -> usize {
    heap_bytes(s.capacity())
}

fn opt_string(s: &Option<String>) -> usize {
    s.as_ref().map_or(0, owned_string)
}

/// Heap bytes owned by an optional dictionary text: zero when it is borrowed
/// from the static dictionary.
fn owned_len(s: &Option<Cow<'static, str>>) -> usize {
    match s {
        Some(Cow::Owned(o)) => owned_string(o),
        _ => 0,
    }
}

/// Heap of a `Vec<T>`'s buffer (by capacity).
fn vec_heap<T>(v: &Vec<T>) -> usize {
    heap_bytes(v.capacity() * std::mem::size_of::<T>())
}

/// Heap of a hashbrown table: buckets of `size_of::<(K, V)>()` plus one
/// control byte each (and a group of 16), at 7/8 maximum load.
fn map_heap<K, V>(m: &HashMap<K, V>) -> usize {
    if m.capacity() == 0 {
        return 0;
    }
    let buckets = (m.capacity() * 8 / 7).next_power_of_two().max(4);
    heap_bytes(buckets * (std::mem::size_of::<(K, V)>() + 1) + 16)
}

/// Separators / encoding characters extracted from MSH-1 (field separator) and MSH-2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delimiters {
    /// MSH-1: separates fields within a segment (standard: `|`).
    pub field: char,
    /// MSH-2 first char: separates components within a field (standard: `^`).
    pub component: char,
    /// MSH-2 second char: separates repetitions of a field (standard: `~`).
    pub repetition: char,
    /// MSH-2 third char: introduces escape sequences (standard: `\`).
    pub escape: char,
    /// MSH-2 fourth char: separates sub-components (standard: `&`).
    pub subcomponent: char,
}

impl Delimiters {
    /// The MSH header form: field separator followed by the four encoding
    /// characters, e.g. `"|^~\\&"`.
    pub fn as_msh_prefix(&self) -> String {
        [
            self.field,
            self.component,
            self.repetition,
            self.escape,
            self.subcomponent,
        ]
        .iter()
        .collect()
    }
}

/// Serialized as the 5-char MSH header form (`"|^~\\&"`).
impl Serialize for Delimiters {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_msh_prefix())
    }
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
    /// Heap bytes held by this message: every buffer it owns, by capacity,
    /// plus allocator overhead per allocation ([`heap_bytes`]). Includes the
    /// fixed-size `Hl7Segment` / `Hl7Field` structs, which dominate for
    /// ordinary messages — a 731-byte ADT holds about 18 KB, mostly in its
    /// 149 fields (#226). Excludes the `Hl7Message` struct itself, which the
    /// store accounts with the `Arc` that holds it.
    pub fn estimated_bytes(&self) -> usize {
        let mut total = owned_string(&self.raw)
            + owned_string(&self.id)
            + owned_string(&self.source_addr)
            + owned_string(&self.message_type)
            + owned_string(&self.trigger_event)
            + owned_string(&self.message_control_id)
            + owned_string(&self.sending_application)
            + owned_string(&self.sending_facility)
            + owned_string(&self.receiving_application)
            + owned_string(&self.receiving_facility)
            + owned_string(&self.version)
            + opt_string(&self.patient_name)
            + opt_string(&self.patient_id)
            + opt_string(&self.parse_error)
            + opt_string(&self.ack_response)
            + opt_string(&self.ack_code)
            + opt_string(&self.message_type_description)
            + opt_string(&self.charset)
            + opt_string(&self.message_structure)
            + map_heap(&self.typical_segment_descriptions)
            + match &self.wire {
                WireForm::Original(bytes) => heap_bytes(bytes.len()),
                _ => 0,
            }
            + vec_heap(&self.validation_warnings)
            + vec_heap(&self.segments)
            + self.outbound.as_ref().map_or(0, |o| {
                heap_bytes(std::mem::size_of::<Outbound>())
                    + owned_string(&o.target)
                    + owned_string(&o.requested_by)
                    + owned_string(&o.run)
                    + match &o.origin {
                        Origin::Resend {
                            message_id,
                            changes,
                        } => {
                            owned_string(message_id)
                                + vec_heap(changes)
                                + changes.iter().map(owned_string).sum::<usize>()
                        }
                        Origin::Draft { based_on } => opt_string(based_on),
                        Origin::Template { name } => owned_string(name),
                    }
            });

        for w in &self.validation_warnings {
            total += owned_string(&w.code) + owned_string(&w.message) + owned_string(&w.segment);
        }
        for seg in &self.segments {
            total += owned_string(&seg.name) + owned_len(&seg.description) + vec_heap(&seg.fields);
            for field in &seg.fields {
                total += owned_string(&field.value) + owned_len(&field.description);
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
            delimiters: Delimiters::default(),
            patient_name: None,
            patient_id: None,
            parse_error: None,
            ack_response: None,
            ack_code: None,
            validation_warnings: Vec::new(),
            message_type_description: None,
            typical_segments: &[],
            typical_segment_descriptions: HashMap::new(),
            charset: None,
            wire: WireForm::Utf8,
            message_structure: None,
            outbound: None,
            truncated: false,
        }
    }

    /// `Out` for a message Harux sent, `In` for one it received.
    pub fn direction(&self) -> Direction {
        if self.outbound.is_some() {
            Direction::Out
        } else {
            Direction::In
        }
    }

    /// The bytes this message arrived as (#230): `raw` for UTF-8, `raw`
    /// re-encoded for other charsets, or the stored original.
    pub fn wire_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        match &self.wire {
            WireForm::Utf8 => std::borrow::Cow::Borrowed(self.raw.as_bytes()),
            WireForm::Original(bytes) => std::borrow::Cow::Borrowed(bytes),
            WireForm::Encoded(enc) => match enc.encode(&self.raw).0 {
                std::borrow::Cow::Borrowed(b) => std::borrow::Cow::Owned(b.to_vec()),
                std::borrow::Cow::Owned(v) => std::borrow::Cow::Owned(v),
            },
        }
    }

    /// `wire_bytes().len()` without re-encoding where the length is known:
    /// a single-byte charset writes one byte per character.
    pub fn wire_len(&self) -> usize {
        match &self.wire {
            WireForm::Utf8 => self.raw.len(),
            WireForm::Original(bytes) => bytes.len(),
            WireForm::Encoded(enc) if enc.is_single_byte() => self.raw.chars().count(),
            WireForm::Encoded(_) => self.wire_bytes().len(),
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
    /// `in` (received) or `out` (sent by Harux, #272).
    pub direction: Direction,
    /// For a sent message: the name of its target. The UI groups and
    /// colours a sent message by this, never by `source_addr`.
    pub target: Option<String>,
    /// For a sent message: the stored message it was made from.
    pub origin_id: Option<String>,
    /// For a sent message: plain, TLS or unverified TLS (#273).
    pub transport: Option<Transport>,
    /// For a sent message: what became of it, without the response text.
    pub delivery: Option<DeliverySummary>,
    /// Only the beginning of an over-limit frame is stored (#223).
    pub truncated: bool,
}

/// User-mutable state of a stored message (tags, bookmark).
///
/// Kept **outside** the immutable, `Arc`-shared [`Hl7Message`] payload (#192):
/// tagging or bookmarking only touches this small struct, so it never forces
/// a copy-on-write deep clone of a multi-MB message while the store's write
/// lock is held. The JSON contract is unchanged — [`MessageSnapshot`]
/// flattens both parts into one object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageMeta {
    /// User-defined tag labels attached via the API.
    pub tags: Vec<String>,
    /// Whether the message is pinned; bookmarked messages are protected from
    /// eviction within the store's bookmark budget.
    pub bookmarked: bool,
    /// What became of a sent message (#272); `None` for a received one.
    /// Boxed, so a received message's entry does not grow by its size.
    pub delivery: Option<Box<Delivery>>,
}

impl MessageMeta {
    /// Heap bytes held by the metadata (tag list and strings, the delivery
    /// and its response), for store accounting — same model as
    /// [`Hl7Message::estimated_bytes`].
    pub fn estimated_bytes(&self) -> usize {
        vec_heap(&self.tags)
            + self.tags.iter().map(owned_string).sum::<usize>()
            + self.delivery.as_ref().map_or(0, |d| {
                heap_bytes(std::mem::size_of::<Delivery>()) + d.estimated_bytes()
            })
    }
}

/// A stored message as returned by `GET /api/messages/{id}`: the shared,
/// immutable payload plus its current metadata. Serializes as a single flat
/// JSON object (all `Hl7Message` fields plus `tags` and `bookmarked`), exactly
/// as before the payload/metadata split.
///
/// `#[serde(flatten)]` requires both parts to serialize as maps (structs with
/// named fields). Turning `Hl7Message` or `MessageMeta` into a newtype/tuple
/// struct would fail at *runtime* serialization, not at compile time —
/// `test_snapshot_serializes_flat` guards this.
#[derive(Debug, Clone, Serialize)]
pub struct MessageSnapshot {
    #[serde(flatten)]
    pub message: std::sync::Arc<Hl7Message>,
    #[serde(flatten)]
    pub meta: MessageMeta,
    /// `in` or `out` (#272), like the summary.
    pub direction: Direction,
}

impl Hl7MessageSummary {
    /// Build the list-view summary from a message payload and its metadata.
    pub fn new(msg: &Hl7Message, meta: &MessageMeta) -> Self {
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
            tags: meta.tags.clone(),
            bookmarked: meta.bookmarked,
            validation_warning_count: msg.validation_warnings.len(),
            has_segment_errors: msg
                .validation_warnings
                .iter()
                .any(|w| w.code == "MISSING_SEGMENT"),
            message_type_description: msg.message_type_description.clone(),
            charset: msg.charset.clone(),
            direction: msg.direction(),
            target: msg.outbound.as_ref().map(|o| o.target.clone()),
            origin_id: msg
                .outbound
                .as_ref()
                .and_then(|o| o.origin.message_id().map(str::to_owned)),
            transport: msg.outbound.as_ref().map(|o| o.transport),
            delivery: meta.delivery.as_ref().map(|d| DeliverySummary {
                state: d.state,
                ack_code: d.ack_code.clone(),
                latency_ms: d.latency_ms,
                error: d.error.clone(),
            }),
            truncated: msg.truncated,
        }
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    /// The spellings `static/render.js` compares against (#285 review): a
    /// rename on this side would otherwise drop the row's TLS label silently.
    #[test]
    fn transport_and_direction_spellings() {
        let s = |v: serde_json::Value| v.as_str().unwrap().to_string();
        assert_eq!(s(serde_json::json!(Transport::Plain)), "plain");
        assert_eq!(s(serde_json::json!(Transport::Tls)), "tls");
        assert_eq!(
            s(serde_json::json!(Transport::TlsUnverified)),
            "tls_unverified"
        );
        assert_eq!(s(serde_json::json!(Direction::Out)), "out");
        assert_eq!(s(serde_json::json!(DeliveryState::Acked)), "acked");
    }
}
