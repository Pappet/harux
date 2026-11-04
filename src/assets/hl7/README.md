# Embedded HL7 assets

Both files are compiled into the binary (`include_str!`) and parsed once at startup.

## `v2.5.1.json` — field / segment dictionary

Used by `src/dictionary.rs` for field and segment descriptions and by
`src/validation.rs` for datatype checks.

Contract the code relies on — check these when refreshing or replacing the file
(e.g. for version-aware dictionaries, #49):

- **MSH is numbered from `seq 1 = Field Separator`, `seq 2 = Encoding Characters`.**
  The parser inserts a synthetic MSH-1 so MSH fields use HL7 numbering
  (see the MSH quirk in `CLAUDE.md`). Guarded by
  `test_msh_dictionary_numbering_contract` and
  `test_msh_shift_dictionary_alignment`.
- **Within a segment, every `seq >= 1` is unique.** `SegmentDef::field_by_seq`
  would otherwise resolve by file order. Guarded by
  `test_field_seq_unique_per_segment`.
- `seq 0` rows (segment-level notes in some published tables) are allowed and
  ignored — no HL7 field index is 0.

## `message_types.json` — message-type registry

Used by `src/hl7/message_types.rs` (description and typical segments per
message type).
