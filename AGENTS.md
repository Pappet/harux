# AGENTS.md

The project guide for AI coding agents (Claude Code, Codex, Gemini, Jules, …)
and for humans who want the same overview. **This file is the single source of
truth for agent rules.** `CLAUDE.md` and `GEMINI.md` only point here and add
what is specific to their tool; `.jules/` holds historical learning notes.

Volatile planning facts (current milestone, next work) live in
[`docs/ROADMAP.md`](docs/ROADMAP.md), not here.

---

## Project Overview

Harux is a single-binary MLLP server with an embedded real-time web UI for
inspecting HL7 v2.x messages. It is written in Rust and runs as a Windows
Service on a hospital integration server; several developers use it at the
same time through their browsers.

**Primary use case:** receiving, storing and displaying HL7 v2.x messages from
Orchestra (a healthcare integration platform) for development and debugging.
MDM messages with Base64-encoded attachments are common daily traffic.

**Scope today:** receive and inspect, plus — off by default — resending a
stored message (unchanged or with fields changed), over plain MLLP or TLS,
and writing or editing a message as a draft, also field by field or from a
template, and sending it — also several messages in one run: the filtered
list, or a draft repeated — and loading an exported `.hl7` file back into the
list — and, for everyone, a watch that tells a background tab about new
messages matching a filter (Milestone 4, released as v1.0.0). Persistence
(Milestone 5) is planned —
see the roadmap and `docs/MILESTONE_4_PLAN.md`. Do not implement roadmap
features unless explicitly asked.

---

## Git Workflow — Branch + PR Required

**Never commit directly to `main` or to an unrelated branch.** For every task:

1. **Create a feature branch** from `main` named after the issue (e.g. `feat/27-bookmark-messages`, `fix/15-eviction-bug`).
2. **Make all commits on that branch.**
3. **Open a Pull Request** to `main` when the work is complete and all checks pass.
4. Include the related issue number in the PR body (e.g. `Closes #27`).

Do not push changes without a dedicated branch and PR — even for small fixes.

## Build & Run

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo run --release            # Run server (MLLP + Web)
cargo test                     # All Rust tests (unit + integration)
cargo test <test_name>         # A single test
cargo clippy -- -D warnings    # Must pass clean before every commit
node --test 'tests/js/*.test.mjs'   # Frontend logic tests (no npm)
```

Environment variables: `MLLP_PORT` (default 2575), `WEB_PORT` (default 8080),
`RUST_LOG` (log level, overrides `logging.level`). Everything else is set in
`harux.toml` — see `examples/harux-example.toml` for every key with its
default. The configuration fails closed: an unknown key, a wrong type or a
value out of range stops the start with an error naming the key (#233).

Manual MLLP testing: `tests/test.sh` (Linux/macOS, needs `nc`) or
`.\tests\test.ps1` (Windows) — sends ADT^A01, ORU^R01, SIU^S12 plus error
cases and a load test.

## Branch Protection & CI

`main` is protected; every change goes through a Pull Request. The CI workflow
(`.github/workflows/ci.yml`) runs, and all of these **must pass**:

1. `cargo fmt --check`
2. `cargo clippy -- -D warnings`
3. `cargo build`
4. `cargo test`
5. `node --test 'tests/js/*.test.mjs'` — frontend logic (e.g. the diff pairing) loaded into `node:vm` with stubbed globals; no npm, no dependencies

A separate workflow (`.github/workflows/audit.yml`) runs `cargo audit` when
dependencies change and daily; `.cargo/audit.toml` lists ignored advisories,
each with a reason. Dependabot proposes weekly crate and Actions updates.

Run these locally before pushing. A task whose build, tests or clippy fail is
not done.

## Commit & Changelog Workflow

**Before every commit and push**, update `CHANGELOG.md` in the same commit:

1. For substantive changes (feature, fix, refactor): add or update the `### Added`, `### Fixed`, `### Changed` (or `### Security` / `### Upgrade Notes`) entry in the `[Unreleased]` section, with the issue number (e.g. `(#178)`).
2. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
3. There is **no per-commit hash table** — PRs are squash-merged, so branch commit hashes never survive on `main`. Git history is the commit log.

Never create a standalone "update changelog" commit — the entry belongs to the commit it documents.

Commit messages use a type prefix: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `ci:`, `chore:`.

---

## Architecture

Harux is an MLLP server with a real-time web UI for inspecting HL7 v2.x messages. Two async Tokio tasks run concurrently via `tokio::select!` in `main.rs`:

1. **MLLP Server** (`mllp.rs`) — TCP listener accepting HL7 messages wrapped in MLLP framing (VT `0x0B` start, FS `0x1C` + CR `0x0D` end). Parses each message, stores it, and returns ACK/NACK.

2. **Web Server** (`web.rs`) — Axum HTTP server serving a REST API and WebSocket endpoint. The SPA frontend (`static/`) is embedded in the binary via `rust-embed`.

**Shared state** flows through `MessageStore` (`store.rs`) — an `Arc<RwLock<>>` in-memory store with a `tokio::sync::broadcast` channel that pushes new messages to WebSocket subscribers in real-time. Every `StoreEvent` is sent **while the write lock is held** (`insert`, `mutate_meta`, `evict_now`, `clear`), so the order on the channel is the order in which the store changed (#232) — don't move a `tx.send` after the lock is released. The channel carries `SharedEvent` = `Arc<EventFrame>`: the event plus its WebSocket JSON, rendered once by the first subscriber through `EventFrame::cached_json(ws_event_json)` and reused by every other connected browser (#196). The wire format lives in `ws_event_json` (`src/web.rs`) only.

### Store Capacity & Eviction

Limits are **config-driven** via `StoreConfig` (`src/config.rs`), not hardcoded constants.

- **Count limit:** `store.max_messages` (default `10_000`) — secondary safeguard
- **Size limit:** `store.max_memory_mb` (default `512` MB) — primary safeguard
- **Trigger:** either limit hit → background eviction task evicts the oldest messages until **both** bytes and count are at or below 90% of their limits (low watermark, byte-targeted — not a fixed share of the count)
- **Bookmark protection:** bookmarked messages are skipped during eviction (popped and pushed back to the front in original order) — but only within `store.bookmark_budget_percent` (default 50%, clamped to max 80%) of either limit. Under eviction pressure, bookmarks beyond that budget are evicted oldest-first, so bookmarking can never lift the memory cap. Bookmark count/bytes are tracked incrementally, so checking the budget is O(1). See `StoreInner::evict` in `src/store.rs`
- **Accounting (#226):** each entry is accounted as `entry_bytes` = `Hl7Message::estimated_bytes()` (every owned buffer by **capacity** plus allocator overhead via `heap_bytes`, including the 56-byte `Hl7Field` structs) + `MessageMeta::estimated_bytes()` + `entry_overhead` (the `Arc`, ID copies, `order` and map slots). `tests/alloc_accounting.rs` (own counting `#[global_allocator]`) fails if the estimate drops below the modeled heap of a real message or exceeds it by more than 5 % — when adding a heap-owning field to `Hl7Message`, count it in `estimated_bytes`. `heap_bytes` models glibc; on Windows it over-counts short strings (safe side). Frames with more than `MAX_FIELDS_PER_MESSAGE` (250,000) fields are stored unparsed. Process RSS can exceed the limit by the transient parse tree of frames in flight (≤ ~22 MB each) plus memory the allocator keeps after eviction. `/api/stats` exposes `store_bytes` / `store_max_bytes`
- **Payload vs. metadata:** each message is stored as a `StoredEntry` — the immutable `Arc<Hl7Message>` payload, its mutable `MessageMeta` (tags, bookmark) and its cached accounted `bytes`. Never mutate the payload after insert (no `Arc::make_mut`): tag/bookmark changes go through `mutate_meta`, which touches only the metadata, so a held snapshot never forces a deep clone under the write lock (#192). `get_by_id` returns a `MessageSnapshot` that serializes flat (payload fields + `tags`/`bookmarked`)
- **UI sync:** every eviction pass broadcasts `StoreEvent::Evicted(ids)` → WS `evicted`; `removeEvictedMessages` in `static/ws.js` removes the rows and undoes their counter contributions
- **Reason:** MDM messages with Base64-encoded attachments can be several MB each; count-only eviction is insufficient for real-world Orchestra traffic

### HL7 Parsing (`src/hl7/`)

- `parser.rs` — Parses raw HL7 text by extracting delimiters from the MSH segment, splitting on `\r`/`\n` into segments and segments into fields. Also builds ACK responses. Delimiters must be printable, distinct and not letters/digits; only leading whitespace is skipped, so every non-blank line of `raw` rebuilds its segment exactly — `tests/fuzz_ingest.rs` checks this, the ACK echo and "no panic" on mutated fixtures (#243).
- **Field structure (#185):** repetitions (`~`), components (`^`), sub-components (`&`) and escapes (`\F\`, `\.br\`, …) are **derived from `Hl7Field::value` on demand**, using the message's own `Delimiters` (`Hl7Message::delimiters`, also copied into each `Hl7Segment`): `field.repetitions(d)`, `field.leaf(d, rep, comp, sub)` (escape-decoded, `Cow::Borrowed` when nothing to decode), `field.structure(d)`. Nothing structural is stored. The detail JSON gets each structured field's `repetitions` (`[rep][comp][sub]`, decoded) from `Hl7Segment`'s hand-written `Serialize`, plus the message's `delimiters` (`"|^~\\&"`); MSH-1/MSH-2 are never structured. Server-side readers (patient ID/name, datatype validation) use the first repetition / each repetition via these accessors — never `value.split('^')`.
- `escape.rs` — HL7 escape sequences in both directions: `encode` (ACK MSA-3 text) and `decode` (display/analysis only; `value` and `raw` always stay as received).
- `types.rs` — Data structures: `Hl7Message`, `Hl7MessageSummary`, `Hl7Segment`, `Hl7Field`, `Delimiters`.
- `charset.rs` — `decode_payload` turns an MLLP payload's bytes into text before parsing. It reads MSH-18 from the bytes with the parser's own `parse_delimiters` (byte-based) and `msh_split_index`, so there is only one definition of the MSH layout; don't add a second MSH scanner (#188). If the declared charset doesn't fit the bytes, it falls back (Latin-1 declared + UTF-8 bytes → UTF-8; invalid UTF-8 → Windows-1252) and adds a `CHARSET_FALLBACK` validation warning to the message instead of producing silent mojibake.
- **No duplicate payload copies (#180):** a message's text lives in `Hl7Message::raw` and, per field, in `Hl7Field::value` — nothing else. Segments carry no text of their own (the UI derives segment lines from `raw`, split on `\r`/`\n` skipping blank lines — `segmentLines()` in `static/app.js`, guarded by `test_segments_match_nonblank_raw_lines`); `Hl7Field` stores only `value` — its structure is derived on demand (#185), so even an ED attachment (`^application^pdf^Base64^…`) isn't held twice; dictionary descriptions are `Cow::Borrowed` from the static dictionary, and `typical_segments` / `typical_segment_descriptions` borrow from the static registry. Don't reintroduce per-field/per-segment copies — for MDM payloads each copy is another full Base64 attachment in RAM. The received bytes (#230) are not stored again either: `WireForm` records how to reproduce them from `raw` (UTF-8 as is, other charsets re-encoded); only a payload no encoding reproduces keeps `WireForm::Original`. `raw` is the decoded text untrimmed, including the final segment terminator.

**⚠ MSH field indexing quirk:** MSH-1 is the field separator character itself (`|`). The parser inserts a synthetic `Hl7Field { index: 1, value: "|" }` and shifts all other fields up by 1, so that `get_field_value(msh, 3)` correctly returns Sending Application (HL7 standard). Do not change this logic without understanding it — it will silently break all MSH field extractions. The coupling is guarded by `test_msh_shift_dictionary_alignment` (`src/hl7/parser.rs`) and `test_msh_dictionary_numbering_contract` (`src/dictionary.rs`); if either fails, fix the shift/dictionary mismatch rather than the test.

### MLLP Protocol

- Start: `0x0B` (VT, Vertical Tab)
- End: `0x1C 0x0D` (FS + CR)
- Max payload: `mllp.max_message_size_mb` (10 MB) — an over-limit frame is answered with `AR` (`build_reject`), stored as a parse-error entry holding only its first 4 KB, and the rest of the frame is skipped; the connection stays open (#223)
- **Never lose a frame silently:** decode/parse run under `catch_unwind` (`ingest` in `src/mllp.rs`) — a panic becomes a stored parse error + NACK (#235). Messages are stored *before* the ACK/NACK is written. `active_connections` is maintained by `ConnectionGuard` (drop guard); shutdown waits via `mllp::wait_for_drain` on the count, not on a single `Notify` wake-up (#241). An `accept()` error is logged and retried, never fatal (#224)
- Covered end to end by `tests/mllp_tcp.rs` (real TCP, ephemeral port via `mllp::serve`)
- **One frame splitter (#281):** `FrameSplitter` (`src/mllp.rs`) finds frames in bytes that arrive in pieces and cuts over-limit ones (`Frame::Oversize`, the rest skipped). `handle_connection` and the import (`Importer`) share it — don't write a second one. `FrameSplitter::ignoring_noise` (the import) drops and counts bytes outside frames instead of turning them into an over-limit entry as the listener does
- Read timeout: 60s, Write timeout: 30s
- **ACK/NACK:** `build_ack` / `build_nack` / `build_reject` are thin wrappers around `render_ack` (`src/hl7/parser.rs`) — change the MSH/MSA/ERR layout there only. ACKs reuse the sender's delimiters (echoed values stay valid) and get a fresh MSH-10 and an MSH-7 in **local time** without offset (`parser::hl7_timestamp`, the same helper as `{{now}}` — never UTC, #288); NACKs (unparseable frames) use default delimiters and MSA-2 `UNKNOWN`. A NACK or reject carries the reason in MSA-3 (cut to 80 characters, the v2.5 length) **and** an ERR segment: ERR-3 from HL7 table 0357 (`100` Segment sequence error for parse errors, `207` Application internal error for a parser panic or an oversized frame — `AckError`), ERR-4 `E`, ERR-8 the reason (cut to 250, marker `...`). All text is escaped (#218). Replies are written in the sender's charset (`reply_bytes` in `src/mllp.rs`, from `WireForm`) and declare it in MSH-18 unless the payload needed a `CHARSET_FALLBACK` (`reply_charset`); text Harux writes itself into a reply must stay ASCII so it fits any charset
- **ACK storm prevention:** incoming messages with `message_type.starts_with("ACK")` are stored but never ACK'd back. Violating this rule creates an infinite ACK ping-pong with Orchestra.

### Sending (`src/send.rs`, `src/mllp_client.rs`)

Off unless `send.enabled = true` (#272): every send route answers `403` and the UI shows nothing new. `POST /api/send` resends one stored message to the destination the request names; the server keeps no list of targets (each user keeps theirs in the browser). The rules:

1. **Everything Harux sends, or tries to send, is stored — before it connects** (`MessageStore::insert_sent`, delivery `pending`). A refused or timed-out send is a row too, and whatever the destination forwards because of the send is listed after it. Don't move the insert after the connect.
2. **Unchanged means byte for byte:** an unchanged resend writes `wire_bytes()` (#230), borrowed from the stored sent message (`Arc`), never a re-encoding of `raw`.
3. **Every send ends with an outcome** via `set_delivery` (`mutate_meta`, broadcast `delivery_updated` under the lock, #232): a response, a transport failure, shutdown, or a panic in the exchange task. No row stays `pending`.
4. **The payload stays immutable** (#192): the outcome and the response live in `MessageMeta::delivery`, accounted like tags (`set_delivery` wakes eviction — a response can push the store over its limit). `ack_code` / `ack_response` keep meaning "what Harux wrote back" and stay empty on a sent message.
5. **No response text in summaries:** `Hl7MessageSummary` gets `direction`, `target`, `origin_id`, a small `delivery` — the response (cut at 64 KB) is in the detail JSON only.
6. **A destination is never shown as a sender:** the UI keys sent messages by `sourceKeyOf(msg)` (`→ target`), not by `source_addr` — rail, counters, bar colour and highlight. `rate` / `last` count received messages only.
7. **No store lock across network I/O**; the exchange runs in its own task, limited by `send.max_concurrent_runs`. Log lines carry metadata only, never content.
8. **TLS for outgoing connections** (`src/tls.rs`, #273): per target (`tls`, `verify` in the request). `TlsContext` is built once at start from `[send.tls]` (OS store via `rustls-native-certs`, `ca_file`, client certificate) — a bad file stops the start. The handshake counts into `send.connect_timeout_secs`. Every TLS failure becomes a reason naming the setting that answers it (`mllp_client::TlsFailure`); with TLS 1.3 a refused client certificate surfaces only while reading, which `after_handshake` re-reads. `verify: false` only with `send.tls.allow_unverified`, recorded as `Transport::TlsUnverified`. Never log, return or `Debug`-print key material. The crypto provider is `ring` (`default-features = false` on `tokio-rustls`) — don't let `aws-lc-rs` in, it needs CMake/NASM on Windows.
9. **Overrides (`set`, #275) are written only by `src/hl7/compose.rs`** (`FieldPath`, `set_field`, `fill_placeholders`) — the one place that writes a field into a message; the draft editor (M4-5) uses it too. Only the one field's text changes; segments are found as the parser finds them and the MSH shift goes through `msh_split_index`. A value may not contain the field separator, `CR`, `LF` or an MLLP frame byte; MSH-1/MSH-2 and fields 0 or > 1000 are refused; the list of segments must stay the same, compared with the parser's own segment list. `send::changed_payload` encodes the result like the original (`charset::encode_for_send`: `WireForm`, a pure-ASCII original per MSH-18; a changed MSH-18 takes the new declaration's encoding; an unmappable character is an error naming it, never `?`), checks that `decode_payload` reads the bytes back as the same text without a new `CHARSET_FALLBACK`, refuses a frame byte in the bytes, and the sent message is then built with `mllp::ingest` like a received frame. Only an unchanged resend may use `WireForm::Original`. `{{id}}` is `parser::new_control_id`, shared with `render_ack`.
10. `allowed_destinations` compares normalized hosts (`config::normalize_host`) and never resolves names. An over-limit entry (`truncated`, #223) cannot be resent; a parse error only with `allow_invalid`. `POST /api/send` refuses unknown fields so an unsupported option never silently sends the message unchanged.
11. **Drafts (`src/draft.rs`, #276)** are text in the browser; the server parses, previews and builds them and stores nothing until a send. A draft from a stored message holds long values (> `DRAFT_VALUE_MAX`, 2000 characters, = `DISPLAY_VALUE_MAX`) as references `{{from:SEG[k]-n}}`, filled in from `based_on` only when the draft is sent, built or previewed with `fill_references` — never send the attachment to the browser. A reference is written only if `FieldPath` reads it back as the same field; `draft_of` collects all of them in one pass and rebuilds the text once (linear in the message, #289 review). Every literal `{{` of the stored text is written `{{open}}`, so text that only looks like a placeholder or reference goes out as it came in. Building a draft (`build_payload`) fills placeholders, references and `{{open}}` in **one pass** (`draft::fill` — what is put in is never read again), normalizes line ends to `CR` with a final `CR`, and encodes per MSH-18 or the named encoding — refusing what the encoding cannot write and MLLP frame bytes, like an override. The sent message is built with `mllp::ingest`; a draft that does not parse needs `allow_invalid`. `/api/send`, `/api/parse` and `/api/build` have the body limit `web::message_body_limit` (2 × `mllp.max_message_size_mb` + 1 MB) and take a permit (`Sender::large_body_permit`, `LARGE_BODY_PERMITS`) before a body of `LARGE_BODY_BYTES` or more, or of unknown size, is read (`429` with `Retry-After`) — keep new routes that carry a message behind `large_body` in `create_router`. `/api/parse` treats a body as JSON if its `Content-Type` says so or it starts with `{`; anything else is a file. **Work that grows with a draft runs off the async workers** (#301): decoding a file, the preview, `apply_set`, the build and the answer's JSON go through `off_workers` (`src/web.rs`, a `spawn_blocking`), as `draft_of` does — on a worker they hold up the listener's ACKs (a filled preview of an 8 MB MDM: up to 130 ms). Keep new draft work there.
12. **Runs (#280):** one request, several messages — the items in order, `repeat` times (`A B C A B C`), each copy built anew (`{{n}}` = its position) — over **one connection** (`mllp_client::Connection`). `Sender::start` refuses everything it can before the first message (an unknown message refuses the whole run; `send.max_run_messages`; the run's estimated bytes against `max_run_bytes`); then the run is a task holding the source messages' `Arc`s, so eviction or *Clear* cannot take one away mid-run. **The limits come before the memory is spent (#298):** first a lower bound of the run's bytes that needs no build (`least_bytes`: a stored message's size, minus the fields `set` replaces) against `max_run_bytes`, then the run permit, and only then `prepare` builds the first copy of each item in `spawn_blocking` as the check — measured and dropped at once, never all held — and the exact size is compared. **Every copy is built when it is sent** (`spawn_blocking`), so `{{now}}` is the time of its send (#305); only a run of one message sends the copy it was checked with. Either way `{{now}}` is the moment the copy is built — before it is stored and connected (rule 1), so a slow connect delays every copy alike; the one message's check build is only milliseconds earlier than a second build would be, so don't add one. The `413` of the lower bound says "at least N MB". Before a message is written on a kept connection, `Connection::is_reusable` peeks without waiting; a message the target never read (`ExchangeError::peer_hung_up`: closed or reset without a byte of an answer, or a write into a closed connection) is written **once** more on a new connection — receivers that close after each ACK are common. Any other transport failure drops the connection; three in a row end the run (`GaveUp`). A connection is kept only after an answer: after a message written without awaiting one (`await_response: false`, an ACK) a late answer would be read as the next message's (#293 review), so that connection is closed. Cancel (`watch` per run) stops before the next message, never mid-exchange. Messages never tried are not stored. Status lives in `RunHandle` (a std `Mutex`, never held across `.await`) in a `Registry`: every run under way by ID, and the newest `KEPT_RUNS` finished (`retire` moves a run there when it ends, O(1)). One message stays a run too: `start` awaits it and answers `200`; several answer `202` with the status. `Sender::send` is the one-message form for library callers.

### Configuration & Logging

- `src/config.rs` loads built-in defaults → `harux.toml` (next to the binary, then the working directory) → environment variables. Every section has `deny_unknown_fields`; `Config::validate` checks ranges, overlapping listener addresses and `logging.file`. Add new keys to `examples/harux-example.toml` and `harux.toml`, both of which `shipped_config_files_load` parses.
- Listeners bind `server.mllp_bind` / `server.web_bind` (default `0.0.0.0`; #237). Bind errors name the address and the keys.
- File logging (`src/logging.rs`): one file per day, `<logging.file>.YYYY-MM-DD`; if it can't be opened, Harux logs to stdout and says so (#234). `logging.max_files` (default 14, `0` = keep all) prunes only files named exactly that way, at startup and hourly (#240) — never use `tracing-appender`'s `max_log_files`, which deletes every file that starts with the log name.
- Shutdown: Ctrl+C and SIGTERM (Unix) trigger the graceful shutdown; MLLP connections drain for up to `server.shutdown_timeout_secs`.

### Web API Routes

**Cross-site guard (#227):** every request passes `guard_cross_site` (`src/web.rs`): `Host` must be an IP literal, `localhost` or a name in `server.allowed_hosts`; on `/ws` and all non-GET routes an `Origin` header, if present, must name this server. `allowed_hosts` entries are names only (no port/scheme; a trailing dot is ignored) — bad entries are warned about at startup. Keep new state-changing routes non-GET so they stay covered. Tests that build requests with a `Host` header must use an allowed one.

- `GET /api/messages?offset=&limit=&include_bookmarks=&before=` — Paginated message list (newest first; `limit` capped at 1000). `include_bookmarks=true` (also `1` / `yes`) adds every bookmarked message older than the page, in the same order — the UI loads with it so bookmarks survive a reload (#231). `before=<id>` pages through older history by cursor instead of `offset` (offsets shift as messages arrive): messages older than that one; an unknown ID gives `[]` (#198)
- `GET /api/messages/{id}` — Full message with segments
- `GET /api/search?q=&limit=&before=` — Server-side search with the UI filter's semantics (`SearchQuery` in `src/store.rs` mirrors `getParsedQuery` / `matchesSearch`: `has:warnings` / `has:errors`, then patient, type, facility, control ID, source, tags; Unicode case-insensitive). `before=<id>` searches only older messages (#197). Keep the two matchers in step
- `GET /api/stats` — Server statistics
- `POST /api/messages/{id}/tags` — Add a tag (JSON body `{"tag": "..."}`)
- `DELETE /api/messages/{id}/tags/{tag}` — Remove a tag
- `POST /api/messages/{id}/bookmark` — Toggle bookmark
- `POST /api/clear` — Clear all messages
- `GET /api/send/config`, `POST /api/send` — Sending (#272), see *Sending* above; `403` while `send.enabled` is off. An item is a stored message (`message_id`) or a draft (`raw`, `charset`, `based_on`, #276); `repeat`, `interval_ms`, `stop_on_nack` make a run (#280): one message `200` with its outcome, several `202` with the run's status
- `GET /api/send/runs/{id}`, `POST /api/send/runs/{id}/cancel` — A run's progress and result, and stopping it before its next message (#280); `404` for a run the server does not keep
- `GET /api/templates`, `GET /api/templates/{id}` — Templates (#279, `src/templates.rs`): the built-in set (`src/assets/templates/`, each must validate cleanly once filled in — `built_in_templates_validate_once_filled_in`) and the site's `send.templates_dir`, read on every request, limited (`MAX_SITE_TEMPLATES`, `MAX_TEMPLATE_BYTES`), never written. **An ID is a key of the listing, looked up in it and never joined into a path**; only regular files (no links) directly in the directory named `[A-Za-z0-9._-]+.hl7` are listed. `403` while sending is off. A draft item's `template` becomes `Origin::Template { name }`
- `POST /api/import?name=` — Import (#281): the body is an MLLP-framed file (what `/api/export` writes), read as a **stream** through `FrameSplitter` (`mllp::Importer`) — nothing beyond the frame in work is held; every frame stored as received from `import:<file name>` (no reply, `MllpStats` untouched), an over-limit one cut as by the listener. At most `store.max_memory_mb` bytes **read** (`413`, checked before a piece is stored) — the file's size, whitespace and noise included, not what is stored; a file without a frame, an empty one included, `422`. An import is stored like any arrival: into a full store it evicts the oldest messages (the UI's confirmation says so, `importRoomNote`), and every insert above the limit wakes an eviction pass, so a large import into a small store logs many short passes. No axum body limit on this route (the handler counts), but the large-body permit, held for the whole import. `403` while sending is off
- `GET /api/dictionary/{segment}` — A segment's fields from the embedded dictionary (number, name, datatype) for the draft editor's *show empty fields* (#278); reference data, not tied to `send.enabled`
- `GET /api/messages/{id}/draft`, `POST /api/parse`, `POST /api/build` — Drafts (#276): a stored message as draft text with references, the preview (JSON body) or a file's bytes decoded (any other body), the bytes a send would write as a framed download; `403` while sending is off
- `GET /api/export?direction=in|out|all` — Default `in` (received messages, as before #272). Download every stored message as one MLLP-framed `.hl7` file (the bytes **as received**, oldest first, directly replayable — via `Hl7Message::wire_bytes()`, #230). Exception: an over-limit frame (#223) is stored with only its first 4 KB, so the export contains that truncated frame (it carries a parse error in the UI). **Streamed** (`MllpExportStream` in `src/web.rs`): the store lock is held only to snapshot payload `Arc`s (`payload_snapshot`), large payloads go out zero-copy, small ones are coalesced into ~64 KiB chunks — never build the whole export in memory
- `WS /ws` — Real-time updates (`init`, `new_message`, `tags_updated`, `bookmark_toggled`, `delivery_updated`, `evicted`, `cleared`, `lagged` events). `evicted` carries the array of IDs removed by an eviction pass; the UI drops those rows and warns when bookmarks were among them

### Frontend (`static/`)

The SPA is split across HTML, CSS, and several JS files (see the full tree in **Source Layout** below), all embedded into the binary at compile time via `rust-embed`. The JS files are plain `<script>` includes with **no module system** — they share a single global `state` object (`state.js`) and must be loaded in dependency order (`state` → `util` → `ws` → `render` → `diff` → `send` → `compose` → `watch` → `app`).

**Important:** After any change to `static/`, a recompile is required — `rust-embed` bakes the files in at build time. There is no hot-reload.

The frontend is **intentional vanilla JS — no framework**. Do not introduce React, Vue, Svelte, or any build toolchain. The embedded SPA approach is a deliberate architectural decision for zero-dependency deployment.

Key frontend behaviors:
- Messages are batched and flushed at most every 250ms (prevents DOM freeze at high message rates)
- When no client-side filters are active, new messages are **prepended** to the list (`prependMessagesToList`); when a filter/search/bookmark filter is active, the list is fully rebuilt (`renderMessageList`)
- ⏸ Pause/▶ Live button buffers incoming messages without displaying them
- **Client buffer cap:** `state.messages` holds at most `CLIENT_MESSAGE_CAP` (2000, `util.js`) messages **plus** bookmarked and selected ones, which are exempt and don't take places in the cap (bookmarks are bounded server-side by `store.bookmark_budget_percent`). The paused `pendingMessages` buffer is capped at the same number, oldest first, without exemptions — new messages are almost never bookmarked yet. Covered by `tests/js/buffer.test.mjs`. Excess oldest messages are dropped via `trimClientBuffer()` (O(1) early exit when only exempt entries are over the cap) and their rows removed bottom-up via `removeRowsFromList()` — no full rebuild. Dropped messages stay on the server; `updateTruncationNote()` keeps a note at the bottom of the list whenever the server holds more than the view, because filters/search only cover loaded messages (server-side search: #197/#198). The header `total` shows the server total (`state.totalMessagesCount`, from WS `init` / `addMessage` / `pollStats` every 3 s — so it can lag by one poll while paused), not the buffer length
- **List (re)load (#239):** WS events go through `handleServerEvent()`. While `loadMessages()` waits for the list (on `init` / `lagged`), events are queued in `state.loadQueue` and replayed in order after the snapshot is applied (`new_message` for IDs already loaded is skipped); a newer load supersedes an older one via `state.loadSeq`. Never apply WS events directly to `state.messages` around a load. Covered by `tests/js/load.test.mjs`
- Search is client-side (filters `messages[]` array via `matchesSearch()`), debounced 300ms
- **Older history (#198, #197):** the note at the end of the list offers "Load 500 older" (`loadOlderMessages`, `/api/messages?before=`) and, while a search is active, "Search older on the server" (`searchOlderOnServer`, `/api/search?before=`). The cursor is `historyCursor()`: the oldest loaded message that is neither bookmarked, selected nor a search hit (`_searchHit`). Merged pages raise the trim cap by `state.olderLoaded`; a (re)load or clear resets it. A page fetched across a list reload is dropped (`loadSeq`)
- **Rows are cheap to build (#202):** `buildMessageRow` clones a prebuilt skeleton (`rowSkeleton()`) and caches parsed SVG icons (`iconNode()`); a row has `data-action="select-message"`, so clicks and Enter/Space go through the document-level delegation in `app.js` — no per-row closures, no per-row `innerHTML` parsing
- **Rows use `content-visibility: auto` (#216)** with `contain-intrinsic-size: auto 60px`: every row is 61 px (60 + 1 px border). If the row layout ever changes height, update the intrinsic size, or the scrollbar drifts. `tickRowRelativeTimes` only writes labels that changed; the health dot's pulse animates `transform`/`opacity` on a pseudo-element (compositor-only) — never animate `box-shadow` or other paint properties in an infinite animation
- **Long values are previewed (#215):** field values, component leaves, raw lines and JSON strings above `DISPLAY_VALUE_MAX` (2000) characters show their first 1000 plus a size note (`previewText`, `truncationNote` in `util.js`). Code that needs a value must read it from the message data, never from the DOM: the field copy handler uses `fullFieldValues`, "Copy All" uses `state.selectedMessage.raw`
- Parse errors are shown with `⚠ PARSE ERROR` in red (`var(--error)`) in the message list
- **Resend (#274, `static/send.js`):** only while `/api/send/config` answers (sending on). Every user keeps **their own target list** in `localStorage` (`harux_targets`; no patient data), plus *This Harux* from the config, never stored; save to / load from a file. Storage that fails is never fatal — the list lasts for the session and one toast says so. The split button **Resend → target** in the title row sends to the target used last (`harux_last_target`); the first use opens the menu. A target marked *ask before sending* asks with `confirm` every time; a message that did not parse needs a second `confirm` (`allow_invalid`). The answer is only reported (toast, last result per target): **the sent row comes from the WebSocket only** — never add it from the response. Tags/Bookmark/Pin diff live in the tools row (`#detail-tools`) below the meta row. Direction filter `state.directionFilter` (In + Out → Received → Sent; `canPrependOnly` knows it), `out` pill (every send attempt, from `/api/stats`), origin link, and trace by control ID (meta row). *Send as* (`state.sendAs`, `harux_send_as`): as received, new control ID, new control ID + current time — sent as `set`; a message that did not parse only as received. IDs come from `newId()` (`util.js`) — never call `crypto.randomUUID` directly: it is missing on plain HTTP, the usual deployment. Covered by `tests/js/send.test.mjs`
- **Drafts (#277, `static/compose.js`):** only while sending is on. **One selection:** `state.selection` is `{kind: 'message' | 'draft', id}`; `state.selectedId` is a getter/setter view of its message part, so `selectedId = null` (what `cleared`, `evicted` and a failed fetch do) never closes an open draft — `showDetailAfterStoreChange()` (`ws.js`) redraws a draft instead of emptying the panel. `state.selectedMessage` is null while a draft is open; `renderTab` hands the open draft to `renderDraftTab`. A draft is text in the tab (`sessionStorage` `harux_drafts`; *keep* → `localStorage` `harux_drafts_kept`; storage that fails toasts once and keeps them in memory). **The kept list and the target list are shared by the browser's tabs (#300):** a save never writes one tab's memory alone, it merges with what is stored now (`mergeStoredList`, `util.js`, by ID) — other tabs' entries stay and come in, what this tab removed (`keptRemoved` / `targetsRemoved`) stays out, and an entry this tab saw stored before (`keptSeen` / `targetsSeen`) that another tab removed is not written back; a `storage` event (`syncKeptDrafts`, `syncTargets`, registered in `app.js`) takes the other tab's list at once. A kept draft another tab removes stays in this tab, not kept, while it is open here, with LF line ends as a `<textarea>` keeps them. Everything HL7 is the server's (#276): preview `POST /api/parse` 300 ms after the last change (`state.draftPreviewSeq`: last wins; the Raw tab is never re-rendered by a preview — only its status line, so the cursor stays), the file `POST /api/parse` with the bytes, *Save as file* `POST /api/build`, the send a draft item in `POST /api/send`. The status line looks the cursor's UTF-16 offset up in the preview's positions (`fieldAt`), never splits the text itself, and never uses a preview with the references filled in (its offsets belong to another text). `markDraftOriginsGone` (called on `evicted` / `cleared`) marks a draft whose original is gone. Drafts are never dropped at load — the `MAX_DRAFTS` limit applies when one is added. *Send anyway* arms on the first click for text that does not parse. The target menu of `send.js` also serves a draft (`sendMenu.forDraft`: no *Send as*). Keyboard: `isTextEntry()` leaves Enter/Space to text entry; `Ctrl/Cmd+Enter` sends the open draft. The sent row still comes from the WebSocket only. **Field by field (#278):** the Segments tab of a draft has an input per field; a change is written back by the server (`writeDraftField` → `POST /api/parse` with `set`; writes are queued in `draftWriteChain`, and a send / *Save as file* awaits it), and the tab is then updated **in place** (`refreshDraftSegmentsInPlace`: checklist, banner, warning marks) — never re-rendered under a focused input, or what the user is typing is lost. A write belongs to the draft whose input changed (its ID is bound when the `change` fires, #303): its answer goes into that draft's text even if another draft or a message is open by then; only the redraw needs it to be open. The title row's buttons (`updateDraftActions`) and the rail's chips (`renderDraftRail`, per chip) are replaced only when their markup changes (#299) — a button replaced between mousedown and mouseup gets no click, so *edit a field, click Send* would need a second click. Segments are added, duplicated and removed as whole lines at the server's `positions` (no client-side HL7 split). *From a file* (#281, `openFile`): a file that starts with an MLLP start byte and holds more than one frame (or is larger than one message can be) is loaded into the list with `POST /api/import` after a `confirm` — the browser streams the file; one message, framed or not, opens as a draft. An `import:<name>` source is one source in the rail, never cut at its colon (`isImportKey`). Covered by `tests/js/compose.test.mjs`
- **Runs (#280, `static/send.js`):** *Resend filtered* (`#btn-resend-filtered`, only while sending is on) opens the target menu under the list (`sendMenu.forRun`, host `#run-menu-host`) and sends what `filteredMessages()` (`render.js`, the list's own filter) shows, **oldest first**; cut-off entries are left out, messages that did not parse need a second `confirm` (`allow_invalid`) and go out only as received. It always asks, naming target, address and number. *Repeat* on a draft (`d.repeat`, `#draft-repeat`) sends it n times in one run (asked first) and notes the run in the draft's history when it ends. A target marked *ask before sending* takes no runs (decision D3). `startRun` posts once; `state.run` follows the run by polling `GET /api/send/runs/{id}` every 500 ms into the bar under the list (`#run-bar`, `runBarHtml`: progress, codes, failures, average time, Cancel / ✕; the fill width is set through the CSSOM — the CSP forbids style attributes). One run per browser at a time. The rows still come from the WebSocket only. Covered by `tests/js/run.test.mjs`
- **Watch (#282, `static/watch.js`):** the bell in the list's controls (`#btn-watch`, shown whether or not sending is on — it sends nothing) takes a **snapshot** of the filter (`watchFromFilter`: search, direction, validation; the bookmark filter is left out, new messages are never bookmarked) into `state.watch` (`sessionStorage` `harux_watch`). `watchNewMessage` (called from `handleServerEvent` for every `new_message`) uses the list's own matcher (`matchesSearch` via `watchMatches`) and counts **only while the tab is hidden**: `(3) Harux` in the title, cleared on `visibilitychange`; optionally a beep made with Web Audio (no sound file; at most one per 2 s) and a desktop notification (at most one per 5 s, type + source + control ID only — never patient data). Desktop notifications exist only in a secure context (HTTPS or `localhost`); on plain HTTP the switch is disabled and says why, and the menu lists what is active (`activeWatchChannels`). Preferences per browser (`localStorage` `harux_watch_prefs`). Messages that arrive while the WebSocket is away (`lagged`, a reconnect) come with the reloaded list, not as `new_message`, and are not counted — the counter is a nudge, not a tally. The Watch menu shares `#run-menu-host` with the *Resend filtered* menu; they are never open together (opening one is a click outside the other). No server code. Covered by `tests/js/watch.test.mjs`

## Critical Knowledge — Read Before Changing Anything

1. **MSH field index offset.** MSH-1 is the field separator itself; the parser inserts a synthetic MSH-1 and shifts the rest (see *HL7 Parsing* above). Do not "simplify" it — it breaks every MSH field extraction without a compile error.
2. **ACK storm prevention.** A message whose `message_type` starts with `"ACK"` is stored but never ACK'd (`if msg.message_type.starts_with("ACK")` in `src/mllp.rs`). ACK-of-ACK creates an endless ping-pong with Orchestra.
3. **MLLP is binary framing, not text.** Extract frame boundaries (`extract_mllp_frame`) on bytes first; only then decode the payload, per MSH-18, with `decode_payload` (`src/hl7/charset.rs`).
4. **Parse errors are stored, not discarded.** A frame that fails to parse (or panics while parsing) is stored with `parse_error` and `message_type: "UNKNOWN"` and answered with a NACK. Harux is a debugging tool; broken messages are often the important ones.
5. **Static files are embedded at compile time.** Any change under `static/` needs a rebuild.
6. **Byte accounting stays exact.** `current_bytes` and the bookmark totals are adjusted on insert, metadata change, eviction and `clear()`; tests assert them against recomputed values — keep the tests, don't relax them.

## Constraints — What Not to Change Without Explicit Instructions

| Area | Rule |
|---|---|
| Frontend | **No framework**, no build toolchain, no npm. Vanilla JS, plain `<script>` files. |
| CSS | No CSS frameworks; all styles in `static/style.css` with CSS variables. The CSP forbids inline scripts, inline styles and inline event handlers — use `data-action` delegation. |
| Parser | No zero-copy / lifetime refactoring. Correctness over performance. |
| Store locking | `Arc<RwLock<>>` is intentional: no `dashmap`, no `mpsc`-channel refactor. |
| DOM rendering | Keep the 250 ms flush batching. New messages prepend on the no-filter hot path; full rebuild only when a filter is active. No virtual DOM, no per-message full rebuilds. |
| Dependencies | No new crates without explicit instruction. Check `Cargo.toml` first; prefer what is already in the dependency tree. (The TLS crates `tokio-rustls` and `rustls-native-certs` came in with #273 on the owner's decision D4.) A pull request that changes `Cargo.toml`/`Cargo.lock` also runs the release builds for Windows, macOS and Linux (`build.yml`) and `cargo audit`. |
| Eviction | No count-only eviction, no bookmarks outside the budget, no payload mutation (`Arc::make_mut`) for tags/bookmarks. |
| ACK behaviour | Never ACK an incoming ACK. |

## Deployment Context

- **OS:** Windows Server (primary). The `.exe` runs as a Windows Service via NSSM.
- **Build pipeline:** GitHub Actions builds Windows, macOS (Apple Silicon) and Linux binaries on every push to `main` and attaches them to releases.
- **Users:** several developers at the same time via browser — no local setup, no RDP window.
- **Network:** hospital internal network, no internet access from the server. Network placement is the only access control: there is no authentication and no TLS (`server.mllp_bind` / `server.web_bind`, `server.allowed_hosts`).
- **Traffic:** HL7 v2.x from Orchestra — ADT, ORU, ORM, SIU, MDM. MDM with Base64-encoded PDF attachments is daily traffic, not an edge case.
- **Deployment:** single binary, xcopy-style. No installer, no runtime dependencies, no Docker.

## Testing

```bash
cargo test                          # unit + integration tests
node --test 'tests/js/*.test.mjs'   # frontend logic
tests/test.sh                       # functional + load test, Linux/macOS (needs nc, 100 messages)
.\tests\test.ps1                    # functional + load test, Windows (persistent TCP, 1000 messages)
```

`tests/test.sh` starts a new `nc` process per message, so its load numbers
measure process overhead, not server throughput. `tests/fuzz_ingest.rs` runs
20,000 mutated frames by default; `HARUX_FUZZ_ITERATIONS` changes the count.

## Common Mistakes to Avoid

- Changing MSH field indices without understanding the +1 offset in `parse_segment()`
- Removing the ACK-type check in `src/mllp.rs` as "dead code"
- Adding a framework or inline handlers to `static/` (breaks the single binary and the CSP)
- Raising store limits without thinking about MDM payload sizes
- Forgetting to adjust `current_bytes` when changing insert, eviction or clear
- Treating MLLP as plain TCP text
- Running only `cargo build` instead of the full CI list above
- Storing a sent message after connecting, or adding the response text to `Hl7MessageSummary` (#272)
- Keying a sent message by `source_addr` in the UI (it is the destination, not a sender)

---

## Source Layout

```
src/
├── main.rs          # Entry point, tokio::select! over MLLP + Web tasks
├── lib.rs           # Library crate root (re-exports modules for integration tests)
├── config.rs        # TOML → env → defaults config loading; fails closed: unknown keys, bad values, bad port env → startup error (#233); bind addresses (#237)
├── draft.rs         # Drafts: draft text with references, preview + field positions, build, file decoding (#276)
├── logging.rs       # Daily log file: fallible setup with stdout fallback (#234), pruning to logging.max_files by exact name (#240)
├── mllp.rs          # TCP listener, MLLP framing (FrameSplitter), ACK/NACK dispatch, Importer (#281)
├── mllp_client.rs   # Sending side of MLLP: connect, one exchange, response limits (#272)
├── send.rs          # Sending engine: checks, store-before-connect, outcome, /api/send request types (#272)
├── tls.rs           # Outgoing TLS: TlsContext from [send.tls], verifiers, client certificate, issuer names (#273)
├── store.rs         # In-memory store with broadcast channel, dual eviction
├── templates.rs     # Templates: built-in set + read-only send.templates_dir, lookup by key only (#279)
├── web.rs           # Axum router, REST handlers, WebSocket handler
├── dictionary.rs    # Embedded HL7 field/segment dictionary (v2.5.1.json)
├── validation.rs    # Required segment/field + primitive datatype validation
├── hl7/
│   ├── mod.rs
│   ├── charset.rs       # Payload bytes → text per MSH-18, fallback + CHARSET_FALLBACK warning; encode_for_send (#275)
│   ├── compose.rs       # Field overrides + placeholders: the one place that writes a field (#275)
│   ├── escape.rs        # HL7 escape sequences: encode (ACK text) / decode (display)
│   ├── parser.rs        # Raw HL7 → Hl7Message, delimiter extraction, ACK builder
│   ├── types.rs         # Hl7Message, Hl7MessageSummary, Hl7Segment, Hl7Field, Delimiters
│   └── message_types.rs # Message-type registry (message_types.json)
├── assets/hl7/      # Embedded JSON: v2.5.1.json (dictionary), message_types.json
└── assets/templates/ # Built-in .hl7 templates (#279), embedded with include_str!
static/
├── index.html       # HTML skeleton
├── style.css        # Dark theme, CSS variables
├── state.js         # `state` object (all mutable globals)
├── util.js          # Constants + pure helpers
├── ws.js            # WebSocket + ingestion + stats polling
├── render.js        # List, legend, health pills, detail-panel tabs
├── diff.js          # Diff tab (renderDiffTab + builders)
├── send.js          # Resend: the user's target list (localStorage, file), split button and menu (#274)
├── compose.js       # Drafts: rail, draft header and tabs, preview, Edit / New message, send (#277)
├── watch.js         # Watch: the bell, a snapshot of the filter, title counter, sound, desktop notification (#282)
├── app.js           # Init + UI handlers (data-action delegation) + splitter
└── fonts/           # Self-hosted woff2 (Inter, JetBrains Mono)
tests/
├── parser_fixtures.rs            # Parser tests against tests/messages/*.hl7 fixtures
├── mllp_tcp.rs                   # MLLP over real TCP: framing, ACK/NACK, limits, shutdown
├── send_tcp.rs                   # Sending over real TCP and TLS: fake receivers, round trip to Harux, bytes on the wire, TLS failure reasons, runs (order, one connection, cancel, eviction, reconnect) (#272, #273, #280)
├── tls/                          # Static test certificates (CA, server, wrong name, unknown CA, client) and generate.sh
├── fuzz_ingest.rs                # Mutation fuzz of decode → parse → ACK → JSON, no panic + invariants (#243)
├── config_startup.rs             # Real binary: broken config stops the start; unwritable log, bind addresses
├── accept_errors.rs              # Real binary under ulimit: accept() errors are not fatal, SIGTERM (Unix)
├── alloc_accounting.rs           # Counting allocator: estimated_bytes() matches the real heap (#226)
├── doc_paths.rs                  # Paths named in the docs exist (#246)
├── web_api.rs                    # Axum REST/handler integration tests (incl. drafts: MDM reference, Latin-1 round trip, body limits, permits)
├── js/                           # node:test tests for static/*.js (diff, list load queue, client buffer + counters, row builder, long-value preview, sent rows, resend + target list, drafts, runs, watch); fake-dom.mjs is the shared minimal DOM
├── benchmark_concurrent_insert.rs # Concurrent store insert benchmark
├── messages/                     # .hl7 fixtures (valid / errors / unknown_types)
├── test.sh                       # Linux/macOS functional + load test (netcat)
└── test.ps1                      # Windows functional + load test (.NET TcpClient, 1000 msg)
examples/harux-example.toml       # Every configuration key with its default
harux.toml                        # The shipped configuration (defaults)
.cargo/audit.toml                 # cargo-audit policy and ignored advisories (with reasons)
.github/workflows/                # ci.yml (gate), audit.yml, build.yml (releases), release-drafter.yml
docs/                             # ROADMAP, PROJECT_OVERVIEW, STYLE_GUIDE; handoffs/ and audits are historical
```
---

## Communication Preferences

When interacting with the user or external systems (like GitHub issues), you MUST provide detailed, structured, and comprehensive explanations.

Specifically for GitHub issues:
- **Do not** write short, generic closing comments (e.g., "Fixed bug and pushed").
- **Do** write a detailed `### Fix Details` section that explains *why* the issue occurred, the architectural or code-level changes made to fix it, and how the fix was verified. Reference specific files (`src/store.rs`, `static/app.js`, etc.) and the logic changed.
