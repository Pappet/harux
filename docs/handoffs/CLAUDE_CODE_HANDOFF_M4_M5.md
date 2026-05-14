# Harux M4 + M5 — Implementation Handoff for Claude Code

**Reference mockups:** `harux-m4-m5-suggestions.html` (this project, sibling file).
**Scope:** Milestone 4 (Workflow & Testing) + Milestone 5 (Persistence).
**Constraint:** Re-use the existing visual language from the M1–M3 redesign (`harux-redesigned.html`). Same CSS variables, same component primitives — health pills, segment cards, monospace meta rows, action buttons. Don't invent new chrome.

---

## Architecture changes that unlock everything

These two infrastructure pieces are prerequisites. Land them first; everything else clicks into place.

### A. `[targets]` table in `harux.toml`

```toml
[targets.orchestra-staging]
host = "10.4.1.22"
port = 6661
tls = true
default = true
description = "Orchestra · staging"

[targets.orchestra-prod]
host = "orchestra.local"
port = 6661
tls = true
description = "Orchestra · prod"
```

- New struct: `TargetConfig { host, port, tls, default, description, ca_cert? }`
- Loader: `Vec<TargetConfig>` injected into AppState.
- API: `GET /api/targets` → list with live health (last successful TCP connect timestamp + RTT).
- Health-check task: every 30s, `tokio::spawn` per target, `TcpStream::connect_timeout(2s)`, store `(connected_at, rtt_ms, last_error)` in `Arc<RwLock<TargetHealth>>`.

### B. MLLP client module (`src/mllp_client.rs`)

Mirror of the existing `src/mllp.rs` but outbound. Exposes:

```rust
pub async fn send_to_target(
    target: &TargetConfig,
    raw: &[u8],
    wait_for_ack: bool,
    timeout: Duration,
) -> Result<MllpSendResult, MllpClientError>;

pub struct MllpSendResult {
    pub sent_at: DateTime<Utc>,
    pub ack_raw: Option<String>,
    pub ack_code: Option<String>,    // AA / AE / AR
    pub latency_ms: u32,
    pub error: Option<String>,
}
```

Used by replay, generator, and editor.

---

## M4 · Task 1 — Message replay (drawer)

**Mockup:** Idea 1 in `harux-m4-m5-suggestions.html`.

### UI

New right-side drawer. Trigger: "Replay…" button in detail header (next to Bookmark / Pin diff).

**DOM/CSS** — re-use mockup `.replay-drawer`, `.replay-source`, `.field-group`, `.preset-pills`, `.toggle-row`, `.switch`, `.result-banner`.

**Drawer fields:**

- **Source pill** (locked): `MSG_TYPE · patient · control_id`
- **Target connection** (`<select>`): populated from `/api/targets`, default = `target.default == true`
- **Modify before sending** (preset pills, multi-select):
  - `As-is` (mutually exclusive with the rest)
  - `Refresh MSH-7 (timestamp)` → server replaces with `now()`
  - `New MSH-10 (control ID)` → server generates `MSG_<8 hex>`
  - `Rewrite MRN…` → opens small inline form: old MRN → new MRN
- **Toggles:** `wait_for_ack` (default on), `tag_replay_in_inbox` (default on)
- **Footer:** Cancel / Send & open response / Replay (primary)

### API

```
POST /api/messages/:id/replay
{
  "target_id": "orchestra-staging",
  "modifiers": ["refresh_msh7", "new_msh10"],
  "wait_for_ack": true,
  "tag_replay_in_inbox": true
}
→ 200 {
  "sent_at": "...",
  "latency_ms": 42,
  "ack_code": "AA",
  "ack_raw": "MSH|...",
  "tagged_message_id": "..."  // if also re-ingested
}
```

### Wiring

- Server reads stored raw bytes for `id`, applies modifiers (`replace_field("MSH", 7, ts)`).
- Calls `mllp_client::send_to_target`.
- If `tag_replay_in_inbox`, the response message (or echoed source) lands in the inbox with auto-tags `replay`, `from:<source_id>`. Detail header shows a `↻ replay of MSG042185` link.
- Keyboard shortcut: `R` on a focused row → replay-to-default-target without opening the drawer (toast: "Replayed to staging · AA · 42ms").

---

## M4 · Task 2 — Template generator

**Mockup:** Idea 2.

### Templates

Ship as `.hl7` files in `examples/templates/`:

```
examples/templates/
  adt/
    A01-admit.hl7
    A03-discharge.hl7
    A08-update.hl7
  orders/
    ORM-O01.hl7
    OMG-O19.hl7
  results/
    ORU-R01.hl7
```

Each file may include front-matter for dynamic field tags:

```
# harux:dynamic MSH-7=now MSH-10=uuid PID-3.1=faker.mrn
MSH|^~\&|HARUX_TEST|HARUX.LAB|...
```

### UI

Two-column layout — sidebar of templates, main = form view of segments.

**Sidebar:** group by category (ADT / Orders / Results / Custom). Active template gets accent left-border. "+ New from received…" lifts any inbox message into a saved template.

**Main:**

- Tab strip: `Form` / `Raw HL7` / `Preview`
- **Form tab:** segment cards (`.gen-segment-card`), each field as `key | inline-input` row. Dynamic fields show `~ now`, `~ uuid` purple tag and disable manual input by default.
- **Toolbar right:** `Sample faker data` (re-rolls all faker fields), `Save as preset`.
- **Footer:** target chip, `Send 1`, `Send <N>` with editable count, `Send & watch` (primary — sends + jumps to inbox filtered to received responses).

### API

```
GET  /api/templates               → list templates
GET  /api/templates/:id           → full template (raw + dynamic spec)
POST /api/templates               → save new (from form / from existing message)
POST /api/templates/:id/send      → render dynamics, send N copies
{
  "target_id": "...",
  "field_overrides": { "PID-5": "Romero^Marcus^J" },
  "count": 50,
  "interval_ms": 100      // for burst pacing
}
→ stream of SSE events { sent, ack_code, latency_ms } per message
```

### Burst send

Use SSE so the UI shows live progress. Show transient progress panel:

```
▓▓▓▓▓▓▓▓░░░░░░░░  24 / 50 sent · 22 AA · 2 AE · avg 38ms
```

---

## M4 · Task 3 — Raw HL7 editor

**Mockup:** Idea 3.

### UI

Full-screen overlay (escape to close). Triggered from "Edit raw…" on any inbox message, or from the generator's "Raw HL7" tab.

Three-column body:

1. **Gutter** (line numbers, warning markers)
2. **Code area** — `<textarea>` styled to look like the mock, OR CodeMirror 6 with custom HL7 mode (recommend the latter — gives you syntax highlighting + diagnostics gutter for free)
3. **Inspector** — updates on cursor position; reads from existing field dictionary

### Validation

Re-use existing `validation.rs` rules. Run on every keystroke (debounce 200ms). Diagnostics → wavy underlines via CodeMirror's `linter` extension.

### Keyboard

- `⌘↵` / `Ctrl+↵` → Send to current target
- `⌘S` → Save as template
- `⌘⇧F` → Format (rewrite delimiters consistently)
- `Esc` → Close (with unsaved-changes confirm)

### API

`POST /api/raw/send` — body is the raw HL7 + target ID, returns `MllpSendResult`. Same shape as replay endpoint.

---

## M4 · Task 4 — Smart notifications

**Mockup:** Idea 4.

Skip the "ping on every message" version — it's noise and users will turn it off. Build rule-based notifications instead.

### Rules data model

```rust
struct NotificationRule {
    id: Uuid,
    name: String,
    trigger: NotifTrigger,
    enabled: bool,
}

enum NotifTrigger {
    FilterMatch { query: String },           // matches search query syntax
    ParseError,                              // any parse error
    AckCode { codes: Vec<String> },          // AE / AR
    Silence { duration_secs: u32 },          // no messages in N seconds
    SourceDown { target_id: String },        // health check fails
}
```

### UI

- "+ Notify" button next to filter chips when a search has results.
- Rules manager in settings: list of rules with enable toggle.
- Browser `Notification.requestPermission()` on first rule creation.
- In-app toast as fallback if permission denied.
- Click → deep-link to filtered inbox view.

### Implementation

Single `tokio::spawn` rule evaluator. On every new message + every minute (for silence rules), iterate enabled rules, fire matches via WebSocket → frontend creates `new Notification(...)`.

---

## M5 · Task 1 — SQLite backend

**Mockup:** Idea 5.

### Config

```toml
[storage]
backend = "sqlite"          # "memory" (default) | "sqlite" | "postgres"
path = "~/.harux/messages.db"
encryption_key_env = "HARUX_DB_KEY"   # SQLCipher; omit = unencrypted
```

### Schema

```sql
CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    received_at INTEGER NOT NULL,         -- unix ms
    source_addr TEXT NOT NULL,
    message_type TEXT NOT NULL,
    sending_facility TEXT,
    patient_name TEXT,
    patient_id TEXT,
    message_control_id TEXT,
    version TEXT,
    segment_count INTEGER,
    ack_code TEXT,
    ack_response TEXT,
    parse_error TEXT,
    raw BLOB NOT NULL,
    parsed_json TEXT,                      -- cached parse result
    bookmarked INTEGER DEFAULT 0,
    validation_warning_count INTEGER DEFAULT 0,
    has_segment_errors INTEGER DEFAULT 0
);

CREATE INDEX idx_messages_received_at ON messages(received_at DESC);
CREATE INDEX idx_messages_source ON messages(source_addr);
CREATE INDEX idx_messages_bookmarked ON messages(bookmarked) WHERE bookmarked = 1;

CREATE TABLE message_tags (
    message_id TEXT REFERENCES messages(id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    PRIMARY KEY (message_id, tag)
);

CREATE TABLE schema_version (version INTEGER NOT NULL);
INSERT INTO schema_version VALUES (1);
```

### Store trait

Refactor `src/store.rs` to a trait and provide `MemoryStore` + `SqliteStore` implementations. The rest of the code talks to the trait. Keep the existing in-memory ring buffer as a write-through cache in front of SQLite for hot-path reads.

### SQLCipher

Use the `rusqlite` crate with the `sqlcipher` feature. On open, if `encryption_key_env` is set, `PRAGMA key = '<value>'` before any other statement.

---

## M5 · Task 2 — Retention policy

**Mockup:** Idea 5 (retention bar visualization).

### Config

```toml
[storage.retention]
max_age_days = 14
max_count = 50000
keep_bookmarked = true       # bookmarked messages survive retention
prune_interval_minutes = 60
```

### Pruner

`tokio::spawn` task running every `prune_interval_minutes`:

```sql
DELETE FROM messages
WHERE received_at < ?max_age_cutoff
  AND (bookmarked = 0 OR ?keep_bookmarked = 0);

DELETE FROM messages
WHERE id IN (
  SELECT id FROM messages
  WHERE bookmarked = 0 OR ?keep_bookmarked = 0
  ORDER BY received_at DESC
  LIMIT -1 OFFSET ?max_count
);

VACUUM;     -- only if DELETEs > 1000 rows
```

Emit `{type: "pruned", count: N}` over WebSocket so the UI can refresh stats.

### Settings UI

Single page (`/settings/storage` route or modal). Components from mockup:

- `.stat-strip` — 4 cards: stored messages, db size, oldest message, time-until-next-prune. All from `GET /api/storage/stats`.
- `.setting-row` × 6 — backend, age limit, count limit, bookmark behavior, encryption, manual actions.
- `.retention-viz` — horizontal bar showing oldest-message position relative to retention cutoff. Updates live as user drags the age input.

```
[oldest 5d 12h ━━━━━━━━━━━━░░░░░░░░░ prune at 14d]
```

### Manual actions

- `POST /api/storage/prune` → runs prune now
- `POST /api/storage/vacuum` → runs `VACUUM`
- `POST /api/storage/wipe` → drops all messages (confirm dialog)

---

## M5 · Task 3 — Extended exports

**Mockup:** Idea 6.

### Format support

Six formats, each as a card in the modal:

| Ext | Format | Implementation |
|---|---|---|
| `.json` | Structured JSON array | already exists — keep as default |
| `.hl7` | FHS/BHS-framed HL7 batch | concatenate raws with `FHS\|...\rBHS\|...\r<msg>\rBTS\|...\rFTS\|...` |
| `.csv` | Summary | write columns: id, received_at, type, source, facility, patient_name, patient_id, control_id, ack, segment_count, has_warnings |
| `.ndjson` | Newline-delimited JSON | one message per line, full structured form |
| `.zip` | Per-message bundle | `<id>.hl7` files inside a zip, plus `manifest.json` |
| `.harux` | Session bundle | tar/zip of `messages.ndjson` + `tags.json` + `bookmarks.json` + `meta.json` (re-importable) |

### Anonymizer

When "Anonymize PHI" toggle is on, run raws through:

```rust
fn anonymize(raw: &str) -> String {
    // Replace consistently — same input MRN always maps to same fake MRN within a single export
    // Use deterministic hash → faker output
    // Targets: PID-3 (MRN), PID-5 (name), PID-11 (address), PID-13/14 (phone), PID-19 (SSN)
}
```

Critical for sharing test fixtures externally.

### API

```
POST /api/export
{
  "format": "hl7" | "json" | "csv" | "ndjson" | "zip" | "harux",
  "filter": "has:errors source:epic.hospital.local",   // current search query
  "include_validation": false,
  "anonymize_phi": true,
  "ids": ["..."]    // optional, takes precedence over filter
}
→ streamed binary response, Content-Disposition: attachment; filename="..."
```

### Import (for `.harux` bundles)

`POST /api/import` accepts `.harux` files and re-creates messages with original IDs (idempotent — skips duplicates by `(source_addr, message_control_id)`). Round-trip parity is the contract.

---

## M5 · Task 4 — Post-restart context banner

**Mockup:** Idea 7.

When the inbox loads, if `messages.length > 0` AND no new messages have arrived since page load, show a top banner:

```
🌀 14,283 messages restored from disk · last activity 3m ago
   Listener resumed on :2575 · oldest message 5d 12h old        [Dismiss]
```

Auto-dismiss on first new message arrival, or after 10s, or on click.

DOM: re-use the empty-state styling but compact horizontal form. Insert above the message list, below the throughput band.

---

## What NOT to break

- Existing WebSocket message protocol (`init`, `new_message`, `tags_updated`, `bookmark_toggled`, `lagged`, `cleared`)
- Existing API surface — extend with new routes, don't repurpose old ones
- The M1–M3 visual language — replay drawer, generator, editor all use the redesigned components (health pills, segment cards, ACK chips, source dots)
- Diff-vs-pinned logic
- Source color palette / `getSourceColor` algorithm
- `harux.toml` backwards compat — new sections (`[targets.*]`, `[storage]`, `[storage.retention]`) are all optional

---

## Suggested ship order (high leverage first)

1. **`[targets]` config + MLLP client module** — unlocks 3 features
2. **Replay drawer** — most asked-for, simplest UI
3. **SQLite backend** (no retention yet) — survival across restarts is the killer feature
4. **Retention policy + settings page** — guards against unbounded growth
5. **Export modal with all formats** — small, self-contained, high value for support workflows
6. **Template generator** — biggest UI lift, but enormous testing payoff
7. **Raw editor** — power user feature, last
8. **Smart notifications** — quality-of-life, can ship anytime

Replay + persistence (1+2+3) are the two transforms. They turn Harux from a viewer into a workbench.

---

## Open questions for the user

- Confirm Orchestra-specific defaults: TLS on by default for replay targets? -> yes
- PHI anonymization: should we ship a default field list, or require explicit configuration? -> configurable by user in UI.
- Postgres backend: in scope for M5, or defer to a future milestone? -> No. Only sqlite for now.
- Burst-send concurrency: serialize (one at a time) or parallel up to N? Affects ACK ordering in the UI. -> only 1
