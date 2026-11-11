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

- **Network placement**: Run Harux only within trusted network segments. The Web UI and API have no authentication by default. `server.mllp_bind` and `server.web_bind` restrict each listener to one address — e.g. `web_bind = "127.0.0.1"` behind a reverse proxy that adds TLS and login.
- **No TLS**: MLLP connections and the Web UI are unencrypted. Use network-level controls (VPN, firewall rules) to restrict access.
- **PHI exposure**: Messages displayed in the Web UI may contain patient data. Limit browser access to authorized personnel.
- **No persistent storage** (currently): Messages are held in memory only and are lost on restart. This limits exposure but does not eliminate it while the service is running.

## Hardening Measures (Built-in)

- Cross-site protection for the Web UI: requests with an unknown `Host` (DNS rebinding) and cross-origin WebSocket or state-changing requests are refused; names other than IP addresses and `localhost` must be listed in `server.allowed_hosts`
- `frame-ancestors 'none'` (no embedding) and `X-Content-Type-Options: nosniff`
- 10 MB maximum payload size per MLLP message
- 60-second read timeout on MLLP connections
- In-memory store with configurable capacity and automatic eviction
- Graceful shutdown on `Ctrl+C` / SIGTERM
