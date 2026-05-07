<div align="center">

<img src="https://raw.githubusercontent.com/Pappet/harux/main/assets/social-card.png" alt="Harux" width="100%">

<br/>
<br/>

[![Build](https://img.shields.io/github/actions/workflow/status/Pappet/harux/build.yml?branch=main&style=flat-square&logo=github&label=build&color=4caf84)](https://github.com/Pappet/harux/actions)
[![Release](https://img.shields.io/github/v/release/Pappet/harux?style=flat-square&color=6c8cff&logo=rust)](https://github.com/Pappet/harux/releases)
[![License](https://img.shields.io/badge/license-MIT-blue?style=flat-square&color=525775)](LICENSE)
[![Rust](https://img.shields.io/badge/built%20with-Rust-b7410e?style=flat-square&logo=rust)](https://www.rust-lang.org/)

<br/>

**Harux** is a high-performance MLLP server with a real-time web UI for inspecting HL7 v2.x messages — built in Rust, deployed as a single binary.

Designed as a drop-in replacement for HL7 Inspector: runs as a central service, accessible by your entire team via browser. No local setup. No RDP-exclusive windows.

<br/>

[**Download**](https://github.com/Pappet/harux/releases) · [**Docs**](#-documentation) · [**Quickstart**](#-quick-start)

</div>

---

## Features

- **MLLP Server** — async TCP listener with correct `0x0B`/`0x1C 0x0D` framing, auto ACK/NACK
- **Real-time Web UI** — browser SPA with WebSocket push, no page reload, no framework
- **Deep HL7 Parser** — dynamic delimiter detection, full segment/field/component decomposition
- **Five Message Views** — Parsed segments, Raw HL7, sent ACK/NACK, JSON, and Segment Diff
- **HL7 Dictionary Tooltips** — hover any field or segment header for its official HL7 v2.5.1 description; no internet required
- **Message Type Detection** — human-readable type description and "Typical segments" bar per message
- **Validation Engine** — warnings for missing required fields/segments (amber) and invalid field data types (blue); per message type; non-blocking, all messages are stored regardless
- **Validation Filter** — one-click filter to show only messages with warnings or errors; also supports `has:warnings` / `has:errors` search prefixes
- **Segment Diff** — pin any message as a reference and compare it field-by-field; optional toggle to hide always-changing dynamic fields (MSH-7, MSH-10)
- **Search & Filter** — by message type, patient name, facility, message control ID, source IP
- **Bookmark & Tag** — pin important messages (eviction-protected), add custom text tags
- **Session-based Views** — each developer sees their own filters, selection, and scroll position
- **Color-coded Sources** — messages visually distinguishable by sender system/IP
- **Smart Store** — in-memory with configurable capacity and dual eviction (count + size)
- **JSON Export** — export full message data with one click
- **Resizable Panels** — drag splitter between message list and detail view
- **Configurable** — `harux.toml` for ports, memory limits, timeouts, log level
- **Single Binary** — frontend embedded via `rust-embed`, zero runtime dependencies

---

## Quick Start

**Prerequisites:** [Rust toolchain](https://rustup.rs)

```bash
git clone https://github.com/Pappet/harux.git
cd harux
cargo build --release
cargo run --release
```

Open **http://localhost:8080** — the UI connects automatically.

**Defaults:** MLLP port `2575`, Web UI port `8080`. Override via `harux.toml` or environment variables (`MLLP_PORT`, `WEB_PORT`).

### Windows

```powershell
# Direct start
.\harux.exe

# Install as Windows Service (NSSM)
nssm install Harux C:\Tools\harux.exe
nssm start Harux
```

Pre-built binaries for Windows, macOS, and Linux are available on the [Releases](https://github.com/Pappet/harux/releases) page.

---

## Documentation

| Document | Description |
|---|---|
| [PROJECT_OVERVIEW.md](PROJECT_OVERVIEW.md) | Detailed architecture, technical decisions, API reference |
| [ROADMAP.md](ROADMAP.md) | Milestones, planned features, and progress tracking |
| [STYLE_GUIDE.md](STYLE_GUIDE.md) | UI design rules, coding conventions, workflow standards |
| [CHANGELOG.md](CHANGELOG.md) | Full history of every change |
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to contribute |

---

## Contributing

Contributions are welcome. Please open an issue first for larger changes.

```bash
git checkout -b feature/my-feature
cargo test && cargo clippy -- -D warnings
# Submit a pull request against main
```

---

## License

MIT — see [LICENSE](LICENSE) for the full text.

---

<div align="center">
<sub>Built with Rust · <a href="https://github.com/Pappet/harux">github.com/Pappet/harux</a></sub>
</div>
