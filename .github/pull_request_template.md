## Description

<!-- What does this PR do? Why is it needed? -->

## Related Issue

<!-- Link the issue this PR addresses, e.g. "Closes #42" -->

## Type of Change

- [ ] 🐛 Bug fix (non-breaking change that fixes an issue)
- [ ] ✨ New feature (non-breaking change that adds functionality)
- [ ] 💥 Breaking change (fix or feature that would cause existing functionality to change)
- [ ] 📝 Documentation update
- [ ] ♻️ Refactor (no functional change)
- [ ] 🧪 Test improvement
- [ ] 🔧 CI / build / tooling

## Component(s) Affected

- [ ] MLLP Server (`mllp.rs`)
- [ ] Sending (`send.rs`, `mllp_client.rs`, `tls.rs`, `draft.rs`, `templates.rs`)
- [ ] HL7 Parser (`hl7/`) / Validation (`validation.rs`)
- [ ] Configuration / Logging (`config.rs`, `logging.rs`)
- [ ] Message Store (`store.rs`)
- [ ] Web Server / API (`web.rs`)
- [ ] Web UI (`static/`)
- [ ] Build / CI (`.github/`)
- [ ] Documentation

## How to Test

<!-- Steps for the reviewer to verify this change works -->

1. ...
2. ...

## Checklist

- [ ] `cargo test` passes
- [ ] `node --test 'tests/js/*.test.mjs'` passes (if `static/` changed)
- [ ] `cargo clippy -- -D warnings` is clean
- [ ] `cargo fmt --check` passes
- [ ] I updated `CHANGELOG.md` under `[Unreleased]` in the same commit
- [ ] I updated the docs (`AGENTS.md`, `README.md`, `docs/`) if behaviour, configuration or layout changed
- [ ] I added/updated tests for new behavior
- [ ] I added doc comments for new public types/functions

## Screenshots

<!-- If this changes the Web UI, include before/after screenshots -->
