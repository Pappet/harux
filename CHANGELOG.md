# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)
and this project follows [Semantic Versioning](https://semver.org/lang/en/).

---

## [Unreleased]

### Added
- **Keyboard accessibility for custom elements** — added `tabindex`, `role="button"`, and Enter/Space keyboard activation for segment headers, copy buttons, field value cells, and tagging elements.
- **Keyboard accessibility for source chips** — added `tabindex="0"`, `role="button"`, and Enter/Space keyboard activation for source chips in the message list header, allowing filter toggling via keyboard navigation.

### Changed
- **Refactor — split `static/app.js` into modules** — the 1987-line `app.js` was broken into six focused files loaded with `<script defer>` in order: `state.js` (single mutable state object, replacing 24+ scattered top-level `let`s), `util.js` (constants + pure helpers: icons, `esc`/`escAttr`/`escJS`, `bucketKey`, `formatRelativeTime`, `saveSession`/`loadSession`, query parser, JSON cache), `ws.js` (WebSocket lifecycle, event dispatch, `addMessage` batching, `pollStats`), `render.js` (message list, source legend, health pills, detail panel + parsed/raw/ack/json tab builders), `diff.js` (segment diff view, refactored into `buildDiffHeader`/`buildDiffSegmentBlock`/`buildDiffFieldRow`/`buildDiffSummary` helpers — paying down the F4 nested-template debt at the same time), `app.js` (init, UI handlers wired up by inline `onclick`, panel splitter, click/keydown delegation). All cross-file references go through `state.X` instead of bare globals; the legacy 12+ top-level `let`s are gone. Largest remaining file is `render.js` at ~960 lines (down from 1987). No framework, no toolchain, no bundler — still vanilla JS embedded via `rust-embed`. Also fixed the silently-swallowed `pollStats` error (now logs at `console.debug`). (#136, partially #138, partially #139)
- **Refactor — parser modularization** — split the monolithic `parse_message` function into composable passes (`parse_structure`, `enrich_msh`, `enrich_pid`, `inject_descriptions`, `annotate_message_type`, `validate`) making it easier to test and extend. (#122)
- **Performance — background store eviction** — moved the store eviction scan out of the `insert` hot path and into a background Tokio task, driven by a `Notify` trigger. This eliminates `MessageStore` write-lock contention under heavy loads, ensuring concurrent readers (WebSocket, search, HTTP API) no longer stall during eviction. (#130)
- **Performance — detail panel DOM builders** — replaced the parsed/raw/ack tab `innerHTML` mega-string concatenation (nested `msg.segments.map().join('')` × `seg.fields.map().join('')`) with real `document.createElement` builders that `replaceChildren` once. Opening a message with many segments/fields no longer pays for thousands of intermediate string allocations. Segment collapse is now a CSS class on `.segment-block` — the field table is rendered once and hidden via `.segment-block.collapsed .field-table { display: none }`, so expanding/collapsing never rebuilds the DOM. JSON tab caches the `JSON.stringify` output in a module-scoped `WeakMap` keyed on the message object (no mutation of the data model) and invalidates the entry when tags/bookmark change.
- **Performance — store eviction in O(n)** — rewrote the eviction loop in `MessageStore::insert`. The previous code collected indices and called `VecDeque::remove(i)` per candidate, which is O(n) each — combined with the candidate loop this was O(n²) and kept the write lock held during the whole pass, blocking MLLP ingestion and UI reads. The new loop pops candidates from the front, sets bookmarked ones aside, evicts the rest, and pushes the bookmarks back. Single linear pass, no per-element shift. Added test `test_eviction_skips_scattered_bookmarks` covering scattered bookmarks among eviction candidates.
- **Performance — Arc-backed message storage with HashMap index** — split `StoreInner` into `order: VecDeque<String>` (insertion order) and `messages: HashMap<String, Arc<Hl7Message>>` (lookup). `get_by_id` is now O(1) and hands out an `Arc` snapshot — no deep clone of the (potentially multi-MB) `raw` + `segments` + `validation_warnings` payload on every detail-panel click. `GET /api/messages/:id` returns `Json(msg)` where `msg: Arc<Hl7Message>` serializes directly (enabled `serde/rc`), eliminating the previous `serde_json::to_value` intermediate `Value`. Tag / bookmark mutations use `Arc::make_mut` (copy-on-write) — only allocate when a reader holds a concurrent snapshot. Added test `test_get_by_id_returns_shared_snapshot` confirming the `Arc::ptr_eq` invariant.
- **Performance — incremental message-list rendering + source-counts cache** — `flushAndRender` now prepends only new rows to the DOM via `prependMessagesToList` when no client-side filter is active, instead of removing every `.message-row` / `.group-header` and rebuilding from scratch each 250 ms flush. Selection changes call `updateRowSelection` (class toggle only), and tag/bookmark updates call `patchRow` to mutate just the affected row (the previous code re-rendered the entire list per WebSocket tag/bookmark event). Source counts are maintained in a `sourceCounts: Map` updated incrementally in `addMessage` and recomputed only when labelling changes (`toggleColorByPort`) or the buffer is replaced (`loadMessages`, `clearMessages`, server `cleared`), so `renderSourceLegend` no longer iterates `messages[]` on every flush. Group headers carry `data-bucket` so the prepend path knows when to emit a new time-bucket header.

### Commit History (chronological)

#### 2026-05-15

| Commit | Description |
|--------|-------------|
| `fix(ui):` Bookmark count badge live update + detail restore on refresh |
| [`b3ffbfa`](https://github.com/Pappet/harux/commit/b3ffbfa5b0c4b4ce25881264bdda87773ba14504) | `refactor(ui):` split app.js into modules (#136) |
| [`fb41636`](https://github.com/Pappet/harux/commit/fb41636315214e0195ef1030ae4eb20e454ad35d) | `refactor:` split parse_message into composable passes |
| [`8ed739b`](https://github.com/Pappet/harux/commit/8ed739bbf9ffff46c1a058043eaed547a7e2dd7b) | `perf:` move eviction to background task (#130) |

#### 2026-05-14

| Commit | Description |
|--------|-------------|
| [`4f0fc13`](https://github.com/Pappet/harux/commit/4f0fc130b439b196b1e12b2ade00295a47e1c441) | `test:` address PR review feedback on parser tests |
| [`375af95`](https://github.com/Pappet/harux/commit/375af9513e4124d69774d4ba25fc4cc73bf37f9c) | `style:` run cargo fmt and fix clippy |
| [`40bd525`](https://github.com/Pappet/harux/commit/40bd525d4d7a445c57cba0e41f8ccb7b5a577b9c) | `test:` add parser integration test suite |
| [`655edfb`](https://github.com/Pappet/harux/commit/655edfbad9d434f75f6428bbb5152bc0a63143a9) | `chore:` organize repo — add docs/ folder, move project docs |
| [`6f1d19a`](https://github.com/Pappet/harux/commit/6f1d19a86fc580f28d1a031eb059d854e72786b8) | `perf(ui):` rebuild detail panel via DOM builders + CSS collapse |
| [`5994df0`](https://github.com/Pappet/harux/commit/5994df03294bf215d3d5ea16d6061c548b6bd0cb) | `perf(store):` rewrite eviction loop in O(n) |
| [`e3077c1`](https://github.com/Pappet/harux/commit/e3077c1d3c43e008623d223ab9d9f40627c7f3c7) | `perf(store):` Arc-backed messages with HashMap index |
| [`6392601`](https://github.com/Pappet/harux/commit/639260177401c6246a3c04810c31d152b5c66c20) | `perf(ui):` incremental list rendering + source-counts cache |
| [`138784a`](https://github.com/Pappet/harux/commit/138784adeac4433de54160ab290670e6137b6952) | `refactor(ui):` move detail JSON cache from property to WeakMap |

#### 2026-05-09

| Commit | Description |
|--------|-------------|
| [`39cf3b1`](https://github.com/Pappet/harux/commit/39cf3b1) | `docs:` update documentation to reflect v0.5.0 UI redesign |
| [`4f150f7`](https://github.com/Pappet/harux/commit/4f150f7) | `feat(a11y):` add keyboard accessibility to source chips |
| [`51ca703`](https://github.com/Pappet/harux/commit/51ca703) | `feat(a11y):` add keyboard accessibility to custom UI elements |

---

## [0.5.0] – 2026-05-08 – UI Redesign

> Visual + IA refactor of the entire frontend (`CLAUDE_CODE_HANDOFF.md`, items 1–14, plus the chrome-cleanup follow-up). Six numbered phases shipped between #87 and #103 plus a chrome-cleanup pass in #105. Two perf wins (#102 parser allocations, #100 frontend search filter) round out the release. No protocol, API, or storage changes.

### Added
- **Keyboard accessibility for message rows** — message list rows now support `Tab` focus navigation and `Enter`/`Space` to select; a 2px accent outline appears on `:focus-visible`. Improves screen-reader and keyboard-only workflows.
- **ARIA labels on message rows + bookmark/pin controls** — each row exposes a synthesized `aria-label` (facility, type, time); the bookmark and pin controls were promoted from `<span>` to native `<button>` so they receive `Tab` focus, keyboard activation, and `aria-label`. Pin clicks now `stopPropagation` so the row beneath does not also select.
- **Global `:focus-visible` outline** — every focusable element gets a 2 px accent outline when reached via keyboard, addressing missing focus rings across header buttons, search input, and inline controls.

### Fixed
- **Pin click leaked to row selection** — clicking the diff-pin icon previously also selected the underlying message row. `toggleDiffPin` now calls `event.stopPropagation()` when invoked from the row.

### Changed
- **Chrome cleanup** — restructured the topbar and inbox header for clearer information architecture: list-scoped controls (`Pause`, `Auto`, `Bookmarks`, validation filter) moved out of the topbar into a new `.inbox-controls` row beneath the search bar; the topbar keeps only `Export` and `Clear`. Three new pills join the topbar — `total` (current message buffer), `validation` (sum of warnings + segment errors, amber when nonzero), `parse errors` (renamed from `errors`, server-side MLLP parse failures, red when nonzero). The standalone throughput band between the source rail and message list is removed — the rate sparkline already lives in the `rate` pill.
- **Performance — HL7 parser allocations** — eliminated multiple redundant `Vec` allocations in the HL7 parsing engine (`parse_segment` and `parse_message`), substantially reducing heap allocations per message and improving throughput (#102)
- **Performance — frontend search filter** — `matchesSearch` now uses a pre-parsed query (`getParsedQuery`) so the `has:warnings` / `has:errors` prefix stripping and lowercasing happen once per query instead of once per message. Per-message field checks short-circuit on the first hit and skip the lowercasing when the field is empty. Bounded `parsedQueryCache` (max 100 entries) prevents memory growth from many unique searches (#100)
- **Typography** — adopted Inter (sans) and JetBrains Mono (mono) via Google Fonts; first phase of the v0.5.0 UI redesign (#86)
- **Color palette** — refreshed dark theme: deeper backgrounds (`--bg-primary` `#0b0d12`, `--bg-secondary` `#131620`, `--bg-tertiary` `#1b1f2c`), cooler accent (`--accent` `#7aa2ff`), tuned success/error tones (`--success` `#4fb98a`, `--error` `#ea6363`) for higher contrast against the deeper canvas (#86)
- **Top-bar health pills** — replaced the dot-and-counter `.stats-bar` with a row of pill-shaped status indicators: `listening :PORT` (green pulsing dot when WebSocket connected, red static when disconnected), `conns N / MAX`, `rate N/min` with a 60-second sparkline, `last Ns ago` (refreshed once per second), `errors N` (red value when nonzero). The previously hidden "rejected" stat is preserved as a hidden pill that surfaces only when `rejected_connections > 0`. The `total messages` counter was removed from the header; the rolling rate pill replaces it as the live-traffic signal (#92)
- **Message rows — two-line layout** — dropped the dense 9-column grid (`12px 100px 90px 1fr 140px 60px 40px 24px 24px`) for a `[3 px source bar] [body] [actions]` layout. Row 1 carries `[type] [patient] [validation badge] [tags] [ACK chip]`; row 2 carries monospace `facility · source · N segs` with right-aligned time. The `Type / Facility / Patient / Date / Time / Segs / ACK` `.list-header` is removed (column titles are no longer needed). Actions column now shows the bookmark star only — the pin-for-diff icon was removed from the row and lives in the detail panel instead (#94)
- **Time-bucket group headers** — messages are grouped under sticky headers: `Live` (last 30 s), `Last few minutes` (30 s – 5 min), `Earlier today`, `Yesterday`, and `MMM D` for older days. The header sticks to the top of the message list while scrolling so the bucket label stays visible (#94)
- **ACK chips** — replaced the inline-styled `color:` ACK rendering with colored chip pills: `.msg-ack.aa` green, `.msg-ack.ae` red, `.msg-ack.ar` amber, `.msg-ack.none` muted. Easier to scan a column of failures at a glance (#94)
- **Relative time on row 2** — messages in the `Live` and `Last few minutes` buckets show `Ns ago` / `Nm ago`; `Earlier today` shows `HH:mm:ss`; older messages show `MMM D HH:mm`. A 1-second tick refreshes the time labels in place without re-rendering the whole list (#94)
- **Always-visible source rail** — the source legend (previously hidden until toggled) is now a permanent rail above the message list. Each source renders as a `.source-chip` with colored dot, host (or `host:port`), and per-source message count. Click toggles `highlightedSource` and dims non-matching rows. The rail scrolls horizontally when chips overflow, with hidden scrollbars (#96)
- **Color by Port moved to overflow menu** — the `Color by Port` toggle is no longer inline with the source chips. It lives in a `<details>` overflow popover (⋯) at the right edge of the rail, freeing the rail for scannable source identification (#96)
- **Throughput band** — a new row between the source rail and the message list shows four counters (`total`, `per min`, `warnings`, `errors`) plus a 60-bar histogram representing the last 60 seconds of message arrivals. Bars rotate every second; opacity ramps from 0.4 (oldest) to 1.0 (newest). `warnings` paints amber when nonzero, `errors` paints red. Backed by a `rateBuckets` ring buffer that is incremented by `addMessage` and rotated by a 1-second interval (#96)
- **Detail header restructure** — replaced the single-row `[h2 + meta + tag controls]` header with a layered structure: type chip + human title on row 1, optional description on row 2, monospace metadata row (`patient · MRN · control · v · received`) with click-to-copy on the control ID, plus a tags row. Bookmark and Pin-for-diff are promoted to text+icon `.action-btn` buttons in a dedicated actions column on the right (#98)
- **Detail tabs — larger, with segment badge** — bumped tab padding to `11px 14px`. Segments tab now carries a `(N)` badge equal to the message's segment count. Diff tab is `margin-left: auto` so it visually separates from the "view this message" tabs (#98)
- **Typical segments checklist** — wrapped in a card (`.seg-checklist`) and each pill carries an explicit symbol: `✓` present, `⚠` flagged, `✕` required-but-missing, blank for absent-and-optional (#98)
- **Validation summary banner** — replaced the bulleted `.validation-warnings-panel` with a one-line summary banner at the top of the segments view: `⚠ 2 validation warnings · required field missing in PD1-3, expected segment not sent: OBX`. The full bulleted list is preserved as a collapsible `<details>` body (#98)
- **Field rows show description inline** — the field-dictionary description is no longer hover-only. It renders as a `.desc-text` sub-line under the field index (e.g. `PID-5 / Patient name`). Rows that triggered a `MISSING_FIELD` warning are tinted amber and gain a `⚠ required` suffix on the value cell. The hover-tooltip CSS for `.field-idx.has-tooltip` is removed in favor of the inline line (#98)
- **Empty-state CLI hint** — the message-list empty state is now an `.empty-card` with a tray icon, the live listening port, "No messages received yet", and a copy-able `mllp-send` snippet whose port is wired to `stats.mllp_port` so it tracks the actual listener (#101)
- **Detail-panel empty state** — the cold "Click a message to view details" placeholder becomes a matching `.empty-card` with a small document icon and a one-line hint about what each tab shows (#101)
- **Search shortcut** — `Cmd+K` (or `Ctrl+K` on non-Mac) focuses the filter input. The hint chip (`⌘K` / `^K`) sits inside the search field and fades out on focus or when the user is already typing (#101)
- **Search bar polish** — the filter input gains a leading magnifier icon and the placeholder now hints at `has:errors` operators (#101)
- **Tag-chip remove control** — replaced the `×` text glyph with the same lucide-style x SVG used elsewhere; the chip itself is now a pill-shaped `.msg-tag` with an inline-grid hover halo around the remove control (#101)
- **Validation summary chevron** — the trailing `▸` text glyph is replaced with the chevron SVG; CSS rotates it 90° when the `<details>` is open (#101)
- **`tr.warn` "required" suffix** — dropped the `⚠` glyph; the suffix now renders as a small uppercase `required` chip in `--warning` color (#101)
- **Diff tab affordance** — the right-aligned Diff tab gains a chevron icon and reads `Diff vs pinned` so it visually announces "leads to the comparison view" (#101)
- **Source-chip count badge + tab font weight** — source-chip count badges get a tinted background so they pop against the chip body; active tabs render at `font-weight: 600` for a clearer reading rhythm (#101)

### Commit History (chronological)

#### 2026-05-08

| Commit | Description |
|--------|-------------|
| `release/v0.5.0` (this commit) | `chore(release):` v0.5.0 — UI Redesign |
| [`f99e426`](https://github.com/Pappet/harux/commit/f99e426) | `feat(ui):` chrome cleanup — topbar pills + inbox controls row (#105) |
| [`3c64799`](https://github.com/Pappet/harux/commit/3c64799) | `perf(ui):` memoize parsed search query + short-circuit field checks (#104) |
| [`6040b4c`](https://github.com/Pappet/harux/commit/6040b4c) | `perf(parser):` eliminate redundant Vec allocations in HL7 parsing (#102) |
| [`9fa7368`](https://github.com/Pappet/harux/commit/9fa7368) | `feat(ui):` empty-state CLI hint + Phase 6 polish pass (#103) |
| [`549919e`](https://github.com/Pappet/harux/commit/549919e) | `feat(ui):` detail panel restructure — header / tabs / validation / fields (#99) |
| [`1ea2e2b`](https://github.com/Pappet/harux/commit/1ea2e2b) | `feat(ui):` always-visible source rail + 60-bar throughput band (#97) |
| [`bdf92b0`](https://github.com/Pappet/harux/commit/bdf92b0) | `feat(ui):` two-line message rows with time grouping + ACK chips (#95) |
| [`4426a61`](https://github.com/Pappet/harux/commit/4426a61) | `feat(ui):` top-bar health pills replace stats dots (#93) |
| [`90c7520`](https://github.com/Pappet/harux/commit/90c7520) | `feat(a11y):` ARIA labels, native buttons, global focus ring (#91) |
| [`c4089ad`](https://github.com/Pappet/harux/commit/c4089ad) | `feat(ui):` typography + color foundation for v0.5.0 redesign (#87) |
| [`2569bd3`](https://github.com/Pappet/harux/commit/2569bd3) | `feat(a11y):` keyboard navigation for message list rows (#88) |
| [`dfdda56`](https://github.com/Pappet/harux/commit/dfdda56) | `🛡️ Sentinel:` [HIGH] Fix Cross-Site Scripting (XSS) vulnerability (#85) |
| [`e6fa07c`](https://github.com/Pappet/harux/commit/e6fa07c) | `fix:` resolve XSS vulnerability in HTML attribute injection (#84) |
| [`1ac0b4e`](https://github.com/Pappet/harux/commit/1ac0b4e) | `⚡ Bolt:` optimize dictionary lookup to O(1) where possible (#83) |
| [`47d1fa7`](https://github.com/Pappet/harux/commit/47d1fa7) | `fix:` resolve ISO-8859-1 charset encoding issues (#82) |

---

## [0.4.0] – 2026-03-08 – Message Analysis

> Completes Milestone 3 (Message Analysis). Harux now understands the content of messages: field names from the HL7 v2.5.1 spec, validation with severity-coded badges, a segment diff view, and quick copy-to-clipboard throughout.

### Added
- **HL7 field dictionary** — hover over any field in the detail view for a CSS tooltip with its official HL7 v2.5.1 description (e.g. "Patient Name" for PID-5); powered by a compiled-in zero-overhead JSON dictionary (#48)
- **Message type detection** — detail header shows a human-readable type description (e.g. "Admit / Visit Notification") and a "Typical segments" bar with coloured badges indicating which segments are present vs absent (#45, #50)
- **Segment description tooltips** — hovering a segment header (e.g. `MSH`, `PID`) shows its official HL7 description as a CSS tooltip below the header; the same description appears as a native `title` tooltip on every typical-segment badge (#45)
- **Validation engine** — rule-based validator checks required MSH fields and message-type-specific segments (ADT, ORU, ORM, OML, SIU, MDM); warnings shown as an amber badge in the message list and a collapsible panel in the detail view (#46, #51)
- **Data type validation** — the validation engine checks NM (numeric), DT (date), TS (timestamp), and SI (sequence ID) field values against the HL7 v2.5.1 dictionary; violations appear as blue `INVALID_DATATYPE` badges in the detail view, distinct from missing-field (amber) and missing-segment (red) warnings; only first field component is validated to avoid false positives on composite values (#62)
- **Validation status filter** — header button cycles through All → Warnings Only → Errors Only; also supports `has:warnings` / `has:errors` prefixes in the search bar for combined filtering (#60)
- **Segment diff** — pin any message as a reference with the `◎` button in the list row, then open the Diff tab on another message to see a field-level side-by-side comparison with red/green highlighting (#47, #52)
- **Diff: ignore dynamic fields toggle** — toggle switch in the Diff tab hides MSH-7 (Date/Time) and MSH-10 (Message Control ID) from the comparison; the summary shows how many dynamic fields were hidden so nothing is silently lost (#61)
- **Quick copy to clipboard** — three copy targets in the detail view: (1) segment header shows a 📋 icon on hover that copies the raw segment string; (2) any field value cell is click-to-copy with a green flash feedback; (3) Raw tab has a "📋 Copy All" button that copies the entire message text (#63)
- **Detail header layout** — message title, type description, and meta line are stacked vertically on the left; Bookmark button and tag controls are grouped on the right side of the header

### Fixed
- **ISO 8859/1 Encoding** — implemented two-pass MLLP frame decoding with `encoding_rs` to respect the charset declared in MSH-18 before parsing, preventing corruption of extended Latin characters (#78)
- **Missing CSS closing brace** — `.validation-seg` rule was missing its closing `}` in the merged main branch, causing the diff-view CSS block to be incorrectly scoped
- **Message list date/time** — replaced `toLocaleTimeString` with manual formatting to ensure consistent `YYYY-MM-DD HH:mm:ss` display (#55)
- **Message row layout shift** — added transparent left borders to all message rows to prevent horizontal shifting when a row is selected (#56)
- **Duplicate CSS blocks** — removed redundant `.theme-toggle` styles from `style.css` (#57)
- **Missing ACK styling** — added the missing `.msg-ack` class to center align and correctly style ACK codes (#58)
- **Pin button highlight color** — decoupled pin button styling from bookmarks to ensure pinned items highlight in blue instead of yellow (#59)
- **Diff table column widths** — enforced strict layout on the segment diff table using `table-layout: fixed` so right-hand columns don't stretch (#67)
- **Typical-segment badge colour for data type warnings** — `INVALID_DATATYPE` warnings no longer turn a segment badge yellow; only `MISSING_FIELD` triggers amber, keeping the badge colour semantics accurate (red = missing segment, amber = missing required field, blue = present)

### Commit History (chronological)

#### 2026-03-08

| Commit | Description |
|--------|-------------|
| [`7cd9062`](https://github.com/Pappet/harux/commit/7cd9062) | `feat(ui):` Quick copy-to-clipboard for segments, fields, and raw message (#76) |
| [`598c713`](https://github.com/Pappet/harux/commit/598c713) | `feat(validation):` Data type validation for NM, DT, TS, SI fields (#74) |
| [`7f040df`](https://github.com/Pappet/harux/commit/7f040df) | `feat(ui):` Dynamic field toggle in diff view (#73) |
| [`dd8a216`](https://github.com/Pappet/harux/commit/dd8a216) | `feat(ui):` Validation status filter (#72) |
| [`56bb394`](https://github.com/Pappet/harux/commit/56bb394) | `fix(ui):` Enforce diff table column widths (#70) |
| [`87de6d1`](https://github.com/Pappet/harux/commit/87de6d1) | `fix(ui):` Accent color for pinned messages in detail view (#69) |
| [`18484c4`](https://github.com/Pappet/harux/commit/18484c4) | `fix(ui):` Add missing .msg-ack CSS class (#68) |
| [`b70ff1f`](https://github.com/Pappet/harux/commit/b70ff1f) | `style(ui):` Remove duplicate theme-toggle CSS block (#66) |
| [`4fdc373`](https://github.com/Pappet/harux/commit/4fdc373) | `fix(ui):` Prevent message row content shift on selection (#65) |
| [`3ef653e`](https://github.com/Pappet/harux/commit/3ef653e) | `fix(ui):` Show full date and time in message list (#64) |
| [`292ef35`](https://github.com/Pappet/harux/commit/292ef35) | `feat:` Color-code typical segment badges by validation state (#54) |
| [`91d03dd`](https://github.com/Pappet/harux/commit/91d03dd) | `feat:` HL7 validation engine with UI warning display (#51) |
| [`28cbb88`](https://github.com/Pappet/harux/commit/28cbb88) | `feat:` Message type detection with descriptions and typical segments (#50) |
| [`f36c3de`](https://github.com/Pappet/harux/commit/f36c3de) | `feat:` Segment diff — compare two messages side by side (#52) |
| [`60ba09d`](https://github.com/Pappet/harux/commit/60ba09d) | `feat:` Embed HL7 v2.5.1 JSON dictionary for field hover tooltips (#48) |

---

## [0.3.0] – 2026-03-07 – Multi-User Experience

> Completes Milestone 1 (Team-Ready Server) and Milestone 2 (Multi-User Experience). Harux is now fully multi-user capable, production-configurable, and deployed as a stable Windows service.

### Added
- **Bookmark/pin messages** — star icon on each message row to bookmark important messages; bookmarked messages are protected from eviction; state syncs across tabs via WebSocket (#27)
- **Message tagging** — manual tagging of messages for attribution (e.g., "Bug #1234"); tags filterable via search and synchronized across clients (#26)
- **WebSocket exponential backoff** — reconnection uses exponential backoff (1s to 60s cap) with jitter to prevent thundering herd; status bar shows reconnection countdown (#12)
- **Resizable panel splitter** — drag the border between message list and detail panel to resize; double-click to reset; width persists across sessions via localStorage (#4)
- **Color-coded source markers** — messages in the list show a colored dot mapped by source IP address (with an optional "Color by Port" toggle), along with a source legend for quick identification (#25)
- **Session-based views** — each developer sees their own filter configuration, active tab, and scroll position independent of other users via `sessionStorage` (#24)
- **Connection limits** — configurable `max_connections` (default 100) for the MLLP server using a `tokio::sync::Semaphore`; rejected connections are counted and exposed via `/api/stats` (#6)
- **ACK UI** — sent ACK/NACK messages (AA, AE, AR) are now stored and viewable in an "ACK" tab (#19)
- **Graceful shutdown** — MLLP server active connections are cleanly drained on `Ctrl+C` or service stop before exiting (#7)
- **Configuration file** (`harux.toml`) — ports, memory limits, log level, MLLP timeouts and max message size configurable without recompilation
  - Load priority: config file (next to binary or CWD) → environment variables (`MLLP_PORT`, `WEB_PORT`, `RUST_LOG`) → built-in defaults
  - New `src/config.rs` module with `Config`, `ServerConfig`, `LoggingConfig`, `StoreConfig`, `MllpConfig` structs
  - Example `harux.toml` included with all defaults commented out
- STYLE_GUIDE.md detailing design, architecture, and workflow conventions
- Branch protection rules (main branch requires PRs and successful CI checks)
- GitHub Actions CI workflow (`.github/workflows/ci.yml`) for `fmt`, `clippy`, `build`, and `test`
- Templates for Bugs and Feature Requests
- CONTRIBUTING, SECURITY, and Pull Request templates
- **Tests**: Add regression test for MSH field indexing quirk (#11)

### Changed
- `MessageStore::new()` now accepts `StoreConfig` — store capacity and memory limit are configurable
- `start_mllp_server()` now accepts `MllpConfig` — timeouts and max message size are configurable
- Hardcoded constants (`DEFAULT_CAPACITY`, `MAX_STORE_BYTES`, `MAX_MESSAGE_SIZE`, `READ_TIMEOUT`, `WRITE_TIMEOUT`) replaced with config values
- Effective configuration is logged at startup
- Translated all German text to English across the codebase
- Documentation refactor — merged MILESTONES.md into ROADMAP.md; separated concerns across README (landing page), ROADMAP (planning), CHANGELOG (history), STYLE_GUIDE (rules), PROJECT_OVERVIEW (architecture + decisions)

### Fixed
- Fixed XSS vulnerabilities in message parsed view by escaping segment data and removing inline onclick handlers (#20)
- Fixed UI sync on clear database
- Fixed clippy warnings: derivable impl, char comparison pattern, `to_string` in format args, large enum variant

### Commit History (chronological)

#### 2026-03-07

| Commit | Description |
|--------|-------------|
| [`5e4c4ec`](https://github.com/Pappet/harux/commit/5e4c4ec7a360c0b9bb249cc7c138454ca99a3a52) | `docs:` Remove Windows Service tasks from Milestone 1, add issue-based branch naming |
| [`70723f5`](https://github.com/Pappet/harux/commit/70723f5bffd6a9a33cd682ee8408c87bfb8e38a1) | `docs:` Refactor documentation structure, merge MILESTONES.md into ROADMAP.md |
| [`5cc749f`](https://github.com/Pappet/harux/commit/5cc749fe76e9f29c6827d888b702ff09dba61e0c) | `feat:` Bookmark/pin messages with eviction protection (#27) |

#### 2026-03-06

| Commit | Description |
|--------|-------------|
| [`a143155`](https://github.com/Pappet/harux/commit/a143155) | `feat:` show ACK message in message selection (#19) |

#### 2026-03-05

| Commit | Description |
|--------|-------------|
| [`16d4da8`](https://github.com/Pappet/harux/commit/16d4da82c54484485c7af0d56d00cef1f78e7b49) | `test:` Add regression tests for MSH field indexing quirk (#11) |

#### 2026-02-28

| Commit | Description |
|--------|-------------|
| [`4ea0538`](https://github.com/Pappet/harux/commit/4ea05380ab3adaef719f6afb5eb23bb45dd8e5f0) | `docs:` Add STYLE_GUIDE.md detailing design, architecture, and workflow conventions |

#### 2026-02-22

| Commit | Description |
|--------|-------------|
| [`5cf9f29`](https://github.com/Pappet/harux/commit/5cf9f29) | `docs:` document Branch Protection and CI workflow rules in CLAUDE.md, AGENTS.md, and CHANGELOG.md |
| [`da5738a`](https://github.com/Pappet/harux/commit/da5738a5ced7625ddf524b87fd0a5e6558b1f275) | `feat:` add harux.toml configuration file support |
| [`29e5e8c`](https://github.com/Pappet/harux/commit/29e5e8c6a7336400a1e02e05687ad1976cfec330) | `docs:` add CONTRIBUTING, SECURITY, and PR template |
| [`89ba306`](https://github.com/Pappet/harux/commit/89ba3063590a0ee3aef05f0e3ebf5b07921dcfd4) | `chore:` Add Templates for Bugs and Feature Requests |

#### 2026-02-21

| Commit | Description |
|--------|-------------|
| [`efb0bcc`](https://github.com/Pappet/harux/commit/efb0bcc2025500ba7e26e56494dfc8aefbbd0e33) | `docs:` Update ROADMAP with completed Phase 1 tasks, refine deployment and memory management sections, clarify non-goals, and add development guidelines. |
| [`8128ab4`](https://github.com/Pappet/harux/commit/8128ab456f0a0ff23c3739481fd2194a8934a3aa) | `docs:` add detailed issue comment preferences for AI agents |
| [`a1837a4`](https://github.com/Pappet/harux/commit/a1837a45d7af2898ee1f693e78d509b56688f51a) | `fix:` Fix UI sync on clear database |
| [`09121a5`](https://github.com/Pappet/harux/commit/09121a50a319c494c233f050df7eb561aa4c49f0) | `docs:` translate all German text to English across the codebase |

---

## [0.2.0] – 2026-02-21 – Stabilization & Robustness

> ACK storm prevention, size-based store eviction, UI improvements and error handling for production use with Orchestra/MDM traffic.

---

### Commit History (chronological)

#### 2026-02-21

| Commit | Description |
|--------|-------------|
| [`868b440`](https://github.com/Pappet/harux/commit/868b440) | `docs:` fix commit hash in CHANGELOG for fdb6e05 |
| [`fdb6e05`](https://github.com/Pappet/harux/commit/fdb6e05) | `docs:` document mandatory changelog workflow in CLAUDE.md and AGENTS.md |
| [`79cec90`](https://github.com/Pappet/harux/commit/79cec901f6141ba32c719dda2829e15d20f9f5d3) | `docs:` update CLAUDE.md, add AGENTS.md with architecture and deployment context |
| [`041cb04`](https://github.com/Pappet/harux/commit/041cb0417f821b354fce1c021148d4e18a78cd01) | `fix:` ACK storm prevention, search debounce (300 ms), size-based store eviction (`MAX_STORE_BYTES`) |
| [`44afeb9`](https://github.com/Pappet/harux/commit/44afeb9364ce45d2073da84cd840d72ccd6e1882) | `.gitignore` updated |

#### 2026-02-20

| Commit | Description |
|--------|-------------|
| [`1ee69f3`](https://github.com/Pappet/harux/commit/1ee69f3655a286ea6dfeb2bec71d49f6f56a5270) | `.gitignore` extended |
| [`2f45c7e`](https://github.com/Pappet/harux/commit/2f45c7e19f1f8c966e857396bc1df178bc0abb4f) | `ux:` various UX optimizations |
| [`f10cd40`](https://github.com/Pappet/harux/commit/f10cd40efab007e5d629942ed6cd91611488d468) | `feat:` store failed messages; introduce UI batching (250 ms); split static assets into separate files (`index.html`, `style.css`, `app.js`) |
| [`ee0dcda`](https://github.com/Pappet/harux/commit/ee0dcda6cd673733bd37e9ab01e9b6933625264b) | `docs:` add PowerShell test script (`tests/test.ps1`); document both test runners |
| [`1bd7c9f`](https://github.com/Pappet/harux/commit/1bd7c9f841fee412d172599e137e8a91bb06eb16) | create PowerShell test script for Windows load tests (1000 messages, persistent TCP connection) |
| [`7a08611`](https://github.com/Pappet/harux/commit/7a0861102763aed1898eb4c24f1d344978a92cb7) | rename social card image (`harux-card.png` → `social-card.png`) |
| [`fd7178a`](https://github.com/Pappet/harux/commit/fd7178ae69eb4dfe90fb8e5725c2d51db15e9da6) | `docs:` completely revamp README.md |
| [`a6860b1`](https://github.com/Pappet/harux/commit/a6860b125d40031c131a9c8dc4a9f7e64c3d7b10) | `.gitignore` extended |
| [`71eb33c`](https://github.com/Pappet/harux/commit/71eb33cdb4d232977b4045c26531de2e4f3b8b0a) | `ci:` migrate release upload from `softprops/action-gh-release` to `gh release upload` |
| [`564aba3`](https://github.com/Pappet/harux/commit/564aba33437afc23e8657f8518982c89366414a8) | `ci:` simplify build pipeline to three independent jobs: Windows, macOS Apple Silicon, Linux |
| [`c8711e8`](https://github.com/Pappet/harux/commit/c8711e840d6e1db31867f0577be01a4d2ec03fda) | `ci:` cross-compile Intel macOS binary on Apple Silicon runner |
| [`acc9908`](https://github.com/Pappet/harux/commit/acc9908b5c222fbfa7956d87c8cabe0083c27dd1) | `ci:` add macOS builds for Intel and Apple Silicon |
| [`8c0db5f`](https://github.com/Pappet/harux/commit/8c0db5fd3821715e03bae3c8a7f17ac90cfeff7d) | `ci:` set `contents: write` permission for release asset upload |
| [`9a14291`](https://github.com/Pappet/harux/commit/9a14291ba9281c9b135cf8d3b07ae73934db7cbd) | add MIT license (`LICENSE`) |
| [`5bd3579`](https://github.com/Pappet/harux/commit/5bd357946f568dc7806ea0ca38f4367023c014a4) | `docs:` clarify ACK behavior for unknown message types; revise `test.sh` |
| [`9ef8ed9`](https://github.com/Pappet/harux/commit/9ef8ed9cf631b442ebf6e10d7767fcdff5b6e3c1) | `fix:` align MSH field indices with HL7 standard (correct +1 offset); add graceful shutdown via `Ctrl+C` signal handler |
| [`bbde980`](https://github.com/Pappet/harux/commit/bbde98060ccc46fbe5a9ed238907a73ebcd9af21) | `fix:` harden MLLP server and message store against load spikes and DoS (connection timeouts, 10 MB payload limit) |
| [`fa11aa4`](https://github.com/Pappet/harux/commit/fa11aa42ae65bde3926349bea3ee81d2b3d9714c) | `polish:` add Cargo metadata; clean up Tokio features; introduce toast notifications in UI |
| [`f33ccfc`](https://github.com/Pappet/harux/commit/f33ccfccdfb12e16f4879e06fb8a3a9b8802919) | `fix:` UI polish and pre-release fixes (correct Axum route `:id`, clean up compiler warnings) |
| [`696522c`](https://github.com/Pappet/harux/commit/696522c4126fc45096687fdb5ef38d6462f593b2) | `docs:` revamp README with feature overview, Windows deployment guide, and milestone table |
| [`679aad3`](https://github.com/Pappet/harux/commit/679aad3e02d1d0c89299ad8f22b38e95b10bb37c) | `ci:` initial GitHub Actions build workflow |
| [`0c46811`](https://github.com/Pappet/harux/commit/0c468114f5080b9da02ea6e3b4a22796e56337f2) | `docs:` add ROADMAP.md as strategic planning document |
| [`6cedfc6`](https://github.com/Pappet/harux/commit/6cedfc6b8cd47c94cf475d6905a00b53f7540fc1) | `docs:` create MILESTONES.md with 6 structured milestones from ROADMAP phases 2–4 |
| [`f087a62`](https://github.com/Pappet/harux/commit/f087a62b435ecf3d8e6e7d9dc7c5902f4d9d8b82) | `docs:` add CLAUDE.md with build commands and architecture overview for AI agents |
| [`f6fef07`](https://github.com/Pappet/harux/commit/f6fef074115caf756797f5257578349c583c7bec) | **Initial commit:** Harux MLLP server with real-time web UI |

---

### Added

#### MLLP Server (Backend)
- Asynchronous TCP listener based on **Tokio** (`rt-multi-thread`)
- Correct MLLP framing: start block `0x0B`, end block `0x1C 0x0D` per the HL7 MLLP standard
- **ACK/NAK generation**: automatic response with `AA` (Application Accept) for valid messages and unknown message types; `AE` (Application Error) for missing or malformed MSH segments
- **ACK storm prevention**: incoming ACK messages are never ACK'd back
- Parallel client connections via `tokio::spawn` per connection
- **DoS hardening**: 10 MB payload limit, connection timeouts
- **Graceful shutdown**: clean termination of active connections on `Ctrl+C` via signal handler

#### HL7 v2.x Parser
- Dynamic delimiter detection from the MSH segment (field separator, component, subcomponent, escape, and repetition separators)
- Extraction of key MSH fields: message type, trigger event, sending/receiving facility & application, message ID, timestamp
- MSH field indices correctly aligned with the HL7 standard (+1 offset)
- PID segment extraction: patient ID (PID-3), patient name (PID-5)
- Robust error handling: parse errors result in a PARSE ERROR marker in the UI rather than a server crash; failed messages are stored in the store
- Structured segment and field representation (`Hl7Message`, `Hl7Segment`, `Hl7Field`, `Delimiters`)

#### In-Memory Message Store
- Central store with `Arc<RwLock<>>` for thread-safe access
- **Dual eviction**: messages are evicted when either the maximum count (`DEFAULT_CAPACITY`) or the maximum byte size (`MAX_STORE_BYTES`) is exceeded — no OOM
- Broadcast channel (`tokio::sync::broadcast`) for real-time notification of all active WebSocket clients on new messages
- Each message receives a unique UUID v4 and an ISO 8601 receive timestamp

#### Web API (Axum)
- `GET /api/messages` — list of all stored messages (paginated, as `Hl7MessageSummary`)
- `GET /api/messages/:id` — full message details including parsed segments and raw HL7
- `GET /api/search` — search endpoint with query parameter `q`
- `GET /api/stats` — live statistics: received messages, parse errors, active connections, MLLP port
- `POST /api/clear` — clear the store
- `GET /ws` — WebSocket endpoint for real-time message push
- CORS middleware for browser access
- Embedded static files via `rust-embed` (no separate web server required)

#### Web UI (Embedded SPA, Vanilla JS)
- **Real-time message list** via WebSocket — new messages appear instantly without page reload
- **Batch rendering** every 250 ms — prevents DOM freezing at high message volumes
- **Pause / Live mode** — button buffers incoming messages; flush and return to live mode
- **Toast notifications** — subtle in-app notifications for relevant events
- **Detail view** with three tabs: `Parsed`, `Raw`, `JSON`
- **Search & Filter** — by message type, patient name, facility, message control ID, source IP; supports `has:warnings`/`has:errors` (#60)
- **Validation Filter** — 3-state toggle in the header to quickly isolate messages with warnings or errors (#60)
- **Bookmark & Tag** — pin important messages (eviction-protected), add custom text tags
- **JSON export** — download individual messages as `.json`
- Dark theme with CSS variables
- PARSE ERROR marker in red for failed messages

#### Build & Deployment
- Single Rust binary, no external runtime dependencies
- **GitHub Actions CI/CD**: three independent build jobs for Windows (`.exe`), macOS Apple Silicon, and Linux on every push to `main`
- Build artifacts are automatically attached to GitHub Releases
- MIT license
- Test scripts: `tests/test.sh` (Linux/macOS, netcat, 100 messages) and `tests/test.ps1` (Windows, .NET TcpClient, 1000 messages, persistent connection)

### Fixed

- MSH field indices aligned with the correct HL7 standard offset (`9ef8ed9`)
- Route for the message detail view corrected from `{id}` to `:id` (correct Axum syntax) (`f33ccfc`)
- Compiler warnings (`dead_code`, `unused_variables`) cleaned up (`f33ccfc`)
- ACK storm prevention: incoming ACK messages are detected and never ACK'd back (`041cb04`)

---

| [`47d1fa7`](https://github.com/Pappet/harux/commit/47d1fa7) | `security:` fix XSS vulnerability in JS event handlers |

---

## [0.3.0] – 2026-03-01 – UI Enhancements & Security Fixes

> This release includes a critical security fix for a Cross-Site Scripting (XSS) vulnerability in the UI.

### Added
### Fixed
- Fixed a Cross-Site Scripting (XSS) vulnerability in the UI caused by improper escaping of user input in JavaScript event handlers.

---

[Unreleased]: https://github.com/Pappet/harux/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/Pappet/harux/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/Pappet/harux/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Pappet/harux/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Pappet/harux/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Pappet/harux/releases/tag/v0.1.0
