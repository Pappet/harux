# Milestone 4 — Workflow & Testing: implementation plan

> **Status:** proposal, 2026-11-12. Nothing in it is implemented yet. Section 11
> lists the decisions that are the owner's to make; the plan gives a
> recommendation for each and is written as if the recommendation were taken.
>
> **Planning document.** It names files, configuration keys and routes that do
> not exist yet, so it is not among the documents `tests/doc_paths.rs` checks.
> When the milestone is done it becomes a historical document like the ones in
> `docs/handoffs/`; the rules that last move into `AGENTS.md`.
>
> **Mockup:** `docs/design/harux-m4-workbench.html` (open it from a checkout) —
> three states of the window, drawn with the application's real stylesheet.
>
> This plan replaces the Milestone 4 part of
> `docs/handoffs/CLAUDE_CODE_HANDOFF_M4_M5.md`; section 13 says what changed
> and why.

---

## 1. What the milestone is for

Today Harux is the receiving end of a test. A developer pushes a message into
Orchestra with another tool, Orchestra processes it, and Harux shows what comes
out. Milestone 4 adds the sending end, so the whole loop runs in one window:

```
pick or write a message ─► send it to Orchestra ─► Orchestra forwards ─► Harux receives
        ▲                                                                     │
        └────────────── change it, send again ◄── compare (diff) ◄────────────┘
```

| Roadmap task | Where it lands in this plan |
|---|---|
| Message replay | **Resend** (4.2), stage 1. Several messages at once: stage 4 |
| Message editor | **Drafts**, Raw tab (4.3), stage 2 |
| Test message generator | **Templates** and the editable Segments tab (4.3, 4.4), stage 3 |
| Auto-refresh trigger / desktop notification | **Watch** (4.6), stage 5 |

The three acceptance criteria of the roadmap are met after stages 1, 2 and 3.
Stage 1 alone already gives the team one-click replay.

---

## 2. The design: one lifecycle, one window

The earlier mockups gave each task its own surface: replay in a drawer, the
editor as a full-screen overlay, the generator as a third page. Each of them
needs a target picker, a send button and a result display, so the same things
would exist three times, and the features would sit next to the inspector
instead of in it.

This plan has one model:

```
template ─┐
received ─┼─► DRAFT ──send──► SENT ──(Orchestra)──► RECEIVED
sent ─────┘     ▲               │                       │
                └──── Edit ─────┴───────── Edit ────────┘

Resend = send a stored message again without opening a draft.
```

- A **draft** is a message that has not been sent yet. It is shown where
  messages are shown, in the detail panel, with the same Segments and Raw
  tabs. The only difference is that they can be edited.
- A **sent message** is a message. It is a row in the same list as the received
  ones and has the same detail view, tags, bookmark, diff, search and eviction.
  The response it got is to it what the ACK Harux wrote is to a received
  message, and it is shown in the same two places: the chip in the row and the
  ACK tab.
- **Replay, edit and templates** are three ways to reach a draft, or to skip
  it. None of them has a screen of its own.

A mail program works the same way (inbox, sent items, drafts, compose in the
reading pane), so nobody has to learn it.

### Rules that follow from the model

They become rules in `AGENTS.md` when the code exists.

1. **Everything Harux sends, or tries to send, is stored — before it connects
   and before the first byte is written.** This is the sending counterpart of
   "never lose a frame silently": a send that was refused or timed out is a
   row too. Storing first also keeps the list in causal order: what Orchestra
   forwards because of a send can never appear above the send that caused it,
   even when Orchestra answers only after it has processed the message.
2. **Unchanged means byte for byte.** Resending a stored message without
   changes sends `Hl7Message::wire_bytes()`, the bytes as they arrived
   (#230). Every change is one the user asked for and the request names.
3. **Sending fails closed.** Harux connects only to targets and destinations
   the configuration allows. Without configuration it reaches nothing but its
   own listener.
4. **One of each.** One MLLP framing (`src/mllp.rs`), one parser, one
   validator, one charset table (`src/hl7/charset.rs`). The browser never
   parses HL7 on its own; drafts are parsed by the server.
5. **The payload stays immutable** (#192). What was sent is known at insert
   time and never changes. What comes back later (the response, a timeout)
   lives in the entry's mutable metadata, next to tags and bookmark.
6. **No new state on the server's disk.** Targets and shared templates are
   read-only configuration; drafts live in the browser. Saving things on the
   server is Milestone 5.
7. **The existing constraints hold:** no framework, no build step, no new
   crate, `Arc<RwLock<>>` store, 250 ms flush, prepend on the hot path, rows of
   61 px, CSP without inline code, never ACK an ACK, the MSH shift only
   through `msh_split_index`.

---

## 3. Vocabulary

Used the same way in the UI, the API, the code and the documentation.

| Term | Meaning |
|---|---|
| **Target** | A named place to send to, defined in `harux.toml` (`[[send.targets]]`). |
| **Destination** | A `host:port`. A target has one; a user may type one if the configuration allows it. |
| **Send** | Write one message to a destination and read the response. |
| **Resend** | Send a stored message (received or sent) again — the roadmap's "replay". |
| **Draft** | A message being written. Lives in the browser. |
| **Template** | A draft to start from, with placeholders such as `{{id}}`. |
| **Sent message** | A stored message with direction `out`. The others have direction `in`. |
| **Delivery** | What became of a sent message: `pending`, `acked`, `sent` (no response awaited) or `failed`. |
| **Response** | The reply the destination wrote. Its MSA-1 is the delivery's ACK code. |
| **Run** | One request to send: one message, or several in order over one connection. |
| **Watch** | A browser-side notification for new messages that match the current filter. |

---

## 4. User experience

The three frames of the mockup show 4.1 – 4.3 and 4.5.

```
+---------------------------------------------------------------------------------------------------------------------+
| Harux   listening, conns, rate, total, sent, validation, errors        [New message v] [Export] [Clear]             |
+------------------------------------------------------+--------------------------------------------------------------+
| search                                               | [SENT] [ADT^A01] Müller, Max  [Resend -> target v] [Edit]    |
| Pause Auto | Bookmarks All In+Out | Watch            | origin, who asked for it                                     |
| SOURCES 10.4.1.22  10.4.1.30 | -> orchestra-test     | meta row: control, to, sent, response                        |
| DRAFTS  [ADT^A01 Müller *] [ORU^R01 ...]             | tags, bookmark, pin diff                                     |
+------------------------------------------------------+--------------------------------------------------------------+
| in   ADT^A01  Müller   10.4.1.22           AA        | Segments | Raw | Response | JSON | Diff vs pinned            |
| out  ADT^A01  Müller   -> orchestra-test   AA 42 ms  |                                                              |
| in   ORU^R01  Schmidt  10.4.1.30           AA        | One set of views for a received message (ACK tab),           |
| out  ADT^A08  Weber    -> orchestra-test   x timeout | a sent message (Response tab) and a draft (Segments          |
| in   ADT^A01  Müller   10.4.1.22           AA        | and Raw editable; target and Send in the title row).         |
|                                                      |                                                              |
| run: 24 / 50, 22 AA, 2 AE  [Cancel]                  |                                                              |
+------------------------------------------------------+--------------------------------------------------------------+
```

### 4.1 The list

- **Sent rows** look like received rows, with three differences: an `↑` in
  front of the type, a dashed bar in the accent colour instead of the source
  colour, and `→ target` where the source address is. The chip shows the
  response code (`AA`, `AE`, `AR`), `…` while pending, or `✕` with the reason
  (`timeout`, `refused`, `closed`). The round-trip time follows the chip. A
  small label says where the message came from: `resend`, `draft`, `template`.
- **Row height stays 61 px** (`content-visibility`, #216). The `↑`/`↓` column
  is shown only while the list holds sent messages, so a team that never sends
  sees the list exactly as today.
- **Direction filter:** a button in the control row that cycles
  `In + Out → Received → Sent`, like the validation button. The search gets the
  prefixes `is:sent` and `is:received`, in the client matcher and in
  `SearchQuery` alike.
- **Source rail:** targets appear after the sources as dashed chips
  (`→ orchestra-test-adt 12`). Clicking one highlights its messages, as a
  source chip does.
- **Header:** a `sent` pill (hidden while it is 0). `rate` and `last` keep
  counting received messages only. A **New message** button opens a menu: blank
  message, the templates, from the clipboard, from a file.
- **Drafts rail:** one chip per open draft, under the source rail, shown only
  while drafts exist. A dot marks a draft that changed since it was last sent.
- **Export** keeps its meaning: received messages, byte for byte.
  `GET /api/export?direction=out` or `all` gives the others.

### 4.2 Resend

- Every stored message gets two buttons on the title row: **Resend → target**
  (a split button) and **Edit**. Tags, Bookmark and Pin diff move to a row of
  their own below the meta row. This is the layout `docs/STYLE_GUIDE.md`
  already describes, and it is the only change to an existing part of the
  window.
- **One click** on the left part sends to the target used last, in the way
  used last. The label always names the target. The first use, and every
  target marked `confirm`, opens the menu instead.
- **The menu** (the caret) lists the targets with their last result
  (`AA · 42 ms · 2 min ago`, `refused · yesterday`), an input for another
  `host:port` if the configuration allows one, and how to send: *as received*
  (byte for byte), *with a new control ID* (MSH-10), *with a new control ID
  and the current time* (MSH-10, MSH-7). The choice is remembered per browser.
- **Result:** a toast (`Sent to orchestra-test-adt · AA · 42 ms`), and the sent
  message appears in the list at once with `…`, then with its response code.
- **Sent message, detail view:** badge `SENT`, the description names the origin
  (`resent from MSG00042185`, a link) and who asked for it (the client's IP
  address). The ACK tab is labelled **Response** and shows a result line, the
  response as raw lines, and whether MSA-2 repeats the control ID that was
  sent.
- **Trace:** clicking a control ID in a meta row filters the list by it. With
  a new control ID per send this isolates one exchange: the sent message and
  what Orchestra made of it. Useful for received messages too.

### 4.3 Drafts

A draft opens from **New message**, from a template, or from **Edit** on any
stored message.

- **Header:** badge `DRAFT`, type and title from the text itself, a target
  button and **Send** (`Ctrl+Enter`). The description names the origin and says
  whether the draft changed since the last send. The meta row shows the
  destination, the encoding, the number of segments and the size.
- **Tabs:** Segments and Raw (both editable), Response (the last result and
  the earlier sends of this draft, each a link to the sent message),
  Diff vs pinned (the existing diff with the draft as the current message, so
  pinning the original shows exactly what was changed).
- **Raw tab:** a plain `<textarea>`, one segment per line. A status line names
  the field under the cursor (`PID-5 · Patient Name · XPN`). Any text is
  allowed, including text that is not HL7.
- **Segments tab:** the blocks of the inspector with inputs as values. The
  typical-segments checklist adds a missing segment with one click. Each
  segment offers *show empty fields* (from the dictionary), *duplicate* and
  *remove*. MSH-1 and MSH-2 are changed in the Raw tab only.
- **Validation is live** and identical to the inspector's: the same banner,
  badges and checklist, computed by the server from the draft's text
  (5.5).
- **Placeholders** are visible in the text: `{{id}}` in MSH-10 means a new
  control ID for every send. Quick actions on MSH-7 and MSH-10 insert them.
  What the fields show is what is sent, after the placeholders are filled in.
- **Encoding:** follows MSH-18; a draft made from a stored message keeps that
  message's encoding. It can be changed in the meta row. A character the
  encoding cannot write stops the send with an error that names it — never a
  silent `?`.
- **Long values** (the Base64 of an MDM) are shown as a chip with their size
  and are kept out of the textarea and the inputs. They are sent in full. An
  MDM can so be edited without freezing the tab (#215).
- **Broken on purpose:** a draft that does not parse can be sent after a
  second click (*Send anyway*). It is stored with its parse error, like a
  broken received frame. Testing how Orchestra handles a bad message is a
  normal use.
- **Where drafts are kept:** in the browser tab (`sessionStorage`), so they
  survive a reload but not the end of the session. *Keep on this computer*
  moves one to `localStorage`; *Save as file* downloads it as `.hl7`. See 6 for
  why keeping is not the default.

### 4.4 Templates

- **Built in:** a small set embedded in the binary — ADT^A01, A03, A04, A08,
  ORM^O01, ORU^R01, SIU^S12, MDM^T02 — with placeholders in MSH-7 and MSH-10.
  A test requires each of them to parse without a validation warning.
- **From the site:** `.hl7` files in the directory `send.templates_dir`.
  Harux only reads it. The team can keep the directory in version control.
- **Any stored message is a template:** *Edit*. Until Milestone 5, bookmarks
  and tags are the team's shared set of reference messages.
- **Personal:** drafts kept on the computer (4.3).

There is no template editor and no separate generator. The roadmap's "fill out
a template" is: choose it under *New message*, change the fields in the
Segments tab, send.

### 4.5 Sending several messages

- **Resend the filtered messages:** an action on the list that sends the
  loaded messages the current filters show, oldest first, to one target
  (`Resend 37 messages → …`, always with a confirmation that names target and
  number). Filter by source, tag or bookmark first — "resend all bookmarked"
  is a small regression suite.
- **Repeat a draft** *n* times. Placeholders are filled in again for every
  copy; `{{n}}` counts.
- A run uses one connection, sends in order, optionally with a pause between
  messages, and can be cancelled. Its progress is a bar under the list
  (`24 / 50 · 22 AA · 2 AE · avg 38 ms`).
- Every message of a run is a row. Runs are limited to
  `send.max_run_messages` (default 1,000). Load tests with tens of thousands
  of messages are not part of this milestone.
- **Import:** an `.hl7` file — the format `/api/export` writes, or plain
  messages — is loaded into the store as received from `import:<file name>`.
  From there it is resent like anything else. This also carries a set of test
  messages across a restart until Milestone 5.

### 4.6 Watch

A bell in the control row: *tell me when a message arrives that matches the
current filter*. It is the roadmap's notification task, limited to what is
useful: nobody wants a notification for every message.

- While the tab is in the background, matching messages raise a counter in the
  tab title (`(3) Harux`), play a short sound if wanted, and show a desktop
  notification.
- **Desktop notifications need a secure context** (HTTPS or `localhost`).
  Browsers refuse them on `http://server:8080`. Title counter and sound work
  everywhere; the UI says which of the three are active.
- It is per browser and needs no server code.

### 4.7 What a team sees that never sends

With the defaults (no targets, the loopback target on) the window gains the
**New message** button and the **Resend** and **Edit** buttons, and the only
place to send to is Harux itself. With `send.loopback_target = false` and no
targets, sending is off: no button, no rail, and the routes answer `403`.

---

## 5. Architecture

```
browser ── POST /api/send ──► web.rs ──► send.rs (run) ──► mllp_client.rs ══ TCP ══► target
                                            │  1. build the bytes (compose.rs, charset.rs)
                                            │  2. store.insert(sent message, delivery pending) ─► WS new_message
                                            │  3. connect, write the frame, read the response
                                            └─ 4. store.set_delivery(id, outcome) ─────────────► WS delivery_updated
```

### 5.1 Data model (`src/hl7/types.rs`)

- `Hl7Message` gets `outbound: Option<Box<Outbound>>`. `None` is a received
  message. `Outbound { target, origin, requested_by, run }` holds the facts of
  the send that never change: the target's name (or `host:port`), where the
  message came from (`Resend { message_id }`, `Draft { based_on }`,
  `Template { name }`), the client's IP address and the run.
- For a sent message `source_addr` holds the destination (`host:port`) and
  `received_at` the moment the send began. Both names stay as they are, since
  the UI and the API use them everywhere; the meaning is "the peer" and "when
  it entered the list".
- `MessageMeta` gets `delivery: Option<Delivery>`:
  `Delivery { state, ack_code, response, error, latency_ms, control_id_matches }`.
  It is `None` for received messages.
- `Hl7MessageSummary` gets `direction`, `target`, `origin_id`,
  `delivery_state` and `latency_ms`. For a sent message its `ack_code` and
  `ack_response` carry the response, so the row chip needs no second code
  path. The detail JSON has the whole `delivery` object.
- The detail JSON also gains each field's `datatype` from the dictionary, for
  stored messages and previews alike (the draft editor shows it).
- **Accounting (#226):** `Hl7Message::estimated_bytes` counts `Outbound`,
  `MessageMeta::estimated_bytes` counts `Delivery`, and
  `tests/alloc_accounting.rs` gets a sent message with a response. A response
  that arrives later changes the entry's bytes through the existing
  `mutate_meta` delta.

### 5.2 Store (`src/store.rs`)

- A sent message is inserted with `delivery: pending`, through a second entry
  point next to `insert`, which keeps its signature.
- `set_delivery(id, delivery)` goes through `mutate_meta` and broadcasts
  `StoreEvent::DeliveryUpdated(summary)` under the write lock (#232). The
  frame is rendered once (#196) as `delivery_updated` in `ws_event_json`.
- Eviction, bookmarks, `clear` and the search treat sent messages like all
  others. `SearchQuery` learns `is:sent` / `is:received`.
  `payload_snapshot` takes a direction for the export.
- If a pending message is evicted or cleared before its response arrives,
  `set_delivery` finds nothing and the outcome is only logged.

### 5.3 Sending (`src/mllp_client.rs`, `src/send.rs`)

`src/mllp_client.rs` does one exchange on an open connection and knows nothing
about the store: write `VT payload FS CR`, read until `FS CR`, with a timeout
for each step and a size limit for the response (1 MB). It uses `wrap_mllp`
and `extract_mllp_frame` from `src/mllp.rs`, which become `pub(crate)`. The
response is decoded with `decode_payload` and read with `parse_message` under
`catch_unwind`, as incoming frames are. From it come MSA-1 (`AA`/`CA`
accepted, `AE`/`CE` error, `AR`/`CR` rejected), MSA-2 and the text. A response
that is not HL7 is kept as text and marked as such.

`src/send.rs` is the engine. A **run** is a task that:

1. resolves the target and checks it against the policy (5.7);
2. for each item: builds the bytes (5.4), parses them into an `Hl7Message`
   with `outbound` set and inserts it as `pending`;
3. connects if the run has no open connection (`send.connect_timeout_secs`),
   does the exchange, and records the outcome with `set_delivery`;
4. waits `interval_ms` between items, stops on cancel and on shutdown.

- A transport failure (connect, write, timeout, closed) marks the item
  `failed`; the run reconnects for the next item and gives up after three
  failures in a row. Items it never tried are not stored. A NACK never stops
  a run unless the request says so.
- An item whose stored message is gone (evicted since the request was made)
  is skipped and counted as such in the run's totals.
- No row stays at `…` forever: every way an item can end records an outcome —
  a response, a transport failure, a cancel, shutdown, and a panic while
  building or reading, which is caught as in `ingest` (#235) and recorded as
  `failed`.
- At most `send.max_concurrent_runs` runs at a time (a semaphore); further
  requests get `429`. Finished runs are kept in a small in-memory registry for
  status requests.
- No store lock is held across network I/O. A run never blocks the listener.
- Every send is logged at `info` with target, type, control ID, size, outcome
  and the requesting address — never with content. Failures are `warn`.
- `MllpStats` stays about received frames. Sending has its own counters
  (`sent`, `send_failed`), shown in `/api/stats` and reset by *Clear*.

### 5.4 Building the bytes (`src/hl7/compose.rs`, `src/hl7/charset.rs`)

| Source | Change | Bytes that are sent |
|---|---|---|
| Stored message | none | `wire_bytes()` — exactly as stored. This includes a frame that failed to parse: it goes out as it came in, after the same second click a broken draft needs (`allow_invalid`). The one exception is an over-limit entry (#223): it holds only the first 4 KB of its frame, so it gets a marker when it is stored and cannot be resent |
| Stored message | field overrides | `raw` with those fields replaced, encoded with the message's own encoding (`WireForm`). A message whose bytes no encoding reproduces (`WireForm::Original`) can only be resent unchanged; the request is refused with that reason |
| Draft text | — | Line ends normalized to `CR`, final `CR` added, placeholders filled in, encoded with the draft's encoding (MSH-18, else UTF-8). An unmappable character refuses the send |

- **Field overrides** are `"MSH-10": "value"` pairs (segment, optional
  occurrence `OBX[2]-5`, field). They are applied to the text with the
  message's own delimiters and `msh_split_index`; a missing field is created
  by adding separators, a missing segment is an error. The menu's
  "new control ID" is the override `MSH-10 = {{id}}`.
- **Placeholders** are filled in for override values and for draft text, never
  for the text of a stored message. Only these names are recognized; anything
  else in double braces is sent as written.

  | Placeholder | Value |
  |---|---|
  | `{{now}}` | Local time, `YYYYMMDDHHMMSS` |
  | `{{today}}` | Local date, `YYYYMMDD` |
  | `{{id}}` | A new 20-character control ID (the generator `render_ack` uses) |
  | `{{n}}` | Position of the message in its run, from 1 |
  | `{{rand:6}}` | That many random digits |

- `charset.rs` gets the reverse of `decode_payload`: text plus encoding choice
  → bytes plus `WireForm`, using the same `encoding_for` table.

### 5.5 Parse preview

`POST /api/parse` takes a draft's text and returns what
`GET /api/messages/{id}` returns for a stored message — segments,
descriptions, type information, validation warnings or the parse error — plus
each field's datatype. Nothing is stored. Placeholders are filled in with
sample values first, so `{{now}}` is not reported as an invalid timestamp.

The draft editor asks for it 300 ms after the last change (the debounce the
search uses) and renders it with the builders `static/render.js` already
has. That is why the browser needs no parser, and why a draft is validated by
exactly the rules a received message is. The only text operation in the
browser is writing one field back into its line (split at the field separator,
replace, join — with the MSH shift), covered by a JS test. Long values are
replaced by tokens before the text is sent for preview, so previewing an MDM
costs a few kilobytes.

`GET /api/dictionary/{segment}` returns a segment's field list (number, name,
datatype) for *show empty fields*.

### 5.6 Templates (`src/templates.rs`)

Built-in templates are embedded with `include_str!` from
`src/assets/templates/`. Site templates are read from `send.templates_dir` on
every list request (the directory is small; no cache to go stale), limited in
number and size, never written. `GET /api/templates` lists both with name,
group and message type; `GET /api/templates/{id}` returns the text.

### 5.7 Configuration (`src/config.rs`)

```toml
[send]
# Offer Harux's own MLLP listener as a target. With this off and no targets,
# sending is disabled altogether.
loopback_target = true
connect_timeout_secs = 5
response_timeout_secs = 30
max_run_messages = 1000
max_concurrent_runs = 4
# Destinations users may type themselves: host, host:port or host:from-to.
# Empty = only the named targets below can be chosen.
# allowed_destinations = ["orchestra-test.klinik.local:6000-6999"]
# Directory with .hl7 templates for everyone (read only).
# templates_dir = "templates"

# [[send.targets]]
# name = "orchestra-test-adt"
# host = "orchestra-test.klinik.local"
# port = 6661
# description = "Orchestra TEST, ADT inbound"
# confirm = false        # true: ask before every send (e.g. a production system)
```

- Fails closed like the rest (#233): unknown keys, duplicate or empty names, a
  port of 0, a malformed `allowed_destinations` entry and zero limits stop the
  start with an error that names the key. "Effective configuration" lists the
  targets.
- A typed destination is allowed when its host equals an entry (names without
  case and trailing dot, IP addresses in canonical form) and its port lies in
  the entry's range. Nothing is resolved for the check. Named targets are
  allowed because the administrator wrote them.
- The loopback target is the address the MLLP listener is bound to
  (`127.0.0.1` when it listens on every interface) and `server.mllp_port`.
- The largest message that can be sent is `mllp.max_message_size_mb`, the
  largest that can be received.
- New keys go into `harux.toml` and `examples/harux-example.toml`
  (`shipped_config_files_load`).

### 5.8 HTTP API and WebSocket (`src/web.rs`)

All new routes that change something are `POST`, so `guard_cross_site`
covers them (#227).

| Route | Purpose |
|---|---|
| `GET /api/send/targets` | Whether sending is on, the targets with their last result, the allowed destination patterns, the limits |
| `POST /api/send` | Start a run (below) |
| `GET /api/send/runs/{id}` | Progress and result of a run |
| `POST /api/send/runs/{id}/cancel` | Stop a run |
| `POST /api/parse` | Parse preview of a draft (5.5) |
| `GET /api/dictionary/{segment}` | Field list of a segment |
| `GET /api/templates`, `GET /api/templates/{id}` | Templates (5.6) |
| `POST /api/import` | Load an `.hl7` file into the store (4.5) |
| `GET /api/export?direction=in\|out\|all` | Default `in`: unchanged behaviour |
| `GET /api/stats` | Gains `sent`, `send_failed`, `active_runs`, `sending_enabled` |

```jsonc
// POST /api/send
{
  "target": "orchestra-test-adt",            // or {"host": "…", "port": 6662}
  "items": [{ "message_id": "…" }],          // or [{ "raw": "MSH|…", "charset": "auto",
                                             //        "based_on": "<message id>", "template": "<name>" }]
  "set": { "MSH-10": "{{id}}" },             // optional field overrides, for every item
  "repeat": 1,                               // each item this many times
  "interval_ms": 0,
  "stop_on_nack": false,
  "allow_invalid": false,                    // send text that does not parse
  "confirmed": false                         // required for targets with confirm = true
}
```

- **One message:** the request waits for the outcome and answers `200` with
  the stored ID and the delivery. A failed delivery (refused, timeout, NACK)
  is a `200` too: the request worked, the send did not.
- **Several messages:** `202` with the run's ID; the rows report progress over
  the WebSocket, the totals come from `GET /api/send/runs/{id}`.
- **Refusals:** `400` malformed request or field path · `403` sending off or
  destination not allowed · `404` unknown target or message · `409` target
  needs `confirmed` · `413` message or run too large · `422` text does not
  parse (without `allow_invalid`), unmappable character, changed resend of an
  irreproducible message · `429` too many runs.
- The body limit of `/api/send`, `/api/parse` and `/api/import` is raised
  from axum's default 2 MB to fit `mllp.max_message_size_mb`.
- The requesting address comes from the connection (`ConnectInfo`). Behind a
  reverse proxy it is the proxy's.

**WebSocket:** one new event, `delivery_updated`, with a summary like
`tags_updated`. `new_message` carries the new summary fields. Clients that
ignore unknown fields and events keep working.

### 5.9 Frontend (`static/`)

Two new files, loaded after `diff.js` and before `app.js`:

- `send.js` — targets, the target menu, resend, runs and their bar, toasts.
- `compose.js` — drafts: the rail, the draft header, editable Segments and Raw
  tabs, parse preview, placeholders, long-value tokens, storage.

Changes to existing files:

- `state.js` — targets, drafts and the selected draft, direction filter, watch.
- `ws.js` — `delivery_updated` in `handleServerEvent` (queued during a list
  load like the other events, #239); sent messages are left out of `rateWindow`.
- `render.js` — the row skeleton gets the direction element and the `outbound`
  class; `patchRow` learns `delivery`; the detail header gets the tools row and
  the send buttons; the ACK tab renders a response.
- `util.js` — `getParsedQuery` learns `is:sent` / `is:received`; new icons.
- `app.js` — new `data-action` cases; `Ctrl+Enter`.
- `index.html`, `style.css` — the new elements. Toggles follow the style
  guide (no native checkboxes); nothing inline.

The selection is either a stored message or a draft. `renderDetail` keeps
serving messages; `compose.js` renders drafts into the same container. Every
new control is reachable with `Tab` and works with Enter and Space through the
existing delegation, like the rows and chips today.

---

## 6. Safety and security

Sending is the first feature with which a user of the web UI can change
something outside Harux. Harux has no login, so the limits must be in the
configuration.

| Risk | Answer |
|---|---|
| A test message reaches a **production system** | Only configured targets and allowed destinations can be reached; nothing is configured by default. Targets can require confirmation (`confirm = true`) — a guard against a slip, not against intent. The button always names the target. |
| Harux as a **stepping stone** into the network (connect to any host and port, read what answers) | The same allowlist, with port ranges. No background connections: there is no health check that probes targets; the last result of a real send is shown instead. |
| **Patient data leaves** towards a system that should not get it | The allowlist is the control. `SECURITY.md` says so. |
| **Another website** makes the browser send | Send routes are `POST`; `guard_cross_site` refuses foreign origins. |
| **Flooding** a target | Run size and number of parallel runs are limited; a run can be cancelled; every message is visible. |
| **Nobody knows who sent** something | Every send is a row everyone sees, with the requesting address, and a log line. |
| **Patient data in the browser** | Drafts made from real messages contain patient data. They stay in the tab by default and are written to the computer only on request. |
| Content in the **log** | Send log lines carry metadata only. |

`SECURITY.md` gets a section on sending, and the changelog an upgrade note:
after the update Harux can send to itself; any other destination needs an
entry in `harux.toml`.

---

## 7. Stages and work packages

Each work package is one issue, one branch and one squash-merged pull request.
Each leaves `main` releasable and wires up everything it adds: the binary
target reports unused code, so a package cannot add an engine without a route
that uses it. Sizes are relative (S, M, L).

```
M4-1 ─► M4-2 ─► M4-3 ─► M4-4 ─► M4-5 ─► M4-9 (v0.10.0)
          ├───► M4-6 ─► M4-7                (v0.10.x)
          └───► M4-8                        (v0.10.x)
```

### Stage 1 — Resend

**M4-1 · Sending over the API** (L, backend)
`[send]` configuration and policy; `mllp_client.rs`; `send.rs` with runs of
one stored message; `Outbound`, `Delivery`, `set_delivery`, `delivery_updated`;
accounting; `GET /api/send/targets`, `POST /api/send` (stored messages,
overrides, placeholders in overrides); export direction; `is:sent` in
`SearchQuery`; send counters; log lines; `SECURITY.md`, `AGENTS.md`,
`docs/PROJECT_OVERVIEW.md`, example configuration.
*Done when* `curl -X POST /api/send` resends a stored message to a configured
target, the sent message and its response are in `/api/messages`, and a
destination that is not allowed is refused.

**M4-2 · Resend in the UI** (M, frontend)
Target menu and split button; sent rows; direction filter and `is:` prefixes;
target chips; `sent` pill; Response tab; origin links; trace by control ID;
tools row; loopback target.
*Done when* the roadmap's first criterion holds: a received message is resent
to a configured target with one click, and the response is shown.

### Stage 2 — Drafts

**M4-3 · Write and send a draft** (L)
`POST /api/parse`; draft items in `POST /api/send` (normalization, charset,
placeholders, `allow_invalid`); `compose.js` with the drafts rail, the draft
header, the Raw tab with the field status line, a read-only Segments preview
with live validation, the Response tab with the draft's history,
Diff vs pinned, long-value tokens, tab storage; *Edit* on stored messages;
*New message* (blank, clipboard).
*Done when* the roadmap's third criterion holds: raw HL7 is edited and sent,
including a Latin-1 message with umlauts and an MDM with an attachment.

**M4-4 · Edit fields** (M)
Inputs in the Segments tab; add, duplicate and remove segments;
`GET /api/dictionary/{segment}` and *show empty fields*; quick actions for
MSH-7 and MSH-10.
*Done when* a message can be changed field by field without touching the Raw
tab, and both tabs always show the same text.

### Stage 3 — Templates

**M4-5 · Templates** (S–M)
Built-in set, `send.templates_dir`, the two routes, the *New message* menu,
*Save as file*, *Keep on this computer*, open a file as a draft.
*Done when* the roadmap's second criterion holds: a template is filled out,
sent, and the response displayed.

### Stage 4 — Several messages

**M4-6 · Runs** (M)
Several items and `repeat` in `POST /api/send`; run status and cancel; the run
bar; *Resend n filtered*; *Repeat* on a draft; limits.
*Done when* fifty filtered messages are resent in their original order over
one connection, with progress, and can be cancelled.

**M4-7 · Import** (S, optional)
`POST /api/import`, *From file* for whole files.
*Done when* a file written by `/api/export` can be loaded and resent.

### Stage 5 — Watch

**M4-8 · Watch** (S, frontend only)
The bell, title counter, sound, desktop notification where the browser allows
it. Independent of the other packages once M4-2 is in.

### Release

**M4-9 · Release v0.10.0 "Workflow & Testing"** (S), after stage 3
`README.md` features, `docs/ROADMAP.md`, the full API and configuration tables
in `docs/PROJECT_OVERVIEW.md`, upgrade notes, version. Stages 4 and 5 follow
as v0.10.x. `main` is usable after every package, so the team does not have to
wait for the tag.

### Done means, for every package

- The CI list of `AGENTS.md` passes: `cargo fmt --check`,
  `cargo clippy -- -D warnings`, `cargo build`, `cargo test`, the JS tests.
- `CHANGELOG.md` is updated in the same commit, with the issue number.
- The documents that describe what changed are updated in the same pull
  request, and `tests/doc_paths.rs` still passes.
- The tests of section 8 that belong to the package exist.
- Pull requests that change the UI carry screenshots.

---

## 8. Tests

| Layer | What is tested | Where |
|---|---|---|
| Wire | Response in one or many reads; delayed; never; connection closed; refused; not framed; larger than the limit; two messages on one connection | new `tests/send_tcp.rs`, fake receivers on `127.0.0.1:0` like `tests/mllp_tcp.rs` |
| Round trip | Send to Harux's own `mllp::serve`: the sent message is stored before the received one; code `AA`; MSA-2 matches | `tests/send_tcp.rs` |
| Bytes | Unchanged resend equals the received bytes for UTF-8, Latin-1 and an irreproducible payload; override keeps every other byte; unmappable character refused | unit tests, `tests/send_tcp.rs` |
| Compose | Field paths incl. MSH shift, custom delimiters, occurrences, new fields; placeholders; line ends | unit tests in `src/hl7/compose.rs` |
| Fuzz | Mutated responses through the response reader, mutated text through override and placeholder code: no panic, and an override is found again by the parser | `tests/fuzz_ingest.rs` or a sibling |
| Policy and config | Allowed and refused destinations, ranges, names vs. addresses; every invalid `[send]` value stops the start | `src/config.rs`, `tests/config_startup.rs` |
| Store | `delivery_updated` order and frame; bytes exact after a response; eviction and clear with pending messages | `src/store.rs`, `tests/alloc_accounting.rs` |
| API | Status codes of 5.8; origin guard on the new routes; `/api/parse` equals the detail JSON; export direction | `tests/web_api.rs` (`MockConnectInfo` for the client address) |
| Templates | Every built-in template parses without a warning | unit test |
| Frontend | Sent rows and the skeleton; `delivery_updated` while a list loads; counters; direction filter; writing a field into a line; long-value tokens; draft storage that fails; the watch matcher | `tests/js/*.test.mjs` |

The manual scripts `tests/test.sh` and `tests/test.ps1` need no change: the
loopback target sends to the listener they test.

---

## 9. Documentation

Updated with the package that changes the behaviour, not at the end:
`AGENTS.md` (architecture section "Sending", the rules of section 2, source
layout, common mistakes), `docs/PROJECT_OVERVIEW.md` (architecture, API,
events, configuration), `README.md` (features; the sentence that sending is on
the roadmap), `SECURITY.md`, `docs/STYLE_GUIDE.md` (new components, the
detail header), `docs/ROADMAP.md`, `examples/harux-example.toml`,
`harux.toml`, `CHANGELOG.md`.

---

## 10. Risks

| Risk | What the plan does about it |
|---|---|
| Sent messages crowd the list and the store | Direction filter, remembered per session; run limit; sent messages are evicted like others. A replayed MDM is stored again — the cost of rule 1 |
| The editor grows into a framework | A textarea and inputs; the server parses; no syntax-highlighting editor, no editing below field level in this milestone |
| Mojibake when sending | The byte-for-byte path for unchanged messages, an explicit encoding for drafts, refusal instead of replacement, tests with Latin-1 |
| A response arrives for a message that is gone | `set_delivery` on a missing entry is a logged no-op |
| A slow or silent target | Timeouts on connect, write and response; cancel; the row shows `…` meanwhile |
| Two people send at once | Runs are independent connections; every row names its requester |
| The scope grows (load tests, TLS, ACK rules, shared editing) | Section 12 |

---

## 11. Decisions for the owner

The plan assumes the recommendation in each row.

| # | Question | Recommendation |
|---|---|---|
| D1 | Do sent messages appear in the shared list, or in a separate outbox? | **Shared list** with a direction filter. It is what makes diff, tags, search and "who sent this" work without new code, and it keeps send and result next to each other |
| D2 | Who defines where Harux may send? | **`harux.toml`**: named targets, plus optional patterns within which users may type a `host:port`. No free entry |
| D3 | May production systems be targets? | **Yes, if the administrator lists them**, with `confirm = true`. Resending a lost message is a real task; forbidding it would move it to another tool |
| D4 | TLS for outgoing MLLP? The 2026-05 notes say "yes, by default" | **Not in this milestone.** It needs a TLS crate (`tokio-rustls`) and certificate settings, and belongs with MLLP/S for the listener (Milestone 6). To decide: does any target in use require TLS today? |
| D5 | Where do templates and drafts live? | **Built-in set, a read-only directory, drafts in the browser.** Nothing is saved on the server before Milestone 5 |
| D6 | Is the loopback target on by default? | **Yes.** It reaches only Harux's own listener and makes the feature usable and testable without configuration |
| D7 | How large may a run be? | **1,000 messages** by default, every one stored. Larger load tests are a later feature |
| D8 | Watch without desktop notifications on plain HTTP — acceptable? | **Yes**: title counter and sound everywhere, desktop notification behind HTTPS or on `localhost` |
| D9 | Import (M4-7) in this milestone? | **Optional**, last. Small, and useful until persistence exists |
| D10 | One release at the end, or one per stage? | **`main` is usable after every package; tag v0.10.0 after stage 3**, v0.10.x for stages 4 and 5 (v0.9.0 is the hardening release) |

---

## 12. Not in this milestone

- **TLS** for outgoing connections (D4).
- **ACK rules** — answering `AE`/`AR` or nothing on purpose, to test
  Orchestra's retry. It is a testing feature and a natural next step, but it
  changes how Harux receives and needs its own design. It stays in
  Milestone 6.
- **Saving on the server:** shared templates edited in the UI, saved targets,
  drafts for the team — Milestone 5.
- **Load tests** beyond the run limit, replay with the original timing.
- **Anonymizing** messages before sending them to another system.
- **Editing components and sub-components** in a form; HL7 batch files
  (FHS/BHS); a syntax-highlighting editor.
- **Probing targets** in the background.

---

## 13. What changed against the 2026-05 handoff

| 2026-05 handoff | This plan | Why |
|---|---|---|
| Replay drawer, generator page, editor overlay | One draft in the detail panel, resend from the header | One model instead of three surfaces (section 2) |
| Replay result "tagged in the inbox" on request | Every sent message is a stored message | Rule 1; diff and search work on sent messages for free |
| `[targets.*]` with `tls` and `default` | `[[send.targets]]` with `confirm`, plus `allowed_destinations` | Safety first; the "default" is the target each browser used last |
| Health check: connect to every target every 30 s | Last result of a real send | No traffic nobody asked for; some receivers log every connection |
| CodeMirror 6 | `<textarea>`, inputs, server-side parse | No framework, no build step |
| Server-sent events for bursts | Rows over the existing WebSocket, totals by polling the run | One live channel |
| Faker data, front-matter in template files | Five placeholders in the text | Visible and predictable |
| Rule engine for notifications on the server | Watch in the browser | Per-user, no server state |
| Target TLS on by default | Not in this milestone | New crate; open decision D4 |
