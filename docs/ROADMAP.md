# Harux — Roadmap

This document tracks all milestones and planned features. Each milestone lists its tasks, completion status, and acceptance criteria. It is the one place for planning facts (current milestone, next work); `AGENTS.md` and the other guides link here instead of repeating them.

**Current state:** Milestones 0–3 are complete. v0.9.0 shipped the hardening from the v0.8.0 review (#222–#250). **Next planned:** Milestone 4 (Workflow & Testing), which will be released as v1.0.0 — see its [implementation plan](MILESTONE_4_PLAN.md).

---

## Milestone 0 — MVP

**Goal:** Core MLLP server with real-time web UI for receiving and inspecting HL7 v2.x messages.

**Status: Complete**

### Tasks

- [x] MLLP server with Tokio (async TCP, correct `0x0B`/`0x1C 0x0D` framing)
- [x] ACK/NAK responses (AA, AE)
- [x] HL7 v2.x parser (dynamic delimiter detection, MSH/PID extraction)
- [x] In-memory message store with broadcast channel
- [x] Web UI with real-time message list (WebSocket)
- [x] Segment/field detail view (Parsed, Raw, JSON tabs)
- [x] Search filter (message type, patient, facility, ID)
- [x] JSON export (replaced in #160 by the MLLP-framed `.hl7` export, byte-faithful since #230)
- [x] Route fix for message detail view (`:id` instead of `{id}`)

### Acceptance Criteria

- [x] Messages are received via MLLP and displayed in real-time in the browser
- [x] ACK/NACK responses are sent correctly
- [x] Messages can be searched and exported as JSON

---

## Milestone 1 — Team-Ready Server

**Goal:** Harux runs stably as a Windows service on the dev server, is configurable without recompilation, and holds up under high load.

**Status: Complete**

### Requirements

- Milestone 0 (MVP) fully complete

### Tasks

- [x] **Configuration file** (`harux.toml`) — ports, memory limits, log level, retention configurable
- [x] **File Logging** — daily log files with retention (`logging.max_files`, #240) and a stdout fallback when the file can't be written (#234)
- [x] **Portable binary** — single `.exe` without dependencies, xcopy deployment
- [x] **Backpressure handling** — evict oldest messages when the store is full instead of OOM
- [x] **Memory Management** — configurable memory budget (e.g. max 512 MB RAM) with automatic eviction
- [x] **Connection limits** — cap maximum concurrent MLLP connections
- [x] **Graceful shutdown** — cleanly terminate active connections on service stop (Ctrl+C, and SIGTERM on Unix since #225)

### Acceptance Criteria

- [x] Server starts via `harux.toml` with configured ports and limits
- [x] When the memory budget is reached, old messages are evicted — no OOM
- [x] `Ctrl+C` or service stop terminates active MLLP connections cleanly

> **Note:** Windows Service installation is handled externally via NSSM (`nssm install Harux harux.exe`). No native `windows-service` crate integration needed.

---

## Milestone 2 — Multi-User Experience

**Goal:** Multiple developers work productively against the same server simultaneously without interfering with each other.

**Status: Complete**

### Requirements

- Milestone 1 complete (stable server with configuration)

### Tasks

- [x] **Session-based views** — each developer sees their own filter configuration, scroll position and selection
- [x] **Color-coded source markers** — messages visually distinguishable by sender system/IP
- [x] **Message tagging** — manual tagging (e.g. "Bug #1234", "Test scenario A") for attribution
- [x] **Bookmark/Pin** — mark important messages so they don't get lost in the stream; eviction-protected within `store.bookmark_budget_percent` of the store

### Acceptance Criteria

- [x] Two browser tabs show independent filters and selections
- [x] Messages from different source IPs are visually distinguishable
- [x] Tags persist across page reloads (session scope)
- [x] Bookmarked messages are protected from eviction and persist across page reloads

---

## Milestone 3 — Message Analysis

**Goal:** Developers understand HL7 messages directly in the UI — field names, validation, diff.

**Status: Complete**

### Requirements

- Milestone 1 complete (stable server)

### Tasks

- [x] **HL7 field dictionary** — hover tooltips with field descriptions (e.g. "PID-5: Patient Name") based on embedded HL7 v2.5.1 spec (#48)
- [x] **Segment description tooltips** — hover segment headers and typical-segment badges for their HL7 description (#45)
- [x] **Message type detection** — ADT, ORM, ORU, SIU, MDM etc. with human-readable description and a "Typical segments" bar showing present/absent segments (#45, #50)
- [x] **Validation** — rule-based engine checks required MSH fields and message-type-specific segments; warnings shown as amber badges in the list and a warning panel in the detail view (#46, #51)
- [x] **Data type validation** — validation engine extended to check NM, DT, TS, SI field values against the v2.5.1 dictionary; type violations shown as distinct blue badges; first-component-only check avoids false positives on composite types (#62)
- [x] **Validation status filter** — header button and `has:` search prefix to filter the message list by validation status (all / warnings / errors only) (#60)
- [x] **Segment diff** — pin any message as a reference with the `◎` button; open the Diff tab on any other message for a field-level side-by-side comparison with red/green highlighting (#47, #52)
- [x] **Diff: ignore dynamic fields** — toggle in the Diff tab to hide MSH-7/MSH-10 so always-changing metadata does not obscure logical differences; hidden count shown in summary (#61)
- [ ] **Dictionary Version Support** — verify HL7 versions to officially support and define a distribution/toggle strategy (deferred to post-M3)

### Acceptance Criteria

- [x] Hovering over an HL7 field shows its name and description from the spec
- [x] Validation warnings appear for missing required fields
- [x] Diff view shows field differences between two messages highlighted in colour

---

## Milestone 4 — Workflow & Testing

**Goal:** Rapid testing against Orchestra — send, edit, replay messages directly from the UI.

**Status: Planned** — the [implementation plan](MILESTONE_4_PLAN.md) has the design, the API, the safety rules and the work packages M4-1 … M4-12. The owner's decisions of 2026-11-12 are in its section 11; one question is still open there.

### Requirements

- Milestone 1 complete (stable server)

### Tasks

- [ ] **Message replay** — resend stored messages to a configurable target address/port (MLLP client, over TLS where the target needs it)
  - [x] M4-1 Send a stored message, unchanged (#272): MLLP client, switch and limits in `harux.toml`, sent messages and their responses in the store and shown as sent
  - [x] M4-2 TLS for outgoing connections (#273)
  - [ ] M4-3 Resend in the UI: a target list for every user, one-click resend, direction filter
  - [ ] M4-4 Changes on resend: new control ID, current time, field overrides
- [ ] **Message editor** — edit raw HL7 in the UI and send
  - [ ] M4-5 Drafts on the server: preview, field write-back, building the bytes
  - [ ] M4-6 Drafts in the UI: write or edit a message in the detail panel and send it, validated live
  - [ ] M4-7 Edit a draft field by field
- [ ] **Test message generator** — templates for common types (ADT^A01, ORM^O01, ORU^R01) with editable fields
  - [ ] M4-8 Built-in templates and a template directory
- [ ] **Several messages** — resend the filtered messages in order, repeat a draft
  - [ ] M4-9 Send runs with progress and cancel
  - [ ] M4-10 Import an `.hl7` file into the store
- [ ] **Auto-refresh trigger** — desktop notification on new messages (optional)
  - [ ] M4-11 Watch: notify about new messages that match the current filter
- [ ] M4-12 Release v1.0.0

### Acceptance Criteria

- [ ] A received message can be replayed to a configured target address with a single click
- [ ] A template can be filled out in the UI, sent, and the response (ACK/NAK) displayed
- [ ] Raw HL7 can be edited and sent directly
- [x] A message can be sent to a target that requires TLS
- [x] Sending is off until `harux.toml` switches it on, and can be limited to listed destinations; everything Harux sends is visible in the message list with the address that asked for it

---

## Milestone 5 — Persistence

**Goal:** Messages survive server restarts. Automatic cleanup after configurable retention.

**Status: Planned**

### Requirements

- Milestone 1 complete (configuration file in place)

### Tasks

- [ ] **SQLite backend** — optional persistence, enabled via `harux.toml`
- [ ] **Retention policy** — automatic deletion after X days or X messages
- [ ] **Extended export** — CSV export, filtered exports (the full `.hl7` export of all stored messages exists since #160)

### Acceptance Criteria

- [ ] After a server restart, persisted messages are visible again
- [ ] Retention deletes messages automatically after configured age/count
- [ ] Export as CSV and `.hl7` works for filtered results

---

## Milestone 6 — FHIR & Monitoring

**Goal:** Future-proofing through FHIR preview and observability through a monitoring dashboard.

**Status: Planned**

### Requirements

- Milestone 3 complete (message analysis as basis for FHIR mapping)

### Tasks

**FHIR R4 Preview**
- [ ] **HL7 v2 → FHIR R4 mapping** — display ADT messages as FHIR bundles (Patient, Encounter)
- [ ] **FHIR JSON view** — additional tab in the detail view
- [ ] **FHIR HTTP endpoint** — REST endpoint for FHIR bundles/resources

**Monitoring & Statistics**
- [ ] **Dashboard view** — messages per minute/hour, type distribution, error rate as charts
- [ ] **Latency tracking** — time difference between MSH-7 and receive time
- [ ] **Alerting** — configurable warnings at error rate > X% or outage > Y minutes
- [ ] **Health endpoint** — `/api/health` for monitoring tools (Zabbix, PRTG)

**Extended Features**
- [ ] **Multi-port listener** — multiple MLLP ports simultaneously (test separate Orchestra channels)
- [ ] **TLS support for the listener** — encrypted incoming MLLP connections (MLLP/S); outgoing connections get TLS in Milestone 4. Until then, `server.web_bind = "127.0.0.1"` behind a TLS reverse proxy covers the web side (#237)
- [ ] **ACK configuration** — customizable ACK responses (e.g. always send NAK to test retry logic)
- [ ] **Regex filter** — extended search with regular expressions
- [ ] **Dark/Light theme toggle**

### Acceptance Criteria

- [ ] ADT messages can be displayed as a FHIR bundle (JSON)
- [ ] Dashboard shows message volume and error rate as a chart
- [ ] `/api/health` returns server status for external monitoring tools
- [ ] Multi-port listener receives on configured ports in parallel

---

## Nice-to-Have (No Priority)

- [ ] **Plugin system** — load custom parsers/transformers as WASM modules
- [ ] **REST API for CI/CD** — send messages programmatically and verify results (automated integration tests)
- [ ] **Audit log** — who viewed/sent which message and when
- [ ] **Import from HL7 Inspector** — migrate existing message collections
- [ ] **Orchestra log correlation** — link message IDs to Orchestra channel logs
