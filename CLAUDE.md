# CLAUDE.md

Guidance for Claude Code in this repository.

**The project guide is [`AGENTS.md`](AGENTS.md)** — git workflow, CI gate,
changelog rules, architecture, critical knowledge, constraints and source
layout. It is the single source of truth for every coding agent; do not copy
its rules here, change them there (#246). Planning (current milestone, next
work) lives in [`docs/ROADMAP.md`](docs/ROADMAP.md).

@AGENTS.md

## Claude Code specifics

- Before you push, run the full CI list from `AGENTS.md` → *Branch Protection & CI*, including the JS tests.
- Close issues with the detailed `### Fix Details` comment described in `AGENTS.md` → *Communication Preferences*.
