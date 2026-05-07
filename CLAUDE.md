# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Git Workflow — Branch + PR Required

**Never commit directly to `main` or to an unrelated branch.** For every task:

1. **Create a feature branch** from `main` named after the issue (e.g., `feat/27-bookmark-messages`, `fix/15-eviction-bug`).
2. **Make all commits on that branch.**
3. **Create a Pull Request** to `main` using `gh pr create` when the work is complete and all checks pass.
4. Include the related issue number in the PR body (e.g., `Closes #27`).

Do not push changes without a dedicated branch and PR — even for small fixes.

## Build & Run Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo run --release            # Run server (MLLP + Web)
cargo test                     # Run all unit tests
cargo test <test_name>         # Run a single test
cargo clippy -- -D warnings    # Must pass clean before every commit
```

Environment variables: `MLLP_PORT` (default 2575), `WEB_PORT` (default 8080), `RUST_LOG` for tracing.

Manual MLLP testing: `./test.sh` (Linux/macOS) or `.\tests\test.ps1` (Windows) — sends ADT^A01, ORU^R01, SIU^S12 plus error cases and a load test.
## Branch Protection & CI

The `main` branch is protected. All changes must be made via Pull Requests. 
Before a PR can be merged, the automated CI workflow will run the following checks and they all **must pass**:
1. `cargo fmt --check`
2. `cargo clippy -- -D warnings`
3. `cargo build`
4. `cargo test`

You must ensure your code satisfies these requirements locally before pushing.

## Commit & Changelog Workflow

**Before every commit and push**, update `CHANGELOG.md`:

1. Add a new row to the commit-history table under today's date (add a new `#### YYYY-MM-DD` heading if it doesn't exist yet).
2. Row format: `| [\`<short-hash>\`](<https://github.com/Pappet/harux/commit/<full-hash>>) | \`<type>:\` Short description |`
   — The short hash is the first 7 characters of the commit hash.
3. For substantive changes (new feature, fix, refactor): also update the relevant `### Added`, `### Fixed`, or `### Changed` entries in the `[Unreleased]` or the active release section.
4. The changelog format follows [Keep a Changelog](https://keepachangelog.com/de/1.0.0/).

The commit hash is only available **after** `git commit`. Update the changelog as part of the same commit — amend or add a follow-up commit if needed. Do not create a separate "update changelog" commit when it can be included in the main commit.

## Architecture

Harux is an MLLP server with a real-time web UI for inspecting HL7 v2.x messages. Two async Tokio tasks run concurrently via `tokio::select!` in `main.rs`:

1. **MLLP Server** (`mllp.rs`) — TCP listener accepting HL7 messages wrapped in MLLP framing (VT `0x0B` start, FS `0x1C` + CR `0x0D` end). Parses each message, stores it, and returns ACK/NACK.

2. **Web Server** (`web.rs`) — Axum HTTP server serving a REST API and WebSocket endpoint. The SPA frontend (`static/`) is embedded in the binary via `rust-embed`.

**Shared state** flows through `MessageStore` (`store.rs`) — an `Arc<RwLock<>>` in-memory store with a `tokio::sync::broadcast` channel that pushes new messages to WebSocket subscribers in real-time.

### Store Capacity & Eviction

- **Count limit:** `DEFAULT_CAPACITY = 10_000` messages (secondary safeguard)
- **Size limit:** `MAX_STORE_BYTES = 512 MB` (primary safeguard)
- **Trigger:** either limit hit → evict oldest 10% of messages
- **Reason:** MDM messages with Base64-encoded attachments can be several MB each; count-only eviction is insufficient for real-world Orchestra traffic

### HL7 Parsing (`src/hl7/`)

- `parser.rs` — Parses raw HL7 text by extracting delimiters from the MSH segment, splitting on `\r`/`\n` into segments, and decomposing fields/components. Also builds ACK responses.
- `types.rs` — Data structures: `Hl7Message`, `Hl7MessageSummary`, `Hl7Segment`, `Hl7Field`, `Delimiters`.

**⚠ MSH field indexing quirk:** MSH-1 is the field separator character itself (`|`). The parser inserts a synthetic `Hl7Field { index: 1, value: "|" }` and shifts all other fields up by 1, so that `get_field_value(msh, 3)` correctly returns Sending Application (HL7 standard). Do not change this logic without understanding it — it will silently break all MSH field extractions.

### MLLP Protocol

- Start: `0x0B` (VT, Vertical Tab)
- End: `0x1C 0x0D` (FS + CR)
- Max payload: 10 MB — connection closed immediately if exceeded
- Read timeout: 60s, Write timeout: 30s
- **ACK storm prevention:** incoming messages with `message_type.starts_with("ACK")` are stored but never ACK'd back. Violating this rule creates an infinite ACK ping-pong with Orchestra.

### Web API Routes

- `GET /api/messages?offset=&limit=` — Paginated message list (newest first)
- `GET /api/messages/{id}` — Full message with segments
- `GET /api/search?q=&limit=` — Search messages (client-side in UI, server-side via this endpoint)
- `GET /api/stats` — Server statistics
- `POST /api/clear` — Clear all messages
- `WS /ws` — Real-time updates ("init", "new_message", "lagged" events)

### Frontend (`static/`)

Three files, all embedded into the binary at compile time via `rust-embed`:

```
static/
├── index.html   # HTML structure only
├── style.css    # All styles (dark theme, CSS variables)
└── app.js       # All logic (WebSocket, rendering, search, batching)
```

**Important:** After any change to `static/`, a recompile is required — `rust-embed` bakes the files in at build time. There is no hot-reload.

The frontend is **intentional vanilla JS — no framework**. Do not introduce React, Vue, Svelte, or any build toolchain. The embedded SPA approach is a deliberate architectural decision for zero-dependency deployment.

Key frontend behaviors:
- Messages are batched and rendered at most every 250ms (prevents DOM freeze at high message rates)
- ⏸ Pause/▶ Live button buffers incoming messages without displaying them
- Search is purely client-side (filters `messages[]` array via `matchesSearch()`)
- Search input is debounced 300ms as a safeguard for a future `/api/search` call
- Parse errors are shown with `⚠ PARSE ERROR` in red (`var(--error)`) in the message list

## Deployment Context

- **Primary target:** Windows Server (`.exe`), run as a Windows Service via NSSM
- **Build pipeline:** GitHub Actions builds Windows, macOS (Apple Silicon), and Linux binaries on every push to `main` and attaches them to releases
- **Users:** Multiple developers simultaneously via browser — no local setup, no RDP window

## Constraints — What NOT to Change Without Explicit Instructions

| Area | Constraint |
|---|---|
| Frontend | No framework, no build toolchain, no npm |
| Parser | No zero-copy / lifetime refactoring (premature optimization) |
| Store locking | No dashmap, no mpsc-channel refactor (planned for Milestone 1) |
| DOM rendering | No prepend logic (batching at 250ms is sufficient) |
| Dependencies | Minimize new crates; check `Cargo.toml` before adding |

## Current Milestone: Milestone 3 — Message Analysis

Next planned work (see `ROADMAP.md` for full details):
- HL7 field dictionary (hover tooltips with field descriptions)
- Message type detection with descriptions
- Validation (required fields per message type)
- Segment diff (compare two messages side by side)

Do not implement features from later milestones speculatively.

## Source Layout

```
src/
├── main.rs          # Entry point, tokio::select! over MLLP + Web tasks
├── mllp.rs          # TCP listener, MLLP framing, ACK/NACK dispatch
├── store.rs         # In-memory store with broadcast channel, dual eviction
├── web.rs           # Axum router, REST handlers, WebSocket handler
└── hl7/
    ├── mod.rs
    ├── parser.rs    # Raw HL7 → Hl7Message, delimiter extraction, ACK builder
    └── types.rs     # Hl7Message, Hl7MessageSummary, Hl7Segment, Hl7Field, Delimiters
static/
├── index.html       # HTML skeleton
├── style.css        # Dark theme, CSS variables
└── app.js           # SPA logic (vanilla JS)
tests/
├── test.sh          # Linux/macOS functional + load test (netcat)
└── test.ps1         # Windows functional + load test (.NET TcpClient, 1000 msg)
```
---

## Communication Preferences

When interacting with the user or external systems (like GitHub issues), you MUST provide detailed, structured, and comprehensive explanations. 

Specifically for GitHub issues:
- **Do not** write short, generic closing comments (e.g., "Fixed bug and pushed").
- **Do** write a detailed `### Fix Details` section that explains *why* the issue occurred, the architectural or code-level changes made to fix it, and how the fix was verified. Reference specific files (`src/store.rs`, `static/app.js`, etc.) and the logic changed.
