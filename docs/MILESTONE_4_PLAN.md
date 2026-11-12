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

The roadmap's three functional acceptance criteria are met after stages 1, 2
and 3; the fourth, the send policy, with the first work package. Stage 1 alone
already gives the team one-click replay.

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
3. **Sending fails closed.** Harux connects only to targets and destinations
   the configuration allows. Without configuration it reaches nothing but its
   own listener.
4. **One of each.** One MLLP framing (`src/mllp.rs`), one parser, one
   validator, one charset table (`src/hl7/charset.rs`), one place that writes
   a field into a message (`src/hl7/compose.rs`). The browser never parses
   HL7 on its own: the server parses a draft, writes a changed field back
   into it, and tells the browser where each field is in the text.
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
  (`send.max_run_messages`, default 1,000) and in size: its messages together
  may take at most a quarter of `store.max_memory_mb`, because every one of
  them is stored again. A run keeps hold of the messages it was asked to
  send, so eviction during the run cannot take one away before it is sent.
  Load tests with tens of thousands of messages are not part of this
  milestone.
- A target marked `confirm` takes one message per request, never a run.
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

1. resolves the target and checks it against the policy (5.7); looks up every
   stored message the request names and keeps its `Arc`, as the export's
   `payload_snapshot` does — an unknown ID refuses the whole request, and a
   message evicted while the run is under way is still sent;
2. for each item: builds the bytes (5.4), parses them into an `Hl7Message`
   with `outbound` set and inserts it as `pending`;
3. connects if the run has no open connection (`send.connect_timeout_secs`),
   does the exchange, and records the outcome with `set_delivery`;
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

```toml
[send]
# Offer Harux's own MLLP listener as a target. With this off and no targets,
# sending is disabled altogether.
loopback_target = true
connect_timeout_secs = 5
response_timeout_secs = 30
max_run_messages = 1000
max_concurrent_runs = 4
# Destinations users may type themselves: host:port or host:from-to.
# Empty = only the named targets below can be chosen.
# allowed_destinations = ["orchestra-test.klinik.local:6000-6999"]
# Directory with .hl7 templates for everyone (read only).
# templates_dir = "templates"

# [[send.targets]]
# name = "orchestra-test-adt"
# host = "orchestra-test.klinik.local"
# port = 6661
# description = "Orchestra TEST, ADT inbound"
# confirm = false        # true: one message per request, and ask before each
```

- Fails closed like the rest (#233): unknown keys, duplicate or empty names, a
  port of 0, a malformed `allowed_destinations` entry and zero limits stop the
  start with an error that names the key. "Effective configuration" lists the
  targets.
- **An `allowed_destinations` entry always names a port or a port range.** A
  bare host would allow every port on it and turn Harux into a general TCP
  client for that machine; such an entry stops the start.
- A typed destination is allowed when its host equals an entry (names without
  case and trailing dot, IP addresses in canonical form) and its port lies in
  the entry's range. Nothing is resolved for the check. Named targets are
  allowed because the administrator wrote them.
- **A typed destination that equals a named target is that target.** Its
  `confirm` setting applies; typing the address is not a way around it.
- **`confirm = true`** means: one message per request, never a run, and the
  request must carry `confirmed`, which the UI sets after a dialog that names
  the target. It guards against a slip, not against intent — the API is open
  to everyone who can reach it, as the rest of Harux is.
- The loopback target is the address the MLLP listener is bound to and
  `server.mllp_port`; when the listener is bound to every interface it is
  `127.0.0.1` for `0.0.0.0` and `::1` for `::`.
- The largest message that can be sent is `mllp.max_message_size_mb`, the
  largest that can be received. A run is limited by `max_run_messages` and by
  a quarter of `store.max_memory_mb` (5.3).
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
  "target": "orchestra-test-adt",            // or {"host": "…", "port": 6662}
  "items": [{ "message_id": "…" }],          // or [{ "raw": "MSH|…", "charset": "auto",
                                             //        "based_on": "<message id>", "template": "<name>" }]
  "set": { "MSH-10": "{{id}}" },             // optional field overrides, for every item
  "repeat": 1,                               // each item this many times
  "interval_ms": 0,
  "stop_on_nack": false,
  "await_response": true,                    // false: do not wait for an ACK
  "allow_invalid": false,                    // send text that does not parse
  "confirmed": false                         // required for targets with confirm = true
}
```

- **One message:** the request waits for the outcome and answers `200` with
  the stored ID and the delivery. The exchange itself runs in the run's task,
  so a browser that gives up does not stop a send half-way. A failed delivery
  (refused, timeout, NACK) is a `200` too: the request worked, the send did
  not.
- **Several messages:** `202` with the run's ID; the rows report progress over
  the WebSocket, the totals come from `GET /api/send/runs/{id}`.
- **Refusals:** `400` malformed request or field path · `403` sending off or
  destination not allowed · `404` unknown target or message · `409` target
  needs `confirmed`, or a run to a `confirm` target · `413` message or run too
  large · `422` what 5.4 refuses, text that does not parse (without
  `allow_invalid`), a `truncated` entry, a reference whose message is gone ·
  `429` too many runs or too many large requests at once.
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

- `send.js` — targets, the target menu, resend, runs and their bar, toasts.
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

---

## 6. Safety and security

Sending is the first feature with which a user of the web UI can change
something outside Harux. Harux has no login, so the limits must be in the
configuration.

| Risk | Answer |
|---|---|
| A test message reaches a **production system** | Only configured targets and allowed destinations can be reached; nothing is configured by default. A target can require confirmation (`confirm = true`): one message per request, never a run, and a typed address that equals it is treated as that target. It is a guard against a slip, not against intent. The button always names the target. |
| Harux as a **stepping stone** into the network (connect to any host and port, read what answers) | The same allowlist; every typed-destination pattern must name a port or a range. No background connections: there is no health check that probes targets; the last result of a real send is shown instead. |
| A **second message hidden inside a send** (the MLLP end byte in a draft or an override) | A payload Harux builds must not contain the start or end byte; an override cannot leave its field (5.4). What goes out as one frame is one row. |
| A **response that is too large**, written by the target | Read up to 1 MB to find its end, stored up to 64 KB, never part of the list rows or the WebSocket events (5.1). |
| A **template ID that is a path** (`..`, an encoded `/`) | An ID is looked up in the listing, never joined into a path (5.6). |
| **Many large requests at once** (previews, sends, imports) | A permit is taken before a body is read; the import is streamed (5.8). |
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
M4-1 ─► M4-2 ─► M4-3 ─► M4-4 ─► M4-5 ─► M4-6 ─► M4-7 ─► M4-8 (v0.10.0)
          │                       └───► M4-9 ─► M4-10          (v0.10.x)
          └───► M4-11                                          (v0.10.x)
```

### Stage 1 — Resend

**M4-1 · Send a stored message, unchanged** (L, backend and the minimum of UI)
`[send]` configuration and policy, including the loopback target;
`mllp_client.rs`; `send.rs` with a run of one stored message; `Outbound`,
`Delivery`, `truncated`, `set_delivery`, `delivery_updated`; accounting;
`GET /api/send/targets`; `POST /api/send` for one stored message, unchanged;
`ConnectInfo`; export direction; `is:sent` in `SearchQuery`; send counters;
log lines; `SECURITY.md`, `AGENTS.md`, `docs/PROJECT_OVERVIEW.md`, example
configuration. The list and the detail view show a sent message as sent
(direction, target, the chip from `delivery`, the Response tab), so `main`
never presents a sent message as a received one.
*Done when* `curl -X POST /api/send` resends a stored message to a configured
target, the sent message and its response appear in the UI as sent, and a
destination that is not allowed is refused.

**M4-2 · Resend in the UI** (M, frontend)
Target menu and split button; direction filter and `is:` prefixes; target
chips; `sent` pill; origin links; trace by control ID; tools row.
*Done when* the roadmap's first criterion holds: a received message is resent
to a configured target with one click, and the response is shown.

**M4-3 · Changes on resend** (M)
`src/hl7/compose.rs`: field overrides, placeholders, everything 5.4 refuses;
the encoding step in `charset.rs`; `set` in `POST /api/send`; the menu's
"send as" choices; the fuzz test for this code.
*Done when* a stored message is resent with a new control ID and the current
time and every other byte is unchanged, and a value that carries the MLLP end
byte or a field separator is refused.

### Stage 2 — Drafts

**M4-4 · Drafts on the server** (M, backend)
`GET /api/messages/{id}/draft` with references; `POST /api/parse` (preview,
field write-back, field positions, decoding of a file); `POST /api/build`;
draft items in `POST /api/send` (line ends, encoding, references,
`allow_invalid`); the body limits and the permit middleware; `datatype` in the
detail JSON.
*Done when*, with `curl` alone, a draft can be made from a stored message,
previewed, changed with `set`, downloaded and sent — a Latin-1 message with
umlauts and an MDM whose attachment is a reference.

**M4-5 · Drafts in the UI** (L, frontend)
`compose.js`: the drafts rail, the draft header, the Raw tab with the field
status line, a read-only Segments preview with live validation, the Response
tab with the draft's history, Diff vs pinned, reference chips, tab storage;
the selection model and the three traps of 5.9; *Edit* on stored messages;
*New message* (blank, from the clipboard, from a file).
*Done when* the roadmap's third criterion holds: raw HL7 is edited and sent
from the browser, for the two messages of M4-4 as well.

**M4-6 · Edit fields** (M)
Inputs in the Segments tab, written back through `POST /api/parse`; add,
duplicate and remove segments; `GET /api/dictionary/{segment}` and *show
empty fields*; quick actions for MSH-7 and MSH-10.
*Done when* a message can be changed field by field without touching the Raw
tab, and both tabs always show the same text.

### Stage 3 — Templates

**M4-7 · Templates** (S–M)
Built-in set, `send.templates_dir`, the two routes, the *New message* menu,
*Save as file*, *Keep on this computer*.
*Done when* the roadmap's second criterion holds: a template is filled out,
sent, and the response displayed.

### Release

**M4-8 · Release v0.10.0 "Workflow & Testing"** (S), after stage 3
`README.md` features, `docs/ROADMAP.md`, the full API and configuration tables
in `docs/PROJECT_OVERVIEW.md`, upgrade notes, version. Stages 4 and 5 follow
as v0.10.x. `main` is usable after every package, so the team does not have to
wait for the tag.

### Stage 4 — Several messages

**M4-9 · Runs** (M)
Several items and `repeat` in `POST /api/send`; the size limit and the hold
on the run's messages; run status and cancel; the run bar; *Resend n
filtered*; *Repeat* on a draft (which is why it follows M4-5).
*Done when* fifty filtered messages are resent in their original order over
one connection, with progress, can be cancelled, and none is lost to eviction
while the run is under way.

**M4-10 · Import** (S–M, optional)
The frame splitter moved out of `handle_connection`; `POST /api/import`,
streamed; *From a file* for framed files.
*Done when* a file written by `/api/export` can be loaded and resent, and the
listener's tests in `tests/mllp_tcp.rs` pass unchanged.

### Stage 5 — Watch

**M4-11 · Watch** (S, frontend only)
The bell, title counter, sound, desktop notification where the browser allows
it. Independent of the other packages once M4-2 is in.

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
| Round trip | Send to Harux's own `mllp::serve`: the sent message is stored before the received one; code `AA`; MSA-2 matches | `tests/send_tcp.rs` |
| Bytes | Unchanged resend equals the received bytes for UTF-8, Latin-1 and an irreproducible payload; an override keeps every other byte; a pure-ASCII message declared `8859/1` leaves as Latin-1 after an override with `ü`; unmappable character refused | unit tests, `tests/send_tcp.rs` |
| Compose | Field paths incl. MSH shift, custom delimiters, occurrences, new fields; placeholders and references; line ends. Refused: the start or end byte, a separator or line break in a value, field 0, MSH-1 and MSH-2, a field number above the limit, a changed list of segments | unit tests in `src/hl7/compose.rs` |
| Fuzz | Mutated responses through the response reader, mutated text through override and placeholder code: no panic, an override is found again by the parser, and the built payload never contains a frame byte | `tests/fuzz_ingest.rs` or a sibling |
| Policy and config | Allowed and refused destinations, ranges, names vs. addresses, an entry without a port; a typed address equal to a `confirm` target; a run to a `confirm` target; every invalid `[send]` value stops the start | `src/config.rs`, `tests/config_startup.rs`, `tests/web_api.rs` |
| Store | `delivery_updated` order and frame; no response text in a summary; a response above 64 KB is cut; bytes exact after a response, and it wakes eviction; eviction and clear with pending messages; a run still sends a message evicted after it began | `src/store.rs`, `tests/alloc_accounting.rs`, `tests/send_tcp.rs` |
| API | Status codes of 5.8; origin guard on the new routes; `/api/parse` equals the detail JSON, and its positions point at the fields; a template ID with `..` or an encoded `/`; `429` from the permit; export direction | `tests/web_api.rs` (`MockConnectInfo` for the client address) |
| Templates | Every built-in template parses without a warning once its placeholders are filled in | unit test |
| Frontend | Sent rows and the skeleton; `delivery_updated` while a list loads and before the flush; no second row after a send; counters; direction filter and `canPrependOnly`; an open draft survives `cleared`, `evicted` and a reload; a space typed into a draft input; draft storage that fails; the watch matcher | `tests/js/*.test.mjs` |

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
| Sent messages crowd the list and the store | Direction filter, remembered per session; a run is limited in number and in size; sent messages are evicted like others. A replayed MDM is stored again — the cost of rule 1 |
| A draft's attachment is gone when it is sent | The draft holds a reference, not the megabytes; if the original was evicted, the send is refused with that reason and the draft shows it. A bookmark keeps the original |
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
| D3 | May production systems be targets? | **Yes, if the administrator lists them**, with `confirm = true`: one message per request, never a run, and a question before each. Resending a lost message is a real task; forbidding it would move it to another tool |
| D4 | TLS for outgoing MLLP? The 2026-05 notes say "yes, by default" | **Not in this milestone.** It needs a TLS crate (`tokio-rustls`) and certificate settings, and belongs with MLLP/S for the listener (Milestone 6). To decide: does any target in use require TLS today? |
| D5 | Where do templates and drafts live? | **Built-in set, a read-only directory, drafts in the browser.** Nothing is saved on the server before Milestone 5 |
| D6 | Is the loopback target on by default? | **Yes.** It reaches only Harux's own listener and makes the feature usable and testable without configuration |
| D7 | How large may a run be? | **1,000 messages** by default and at most a quarter of `store.max_memory_mb`, every one stored. Larger load tests are a later feature |
| D8 | Watch without desktop notifications on plain HTTP — acceptable? | **Yes**: title counter and sound everywhere, desktop notification behind HTTPS or on `localhost` |
| D9 | Import (M4-10) in this milestone? | **Optional**, last. Useful until persistence exists; it moves the frame splitter out of the listener's connection loop, which is the reason to decide it separately |
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
| Faker data, front-matter in template files | A handful of placeholders in the text | Visible and predictable |
| Rule engine for notifications on the server | Watch in the browser | Per-user, no server state |
| Target TLS on by default | Not in this milestone | New crate; open decision D4 |
