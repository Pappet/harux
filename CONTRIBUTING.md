# Contributing to Harux

Thanks for your interest in contributing to Harux! This document explains how to get started and what we expect from contributions.

## Getting Started

1. **Fork** the repository and clone your fork
2. Make sure you have the [Rust toolchain](https://rustup.rs) installed
3. Build and run the tests to verify everything works:

```bash
cargo build
cargo test
cargo clippy -- -D warnings
node --test 'tests/js/*.test.mjs'   # frontend logic, needs only Node.js
```

## Opening Issues

Please use the [Issue Templates](https://github.com/Pappet/harux/issues/new/choose) for bug reports and feature requests. If you're unsure whether something is a bug or just have a question, start a [Discussion](https://github.com/Pappet/harux/discussions) instead.

For **larger changes** (new features, architectural changes, new dependencies), please open an issue first so we can discuss the approach before you invest time into implementation.

## Development Workflow

### Branch Naming

Use a type prefix and, where there is one, the issue number:

- `feat/<issue>-<name>` — New features (e.g. `feat/27-bookmark-messages`)
- `fix/<issue>-<name>` — Bug fixes (e.g. `fix/15-eviction-bug`)
- `docs/<name>` — Documentation only
- `refactor/<issue>-<name>` — Code restructuring without behavior change
- `ci/<name>` — CI/CD changes

### Commit Messages

Write clear, concise commit messages. We follow a lightweight conventional format:

```
type: short summary in imperative mood

Optional longer description explaining *why* the change was made.
Reference issues with #123.
```

**Types:** `feat`, `fix`, `docs`, `refactor`, `test`, `ci`, `chore`

Examples:
- `feat: add configurable MLLP port via environment variable`
- `fix: handle missing MSH segment without panic`
- `docs: add WebSocket event examples to README`
- `test: add parser tests for HL7 v2.3 messages`

### Code Style

- Run `cargo fmt` before committing — we use default rustfmt settings
- Run `cargo clippy -- -D warnings` — all warnings must be resolved
- Keep functions focused and reasonably sized
- Add doc comments (`///`) to public types and functions
- Use `tracing` for logging, not `println!`

### Tests

- Add tests for new behavior and bug fixes
- Unit tests go in the same file as the code (`#[cfg(test)]` module)
- Integration tests go in `tests/` (real TCP in `tests/mllp_tcp.rs`, the real binary in `tests/config_startup.rs`, the REST API in `tests/web_api.rs`)
- Frontend logic is tested with `node:test` in `tests/js/` — no npm, no dependencies
- Run the full suite before submitting: `cargo test` and `node --test 'tests/js/*.test.mjs'`

## Pull Request Process

1. Create your branch from `main`
2. Make your changes in focused, logical commits
3. Update `CHANGELOG.md` under `[Unreleased]` in the same commit (features, fixes, refactors — with the issue number)
4. Ensure all checks pass: `cargo fmt --check && cargo clippy -- -D warnings && cargo test && node --test 'tests/js/*.test.mjs'`
5. Open a pull request against `main` and fill out the PR template
6. Link the related issue (e.g. "Closes #42")

PRs will be reviewed as time allows. Small, focused PRs are reviewed faster than large ones.

## What Makes a Good Contribution

- **Focused** — One logical change per PR
- **Tested** — New behavior has tests, bug fixes have regression tests
- **Documented** — Public APIs have doc comments, complex logic has inline comments
- **Clean** — No unrelated formatting changes, no leftover debug output

## Project Structure

```
src/
├── main.rs          # Entry point: config, logging, both servers, shutdown
├── config.rs        # harux.toml + environment, validated at startup
├── logging.rs       # Daily log file with retention
├── mllp.rs          # MLLP TCP listener, framing, ACK/NACK
├── store.rs         # In-memory message store with eviction and broadcast
├── web.rs           # Axum REST API, WebSocket, export, security headers
├── dictionary.rs    # Embedded HL7 v2.5.1 field/segment dictionary
├── validation.rs    # Required segments/fields and datatype checks
└── hl7/             # Parser, charset decoding, escapes, types, message-type registry
static/              # Web UI (vanilla JS, embedded at compile time)
tests/               # Integration tests, JS tests, HL7 fixtures, manual test scripts
```

The full annotated layout and the architecture rules are in [`AGENTS.md`](AGENTS.md).

## HL7 Domain Notes

If you're new to HL7 v2.x messaging:

- Messages are pipe-delimited (`|`), segments are separated by `\r`
- The MSH segment is always first and defines the delimiters
- MLLP framing wraps messages in `0x0B` ... `0x1C 0x0D`
- The receiver responds with an ACK (`MSA|AA`), an error (`MSA|AE`) or a reject (`MSA|AR`)

The [HL7 v2 specification](https://www.hl7.org/implement/standards/product_brief.cfm?product_id=185) is the authoritative reference, but most concepts are well-documented across community resources.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
