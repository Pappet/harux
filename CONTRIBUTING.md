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
```

## Opening Issues

Please use the [Issue Templates](https://github.com/Pappet/harux/issues/new/choose) for bug reports and feature requests. If you're unsure whether something is a bug or just have a question, start a [Discussion](https://github.com/Pappet/harux/discussions) instead.

For **larger changes** (new features, architectural changes, new dependencies), please open an issue first so we can discuss the approach before you invest time into implementation.

## Development Workflow

### Branch Naming

Use descriptive branch names with a prefix:

- `feature/` — New features (e.g. `feature/toml-config`)
- `fix/` — Bug fixes (e.g. `fix/mllp-framing-timeout`)
- `docs/` — Documentation only (e.g. `docs/api-examples`)
- `refactor/` — Code restructuring without behavior change
- `ci/` — CI/CD changes

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
- Integration tests go in `tests/`
- Run the full suite before submitting: `cargo test`

## Pull Request Process

1. Create your branch from `main`
2. Make your changes in focused, logical commits
3. Update `CHANGELOG.md` under `[Unreleased]` if your change is user-facing
4. Ensure all checks pass: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
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
├── main.rs          # Entry point, tokio runtime setup
├── mllp.rs          # MLLP TCP listener, framing, ACK/NACK
├── store.rs         # In-memory message store with broadcast
├── web.rs           # Axum REST + WebSocket handlers
└── hl7/
    ├── mod.rs       # Module exports
    ├── parser.rs    # HL7 message parser, delimiter detection
    └── types.rs     # Core types: Hl7Message, Hl7Segment, etc.
static/
├── index.html       # Web UI (embedded at compile time)
├── app.js           # UI logic
└── style.css        # Styles
```

## HL7 Domain Notes

If you're new to HL7 v2.x messaging:

- Messages are pipe-delimited (`|`), segments are separated by `\r`
- The MSH segment is always first and defines the delimiters
- MLLP framing wraps messages in `0x0B` ... `0x1C 0x0D`
- The receiver responds with an ACK (`MSA|AA`) or NACK (`MSA|AE`)

The [HL7 v2 specification](https://www.hl7.org/implement/standards/product_brief.cfm?product_id=185) is the authoritative reference, but most concepts are well-documented across community resources.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
