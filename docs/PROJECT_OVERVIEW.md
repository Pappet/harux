# Harux — Project Overview

## What Is It

Harux is a high-performance MLLP server with a real-time web UI for inspecting HL7 v2.x messages. It is built in Rust and deployed as a single portable binary. It serves as a central service for interface integration teams to view and inspect MLLP messages in real time without local setup.

## Project Status

- **Language:** Rust
- **Latest Release:** v0.9.0 — Reliability & Security Hardening (audit Phases 0–3, v0.8.0 review) — see the [changelog](../CHANGELOG.md)
- **Scope today:** receive and inspect (Milestones 0–3), and — once `send.enabled` switches it on — resend a stored message unchanged to an MLLP destination (Milestone 4, first package). Editing, templates, runs (the rest of Milestone 4) and persistence (Milestone 5) are planned — see the [roadmap](ROADMAP.md)

---

## Usage Context

- **Team:** Integration team (multiple developers simultaneously)
- **Software:** Orchestra (healthcare integration platform for interface development)
- **Infrastructure:** Windows Server; Harux runs there as a service and the team uses it through the browser (no RDP session needed)
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

- **No complex visual HL7 editor** — simple raw-text editing for testing is planned (Milestone 4), not a replacement for Orchestra's mapping UI
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

Listens on `server.mllp_bind:mllp_port`, handles MLLP framing (`0x0B` start, `0x1C 0x0D` end), decodes the payload per MSH-18 (`src/hl7/charset.rs`), parses it, **stores it, then** sends the ACK/NACK.

- **No frame is lost silently:** decoding and parsing run under `catch_unwind`; a frame that fails to parse — or panics the parser — is stored as a parse-error entry and answered with a NACK.
- **ACK storm prevention:** incoming messages with `message_type.starts_with("ACK")` are stored but never ACK'd back — prevents infinite ping-pong with Orchestra.
- **ACKs** are written in the sender's own delimiters and echo MSH-10 in MSA-2; NACKs use default delimiters and MSA-2 `UNKNOWN`. A NACK or reject says why: MSA-3 (up to 80 characters) and an ERR segment with an HL7 error code (ERR-3, table 0357) and the reason (ERR-8, up to 250 characters). Replies are written in the sender's own charset (e.g. Latin-1 per MSH-18) and declare it in MSH-18.
- **Limits:** `mllp.max_message_size_mb` (10 MB) — a larger frame gets an `AR` reject, its first 4 KB are stored as a parse error, and the connection stays usable; 60 s read and 30 s write timeout; `mllp.max_connections` (100) — connections beyond it are closed and counted as rejected.
- **Robustness:** an `accept()` error (e.g. out of file descriptors) is logged and retried, never fatal; on Ctrl+C or SIGTERM the server stops accepting and waits up to `server.shutdown_timeout_secs` for open connections to drain.

### Sending (`src/send.rs`, `src/mllp_client.rs`)

Off unless `send.enabled = true` (#272). `POST /api/send` resends one stored message, unchanged — the bytes it arrived as — to the destination the request names (every user keeps their own targets in the browser; the server keeps none). `send.allowed_destinations` can limit where users may send.

- **Everything sent is stored first:** the sent message is inserted with delivery `pending` **before** the connection is opened, so a refused or timed-out send is a row too, and what the destination forwards because of it is listed after it. The outcome (`acked` with the MSA-1 code, `failed` with a reason, `sent` when no response is awaited) is recorded in the entry's metadata and broadcast as `delivery_updated`.
- **A sent message is a message:** same list, detail, tags, bookmark, search and eviction. It has `direction: "out"`, `outbound` (target name, origin, requesting client address, run) and its response in `delivery`. Its `source_addr` is the destination. The UI shows it with `↑`, a dashed bar and `→ target`, and the ACK tab becomes **Response**.
- **The exchange** (`mllp_client.rs`): one frame out, one frame back, `TCP_NODELAY`; connect timeout `send.connect_timeout_secs`, response timeout `send.response_timeout_secs`, write timeout `mllp.write_timeout_secs`; responses above 1 MB are refused, at most 64 KB of a response is stored. A message whose type starts with `ACK` is written without waiting for an answer.
- **Limits:** `send.max_concurrent_runs` runs at a time (`429` beyond); a message may take at most `send.max_run_store_percent` of the store when it is stored again. A frame above the size limit (stored cut off, `truncated`) cannot be resent; one that did not parse only with `allow_invalid`.
- Every send is logged at `info` (failure: `warn`) with type, control ID, size, target, destination and requesting address — never with content.

### Web Server (`src/web.rs`)

An Axum-based HTTP server on `server.web_bind:web_port` providing REST API endpoints, the export and a WebSocket endpoint for real-time updates. The embedded SPA is served from the binary.

Every request passes a cross-site guard: the `Host` header must be an IP address, `localhost` or a name in `server.allowed_hosts` (blocks DNS rebinding), and on `/ws` and every state-changing request an `Origin` header, if present, must name this server. Responses carry a Content-Security-Policy without inline scripts or styles, `frame-ancestors 'none'` and `X-Content-Type-Options: nosniff`.

### Message Store (`src/store.rs`)

A thread-safe in-memory buffer with dual eviction:

- **Count limit:** `store.max_messages` (default 10,000)
- **Size limit:** `store.max_memory_mb` (default 512 MB) — measured as what the messages really occupy on the heap, including allocator overhead, so the limit bounds real memory
- **Trigger:** either limit hit → the oldest messages are evicted until both bytes and count are at or below 90 % of their limits
- **Bookmarks** are skipped by eviction, but only within `store.bookmark_budget_percent` (default 50 %, at most 80 %) of either limit; beyond it the oldest bookmarks go too, so bookmarking can never lift the memory cap
- **Payload vs. metadata:** each message is an immutable payload plus mutable tags/bookmark, so tagging never copies a message
- **Change events** (new, tags, bookmark, delivery, evicted, cleared) are broadcast to the WebSocket clients in the order the store changed
- **Rationale:** MDM messages with Base64-encoded attachments can be several MB each; count-only eviction is insufficient.

### HL7 Parser & Dictionary (`src/hl7/`, `src/dictionary.rs`, `src/validation.rs`)

The parser operates in four passes:

1. **Segment parsing** — extracts delimiters from MSH (printable, distinct, no letters or digits), splits on `\r`/`\n` and into fields. Every non-blank line of `raw` is exactly one segment. Repetitions, components, sub-components and escape sequences are derived from each field's raw value on demand with the message's own delimiters (#185); nothing structural is stored twice.
2. **Dictionary injection** — `inject_descriptions` walks every segment and field, filling `Hl7Segment.description` and `Hl7Field.description` from the embedded v2.5.1 JSON via a single `HashMap::get` per segment.
3. **Message type lookup** — `message_types::get_message_type_info` resolves the `TYPE^EVENT` string to a human-readable description and a list of typical segments; also builds the `typical_segment_descriptions` map for the UI badges.
4. **Validation** — `validation::validate_message` applies two layers of checks: (a) rule-based structural checks — universal MSH required fields and per-type required segments/fields for ADT, ORU^R01, ORM^O01, OML^O21, SIU and MDM, following the v2.5.1 message structures (optional groups and conditional fields such as OBR-2/OBR-3 are not reported); (b) data type checks — `validate_data_types` looks up each field's `datatype` in the dictionary and validates NM/DT/TS/SI values: dates must exist (`chrono`), times must be in range, a TS fraction and offset must have the standard form. The first component of each repetition is checked, so composite types don't raise false positives.

Key source files:

- `hl7/parser.rs` — four-pass parse pipeline and ACK/NACK/reject builder
- `hl7/charset.rs` — payload bytes → text per MSH-18, with a `CHARSET_FALLBACK` warning when the declared charset doesn't fit
- `hl7/escape.rs` — HL7 escape sequences (`\F\`, `\.br\`, …) in both directions
- `hl7/types.rs` — `Hl7Message`, `Hl7MessageSummary`, `Hl7Segment` (with `description`), `Hl7Field`, `Delimiters`
- `hl7/message_types.rs` — `OnceLock<HashMap>` registry loaded from the embedded `src/assets/hl7/message_types.json` (106 message types with descriptions and typical segment lists)
- `dictionary.rs` — `OnceLock`-based JSON dictionary engine; `inject_descriptions` fills segment + field descriptions; `get_segment_description` used for typical-segment badges
- `validation.rs` — `ValidationWarning` struct and rule engine; non-blocking (messages stored regardless of warnings)
- `src/assets/hl7/v2.5.1.json` — embedded dictionary source (segment descriptions + field definitions)

**MSH field indexing quirk:** MSH-1 is the field separator character itself (`|`). The parser inserts a synthetic `Hl7Field { index: 1, value: "|" }` and shifts all other fields up by 1, so that `get_field_value(msh, 3)` correctly returns Sending Application per the HL7 standard.

### Frontend (`static/`)

Plain files, all embedded into the binary at compile time via `rust-embed`. The JavaScript has no module system; the files share one global `state` object and load in this order:

- `index.html` — HTML structure only
- `style.css` — All styles (dark theme, CSS variables); fonts are self-hosted in `static/fonts/`
- `state.js` — the `state` object (all mutable globals)
- `util.js` — constants and pure helpers (escaping, formatting)
- `ws.js` — WebSocket, message ingestion, stats polling
- `render.js` — message list, health pills, detail-panel tabs
- `diff.js` — the Diff tab
- `app.js` — initialisation, UI handlers (`data-action` delegation) and the splitter

The Content-Security-Policy forbids inline scripts, styles and event handlers.

Key behaviors:
- Messages are batched and rendered at most every 250ms to prevent DOM freeze at high throughput
- Pause/Live button, Validation Filter, and Bookmarks toggle live in the `.inbox-controls` row; they buffer or filter incoming messages locally.
- The browser keeps at most 2,000 messages (plus bookmarked and selected ones); the server may hold more, and a note at the bottom of the list says so and offers "Load 500 older" and, during a search, "Search older on the server". Loaded pages stay until a reload. Bookmarked messages are loaded on every (re)load, so they survive a page reload.
- Search is client-side over the loaded messages, via a pre-parsed query cache (bounded at 100 entries). Global shortcut is `Cmd+K`.
- Top-bar health pills show connection count, message rate, and error/validation counts.
- Parse errors are shown with `⚠ PARSE ERROR` in red in the message list and tracked in the health pills.
- Message list displays a two-line layout: row 1 has type, patient, tags; row 2 has facility, source, segment count, and relative time (e.g., `1m ago`). Sticky time-bucket group headers (`Live`, `Yesterday`, etc.) separate the messages.
- Validation warnings show as a coloured `⚠ N` badge in the list row and a one-line summary banner in the detail view with a collapsible bulleted list; badge colours: amber = `MISSING_FIELD`, red = `MISSING_SEGMENT`, blue = `INVALID_DATATYPE`
- Message type description displayed in the detail header; "Typical segments" is a checklist card (`.seg-checklist`) with explicit symbols: `✕` (red) = missing required segment, `⚠` (amber) = missing required field, `✓` (blue) = present, blank (grey) = absent.
- Segment headers have a CSS `::after` tooltip showing the segment description on hover
- Fields show their dictionary description inline as a `.desc-text` sub-line
- Segment diff: `diffPinnedMessage` state stores a full message fetched via `/api/messages/{id}`; `renderDiffTab()` pairs segments by name and occurrence (the n-th OBX with the n-th OBX; a segment only one side has shows as added or removed) and builds a field-level two-column table with red/green highlighting (`table-layout: fixed`); optional `diffIgnoreDynamic` toggle hides MSH-7 and MSH-10 rows and shows a count of hidden fields in the summary
- Detail header is a layered structure: type chip + human title on row 1, optional description on row 2, monospace metadata row (`patient · MRN · control · v · received`) on row 3, plus a tags row. Bookmark and Pin-for-diff are explicit buttons in an actions column.
- Full keyboard accessibility: list rows, custom interactive elements, and source chips are navigable via `Tab` and actable via `Enter`/`Space`.

---

## Source Layout

The annotated file-by-file layout is maintained in [`AGENTS.md` → Source Layout](../AGENTS.md#source-layout), so it exists in one place only.

---

## Configuration

Built-in defaults → `harux.toml` (next to the binary, then the working directory) → environment variables (`MLLP_PORT`, `WEB_PORT`, `RUST_LOG`). [`examples/harux-example.toml`](../examples/harux-example.toml) lists every key with its default. An unknown key, a wrong type or an impossible value stops the start with an error naming the key.

| Key | Default | Meaning |
|---|---|---|
| `server.mllp_port` / `server.web_port` | `2575` / `8080` | Listener ports |
| `server.mllp_bind` / `server.web_bind` | `0.0.0.0` | Listener addresses (`127.0.0.1` = this machine only, `::` = IPv6 too) |
| `server.allowed_hosts` | `[]` | DNS names the UI is opened under (IP addresses and `localhost` always work) |
| `server.shutdown_timeout_secs` | `10` | How long shutdown waits for MLLP connections to drain |
| `logging.level` | `info` | Log level or `target=level` list; `RUST_LOG` overrides it |
| `logging.file` | — | Enables file logging: one file per day, `<file>.YYYY-MM-DD` |
| `logging.max_files` | `14` | Daily log files kept (today's included); `0` keeps all |
| `store.max_messages` | `10000` | Count limit |
| `store.max_memory_mb` | `512` | Memory limit |
| `store.bookmark_budget_percent` | `50` | Share of either limit bookmarks may hold under eviction pressure (max 80) |
| `mllp.max_message_size_mb` | `10` | Largest accepted frame |
| `mllp.read_timeout_secs` / `mllp.write_timeout_secs` | `60` / `30` | MLLP timeouts |
| `mllp.max_connections` | `100` | Concurrent MLLP connections |
| `send.enabled` | `false` | Sending from the web UI and the API (`SECURITY.md` → *Sending*) |
| `send.allowed_destinations` | `[]` | Where users may send: `host:port` or `host:from-to`; empty = anywhere |
| `send.loopback_target` | `true` | Offer Harux's own MLLP listener as the target "This Harux" (always allowed) |
| `send.connect_timeout_secs` / `send.response_timeout_secs` | `5` / `30` | Send timeouts |
| `send.max_concurrent_runs` | `4` | Sends at a time |
| `send.max_run_messages` / `send.max_run_store_percent` | `1000` / `25` | Limits of one run; a run's messages may take at most this share of `store.max_memory_mb` |

---

## API Reference

| Method | Endpoint | Description |
|---|---|---|
| `GET` | `/api/messages?offset=0&limit=100&include_bookmarks=true` | Paginated message list, newest first (`limit` capped at 1000); `include_bookmarks` adds every older bookmarked message; `before=<id>` instead of `offset` pages through older history (the UI's "Load older") |
| `GET` | `/api/messages/{id}` | Full message with all segments and fields |
| `GET` | `/api/search?q=ADT&limit=100&before=<id>` | Search like the UI filter (`has:warnings`/`has:errors`, type, patient, facility, control ID, source, tags), newest first; `before` searches only older messages. The UI uses it for "Search older on the server" |
| `GET` | `/api/stats` | Live server stats: message counts, store bytes and limit, parse errors, connections, `sending_enabled`, `sent`, `send_failed`, `active_runs` |
| `GET` | `/api/export?direction=in` | Every stored received message as one MLLP-framed `harux-export.hl7`, byte for byte as received, oldest first (streamed); `direction=out` the sent ones, `all` both |
| `GET` | `/api/send/config` | Whether sending is on, the loopback target, the destination and run limits (`403` while sending is off) |
| `POST` | `/api/send` | Resend one stored message unchanged (below); `403` while sending is off |
| `POST` | `/api/clear` | Delete all messages from store |
| `POST` | `/api/messages/{id}/bookmark` | Toggle bookmark on a message |
| `POST` | `/api/messages/{id}/tags` | Add a tag to a message |
| `DELETE` | `/api/messages/{id}/tags/{tag}` | Remove a tag from a message |
| `WS` | `/ws` | Real-time updates (events below) |

### `POST /api/send`

```jsonc
{
  "target": { "name": "orchestra-test", "host": "orchestra-test.klinik.local", "port": 6661 },  // or "loopback"
  "items": [{ "message_id": "…" }],   // exactly one stored message
  "await_response": true,             // false: do not wait for an answer
  "allow_invalid": false              // resend a message that did not parse
}
```

Answers `200` with `{ "id": "<sent message>", "delivery": { "state", "ack_code", "response", "latency_ms", "error", "control_id_matches", … } }` once the exchange ended — a refused connection or a NACK is a `200` too: the request worked, the send did not. Refusals: `400` malformed request, host or target (unknown fields such as `set` are refused, TLS is not available yet) · `403` sending off, destination outside `send.allowed_destinations` · `404` unknown message · `413` message too large · `422` a cut-off entry, or a message that did not parse without `allow_invalid` · `429` too many sends at once.

### WebSocket Events

```json
// On connect
{ "type": "init", "total": 42 }

// A message was stored / its tags or bookmark changed
{ "type": "new_message",      "data": { /* Hl7MessageSummary */ } }
{ "type": "tags_updated",     "data": { /* Hl7MessageSummary */ } }
{ "type": "bookmark_toggled", "data": { /* Hl7MessageSummary */ } }
// A sent message's outcome arrived (summary with `delivery`, no response text)
{ "type": "delivery_updated", "data": { /* Hl7MessageSummary */ } }

// An eviction pass removed these messages / the store was cleared
{ "type": "evicted", "data": ["<id>", "<id>"] }
{ "type": "cleared" }

// The client fell behind the broadcast buffer; it reloads the list
{ "type": "lagged", "missed": 12 }
```

Events arrive in the order the store changed. State-changing routes are `POST`/`DELETE` so the cross-site guard covers them.

### Error Handling

| Scenario | Response |
|---|---|
| Valid HL7 message | `MSA\|AA` — Application Accept |
| Unknown message type (e.g. `ZZZ^Z99`) | `MSA\|AA` — type-agnostic acceptance |
| Missing or malformed MSH segment (incl. a letter or digit as delimiter) | `MSA\|AE` — Application Error (NACK) + `ERR\|\|\|100^Segment sequence error^HL70357\|E\|\|\|\|<reason>`; stored as a parse error |
| Payload > `mllp.max_message_size_mb` | `MSA\|AR` — Application Reject + ERR `207` (Application internal error); first 4 KB stored as a parse error, connection stays open |
| Incoming `ACK` message | Stored, no response (prevents ACK storms) |
| More than `mllp.max_connections` connections | New connection closed, counted as rejected |

---

## Technology Stack

| Component | Crate / Technology | Version |
|---|---|---|
| Async runtime | `tokio` | 1.x |
| Web framework | `axum` (with WebSocket) | 0.8 |
| Serialization | `serde` + `serde_json` | 1.x |
| Config parsing | `toml` | 0.8 |
| Charset decoding | `encoding_rs` | 0.8 |
| Timestamps / dates | `chrono` | 0.4 |
| UUID generation | `uuid` (v4) | 1.x |
| Logging | `tracing` + `tracing-subscriber` + `tracing-appender` | 0.1 / 0.3 / 0.2 |
| Static files | `rust-embed` + `mime_guess` | 8.x / 2.x |
| Streaming export | `futures-core` | 0.3 |
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

# Frontend logic (Node.js, no npm)
node --test 'tests/js/*.test.mjs'

# Functional + load test (Linux/macOS, requires nc)
tests/test.sh

# Functional + load test (Windows, no external tools required)
.\tests\test.ps1
```

`cargo test` includes MLLP over real TCP (`tests/mllp_tcp.rs`), the real binary's startup and shutdown (`tests/config_startup.rs`, `tests/accept_errors.rs`), the REST API (`tests/web_api.rs`), the memory accounting (`tests/alloc_accounting.rs`) and a 20,000-input mutation fuzz of the ingest path (`tests/fuzz_ingest.rs`).

Both test scripts send the same set of HL7 messages: three valid types (ADT^A01, ORU^R01, SIU^S12), three error cases, followed by a load test. The PowerShell test uses a persistent TCP connection for load testing (1000 messages). The shell script spawns a new `nc` process per message (100 messages).

---

## Additional References

- [README.md](../README.md) — Quick start and feature overview
- [AGENTS.md](../AGENTS.md) — Detailed developer guide: architecture rules, constraints, source layout
- [SECURITY.md](../SECURITY.md) — Deployment security
- [ROADMAP.md](ROADMAP.md) — Milestones and planned features
- [STYLE_GUIDE.md](STYLE_GUIDE.md) — Appearance and functioning rules
- [CHANGELOG.md](../CHANGELOG.md) — Full history of every change
