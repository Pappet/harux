//! #246: the documentation must not point at files that no longer exist.
//!
//! Agents and people act on what these files say; a path that was renamed
//! or deleted is exactly how stale guidance starts (#181, #242). For each
//! current document below, every repository path it names — in a code span
//! (`src/mllp.rs`, `tests/test.sh:12`) or as a relative Markdown link — must
//! exist. Globs (`tests/js/*.test.mjs`), placeholders (`{id}`) and URLs are
//! skipped. Historical documents (handoffs, audits, the changelog) are not
//! checked: they describe the code as it was.
//!
//! Known limits:
//! - A code span starting with a checked directory (`docs/`, `src/`, …) is
//!   taken as a path, so a literal *example* such as `docs/api-examples`
//!   must be written as a placeholder (`docs/<name>`) or it fails the test.
//! - Fenced code blocks are not scanned (they hold commands such as
//!   `./target/release/harux`); a path in a ```` ```bash ```` block is not
//!   checked.
//! - Bare file names (`app.js`) are ambiguous and skipped, except in the
//!   documents listed in [`BASE_RELATIVE_DOCS`], which name the files next
//!   to them.
//!
//! Paths resolve against the crate root (`CARGO_MANIFEST_DIR`), so the
//! result does not depend on the directory `cargo test` runs from.

use std::path::{Path, PathBuf};

/// The repository root, whatever the working directory of the test run.
fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Documents whose bare file names (`v2.5.1.json`) mean files in the
/// document's own directory.
const BASE_RELATIVE_DOCS: &[&str] = &["src/assets/hl7/README.md"];

const CURRENT_DOCS: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    "README.md",
    "CONTRIBUTING.md",
    "SECURITY.md",
    "docs/PROJECT_OVERVIEW.md",
    "docs/ROADMAP.md",
    "docs/STYLE_GUIDE.md",
    "src/assets/hl7/README.md",
    ".github/pull_request_template.md",
];

/// Top-level directories whose paths are checked when named in a code span.
const ROOTS: &[&str] = &[
    "src/",
    "static/",
    "tests/",
    "docs/",
    "examples/",
    "assets/",
    ".github/",
    ".cargo/",
    ".jules/",
];

/// Repository files named without a directory.
const ROOT_FILES: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    "README.md",
    "CONTRIBUTING.md",
    "SECURITY.md",
    "CHANGELOG.md",
    "LICENSE",
    "Cargo.toml",
    "Cargo.lock",
    "harux.toml",
];

/// Text inside single backticks, outside fenced code blocks.
fn code_spans(text: &str) -> Vec<String> {
    let mut spans = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let mut parts = line.split('`');
        parts.next();
        while let (Some(inside), Some(_)) = (parts.next(), parts.next()) {
            spans.push(inside.to_string());
        }
    }
    spans
}

/// Targets of `[text](target)` links.
fn link_targets(text: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("](") {
        rest = &rest[i + 2..];
        if let Some(end) = rest.find(')') {
            targets.push(rest[..end].to_string());
            rest = &rest[end..];
        }
    }
    targets
}

/// The repository path a code span names, if any: `src/web.rs:128` →
/// `src/web.rs`.
fn span_path(span: &str) -> Option<String> {
    let span = span.trim();
    // `./x` is always meant as a path relative to the repository root.
    let explicit = span.starts_with("./");
    let span = span.trim_start_matches("./");
    let path = span.split(':').next()?;
    let is_path =
        explicit || ROOTS.iter().any(|r| path.starts_with(r)) || ROOT_FILES.contains(&path);
    let checkable = !path.contains(['*', '{', '<', ' ', '\\', '$']) && !path.contains("...");
    (is_path && checkable).then(|| path.to_string())
}

/// A bare file name such as `v2.5.1.json`: no directory, a known extension.
fn bare_file_name(span: &str) -> Option<&str> {
    let span = span.trim();
    let known = [
        ".json", ".rs", ".md", ".toml", ".js", ".html", ".css", ".hl7",
    ];
    (!span.contains(['/', '\\', ' ', '*', '{', '<']) && known.iter().any(|e| span.ends_with(e)))
        .then_some(span)
}

#[test]
fn paths_named_in_the_docs_exist() {
    let mut missing = Vec::new();
    let mut checked = 0;
    for doc in CURRENT_DOCS {
        let text = std::fs::read_to_string(repo().join(doc))
            .unwrap_or_else(|e| panic!("cannot read {doc}: {e}"));
        let base = repo().join(Path::new(doc).parent().unwrap_or(Path::new("")));

        for span in code_spans(&text) {
            if let Some(path) = span_path(&span) {
                checked += 1;
                if !repo().join(&path).exists() {
                    missing.push(format!("{doc}: `{span}`"));
                }
            } else if BASE_RELATIVE_DOCS.contains(doc) {
                if let Some(name) = bare_file_name(&span) {
                    checked += 1;
                    if !base.join(name).exists() {
                        missing.push(format!("{doc}: `{span}` (next to the document)"));
                    }
                }
            }
        }
        for target in link_targets(&text) {
            let target = target.split('#').next().unwrap_or_default().trim();
            if target.is_empty() || target.contains("://") || target.starts_with("mailto:") {
                continue;
            }
            checked += 1;
            let resolved: PathBuf = base.join(target);
            if !resolved.exists() {
                missing.push(format!("{doc}: link ({target})"));
            }
        }
    }
    assert!(
        checked > 50,
        "only {checked} paths found — is the parser broken?"
    );
    assert!(
        missing.is_empty(),
        "documentation names paths that do not exist:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn span_path_recognises_paths() {
    assert_eq!(span_path("src/web.rs:128").as_deref(), Some("src/web.rs"));
    assert_eq!(
        span_path("./tests/test.sh").as_deref(),
        Some("tests/test.sh")
    );
    assert_eq!(span_path("harux.toml").as_deref(), Some("harux.toml"));
    assert_eq!(span_path("tests/js/*.test.mjs"), None);
    assert_eq!(span_path("/api/messages/{id}"), None);
    assert_eq!(span_path("store.max_messages"), None);
    assert_eq!(span_path("app.js"), None, "bare file names are ambiguous");
    assert_eq!(bare_file_name("v2.5.1.json"), Some("v2.5.1.json"));
    assert_eq!(bare_file_name("store.max_messages"), None);
    assert_eq!(bare_file_name("src/web.rs"), None);
}
