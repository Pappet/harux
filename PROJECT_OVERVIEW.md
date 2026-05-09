# Harux — Project Overview

## What Is It

Harux is a high-performance MLLP server with a real-time web UI for inspecting HL7 v2.x messages. It is built in Rust and deployed as a single portable binary. It serves as a central service for interface integration teams to view and inspect MLLP messages in real time without local setup.

## Project Status

- **Language:** Rust
- **Latest Release:** v0.5.0 — UI Redesign & Accessibility complete
- **Next Milestone:** Milestone 4 (Workflow & Testing)

---

## Usage Context

- **Team:** Integration team (multiple developers simultaneously)
- **Software:** Orchestra (healthcare integration platform for interface development)
- **Infrastructure:** Windows Server, access via RDP + browser
- **Protocol:** HL7 v2.x over MLLP, FHIR R4 on the horizon
- **Traffic:** Includes ADT, ORU, ORM, SIU, and MDM message types. MDM with Base64-encoded PDF attachments is daily traffic, not an edge case.
- **Network:** Hospital internal network. No internet access from the server.

---

## Technical Decisions

| Topic | Decision | Rationale |
|---|---|---|
| Language | Rust | Performance, memory safety, single binary |
| Async runtime | Tokio | Proven, high throughput, low latency |
| Web framework | Axum | Tokio-native, type-safe, performant |
| UI | Embedded SPA (vanilla HTML/JS/CSS) | Zero dependencies, browser-based, multi-user capable |
| Static embedding | `rust-embed` | Frontend baked into binary at compile time — no separate web server |
| State sharing | `Arc<RwLock<>>` + broadcast channel | Simple, correct, sufficient for current scale |
| Persistence | In-memory (Milestones 0–4), SQLite (Milestone 5+) | Simple start, persistence when needed |
| Deployment | Single portable `.exe` | No installer, no runtime, xcopy deploy |
| CI/CD | GitHub Actions | Cross-platform builds: Windows, macOS, Linux |

## Non-Goals

- **No complex visual HL7 editor** — simple raw text editing for testing is provided, but not a full replacement for Orchestra's mapping UI
- **No external database servers** — no PostgreSQL/MSSQL; uses lightweight local SQLite when persistence is needed
- **No HL7 router** — message routing and transformation stays in Orchestra
- **No CSS/JS frameworks** — the vanilla SPA approach is intentional for zero-dependency deployment

---

## Architecture

Two independent async Tokio tasks share a single `MessageStore` via `Arc<RwLock<>>`:

```
+---------------------+    +------------------+    +------------------+
|  HL7 Sender         |--->|  MLLP Server     |--->|  Message Store   |
|  (HIS / RIS / PACS) |<---|  (Tokio TCP)     |    |  Arc<RwLock<>>   |
+---------------------+ ACK+------------------+    |  + broadcast ch. |
                                                    +--------+---------+
                                                             | push
                                                    +--------v---------+
                                                    |  Web Server      |
                                                    |  (Axum)          |
                                                    |  REST + WebSocket|
                                                    +--------+---------+
                                                             |
                                                    +--------v---------+
                                                    |  Browser SPA     |
                                                    |  (embedded HTML) |
                                                    +------------------+
```

### MLLP Server (`src/mllp.rs`)

Listens on a TCP port, handles MLLP framing (`0x0B` start, `0x1C 0x0D` end), parses HL7 messages, sends ACKs/NACKs, and inserts the result into the MessageStore.

- **ACK storm prevention:** incoming messages with `message_type.starts_with("ACK")` are stored but never ACK'd back — prevents infinite ping-pong with Orchestra.
- **DoS hardening:** 10 MB payload limit, 60s read timeout, 30s write timeout.
- **Connection limits:** configurable `max_connections` via `harux.toml`.

### Web Server (`src/web.rs`)

An Axum-based HTTP server providing REST API endpoints and a WebSocket endpoint for real-time updates.

### Message Store (`src/store.rs`)

A thread-safe in-memory buffer with dual eviction:

- **Count limit:** configurable capacity (default 10,000 messages)
- **Size limit:** configurable memory budget (default 512 MB)
- **Trigger:** either limit hit → evict oldest 10% of non-bookmarked messages
- **Rationale:** MDM messages with Base64-encoded attachments can be several MB each; count-only eviction is insufficient.

### HL7 Parser & Dictionary (`src/hl7/`, `src/dictionary.rs`, `src/validation.rs`)

The parser operates in four passes:

1. **Segment parsing** — extracts delimiters from MSH, splits on `\r`/`\n`, decomposes fields and components.
2. **Dictionary injection** — `inject_descriptions` walks every segment and field, filling `Hl7Segment.description` and `Hl7Field.description` from the embedded v2.5.1 JSON via a single `HashMap::get` per segment.
3. **Message type lookup** — `message_types::get_message_type_info` resolves the `TYPE^EVENT` string to a human-readable description and a list of typical segments; also builds the `typical_segment_descriptions` map for the UI badges.
4. **Validation** — `validation::validate_message` applies two layers of checks: (a) rule-based structural checks — universal MSH required fields and per-type required segments/fields for ADT, ORU^R01, ORM^O01, OML^O21, SIU, and MDM; (b) data type checks — `validate_data_types` iterates all fields, looks up each field's `datatype` from the dictionary, and validates NM/DT/TS/SI values using pure char pattern checks. Only the first component of each field is checked to avoid false positives on composite types.

Key source files:

- `hl7/parser.rs` — four-pass parse pipeline and ACK builder
- `hl7/types.rs` — `Hl7Message`, `Hl7MessageSummary`, `Hl7Segment` (with `description`), `Hl7Field`, `Delimiters`
- `hl7/message_types.rs` — `OnceLock<HashMap>` registry of 80+ HL7 v2.x message types with descriptions and typical segment lists
- `dictionary.rs` — `OnceLock`-based JSON dictionary engine; `inject_descriptions` fills segment + field descriptions; `get_segment_description` used for typical-segment badges
- `validation.rs` — `ValidationWarning` struct and rule engine; non-blocking (messages stored regardless of warnings)
- `src/assets/hl7/v2.5.1.json` — embedded dictionary source (segment descriptions + field definitions)

**MSH field indexing quirk:** MSH-1 is the field separator character itself (`|`). The parser inserts a synthetic `Hl7Field { index: 1, value: "|" }` and shifts all other fields up by 1, so that `get_field_value(msh, 3)` correctly returns Sending Application per the HL7 standard.

### Frontend (`static/`)

Three files, all embedded into the binary at compile time via `rust-embed`:

- `index.html` — HTML structure only
- `style.css` — All styles (dark theme, CSS variables)
- `app.js` — All logic (WebSocket, rendering, search, batching)

Key behaviors:
- Messages are batched and rendered at most every 250ms to prevent DOM freeze at high throughput
- Pause/Live button, Validation Filter, and Bookmarks toggle live in the `.inbox-controls` row; they buffer or filter incoming messages locally.
- Search is purely client-side via a pre-parsed query cache (bounded at 100 entries). Global shortcut is `Cmd+K`.
- Top-bar health pills show connection count, message rate, and error/validation counts.
- Parse errors are shown with `⚠ PARSE ERROR` in red in the message list and tracked in the health pills.
- Message list displays a two-line layout: row 1 has type, patient, tags; row 2 has facility, source, segment count, and relative time (e.g., `1m ago`). Sticky time-bucket group headers (`Live`, `Yesterday`, etc.) separate the messages.
- Validation warnings show as a coloured `⚠ N` badge in the list row and a one-line summary banner in the detail view with a collapsible bulleted list; badge colours: amber = `MISSING_FIELD`, red = `MISSING_SEGMENT`, blue = `INVALID_DATATYPE`
- Message type description displayed in the detail header; "Typical segments" is a checklist card (`.seg-checklist`) with explicit symbols: `✕` (red) = missing required segment, `⚠` (amber) = missing required field, `✓` (blue) = present, blank (grey) = absent.
- Segment headers have a CSS `::after` tooltip showing the segment description on hover
- Fields show their dictionary description inline as a `.desc-text` sub-line
- Segment diff: `diffPinnedMessage` state stores a full message fetched via `/api/messages/{id}`; `renderDiffTab()` builds a field-level two-column table with red/green highlighting (`table-layout: fixed`); optional `diffIgnoreDynamic` toggle hides MSH-7 and MSH-10 rows and shows a count of hidden fields in the summary
- Detail header is a layered structure: type chip + human title on row 1, optional description on row 2, monospace metadata row (`patient · MRN · control · v · received`) on row 3, plus a tags row. Bookmark and Pin-for-diff are explicit buttons in an actions column.
- Full keyboard accessibility: list rows, custom interactive elements, and source chips are navigable via `Tab` and actable via `Enter`/`Space`.

---

## Source Layout

```
src/
├── main.rs              # Entry point, tokio::select! over MLLP + Web tasks
├── config.rs            # Configuration loading (harux.toml + env vars)
├── mllp.rs              # TCP listener, MLLP framing, ACK/NACK dispatch
├── store.rs             # In-memory store with broadcast channel, dual eviction
├── web.rs               # Axum router, REST handlers, WebSocket handler
├── dictionary.rs        # OnceLock JSON dictionary engine (segment + field descriptions)
├── validation.rs        # Rule-based HL7 validator, ValidationWarning struct
└── hl7/
    ├── mod.rs
    ├── parser.rs        # Four-pass parse pipeline, delimiter extraction, ACK builder
    ├── types.rs         # Hl7Message, Hl7MessageSummary, Hl7Segment, Hl7Field, Delimiters
    └── message_types.rs # OnceLock registry of 80+ message types with descriptions
src/assets/hl7/
└── v2.5.1.json          # Embedded HL7 v2.5.1 dictionary (segment + field definitions)
static/
├── index.html           # HTML skeleton
├── style.css            # Dark theme, CSS variables
└── app.js               # SPA logic (vanilla JS)
tests/
├── test.sh              # Linux/macOS functional + load test (netcat, 100 messages)
└── test.ps1             # Windows functional + load test (.NET TcpClient, 1000 messages)
```

---

## API Reference

| Method | Endpoint | Description |
|---|---|---|
| `GET` | `/api/messages?offset=0&limit=100` | Paginated message list, newest first |
| `GET` | `/api/messages/{id}` | Full message with all segments and fields |
| `GET` | `/api/search?q=ADT&limit=100` | Search by type, patient, facility, ID, IP |
| `GET` | `/api/stats` | Live server stats (messages, connections, errors) |
| `POST` | `/api/clear` | Delete all messages from store |
| `POST` | `/api/messages/{id}/bookmark` | Toggle bookmark on a message |
| `POST` | `/api/messages/{id}/tags` | Add a tag to a message |
| `DELETE` | `/api/messages/{id}/tags/{tag}` | Remove a tag from a message |
| `WS` | `/ws` | Real-time updates: `init`, `new_message`, `lagged` events |

### WebSocket Events

```json
// On connect
{ "type": "init", "total": 42 }

// On new message
{ "type": "new_message", "data": { /* Hl7MessageSummary */ } }

// When client falls behind the broadcast buffer
{ "type": "lagged", "missed": 12 }
```

### Error Handling

| Scenario | Response |
|---|---|
| Valid HL7 message | `MSA\|AA` — Application Accept |
| Unknown message type (e.g. `ZZZ^Z99`) | `MSA\|AA` — type-agnostic acceptance |
| Missing or malformed MSH segment | `MSA\|AE` — Application Error (NACK) |
| Payload > 10 MB | Connection closed immediately |

---

## Technology Stack

| Component | Crate / Technology | Version |
|---|---|---|
| Async runtime | `tokio` | 1.x |
| Web framework | `axum` | 0.7 |
| HTTP middleware | `tower-http` (CORS, static FS) | 0.5 |
| Serialization | `serde` + `serde_json` | 1.x |
| Config parsing | `toml` | latest |
| Timestamps | `chrono` | 0.4 |
| UUID generation | `uuid` (v4) | 1.x |
| Logging | `tracing` + `tracing-subscriber` | 0.1/0.3 |
| Static files | `rust-embed` + `mime_guess` | 8.x/2.x |
| Error handling | `anyhow` | 1.x |
| Frontend | Vanilla JS / HTML / CSS | — |

---

## Deployment

- **Primary target:** Windows Server (`.exe`), run as a Windows Service via NSSM
- **Build pipeline:** GitHub Actions builds Windows, macOS (Apple Silicon), and Linux binaries on every push to `main` and attaches them to releases
- **Users:** Multiple developers simultaneously via browser — no local setup, no RDP window
- **Deployment style:** Single binary, xcopy. No installer, no runtime dependencies, no Docker.

---

## Testing

```bash
# Unit tests
cargo test

# Functional + load test (Linux/macOS, requires nc)
./test.sh

# Functional + load test (Windows, no external tools required)
.\tests\test.ps1
```

Both test scripts send the same set of HL7 messages: three valid types (ADT^A01, ORU^R01, SIU^S12), three error cases, followed by a load test. The PowerShell test uses a persistent TCP connection for load testing (1000 messages). The shell script spawns a new `nc` process per message (100 messages).

---

## Additional References

- [README.md](README.md) — Quick start and feature overview
- [ROADMAP.md](ROADMAP.md) — Milestones and planned features
- [STYLE_GUIDE.md](STYLE_GUIDE.md) — Appearance and functioning rules
- [CHANGELOG.md](CHANGELOG.md) — Full history of every change
