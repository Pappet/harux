# Security Policy

## Context

Harux is designed to run in healthcare network environments where it receives and displays HL7 v2.x messages via MLLP. These messages may contain Protected Health Information (PHI) and other sensitive patient data. Security issues in this tool could have serious implications.

## Supported Versions

| Version | Supported          |
|---------|--------------------|
| latest  | ✅ Yes             |
| < latest | ❌ No (please upgrade) |

## Reporting a Vulnerability

**Please do NOT open a public GitHub issue for security vulnerabilities.**

Instead, report security issues privately via one of these methods:

1. **GitHub Private Vulnerability Reporting** (preferred):
   Go to [Security → Advisories → Report a vulnerability](https://github.com/Pappet/harux/security/advisories/new)

2. **Email**: Contact the maintainer directly (see GitHub profile [@Pappet](https://github.com/Pappet))

### What to include

- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if you have one)

### Response timeline

- **Acknowledgment** within 72 hours
- **Assessment and plan** within 1 week
- **Fix or mitigation** as soon as reasonably possible, depending on severity

## Security Considerations

Harux is an **inspection and debugging tool** — not a production message broker. Please keep the following in mind when deploying:

- **Network placement**: Run Harux only within trusted network segments. The Web UI and API have no authentication: anyone who can reach the web port can read every stored message, tag it and clear the store. `server.mllp_bind` and `server.web_bind` restrict each listener to one address — e.g. `web_bind = "127.0.0.1"` behind a reverse proxy that adds TLS and login.
- **No TLS for incoming connections**: the MLLP listener and the Web UI are unencrypted. Use network-level controls (VPN, firewall rules) or a TLS-terminating reverse proxy to restrict access. Outgoing sends can use TLS (see *Sending*).
- **PHI exposure**: Messages displayed in the Web UI may contain patient data. Limit browser access to authorized personnel.
- **No persistent storage** (currently): Messages are held in memory only and are lost on restart. This limits exposure but does not eliminate it while the service is running. Data leaves memory in two ways: the export (`/api/export`) writes every stored message to a file on the user's machine, and the log (stdout, and with `logging.file` the daily log files, kept for `logging.max_files` days) records connections and parse errors, which can quote the start of a broken message.

## Sending

**Sending is off by default.** Once `send.enabled = true` is set in `harux.toml`, everyone who can open the web UI can make the Harux server connect to any host and port the server can reach, and send an HL7 message there — including a production system.

What stands against a mistake once it is on:

- **`send.allowed_destinations`** limits where users may send: entries are `host:port` or `host:from-to` (an entry without a port is refused at start, since it would allow every port on that host). Names are compared as written, nothing is resolved for the check. This is also the control against using Harux as a stepping stone into the network, and against patient data reaching a system that should not get it.
- **Every send is visible:** the sent message is stored before Harux connects, as a row everyone sees, with the destination and the address of the client that asked for it, and a log line (metadata only, never content). Behind a reverse proxy that address is the proxy's.
- **Targets and *ask before sending* live in each user's browser, not on the server.** A target list can be loaded from a file; treat a file someone hands over as unverified input — it can point the UI at any `host:port` the server reaches. The only limit the server enforces is `send.allowed_destinations` (empty by default: anywhere).
- Send routes are `POST` and covered by the cross-site guard; the number of sends at a time is limited (`send.max_concurrent_runs`). A run of several messages (#280) takes at most `send.max_run_messages` messages and, stored again, at most `send.max_run_store_percent` of the store; it is refused as a whole before anything is sent. Anyone who can open the web UI can see and cancel any run whose ID they know — the IDs are random, and network placement is the access control here as everywhere.
- **Import** (`POST /api/import`, #281) is part of sending: `403` while `send.enabled` is off. It writes into the store what the file holds, as received from `import:<file name>` (the name is a label: its last path part, without control characters), at most `store.max_memory_mb` per file, streamed and behind the same large-body permit; it never sends or answers anything.
- **Templates** (#279): `send.templates_dir` is only read, never written. A template is fetched by an ID that is a key of the listing, never joined into a path; only regular files directly in that directory named like `name.hl7` are listed — no links, no subdirectories — so a request cannot reach a file outside it. Anyone who can write to that directory decides what the team's templates contain. The template name a sent message records (`origin: template`) is a label the client sends, cleaned of control characters and cut at 64 characters — not proof that the message came from that template.
- **Drafts** (#276) are previewed and built by the server but stored nowhere until they are sent; a sent draft is a row like any other send. The routes that carry a message (`/api/send`, `/api/parse`, `/api/build`) accept up to twice `mllp.max_message_size_mb` plus 1 MB, and at most 4 bodies of 256 KB or more are read at a time (`429` beyond), so many large requests at once cannot use memory no limit sees. A long value (an MDM attachment) stays on the server as a reference; it reaches a browser only if the user previews the draft with the references filled in.
- A response is read up to 1 MB and stored up to 64 KB; it never travels in list rows or WebSocket events.
- **TLS for outgoing connections** is chosen per target. The target's certificate must be valid for the host name or address and chain to a root in the operating system's store or in `send.tls.ca_file`. A user can switch that check off for a target of theirs only if `send.tls.allow_unverified = true`; every message sent that way is recorded as *TLS unverified* and marked in its row. Harux's own client certificate (`send.tls.client_cert_file` / `client_key_file`) is one for the whole server, read at start, shown only to targets that ask for it, and never logged or returned by the API. The TLS library is rustls with the `ring` provider (no OpenSSL at run time).

## Hardening Measures (Built-in)

- Cross-site protection for the Web UI: requests with an unknown `Host` (DNS rebinding) and cross-origin WebSocket or state-changing requests are refused; names other than IP addresses and `localhost` must be listed in `server.allowed_hosts`
- `frame-ancestors 'none'` (no embedding) and `X-Content-Type-Options: nosniff`
- Content-Security-Policy without inline scripts, styles or event handlers
- Configurable bind addresses per listener (`server.mllp_bind`, `server.web_bind`)
- 10 MB maximum payload size per MLLP message (`mllp.max_message_size_mb`); larger frames are rejected with `AR` and the connection stays usable
- Connection limit (`mllp.max_connections`, default 100), 60-second read and 30-second write timeout on MLLP connections
- Every frame is stored and answered — a parser panic becomes a stored parse error and a NACK, not a dropped connection; the ingest path is fuzz-tested in CI (`tests/fuzz_ingest.rs`)
- In-memory store bounded by real memory use (`store.max_memory_mb`) with automatic eviction
- A broken configuration stops the start instead of running on defaults
- Sending is off unless `send.enabled` is set, and can be limited to `send.allowed_destinations`
- Graceful shutdown on `Ctrl+C` and SIGTERM (Unix)
- Dependencies checked against RustSec advisories daily (`cargo audit`), Dependabot updates
