# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)
and this project follows [Semantic Versioning](https://semver.org/lang/en/).

---

## [Unreleased]

### Added
- **Keyboard accessibility for message rows** — message list rows now support `Tab` focus navigation and `Enter`/`Space` to select; a 2px accent outline appears on `:focus-visible`. Improves screen-reader and keyboard-only workflows.
- **ARIA labels on message rows + bookmark/pin controls** — each row exposes a synthesized `aria-label` (facility, type, time); the bookmark and pin controls were promoted from `<span>` to native `<button>` so they receive `Tab` focus, keyboard activation, and `aria-label`. Pin clicks now `stopPropagation` so the row beneath does not also select.
- **Global `:focus-visible` outline** — every focusable element gets a 2 px accent outline when reached via keyboard, addressing missing focus rings across header buttons, search input, and inline controls.

### Fixed
- **Pin click leaked to row selection** — clicking the diff-pin icon previously also selected the underlying message row. `toggleDiffPin` now calls `event.stopPropagation()` when invoked from the row.

### Changed
- **Typography** — adopted Inter (sans) and JetBrains Mono (mono) via Google Fonts; first phase of the v0.5.0 UI redesign (#86)
- **Color palette** — refreshed dark theme: deeper backgrounds (`--bg-primary` `#0b0d12`, `--bg-secondary` `#131620`, `--bg-tertiary` `#1b1f2c`), cooler accent (`--accent` `#7aa2ff`), tuned success/error tones (`--success` `#4fb98a`, `--error` `#ea6363`) for higher contrast against the deeper canvas (#86)

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

#### 2026-05-08

| Commit | Description |
|--------|-------------|
| [`a14f0de`](https://github.com/Pappet/harux/commit/a14f0de20053257e6b9cd2b9e6063fc8e2daf8dc) | `fix:` resolve ISO-8859-1 charset encoding issues (#78) |
| [`a6f1ce7`](https://github.com/Pappet/harux/commit/a6f1ce7e1e8deb5dbead3248e099423231b1cd09) | `feat(a11y):` ARIA labels, native buttons, global focus ring |
| [`1993bcb`](https://github.com/Pappet/harux/commit/1993bcb07709460bce5d468c046be6e2f1db533c) | `feat(a11y):` keyboard navigation for message list rows |
| [`f5c650a`](https://github.com/Pappet/harux/commit/f5c650aa025132a5a6e660092957fd3ad69099ec) | `feat(ui):` typography + color foundation for v0.5.0 redesign (#86) |

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

[Unreleased]: https://github.com/Pappet/harux/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/Pappet/harux/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Pappet/harux/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Pappet/harux/releases/tag/v0.1.0
