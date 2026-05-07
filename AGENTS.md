# AGENTS.md

This file provides guidance to AI coding agents working with this repository.
For Claude Code specifically, see also `CLAUDE.md`.

---

## Project Overview

Harux is a single-binary MLLP server with an embedded real-time web UI for inspecting HL7 v2.x messages. It is written in Rust and deployed as a Windows Service on a hospital integration server. Multiple developers use it simultaneously via browser.

**Primary use case:** Receiving, storing, and displaying HL7 v2.x messages from Orchestra (a healthcare integration platform) for development and debugging. MDM messages with Base64-encoded attachments are common daily traffic.

---

## Git Workflow — Branch + PR Required

**Never commit directly to `main` or to an unrelated branch.** For every task:

1. **Create a feature branch** from `main` named after the issue (e.g., `feat/27-bookmark-messages`, `fix/15-eviction-bug`).
2. **Make all commits on that branch.**
3. **Create a Pull Request** to `main` using `gh pr create` when the work is complete and all checks pass.
4. Include the related issue number in the PR body (e.g., `Closes #27`).

Do not push changes without a dedicated branch and PR — even for small fixes.

---

## Build & Verification

Always verify your changes compile and pass tests before considering a task complete.

```bash
cargo build --release          # Release build
cargo test                     # All unit tests must pass
cargo clippy -- -D warnings    # Zero warnings policy — must pass clean
```

If any of these three commands fails, the task is not done.

## Branch Protection & CI

The `main` branch is protected. Direct pushes are not allowed. 
All changes must go through a Pull Request and pass an automated CI workflow that runs:
1. `cargo fmt --check`
2. `cargo clippy -- -D warnings`
3. `cargo build`
4. `cargo test`

Only PRs that pass these checks can be merged. Ensure these run successfully on your local branch before creating a PR.

## Changelog — Mandatory on Every Commit

`CHANGELOG.md` must be updated as part of every commit. Do not push without it.

**Steps:**

1. After `git commit`, note the first 7 characters of the commit hash.
2. Open `CHANGELOG.md` and find (or create) the `#### YYYY-MM-DD` heading for today under `## [Unreleased]` → commit history table.
3. Add one row per commit:
   ```
   | [`<hash>`](https://github.com/Pappet/harux/commit/<full-hash>) | `<type>:` Short description |
   ```
4. For features or fixes: also update the corresponding `### Added` / `### Fixed` / `### Changed` prose section of the active release block.
5. Stage `CHANGELOG.md` and amend the commit (`git commit --amend --no-edit`) **or** include it in the same commit from the start.

Never create a standalone "update changelog" commit — it belongs to the commit it documents.

---

## Architecture

Two async Tokio tasks share a single `MessageStore` via `Arc<RwLock<>>`:

```
HL7 Sender (Orchestra) ──MLLP──▶ mllp.rs ──▶ store.rs ──▶ web.rs ──▶ Browser
                        ◀──ACK──                         WebSocket
```

| File | Responsibility |
|---|---|
| `src/main.rs` | Entry point, `tokio::select!` over both tasks |
| `src/mllp.rs` | TCP listener, MLLP framing, ACK/NACK, ACK-storm prevention |
| `src/store.rs` | In-memory store, dual eviction (size + count), broadcast channel |
| `src/web.rs` | Axum REST API, WebSocket push, static file serving |
| `src/hl7/parser.rs` | Raw HL7 → `Hl7Message`, delimiter extraction, ACK builder |
| `src/hl7/types.rs` | All data structures |
| `static/index.html` | HTML skeleton only |
| `static/style.css` | Dark theme, CSS variables |
| `static/app.js` | All SPA logic (vanilla JS, no framework) |

---

## Critical Knowledge — Read Before Changing Anything

### 1. MSH Field Index Offset

HL7 standard: MSH-1 is the field separator character (`|`), not the first data field. The parser accounts for this by inserting a synthetic field at index 1 and shifting all parsed fields up by 1:

```rust
// In parser.rs — parse_segment()
if name == "MSH" {
    for field in fields.iter_mut() {
        field.index += 1;
    }
    fields.insert(0, Hl7Field { index: 1, value: sep.to_string(), ... });
}
```

**Do not remove or "simplify" this.** It ensures `get_field_value(msh, 3)` correctly returns Sending Application per the HL7 spec. Breaking this silently corrupts all MSH field extractions without a compile error.

### 2. ACK Storm Prevention

Incoming messages whose `message_type` starts with `"ACK"` must never receive an ACK response. ACK-of-ACK creates an infinite ping-pong loop with Orchestra that takes down the connection channel.

The check in `mllp.rs` is intentional and must be preserved:

```rust
if msg.message_type.starts_with("ACK") {
    // store but do NOT send ACK
} else {
    // send ACK normally
}
```

### 3. MLLP Framing

MLLP is not a text protocol. Messages are wrapped in binary control characters:

- **Start:** `0x0B` (VT, Vertical Tab)
- **End:** `0x1C 0x0D` (FS + CR)

Do not treat accumulated bytes as UTF-8 text until after the frame boundaries are extracted. `String::from_utf8_lossy` is intentional in `extract_mllp_frame`.

### 4. Store Eviction — Dual Trigger

The store evicts the oldest 10% of messages when **either** limit is reached:

- `DEFAULT_CAPACITY = 10_000` messages (count, secondary)
- `MAX_STORE_BYTES = 512 MB` (byte size of `raw` fields, primary)

The size-based limit exists because MDM messages with Base64 attachments can be several MB each. Evicting on count alone would allow OOM before the count limit is reached. Do not remove `current_bytes` tracking or revert to count-only eviction.

When evicting, subtract the freed bytes from `current_bytes` using `saturating_sub`. Reset `current_bytes` to 0 in `clear()`.

### 5. Static Files Are Compile-Time Embedded

`rust-embed` bakes the contents of `static/` into the binary at compile time. There is no hot-reload. Any change to `static/index.html`, `static/style.css`, or `static/app.js` requires a recompile to take effect.

### 6. Parse Errors Are Stored, Not Discarded

When a message fails to parse, it is still inserted into the store with `parse_error: Some(error_string)` and `message_type: "UNKNOWN"`. This is intentional — Harux is a debugging tool and broken messages are often the most important ones to inspect. Do not silently discard parse failures.

---

## Constraints — What Agents Must Not Change Without Explicit Instructions

| Area | Rule |
|---|---|
| Frontend | **No framework.** Vanilla JS only. No React, Vue, Svelte, npm, or build toolchain. |
| Frontend | **No CSS frameworks.** All styles live in `style.css` with CSS variables. |
| Parser | No zero-copy / lifetime refactoring. Correctness over performance here. |
| Store | No `dashmap`. No `mpsc`-channel refactor. The `Arc<RwLock<>>` is intentional for now. |
| DOM rendering | No prepend/virtual DOM logic. Full re-render on 250ms batch is acceptable. |
| Dependencies | Do not add crates without explicit instruction. Check `Cargo.toml` first. |
| Eviction | Do not revert to count-only eviction or raise `DEFAULT_CAPACITY` back to 100k. |
| ACK behavior | Do not ACK incoming ACK messages under any circumstance. |

---

## Deployment Context

Agents should make decisions with this context in mind:

- **OS:** Windows Server (primary). The `.exe` runs as a Windows Service via NSSM.
- **Users:** Multiple developers simultaneously via browser. Not a single-user tool.
- **Network:** Hospital internal network. No internet access from the server.
- **Traffic:** HL7 v2.x from Orchestra. Includes ADT, ORU, ORM, SIU, and MDM message types. MDM with Base64-encoded PDF attachments is daily traffic, not an edge case.
- **Deployment:** Single binary, xcopy-style. No installer, no runtime dependencies, no Docker.

---

## Planned Work — Do Not Implement Speculatively

The following features are planned but not yet implemented. Do not add them unless explicitly asked:

- HL7 field dictionary tooltips (Milestone 3)
- Message type detection and validation (Milestone 3)
- Segment diff view (Milestone 3)
- Message replay / editor (Milestone 4)
- SQLite persistence (Milestone 5)
- FHIR R4 mapping (Milestone 6)

Full roadmap in `ROADMAP.md`.

---

## Testing

```bash
# Unit tests
cargo test

# Functional + load test (Linux/macOS, requires nc)
./test.sh

# Functional + load test (Windows, no external tools required)
.\tests\test.ps1
```

The PowerShell test uses a persistent TCP connection for load testing (1000 messages). The shell script spawns a new `nc` process per message (100 messages) — inherently slower due to process overhead, not server throughput.

---

## Common Mistakes to Avoid

- **Changing MSH field indices** without understanding the +1 offset in `parse_segment()`
- **Removing the ACK-type check** in `mllp.rs` thinking it's dead code
- **Adding a framework** to `static/` — this breaks the embedded single-binary deployment
- **Raising store capacity** to 100k messages — OOM risk with large MDM payloads
- **Forgetting to reset `current_bytes`** when clearing or evicting messages
- **Treating MLLP as plain TCP text** — always extract frame boundaries first
- **Only running `cargo build`** without `cargo test` and `cargo clippy -- -D warnings`
---

## Communication Preferences

When interacting with the user or external systems (like GitHub issues), you MUST provide detailed, structured, and comprehensive explanations. 

Specifically for GitHub issues:
- **Do not** write short, generic closing comments (e.g., "Fixed bug and pushed").
- **Do** write a detailed `### Fix Details` section that explains *why* the issue occurred, the architectural or code-level changes made to fix it, and how the fix was verified. Reference specific files (`src/store.rs`, `static/app.js`, etc.) and the logic changed.
