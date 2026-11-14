# Milestone 4 — Workflow & Testing: implementation plan

> **Status:** done — released as v1.0.0 on 2026-11-13 (#271, M4-1 … M4-12).
> This is now a historical document: what was built follows it, and the
> rules that last are in `AGENTS.md` (*Sending*). The owner took the
> decisions of section 11 on 2026-11-12; Q1 was answered on 2026-11-13;
> Q2 did not have to be answered for the plan to work.
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

The roadmap's acceptance criteria for replay, the editor and templates are met
after stages 1, 2 and 3; the one for TLS with M4-2, and the one about
visibility and limits with M4-1. Stage 1 alone already gives the team
one-click replay.

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
   One frame on the wire is one stored message: a payload Harux builds never
   contains the MLLP start or end byte, so nothing can travel inside a send
   that the list does not show.
2. **Unchanged means byte for byte.** Resending a stored message without
   changes sends `Hl7Message::wire_bytes()`, the bytes as they arrived
   (#230). Every change is one the user asked for and the request names.
3. **Every user chooses where to send; the configuration can only narrow
   it.** By the owner's decision (D2) each user keeps a list of targets of
   their own, and Harux connects to what a request names. Sending is off
   until `harux.toml` switches it on (D11); it can also be limited to listed
   destinations, and a wrong value there stops the start. Once it is on,
   nothing else stands between a user and a target, so rule 1 carries the
   weight: every send is a row everyone sees, with the address that asked
   for it.
4. **One of each.** One MLLP framing (`src/mllp.rs`), one parser, one
   validator, one charset table (`src/hl7/charset.rs`), one place that writes
   a field into a message (`src/hl7/compose.rs`). The browser never parses
   HL7 on its own: the server parses a draft, writes a changed field back
   into it, and tells the browser where each field is in the text.
5. **The payload stays immutable** (#192). What was sent is known at insert
   time and never changes. What comes back later (the response, a timeout)
   lives in the entry's mutable metadata, next to tags and bookmark.
6. **No new state on the server's disk.** Shared templates are a read-only
   directory; targets and drafts live in the browser. Saving things on the
   server is Milestone 5.
7. **The existing constraints hold:** no framework, no build step,
   `Arc<RwLock<>>` store, 250 ms flush, prepend on the hot path, rows of
   61 px, CSP without inline code, never ACK an ACK, the MSH shift only
   through `msh_split_index`. No new crate — with one exception the owner
   decided: TLS for outgoing connections (D4, 5.10).

---

## 3. Vocabulary

Used the same way in the UI, the API, the code and the documentation.

| Term | Meaning |
|---|---|
| **Target** | A named place to send to: host, port, TLS or not. Each user keeps a list of their own in the browser. |
| **Destination** | A target's `host:port`. `harux.toml` can limit which ones may be used. |
| **Send** | Write one message to a destination and read the response. |
| **Resend** | Send a stored message (received or sent) again — the roadmap's "replay". |
| **Draft** | A message being written. Lives in the browser. |
| **Template** | A draft to start from, with placeholders such as `{{id}}`. |
| **Sent message** | A stored message with direction `out`. The others have direction `in`. |
| **Delivery** | What became of a sent message: `pending`, `acked`, `failed`, or `sent` when no response is awaited (an ACK-type message, or the request says so). |
| **Reference** | A placeholder in a draft that stands for a long value of the message the draft was made from, e.g. `{{from:OBX[1]-5}}`. |
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
  used last. The label always names the target. The first use opens the menu
  instead, and a target marked *ask before sending* asks first.
- **The menu** (the caret) lists the user's own targets with the last result
  this browser saw (`AA · 42 ms · 2 min ago`, `refused · yesterday`) and how
  to send: *as received* (byte for byte), *with a new control ID* (MSH-10),
  *with a new control ID and the current time* (MSH-10, MSH-7). The choice is
  remembered per browser.
- **My targets** (D2): the same menu has *Add target* and *Edit* — a small
  form with name, host, port, TLS on or off (on for a new target) and *ask
  before sending*. Every user keeps a list of their own. It is stored in the
  browser (`localStorage`; it holds no patient data), nothing about it is
  stored on the server, and it can be saved to and loaded from a file to pass
  it to a colleague. *This Harux*, the loopback target, is always in the
  list. A shared list for the whole team is part of the rework the owner
  announced, not of this milestone.
- **Ask before sending** is a mark the user sets on a target of their own,
  typically a production system (D3). Such a target asks before every send
  and takes one message per request, never a run. It protects against a slip
  of one's own hand; the server does not know the mark.
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
- **Not for cut-off entries:** a frame above the size limit is stored with
  its first 4 KB only (#223). Such an entry shows neither button.

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
  *remove*. MSH-1 and MSH-2 are changed in the Raw tab only. A changed field
  is written into the text by the server — the operation a resend "with a new
  control ID" uses (5.4) — so both tabs always show the same text.
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
- **Long values** (the Base64 of an MDM) are not part of a draft's text. The
  draft holds a reference to the value in the message it was made from
  (`{{from:OBX[1]-5}}`), shown as a chip with its size, and the server fills
  it in when it sends. An MDM can so be edited without freezing the tab
  (#215), and megabytes never travel through the browser. If the original is
  no longer stored when the draft is sent, the send is refused and the draft
  says why; a bookmark keeps the original.
- **Broken on purpose:** a draft that does not parse can be sent after a
  second click (*Send anyway*). It is stored with its parse error, like a
  broken received frame. Testing how Orchestra handles a bad message is a
  normal use.
- **Where drafts are kept:** in the browser tab (`sessionStorage`), so they
  survive a reload but not the end of the session. *Keep on this computer*
  moves one to `localStorage`. See 6 for why keeping is not the default.
- **Files:** *Save as file* downloads the bytes the server would send, in the
  draft's encoding; *From a file* hands the file's bytes to the server, which
  decodes them like a received frame. A browser reads and writes text as
  UTF-8 only, so doing either in the browser would damage a Latin-1 file.

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
- Every message of a run is a row. A run is limited in number
  (`send.max_run_messages`, default 1,000) and in size
  (`send.max_run_store_percent`, default 25: its messages together may take
  at most that share of `store.max_memory_mb`, because every one of them is
  stored again). Both are set in `harux.toml` (D7). A run keeps hold of the
  messages it was asked to send, so eviction during the run cannot take one
  away before it is sent.
- A target marked *ask before sending* takes one message per request, never
  a run.
- **Import:** an `.hl7` file in the format `/api/export` writes (MLLP frames)
  is loaded into the store as received from `import:<file name>`. From there
  it is resent like anything else. This also carries a set of test messages
  across a restart until Milestone 5. A single unframed message is opened as
  a draft instead (*New message → From a file*).

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

Nothing new. Sending is off by default (D11): no button, no rail, the send
routes answer `403`, and an update changes nothing for an installation that
does not ask for it. With `send.enabled = true` in `harux.toml` the window
gains the **New message** button and the **Resend** and **Edit** buttons, and
every user's target list starts with *This Harux*.

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
  message. `Outbound { target, tls, origin, requested_by, run }` holds the
  facts of the send that never change: the name the user gave the target,
  whether the connection was encrypted and verified, where the message came
  from (`Resend { message_id }`, `Draft { based_on }`, `Template { name }`),
  the client's IP address and the run.
- For a sent message `source_addr` holds the destination (`host:port`) and
  `received_at` the moment the send began. Both names stay as they are, since
  the UI and the API use them everywhere; the meaning is "the peer" and "when
  it entered the list".
- A destination must never show up as a sender. Everything that groups or
  colours by source — the source rail and its counters (`registerSource`,
  `sourceCounts`, `srcLabelFor`), the row's bar (`getSourceColor`) and the
  highlight — keys a sent message by its target (`Outbound.target`, `target`
  in the summary), not by `source_addr`. The search is the one place that
  uses both: for a sent message the source match covers the destination and
  the target's name, so `orchestra-test` finds what was sent there.
- `MessageMeta` gets `delivery: Option<Delivery>`:
  `Delivery { state, ack_code, response, error, latency_ms, control_id_matches }`.
  It is `None` for received messages. `ack_code` and `ack_response` on the
  message keep one meaning — what Harux wrote back — and stay empty for a
  sent message; what Harux got back is `delivery`, in the list and in the
  detail JSON alike.
- **The response text stays out of the summary.** A target writes it, so its
  size is not Harux's to choose, and summaries are copied under the store
  lock, sent with every list row and held in the 4,096-slot broadcast buffer,
  which `store.max_memory_mb` does not bound. `Hl7MessageSummary` gets
  `direction`, `target`, `origin_id` and a small `delivery`
  (`state`, `ack_code`, `latency_ms`, `error`); the response is in the detail
  JSON only. The stored response is cut at 64 KB with a note, as an
  over-limit frame is cut at 4 KB.
- `Hl7Message` gets `truncated: bool`, set for an over-limit entry (#223),
  which holds only the beginning of its frame. It is in the summary too, and
  it rules out Resend and Edit in the UI and in the API.
- The detail JSON also gains each field's `datatype` from the dictionary, for
  stored messages and previews alike (the draft editor shows it).
- **Accounting (#226):** `Hl7Message::estimated_bytes` counts `Outbound` and
  `MessageMeta::estimated_bytes` counts `Delivery`. `tests/alloc_accounting.rs`
  measures only `Hl7Message` today; it is extended to the metadata, with a
  sent message that has a response. A response that arrives later changes
  the entry's bytes through the existing `mutate_meta` delta.

### 5.2 Store (`src/store.rs`)

- A sent message is inserted with `delivery: pending`, through a second entry
  point next to `insert`, which keeps its signature.
- `set_delivery(id, delivery)` goes through `mutate_meta` and broadcasts
  `StoreEvent::DeliveryUpdated(summary)` under the write lock (#232). The
  frame is rendered once (#196) as `delivery_updated` in `ws_event_json`.
- A response makes an entry larger, and `mutate_meta` does not look at the
  limits today — only `insert` wakes the eviction task. `set_delivery` checks
  them and wakes it the same way.
- Eviction, bookmarks, `clear` and the search treat sent messages like all
  others. `SearchQuery` learns `is:sent` / `is:received`.
  `payload_snapshot` takes a direction for the export.
- If a pending message is evicted or cleared before its response arrives,
  `set_delivery` finds nothing and the outcome is only logged.

### 5.3 Sending (`src/mllp_client.rs`, `src/send.rs`)

`src/mllp_client.rs` does one exchange on an open connection and knows nothing
about the store: write `VT payload FS CR`, read until `FS CR`, with a timeout
for each step and a size limit for the response (1 MB). It uses the framing
constants and `extract_mllp_frame` from `src/mllp.rs` (the function becomes
`pub(crate)`).

How the frame is written follows the export stream (`MllpExportStream`): a
payload below 64 KiB is framed into one buffer and written at once; a larger
one is written in three parts — start byte, payload, end bytes — with the
payload borrowed from the stored sent message (`wire_bytes()` is a `Cow`), so
a large message in flight is never held a third time. Only a re-encoded
charset (`WireForm::Encoded`) needs a buffer of its own, for the one item
being written. The socket gets `TCP_NODELAY`: three small writes on a socket
with Nagle's algorithm wait for the peer's delayed ACK, which would add tens
of milliseconds to every send and to the round-trip time Harux shows.

No response is awaited for a message whose type starts with `ACK` — a
receiver does not answer an ACK, just as Harux never does — or when the
request says so (`await_response: false`). Such a delivery ends as `sent`.

The response is decoded with `decode_payload` and read with `parse_message`
under `catch_unwind`, as incoming frames are. From it come MSA-1 (`AA`/`CA`
accepted, `AE`/`CE` error, `AR`/`CR` rejected), MSA-2 and the text. A response
that is not HL7 is kept as text and marked as such. What is stored of it is
cut at 64 KB (5.1).

`src/send.rs` is the engine. A **run** is a task that:

1. checks the destination the request names against the limits of 5.7; looks
   up every stored message the request names and keeps its `Arc`, as the
   export's `payload_snapshot` does — an unknown ID refuses the whole request,
   and a message evicted while the run is under way is still sent;
2. for each item: builds the bytes (5.4), parses them into an `Hl7Message`
   with `outbound` set and inserts it as `pending`;
3. connects if the run has no open connection (`send.connect_timeout_secs`;
   for a TLS target the handshake of 5.10 is part of connecting), does the
   exchange, and records the outcome with `set_delivery`;
4. waits `interval_ms` between items, stops on cancel and on shutdown.

- A transport failure (connect, write, timeout, closed) marks the item
  `failed`; the run reconnects for the next item and gives up after three
  failures in a row. Items it never tried are not stored. A NACK never stops
  a run unless the request says so.
- A run is refused when its messages together are larger than a quarter of
  `store.max_memory_mb` (4.5). The count limit alone would let a run of large
  MDMs push the rest of the store out.
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
| Stored message | none | `wire_bytes()` — exactly as stored. This includes a frame that failed to parse: it goes out as it came in, after the same second click a broken draft needs (`allow_invalid`). The one exception is an entry marked `truncated` (5.1), which cannot be resent |
| Stored message | field overrides | `raw` with those fields replaced, in the encoding the message arrived in (`WireForm`). A pure-ASCII message carries no evidence of its encoding — it is stored as `WireForm::Utf8` whatever MSH-18 says — so for it the charset MSH-18 declares is used, else UTF-8: an override that brings an `ü` into a message declared as `8859/1` must leave as Latin-1. A message whose bytes no encoding reproduces (`WireForm::Original`) can only be resent unchanged |
| Draft text | — | Line ends normalized to `CR`, final `CR` added, placeholders and references filled in, encoded with the draft's encoding (MSH-18, else UTF-8) |

**What is refused** (`422`, with the reason), for every payload Harux builds —
a draft, or a stored message with overrides:

- **The MLLP start or end byte** (`0x0B`, `0x1C`) anywhere in the payload.
  With the line ends normalized, an end byte followed by a line break would
  close the frame early, and the rest would reach the target as a second
  frame that the list never shows (rule 1). An unchanged resend is exempt:
  its bytes arrived inside one frame.
- **A character the encoding cannot write.** The error names it; nothing is
  replaced by `?`.
- **An override that leaves its field.** The value may contain component,
  repetition and sub-component separators and escapes — it is the field's
  raw value — but not the field separator, `CR` or `LF`. The field number
  must be between 1 and 1,000 (3 or more for MSH: MSH-1 and MSH-2 are the
  delimiters themselves, and `msh_split_index` is defined from 2 up). After
  the overrides the text is parsed again and must have the same segments as
  before.

**How a message is changed:**

- **Field overrides** are `"MSH-10": "value"` pairs (segment, optional
  occurrence `OBX[2]-5`, field). They are applied to the text with the
  message's own delimiters and `msh_split_index`. Segments are found the way
  the parser finds them (split at `CR` and `LF`, blank lines skipped), and
  only the one field's text is replaced: every other byte, the line ends and
  the final terminator stay as they are. A missing field is created by adding
  separators, a missing segment is an error. The menu's "new control ID" is
  the override `MSH-10 = {{id}}`. The draft editor uses the same operation to
  write a changed field back (5.5).
- **Placeholders** are filled in for override values and for draft text, never
  for the text of a stored message. Only these names are recognized; anything
  else in double braces is sent as written.

  | Placeholder | Value |
  |---|---|
  | `{{now}}` | Local time, `YYYYMMDDHHMMSS` |
  | `{{today}}` | Local date, `YYYYMMDD` |
  | `{{id}}` | A new 20-character control ID — the expression `render_ack` uses for MSH-10, moved into one function both call |
  | `{{n}}` | Position of the message in its run, from 1 |
  | `{{rand:6}}` | That many random digits, taken from a UUID (`uuid` is a dependency, `rand` is not) |
  | `{{from:OBX[1]-5}}` | A reference: the value of that field in the message the draft was made from (`based_on`). Filled in from the store when the draft is sent; if that message is gone, the send is refused |

- `charset.rs` gets the reverse of `decode_payload`: text plus encoding choice
  → bytes plus `WireForm`, using the same `encoding_for` table.

### 5.5 Drafts on the server

A draft lives in the browser, but everything that needs to understand HL7 is
done by the server. Nothing in this section stores anything.

| Route | What it does |
|---|---|
| `GET /api/messages/{id}/draft` | The text of a stored message as a draft: every field value above the display limit (`DISPLAY_VALUE_MAX`, 2,000 characters) is replaced by a reference `{{from:…}}`. Returns the text, the references with their sizes, and the encoding. This is what *Edit* loads |
| `POST /api/parse` | The preview. Takes the draft's text, optionally overrides (`set`) and `based_on`. Returns the text after the overrides, and for that text what `GET /api/messages/{id}` returns for a stored message — segments, descriptions, datatypes, type information, validation warnings or the parse error — plus the position of every field in the text |
| `POST /api/parse` with a file's bytes | *From a file*: the bytes are decoded like a received frame (`decode_payload`; the first frame, if the file is MLLP-framed) and returned as text with their encoding |
| `POST /api/build` | *Save as file*: the bytes `POST /api/send` would write for this draft, MLLP-framed like the export, as a download |

- **Validation.** The fields in the answer come from the text as written, so
  an input shows `{{now}}`. The warnings come from the same text with sample
  values filled in, so `{{now}}` is not reported as an invalid timestamp. The
  validator itself knows nothing about placeholders. References are left as
  they are: none of the types the validator checks (NM, DT, TS, SI) holds an
  attachment.
- **Writing a field.** When an input in the Segments tab changes, the browser
  sends the text with `set: {"PID-5": "…"}` and takes the returned text as the
  draft. That is the override code of 5.4 — the one place that knows the MSH
  shift and the delimiters.
- **Where a field is.** The answer gives, for each field, where it starts and
  ends in the text, counted in UTF-16 units as a `<textarea>` counts. The Raw
  tab's status line looks the cursor position up in that list. The browser
  compares numbers; it does not split lines.
- **Diff vs pinned** asks for the preview with the references filled in, so
  an attachment nobody touched compares as equal.
- The editor asks for a preview 300 ms after the last change (the debounce
  the search uses) and renders it with the builders `static/render.js`
  already has. A preview of an MDM costs a few kilobytes, because its
  attachment is a reference.

`GET /api/dictionary/{segment}` returns a segment's field list (number, name,
datatype) for *show empty fields*.

### 5.6 Templates (`src/templates.rs`)

Built-in templates are embedded with `include_str!` from
`src/assets/templates/`. Site templates are read from `send.templates_dir` on
every list request (the directory is small; no cache to go stale), limited in
number and size, never written. `GET /api/templates` lists both with name,
group and message type; `GET /api/templates/{id}` returns the text.

A template's ID is a key of that listing and is looked up in it. It is never
joined into a path: only files directly in the directory whose names consist
of letters, digits, `.`, `_` and `-` and end in `.hl7` are listed, so `..` or
an encoded `/` in the ID finds nothing.

### 5.7 Configuration (`src/config.rs`)

Targets are not configured here: every user defines their own (D2, 4.2).
`harux.toml` holds what applies to everyone — the switch, the limits, and the
TLS settings.

```toml
[send]
# Sending from the web UI and the API. Off unless it is switched on here:
# with it on, everyone who can open the web UI can send to any destination
# this server reaches (or to the ones listed in allowed_destinations).
enabled = false
# Put Harux's own MLLP listener into every user's target list.
loopback_target = true
connect_timeout_secs = 5
response_timeout_secs = 30
# Limits of one run (several messages in one request).
max_run_messages = 1000
max_run_store_percent = 25     # share of store.max_memory_mb a run may take
max_concurrent_runs = 4
# Optional: limit where users may send. Empty = any host and port.
# Entries are host:port or host:from-to.
# allowed_destinations = ["orchestra-test.klinik.local:6000-6999"]
# Directory with .hl7 templates for everyone (read only).
# templates_dir = "templates"

[send.tls]
# Certificates are checked against the operating system's certificate store
# and, if set, against this file (PEM), e.g. the hospital's own CA.
# ca_file = "certs/klinik-ca.pem"
# Let users switch the certificate check off for a target of theirs (test
# systems with self-signed certificates): encrypted, but not verified.
allow_unverified = false
# Harux's own certificate, shown to a target that asks the sender for one.
# Most targets do not; leave it out unless a send fails with that reason.
# client_cert_file = "certs/harux.pem"
# client_key_file = "certs/harux.key"
```

- Fails closed like the rest (#233): unknown keys, a malformed
  `allowed_destinations` entry, a `ca_file` or client certificate that cannot
  be read or holds no certificate, a client certificate without its key (or a
  key that does not belong to it), zero limits and a percentage outside 1–100
  stop the start with an error that names the key. "Effective configuration"
  shows the section, never the key.
- **`allowed_destinations`** is empty by default: any host and port. When it
  has entries, a destination is allowed if its host equals an entry's (names
  without case and trailing dot, IP addresses in canonical form) and its port
  lies in the entry's range; nothing is resolved for the check. Every entry
  names a port or a port range — a bare host is refused at start, since it
  would allow every port on that machine.
- The loopback target is the address the MLLP listener is bound to and
  `server.mllp_port`; when the listener is bound to every interface it is
  `127.0.0.1` for `0.0.0.0` and `::1` for `::`. It is always allowed.
- The largest message that can be sent is `mllp.max_message_size_mb`, the
  largest that can be received.
- New keys go into `harux.toml` and `examples/harux-example.toml`
  (`shipped_config_files_load`).

### 5.8 HTTP API and WebSocket (`src/web.rs`)

All new routes that change something are `POST`, so `guard_cross_site`
covers them (#227).

| Route | Purpose |
|---|---|
| `GET /api/send/config` | Whether sending is on, the loopback target, the destination limits, the run limits, whether an unverified TLS connection may be chosen |
| `POST /api/send` | Start a run (below) |
| `GET /api/send/runs/{id}` | Progress and result of a run |
| `POST /api/send/runs/{id}/cancel` | Stop a run |
| `GET /api/messages/{id}/draft` | A stored message as draft text (5.5) |
| `POST /api/parse` | Preview of a draft, field write-back, decoding of a file (5.5) |
| `POST /api/build` | The bytes of a draft as a download (5.5) |
| `GET /api/dictionary/{segment}` | Field list of a segment |
| `GET /api/templates`, `GET /api/templates/{id}` | Templates (5.6) |
| `POST /api/import` | Load an MLLP-framed `.hl7` file into the store (4.5) |
| `GET /api/export?direction=in\|out\|all` | Default `in`: unchanged behaviour |
| `GET /api/stats` | Gains `sent`, `send_failed`, `active_runs`, `sending_enabled` |

```jsonc
// POST /api/send
{
  "target": {                                // or "loopback"
    "name": "orchestra-test-adt",            // the user's label, shown in the list
    "host": "orchestra-test.klinik.local",
    "port": 6661,
    "tls": true,
    "verify": true                           // false only if send.tls.allow_unverified
  },
  "items": [{ "message_id": "…" }],          // or [{ "raw": "MSH|…", "charset": "auto",
                                             //        "based_on": "<message id>", "template": "<name>" }]
  "set": { "MSH-10": "{{id}}" },             // optional field overrides, for every item
  "repeat": 1,                               // each item this many times
  "interval_ms": 0,
  "stop_on_nack": false,
  "await_response": true,                    // false: do not wait for an ACK
  "allow_invalid": false                     // send text that does not parse
}
```

- **The target travels with the request.** The server keeps no list of
  targets; the browser sends the entry of the user's own list. The name is a
  label only (cut to a sensible length, shown in the row); host, port and the
  TLS choice are what the server acts on and checks.
- **One message:** the request waits for the outcome and answers `200` with
  the stored ID and the delivery. The exchange itself runs in the run's task,
  so a browser that gives up does not stop a send half-way. A failed delivery
  (refused, timeout, NACK) is a `200` too: the request worked, the send did
  not.
- **Several messages:** `202` with the run's ID; the rows report progress over
  the WebSocket, the totals come from `GET /api/send/runs/{id}`.
- **Refusals:** `400` malformed request, host or field path · `403` sending
  off, destination outside `allowed_destinations`, or `verify: false` without
  `allow_unverified` · `404` unknown message · `413` message or run too large ·
  `422` what 5.4 refuses, text that does not parse (without `allow_invalid`),
  a `truncated` entry, a reference whose message is gone · `429` too many runs
  or too many large requests at once.
- **Body limits.** axum's default is 2 MB. `/api/send`, `/api/parse` and
  `/api/build` take JSON and get twice `mllp.max_message_size_mb` plus a
  margin: a message inside a JSON string grows by escaping (every `CR` becomes
  two characters, other control characters six), so a limit equal to the
  message cap would refuse messages the listener accepts. The size that
  counts is checked on the decoded message.
- **Large bodies are limited in number, too.** axum reads a body into memory
  before the handler runs, so many large requests at once would cost memory
  no limit sees. A middleware in front of these routes takes a permit from a
  small semaphore before the body is read and answers `429` when none is
  free. It is written with tokio's `Semaphore`; `tower` is a development
  dependency only.
- **`/api/import`** takes the file as the raw request body and reads it as a
  stream, frame by frame, with the frame splitter of the listener: the part
  of `handle_connection` that finds frames and cuts over-limit ones is moved
  into a type both use, so there is no second splitter. Only MLLP-framed
  files are accepted — finding messages in an unframed file would need a
  second MSH scanner (#188). The file may be as large as
  `store.max_memory_mb`; nothing of it is held beyond the frame in work.
- **The requesting address** comes from the connection (`ConnectInfo`). The
  code does not provide it yet: `src/main.rs` serves the router with
  `axum::serve(listener, app)`. M4-1 switches that call to
  `into_make_service_with_connect_info::<SocketAddr>()`. It is the call that
  carries the graceful shutdown, so the shutdown path
  (`server.shutdown_timeout_secs`, `wait_for_drain`) is tested again, not only
  compiled. Behind a reverse proxy the address is the proxy's.

**WebSocket:** one new event, `delivery_updated`, with a summary like
`tags_updated`. `new_message` carries the new summary fields. Clients that
ignore unknown fields and events keep working.

### 5.9 Frontend (`static/`)

Two new files, loaded after `diff.js` and before `app.js`:

- `send.js` — the user's target list (its form, `localStorage`, save to and
  load from a file), the target menu, resend, runs and their bar, toasts. A
  list that cannot be read or written leaves sending usable for the session.
- `compose.js` — drafts: the rail, the draft header, editable Segments and Raw
  tabs, the calls of 5.5, storage.

Changes to existing files:

- `state.js` — targets, drafts, direction filter, watch, and **one
  selection**: `{ kind: 'message' | 'draft', id }` in place of `selectedId`
  alone.
- `ws.js` — `delivery_updated` in `handleServerEvent` (queued during a list
  load like the other events, #239). It can arrive within the 250 ms before
  its message is flushed to the list, so it updates the summary in
  `state.messages` or in `state.pendingMessages`, as `tags_updated` does. If
  the message is open in the detail panel, the detail is fetched again: the
  event carries no response text. Sent messages are left out of `rateWindow`
  and `lastMessageReceivedAt`.
- `render.js` — the row skeleton gets the direction element and the `outbound`
  class; the chip of a sent row comes from `delivery`; `patchRow` learns
  `delivery`; the detail header gets the tools row and the send buttons; the
  ACK tab renders a response; `canPrependOnly` knows the direction filter.
- `util.js` — `getParsedQuery` learns `is:sent` / `is:received`; new icons;
  the source helpers take the message, not its address (5.1).
- `app.js` — new `data-action` cases; `Ctrl+Enter`.
- `index.html`, `style.css` — the new elements. Toggles follow the style
  guide (no native checkboxes); nothing inline.

Places where the existing code would break a draft, each to be changed with
the package that adds drafts:

- **The detail panel is emptied** by `applyCleared`, `removeEvictedMessages`
  and `restoreAfterLoad` in `ws.js` and by `clearMessages` in `app.js`, and
  the copy helpers read `state.selectedMessage`. All of them check the
  selection's kind; a cleared store does not close an open draft.
- **The keyboard delegation** in `app.js` takes Enter and Space from every
  focused element with a `data-action` (except the tag input), and a click on
  a `.field-val` cell copies its text. Inputs and the textarea of a draft are
  left alone by both, or nobody can type a space into a field.
- **A sent row comes from the WebSocket only.** `addMessage` does not check
  for duplicates, so the code that calls `POST /api/send` must not add the
  row itself from the answer.

`renderDetail` keeps serving messages; `compose.js` renders drafts into the
same container. Every new control is reachable with `Tab` and works with
Enter and Space, like the rows and chips today.

### 5.10 TLS for outgoing connections (`src/mllp_client.rs`)

The owner's decision D4: the targets need it. It covers outgoing connections
only; TLS for the listener (MLLP/S) stays in Milestone 6 and can then use the
same library.

**Who needs a certificate.** When Harux sends, it is the side that connects,
like a browser opening a web page: the target shows a certificate and Harux
checks it. Harux needs no certificate of its own for that. It needs one in
only two cases: a target asks the sender to show one as well (mutual TLS,
covered below), or Harux is the side that is connected to — encrypted
*receiving*, which is the listener and not this milestone.

**What is not known yet** (Q2 in section 11): how the targets' certificates
are issued, and whether a target asks for one from Harux. The package is
built so that nobody has to know in advance — each case has its setting, and
the first real attempt names the case in its failure reason:

| The target's certificate is … | What makes it work |
|---|---|
| issued by a CA the Harux server already trusts (its operating system's certificate store — on the Windows server that is where a hospital CA normally is) | nothing; it works as it is |
| issued by a CA of your own that the server does not know | `send.tls.ca_file` |
| self-signed, as test systems often are | `send.tls.allow_unverified = true`, then the user switches the check off for that target: encrypted, not verified |
| fine, but the target also wants a certificate from Harux | `send.tls.client_cert_file` and `client_key_file` |

- **Per target:** TLS on or off, on for a new target. With TLS on, the
  handshake follows the TCP connect and counts into the connect timeout; the
  MLLP exchange then runs inside it, unchanged.
- **Verification:** the certificate must be valid for the target's host name
  (or IP address) and chain to a root in the operating system's store or in
  `ca_file`. A user can switch the check off for a target of theirs only if
  `allow_unverified` is set; the sent message records that it went out
  unverified, and the row shows it.
- **Harux's own certificate** is one for the whole server, set by the
  administrator, never per user: a private key does not belong in a browser.
  It is read at start, shown to a target only when the target asks, and
  never logged or returned by the API.
- **What a failure looks like:** the delivery fails with a reason a person
  can act on, each naming the setting above that answers it —
  `certificate not trusted` (with the issuer), `certificate is for another
  name`, `the target asks for a certificate from Harux`, `no TLS answer — is
  this a plain MLLP port?`, and, for a plain target that turns out to speak
  TLS, `the target expects TLS`.
- **The library** is rustls through `tokio-rustls`. It is written in Rust, so
  Harux stays one binary without OpenSSL at run time on every platform.
  `rustls-native-certs` reads the operating system's store; the PEM reader of
  `rustls-pki-types` reads the files; the cryptography comes from `ring`,
  which builds on the three release targets without CMake or NASM.
- **These are the first new crates since the rule "no new crates without
  explicit instruction".** D4 is that instruction. The package starts by
  proving the choice: a release build on Windows, macOS and Linux, the size
  of the binary before and after, and a clean `cargo audit`. If `ring` or
  `rustls-native-certs` fails that, the alternatives (`aws-lc-rs`,
  `rustls-platform-verifier`) are tried before anything else is built on it.
- **Not included:** a choice of protocol versions or cipher suites; the
  library's defaults (TLS 1.2 and 1.3) are used.
- **Tests** run against a TLS server in the test itself, with a small CA, a
  server certificate for `localhost` and a client certificate kept in
  `tests/tls/` (static files, no crate to generate them): a trusted
  certificate, an unknown issuer, a wrong name, the unverified switch with
  and without `allow_unverified`, a server that asks for a client
  certificate with and without one configured, a plain server answered with
  TLS and a TLS server answered in plain.

---

## 6. Safety and security

Sending is the first feature with which a user of the web UI can change
something outside Harux, and Harux has no login. The owner decided that every
user defines targets of their own (D2) and that production systems may be
among them (D3), and that sending is off until it is switched on (D11). The
plan follows these decisions and says plainly what switching it on means:

> Sending is off by default. Once `send.enabled = true` is set, everyone who
> can open the web UI can make the Harux server connect to any host and port
> the server can reach, and send an HL7 message there — including a
> production system.

What stands against a mistake once sending is on, and what an administrator
can do:

| Risk | Answer |
|---|---|
| A test message reaches a **production system** | The button always names the target. A user can mark a target of theirs *ask before sending*; it then asks every time and takes one message per request. Every send is a row everyone sees. An administrator switches sending on in the first place, and can limit it to listed destinations (`send.allowed_destinations`). Beyond that there is no technical barrier — that is the decision |
| Harux as a **stepping stone** into the network (connect to any host and port, see whether and what it answers) | Possible once sending is on, for the same reason. `allowed_destinations` narrows it; every entry must name a port or a range. Harux opens no connection on its own: there is no health check that probes targets |
| **Patient data leaves** towards a system that should not get it | The same two settings are the control. `SECURITY.md` says so in its first lines on sending |
| **Nobody knows who sent** something | Every send is a row everyone sees, with the requesting address, and a log line. This is the control that always holds |
| A **second message hidden inside a send** (the MLLP end byte in a draft or an override) | A payload Harux builds must not contain the start or end byte; an override cannot leave its field (5.4). What goes out as one frame is one row |
| An **unverified TLS connection** | Off unless `send.tls.allow_unverified` is set; then a choice per target, recorded on every sent message and shown in its row (5.10) |
| **Harux's private key** (if a client certificate is configured) | One for the server, in a file the administrator names; read at start, never logged, never returned by the API, never in a browser (5.10) |
| A **response that is too large**, written by the target | Read up to 1 MB to find its end, stored up to 64 KB, never part of the list rows or the WebSocket events (5.1) |
| A **template ID that is a path** (`..`, an encoded `/`) | An ID is looked up in the listing, never joined into a path (5.6) |
| **Many large requests at once** (previews, sends, imports) | A permit is taken before a body is read; the import is streamed (5.8) |
| **Another website** makes the browser send | Send routes are `POST`; `guard_cross_site` refuses foreign origins |
| **Flooding** a target | Run size and number of parallel runs are limited; a run can be cancelled; every message is visible |
| **Patient data in the browser** | Drafts made from real messages contain patient data. They stay in the tab by default and are written to the computer only on request. The target list holds addresses only |
| Content in the **log** | Send log lines carry metadata only |

`SECURITY.md` gets a section on sending that starts with the sentences quoted
above, and the changelog an upgrade note with the same content and the two
settings. An update alone changes nothing: sending stays off until someone
switches it on.

---

## 7. Stages and work packages

Each work package is one issue, one branch and one squash-merged pull request.
Each leaves `main` releasable and wires up everything it adds: the binary
target reports unused code, so a package cannot add an engine without a route
that uses it. Sizes are relative (S, M, L).

```
M4-1 ─► M4-2 ─► M4-3 ─► M4-4 ─► M4-5 ─► M4-6 ─► M4-7 ─► M4-8 ─┐
                  │                       └───► M4-9 ─► M4-10 ─┼─► M4-12 (v1.0.0)
                  └───► M4-11 ─────────────────────────────────┘
```

There is one release, at the end, and it is **v1.0.0** (D10). `main` is usable
after every package, so the team does not have to wait for the tag.

### Stage 1 — Send and resend

**M4-1 · Send a stored message, unchanged** (L, backend and the minimum of UI)
The `[send]` section (the switch, off by default; limits;
`allowed_destinations`; loopback);
`mllp_client.rs` over plain TCP; `send.rs` with a run of one stored message;
`Outbound`, `Delivery`, `truncated`, `set_delivery`, `delivery_updated`;
accounting; `GET /api/send/config`; `POST /api/send` for one stored message,
unchanged, to the target the request names; `ConnectInfo`; export direction;
`is:sent` in `SearchQuery`; send counters; log lines; `SECURITY.md`,
`AGENTS.md`, `docs/PROJECT_OVERVIEW.md`, example configuration. The list and
the detail view show a sent message as sent (direction, target, the chip from
`delivery`, the Response tab), so `main` never presents a sent message as a
received one.
*Done when*, with `send.enabled = true`, `curl -X POST /api/send` resends a
stored message to a plain MLLP target and the sent message and its response
appear in the UI as sent; with `allowed_destinations` set a destination
outside it is refused; and without the switch every send route answers `403`
and the UI shows nothing new.

**M4-2 · TLS for outgoing connections** (M, backend)
Everything in 5.10: the build proof for the new crates first, then the
handshake in `mllp_client.rs`, `[send.tls]`, verification, Harux's own
certificate for targets that ask for one, the failure reasons, `tls` on the
sent message, the tests with their certificates.
*Done when* a stored message is sent over TLS to a target whose certificate
comes from a CA in `ca_file`, a target with an unknown certificate is refused
with a reason that names the issuer, a target that asks for a certificate
gets the configured one, and the three release builds are green.

**M4-3 · Resend in the UI** (M, frontend)
The user's target list with its form (name, host, port, TLS, ask before
sending), stored in the browser, saved to and loaded from a file; target menu
and split button; direction filter and `is:` prefixes; target chips; `sent`
pill; origin links; trace by control ID; tools row.
*Done when* the roadmap's first criterion holds: a user adds a target of
their own, resends a received message to it with one click, and sees the
response.

**M4-4 · Changes on resend** (M)
`src/hl7/compose.rs`: field overrides, placeholders, everything 5.4 refuses;
the encoding step in `charset.rs`; `set` in `POST /api/send`; the menu's
"send as" choices; the fuzz test for this code.
*Done when* a stored message is resent with a new control ID and the current
time and every other byte is unchanged, and a value that carries the MLLP end
byte or a field separator is refused.

### Stage 2 — Drafts

**M4-5 · Drafts on the server** (M, backend)
`GET /api/messages/{id}/draft` with references; `POST /api/parse` (preview,
field write-back, field positions, decoding of a file); `POST /api/build`;
draft items in `POST /api/send` (line ends, encoding, references,
`allow_invalid`); the body limits and the permit middleware; `datatype` in the
detail JSON.
*Done when*, with `curl` alone, a draft can be made from a stored message,
previewed, changed with `set`, downloaded and sent — a Latin-1 message with
umlauts and an MDM whose attachment is a reference.

**M4-6 · Drafts in the UI** (L, frontend)
`compose.js`: the drafts rail, the draft header, the Raw tab with the field
status line, a read-only Segments preview with live validation, the Response
tab with the draft's history, Diff vs pinned, reference chips, tab storage;
the selection model and the three traps of 5.9; *Edit* on stored messages;
*New message* (blank, from the clipboard, from a file).
*Done when* the roadmap's third criterion holds: raw HL7 is edited and sent
from the browser, for the two messages of M4-5 as well.

**M4-7 · Edit fields** (M)
Inputs in the Segments tab, written back through `POST /api/parse`; add,
duplicate and remove segments; `GET /api/dictionary/{segment}` and *show
empty fields*; quick actions for MSH-7 and MSH-10.
*Done when* a message can be changed field by field without touching the Raw
tab, and both tabs always show the same text.

### Stage 3 — Templates

**M4-8 · Templates** (S–M)
Built-in set, `send.templates_dir`, the two routes, the *New message* menu,
*Save as file*, *Keep on this computer*.
*Done when* the roadmap's second criterion holds: a template is filled out,
sent, and the response displayed.

### Stage 4 — Several messages

**M4-9 · Runs** (M)
Several items and `repeat` in `POST /api/send`; the two run limits and the
hold on the run's messages; run status and cancel; the run bar; *Resend n
filtered*; *Repeat* on a draft (which is why it follows M4-6).
*Done when* fifty filtered messages are resent in their original order over
one connection, with progress, can be cancelled, and none is lost to eviction
while the run is under way.

**M4-10 · Import** (S–M)
The frame splitter moved out of `handle_connection`; `POST /api/import`,
streamed; *From a file* for framed files.
*Done when* a file written by `/api/export` can be loaded and resent, and the
listener's tests in `tests/mllp_tcp.rs` pass unchanged.

### Stage 5 — Watch

**M4-11 · Watch** (S, frontend only)
The bell, title counter, sound, desktop notification where the browser allows
it (question Q1 in section 11). Independent of the other packages once M4-3
is in.

### Release

**M4-12 · Release v1.0.0 "Workflow & Testing"** (S), after all packages
`README.md` features, `docs/ROADMAP.md`, the full API and configuration tables
in `docs/PROJECT_OVERVIEW.md`, upgrade notes, version. A major version says
that the API and the configuration are meant to stay: the package also goes
through both once more for anything that should be renamed before that
promise is made (`received_at` and `source_addr` on a sent message are the
known candidates).

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
| Wire | Response in one or many reads; delayed; never; connection closed; refused; not framed; larger than the limit; two messages on one connection; no wait after an ACK-type message | new `tests/send_tcp.rs`, fake receivers on `127.0.0.1:0` like `tests/mllp_tcp.rs` |
| TLS | A trusted certificate; an unknown issuer; a wrong name; the unverified switch with and without `allow_unverified`; a server that asks for a client certificate, with and without one configured; a plain server answered with TLS and the reverse; the same exchange inside TLS as outside | `tests/send_tcp.rs`, certificates in `tests/tls/` |
| Round trip | Send to Harux's own `mllp::serve`: the sent message is stored before the received one; code `AA`; MSA-2 matches | `tests/send_tcp.rs` |
| Bytes | Unchanged resend equals the received bytes for UTF-8, Latin-1 and an irreproducible payload; an override keeps every other byte; a pure-ASCII message declared `8859/1` leaves as Latin-1 after an override with `ü`; unmappable character refused | unit tests, `tests/send_tcp.rs` |
| Compose | Field paths incl. MSH shift, custom delimiters, occurrences, new fields; placeholders and references; line ends. Refused: the start or end byte, a separator or line break in a value, field 0, MSH-1 and MSH-2, a field number above the limit, a changed list of segments | unit tests in `src/hl7/compose.rs` |
| Fuzz | Mutated responses through the response reader, mutated text through override and placeholder code: no panic, an override is found again by the parser, and the built payload never contains a frame byte | `tests/fuzz_ingest.rs` or a sibling |
| Limits and config | Sending off by default: every send route answers `403`; with it on, any destination without `allowed_destinations`, allowed and refused ones with it (ranges, names vs. addresses, an entry without a port); the run limits; every invalid `[send]` and `[send.tls]` value stops the start, including a client certificate without its key | `src/config.rs`, `tests/config_startup.rs`, `tests/web_api.rs` |
| Store | `delivery_updated` order and frame; no response text in a summary; a response above 64 KB is cut; bytes exact after a response, and it wakes eviction; eviction and clear with pending messages; a run still sends a message evicted after it began | `src/store.rs`, `tests/alloc_accounting.rs`, `tests/send_tcp.rs` |
| API | Status codes of 5.8; origin guard on the new routes; `/api/parse` equals the detail JSON, and its positions point at the fields; a template ID with `..` or an encoded `/`; `429` from the permit; export direction | `tests/web_api.rs` (`MockConnectInfo` for the client address) |
| Templates | Every built-in template parses without a warning once its placeholders are filled in | unit test |
| Frontend | The target list: add, change, remove, a reload, storage that fails, a file that is not a list; *ask before sending* refuses a run; sent rows and the skeleton; `delivery_updated` while a list loads and before the flush; no second row after a send; counters; direction filter and `canPrependOnly`; an open draft survives `cleared`, `evicted` and a reload; a space typed into a draft input; draft storage that fails; the watch matcher | `tests/js/*.test.mjs` |

Existing tests and declarations that change with the first package, so that
nobody is surprised by them: the `AppState` literals in `tests/web_api.rs`
and `tests/mllp_tcp.rs`; the row skeleton asserted in
`tests/js/rows.test.mjs`; the stubs of the source helpers in the JS tests,
which take an address today; `ws_event_json_keeps_the_wire_format` in
`src/web.rs`; `shipped_config_files_load`; and every new module is declared
twice, in `src/lib.rs` and in `src/main.rs`.

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
| A message goes where it should not (D2, D3) | Section 6: the target is always named, *ask before sending*, every send visible with its requester, and two settings for the administrator |
| Sent messages crowd the list and the store | Direction filter, remembered per session; a run is limited in number and in size; sent messages are evicted like others. A replayed MDM is stored again — the cost of rule 1 |
| A draft's attachment is gone when it is sent | The draft holds a reference, not the megabytes; if the original was evicted, the send is refused with that reason and the draft shows it. A bookmark keeps the original |
| The editor grows into a framework | A textarea and inputs; the server parses; no syntax-highlighting editor, no editing below field level in this milestone |
| Mojibake when sending | The byte-for-byte path for unchanged messages, an explicit encoding for drafts, refusal instead of replacement, tests with Latin-1 |
| The TLS crates: build trouble on one platform, a larger binary, a wider audit surface | The package starts with the build on all three release targets, the binary size before and after, and `cargo audit`; alternatives are named in 5.10 |
| Nobody knows yet how the targets' certificates are issued (Q2) | M4-2 has a setting for each case — a CA file, the unverified switch, a certificate of Harux's own — and a failure reason that names the one that applies (5.10) |
| A response arrives for a message that is gone | `set_delivery` on a missing entry is a logged no-op |
| A slow or silent target | Timeouts on connect, handshake, write and response; cancel; the row shows `…` meanwhile |
| Two people send at once | Runs are independent connections; every row names its requester |
| A target list is lost with the browser profile | Save to and load from a file; a shared list comes with the announced rework |
| The scope grows (load tests, ACK rules, shared editing, TLS for the listener) | Section 12 |

---

## 11. Decisions

Taken by the owner on 2026-11-12. "Proposed" is what the first version of this
plan recommended; where the decision differs, the plan was rewritten.

| # | Question | Proposed | Decided |
|---|---|---|---|
| D1 | Sent messages in the shared list or in a separate outbox? | Shared list with a direction filter | **As proposed** |
| D2 | Who defines where Harux may send? | Named targets and patterns in `harux.toml` | **Every user, for now:** a list of their own, made in a small menu. It will be reworked later. `harux.toml` keeps only a switch and an optional limit (4.2, 5.7, 6) |
| D3 | May production systems be targets? | Yes, if the administrator lists them | **Yes** — it must be possible to test against them. *Ask before sending* is a mark each user sets (4.2) |
| D4 | TLS for outgoing MLLP? | Not in this milestone | **Yes, it is needed.** In the milestone as M4-2 (5.10) |
| D5 | Where do templates and drafts live? | Built-in set, a read-only directory, drafts in the browser | **As proposed** |
| D6 | Loopback target on by default? | Yes | **Yes** |
| D7 | How large may a run be? | 1,000 messages, a quarter of the store | **Set in `harux.toml`:** `send.max_run_messages`, `send.max_run_store_percent`, with those values as defaults |
| D8 | Watch without desktop notifications on plain HTTP? | Yes | **Yes** — answered as Q1 on 2026-11-13 |
| D9 | Import in this milestone? | Optional | **Yes**, M4-10 |
| D10 | One release at the end, or one per stage? | v0.10.0 after stage 3 | **One release at the end, as v1.0.0** |
| D11 | Is sending on or off after an update? (It became a question through D2.) | On, as D2 implies; off was named as the cautious choice | **Off for now**, to be looked at again later: `send.enabled = false` by default |

### Still open

| # | Question | What the plan does until it is answered |
|---|---|---|
| Q1 | **Watch (was D8).** A browser shows its own notification window only for pages opened over HTTPS or on `localhost`; for `http://server:8080` it refuses. Harux can always count in the tab's title (`(3) Harux`) and play a sound. Is that enough, or must the notification window work, which means putting the web UI behind HTTPS? | **Answered 2026-11-13: title counter and sound are enough.** M4-11 shipped that way, with the notification window where the browser allows it |
| Q2 | **The targets' certificates.** How are they issued — by a CA the Harux server already trusts, by a private CA, or self-signed — and does a target ask Harux for a certificate of its own? Asked on 2026-11-12; not known yet | Nothing has to be known in advance: M4-2 has a setting for each case and a failure reason that names the one that applies (5.10). Worth asking whoever runs Orchestra before M4-2 is tested against a real target |

---

## 12. Not in this milestone

- **TLS for the listener** (MLLP/S), that is encrypted receiving — Milestone
  6; it can use the library M4-2 brings.
- **A shared list of targets** for the team, and any management of targets
  on the server — the rework the owner announced with D2.
- **ACK rules** — answering `AE`/`AR` or nothing on purpose, to test
  Orchestra's retry. It is a testing feature and a natural next step, but it
  changes how Harux receives and needs its own design. It stays in
  Milestone 6.
- **Saving on the server:** shared templates edited in the UI, drafts for the
  team — Milestone 5.
- **Load tests** beyond the run limits, replay with the original timing.
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
| `[targets.*]` in `harux.toml`, with `default` | A list per user in the browser; the "default" is the target that browser used last | The owner's decision D2 |
| Target TLS on by default | TLS for outgoing connections, on for a new target | The owner's decision D4, as in 2026-05 |
| Health check: connect to every target every 30 s | Last result of a real send | No traffic nobody asked for; some receivers log every connection |
| CodeMirror 6 | `<textarea>`, inputs, server-side parse | No framework, no build step |
| Server-sent events for bursts | Rows over the existing WebSocket, totals by polling the run | One live channel |
| Faker data, front-matter in template files | A handful of placeholders in the text | Visible and predictable |
| Rule engine for notifications on the server | Watch in the browser | Per-user, no server state |
