# Harux — Style Guide

> *"Peer into the entrails of your messages."*

**Harux** (from Latin *haruspex* — a Roman priest who divined meaning by inspecting entrails) is an MLLP server with a web UI for receiving, parsing, and inspecting HL7 v2.x messages.

This is the reference for all appearance and functioning rules of Harux.

---

## 0. Brand Identity

### Name & Origin

| | |
|---|---|
| **Name** | Harux |
| **Pronunciation** | /haˈɾʊks/ — rhymes with "barracks" |
| **Etymology** | Contraction of *haruspex* (Latin: entrail-inspector) |
| **Tagline** | *Peer into the entrails of your messages.* |
| **Alternate short** | *Message divination for HL7® v2.x* |

### Voice & Tone

Harux is a **clinical tool with a wry mythological edge**. Documentation and UI copy should be:

- **Precise** — segment names, field indices, data types are never vague.
- **Compact** — labels are short; tooltips carry the detail.
- **Occasionally mythological** — release names, error pages, or easter eggs may reference Roman augury. The core UI stays professional.

### Logo Concept

**NOT FINAL**
The Harux mark combines two motifs: **inspection** and **message dissection**.

| Element | Rationale |
|---|---|
| Stylised eye or blade | Represents the act of examining / cutting open |
| Segmented interior lines | Echoes HL7 pipe-delimited segments visible "inside" the message |
| Monochrome + single accent | Must work as a 16×16 favicon, a CLI banner, and a README badge |

Preferred rendering: a **single-stroke glyph** in `--accent` on a `--bg-primary` ground.
The logo must never incorporate HL7® trademarks or the HL7 flame symbol.

### Trademark Notice

When referencing the standard, include the following notice at least once per document or prominent page:

> HL7® is a registered trademark of Health Level Seven International.
> The use of this trademark does not constitute endorsement by HL7.
> Harux is not affiliated with or endorsed by HL7 International.

---

## 1. Color Palette (Dark Theme)

The palette draws from **volcanic amber on deep obsidian** — a nod to Roman-era divination by firelight.

| Role | Value | Note |
|---|---|---|
| **Primary Background** | `#0f1117` | Near-black base |
| **Secondary Background** | `#1a1d27` | Panels, headers |
| **Tertiary Background** | `#242736` | Hover, active, popups |
| **Accent Hover** | `#2a2e3f` | |
| **Borders** | `#2e3247` | |
| **Primary Text** | `#e4e6f0` | |
| **Secondary Text** | `#8b8fa3` | |
| **Muted Text** | `#5c6078` | |
| **Accent (Amber)** | `#d4944c` | Primary brand color (dimmed: `#a06e38`) |
| **Accent Alt (Blue)** | `#6c8cff` | Links, interactive focus rings |
| **Success (Green)** | `#4caf84` | ACK received, valid |
| **Warning (Yellow)** | `#e5a54b` | Missing field |
| **Error (Red)** | `#e05454` | Missing segment, NACK |

### CSS Variables

```css
:root {
  --bg-primary:    #0f1117;
  --bg-secondary:  #1a1d27;
  --bg-tertiary:   #242736;
  --accent-hover:  #2a2e3f;
  --border:        #2e3247;

  --text-primary:  #e4e6f0;
  --text-secondary:#8b8fa3;
  --text-muted:    #5c6078;

  --accent:        #d4944c;
  --accent-dim:    #a06e38;
  --accent-alt:    #6c8cff;
  --success:       #4caf84;
  --warning:       #e5a54b;
  --error:         #e05454;
}
```

### Source Marker Palette

Used for color-coded dots in the message list (mapped by sender IP/port):

```
hsl(30,  85%, 60%)  — Amber       hsl(150, 70%, 55%)  — Green
hsl(210, 90%, 65%)  — Blue        hsl(280, 75%, 65%)  — Purple
hsl(0,   80%, 62%)  — Red         hsl(180, 70%, 50%)  — Teal
hsl(50,  85%, 55%)  — Gold        hsl(330, 75%, 62%)  — Pink
hsl(200, 80%, 55%)  — Sky         hsl(100, 60%, 50%)  — Lime
hsl(260, 65%, 60%)  — Indigo      hsl(15,  90%, 58%)  — Coral
```

> **Note:** Amber is listed first — it is the brand colour and the default marker for the first source.

---

## 2. Typography

- **UI elements:** `Inter`, `-apple-system, system-ui, sans-serif`
- **Data / code:** `JetBrains Mono`, `SF Mono, Cascadia Code, Consolas, monospace`
- **Base font size:** 12–13 px for data-dense views
- **Headers:** Subtle uppercase with small letter-spacing for grid/table headers

---

## 3. Spacing & Density

- **List row padding:** `9px 16px` — compact for data-heavy interfaces
- **Panel gaps:** Use consistent CSS variable spacing
- **Borders:** 1 px solid `var(--border)` between sections

---

## 4. Animations & Transitions

- **Hover/focus transitions:** `0.15s` for background color changes on buttons and list rows
- **New message flash:** Row flashes `rgba(212, 148, 76, 0.15)` and fades out over `0.6s`
- **Toast notifications:** Slide in, auto-dismiss after a short delay

---

## 5. Input Controls

- **No native browser checkboxes or radios** — use custom CSS toggle switches
- Toggle switches use `var(--bg-tertiary)` background and `var(--border)` for resting state
- Active toggles use `var(--accent)` colour with `rgba(212, 148, 76, 0.15)` background glow
- Search inputs are debounced at `300ms`

---

## 6. Frontend Architecture Rules

- **No frameworks:** Vanilla JavaScript only. No React, Vue, Svelte.
- **No build toolchain:** No npm, webpack, vite. Write pure `app.js`, `style.css`, `index.html`.
- **Embedded distribution:** Static files are baked into the binary via `rust-embed`. Changes require recompilation.
- **DOM batching:** Buffer incoming messages and flush to the DOM every `250ms` to prevent freeze at high throughput.
- **Client-side filtering:** Search filters the in-memory `messages[]` array directly. Debounce search input at `300ms`.
- **Full message on demand:** The `messages[]` array holds lightweight `Hl7MessageSummary` objects. Any operation that needs segment data (detail view, diff pin) must fetch the full message via `GET /api/messages/{id}`.

### Tooltip Rules

Two tooltip styles are in use — choose based on the element:

| Context | Style | Implementation |
|---|---|---|
| Field index cell (`field-idx`) | Inline sub-line text | `.desc-text` element within the cell |
| Segment header row | Custom CSS `::after` — below header, fade-in | `data-desc` attribute + `.has-seg-tooltip` class |
| Typical-segment badges | Native `title` attribute | `title="SEG: Description"` on `<span>` |

### Detail Header Layout

The `.detail-header` uses a layered structure:

- **Row 1:** Type chip + Human-readable title
- **Row 2 (Optional):** Segment description or validation summary
- **Row 3:** Monospace metadata string (`patient · MRN · control · version · received`)
- **Tags Row:** Bookmark button, tag chips, Add tag input in a flex container

### Validation Badge Colour Semantics

Three distinct colours are used for validation — do not mix them:

| Colour | CSS class | Warning code | Meaning |
|--------|-----------|--------------|---------|
| **Red** | `.validation-seg.error` | `MISSING_SEGMENT` | A required segment is absent from the message |
| **Amber** | `.validation-seg` (default) | `MISSING_FIELD` | A required field within a present segment is empty |
| **Blue** | `.validation-seg.type` | `INVALID_DATATYPE` | A field value does not match its declared HL7 data type (NM/DT/TS/SI) |

The same three-tier logic applies to **typical-segment badges** in the detail view:

| Badge colour | Symbol | CSS class | Condition |
|---|---|---|---|
| Red | `✕` | `.typical-seg.missing` | `MISSING_SEGMENT` warning for this segment |
| Amber | `⚠` | `.typical-seg.warn` | `MISSING_FIELD` warning for a field in this segment |
| Blue | `✓` | `.typical-seg.present` | Segment is present with no structural warnings (`INVALID_DATATYPE` alone does **not** turn a badge amber) |
| Grey | (blank) | `.typical-seg.absent` | Segment is not present and not required |

**Rule:** `INVALID_DATATYPE` warnings appear in the validation panel only. They must never affect typical-segment badge colours — the segment and field are present, only the value format is suspect.

---

## 7. Backend Coding Rules (Rust)

- **Async runtime:** Tokio with `tokio::select!` for concurrent task management.
- **Web framework:** Axum for HTTP API and WebSocket.
- **State management:** `Arc<RwLock<T>>` with `tokio::sync::broadcast` for real-time push to WebSocket clients.
- **Memory safeguards:** Explicit count limits and byte-size limits on stores. Evict oldest 10 % when either limit is hit.
- **Logging:** Use the `tracing` crate. Never use `println!`.
- **File structure:** One responsibility per file (`web.rs` for routing, `store.rs` for state, `parser.rs` for decoding).

---

## 8. Commit Message Format

Conventional commit format with type prefix:

```
feat: add configurable MLLP port via environment variable
fix: handle missing MSH segment without panic
docs: update API examples
refactor: ...
test: ...
chore: ...
```

---

## 9. Branch Naming

Branches must include the issue number for traceability:

```
feat/<issue>-<name>       e.g. feat/27-bookmark-messages
fix/<issue>-<name>        e.g. fix/15-eviction-bug
docs/<name>               e.g. docs/refactor-documentation
refactor/<issue>-<name>   e.g. refactor/30-store-cleanup
```

---

## 10. CI Checks (Run Before Every Push)

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

All three must pass. Zero tolerance for warnings.
