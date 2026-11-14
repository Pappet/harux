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

The palette is a **cool blue accent on deep obsidian**; amber, green and red are kept for states (warning, success, error). These are the values of `static/style.css` since the UI redesign of v0.5.0 (#86), which replaced the earlier amber accent.

| Role | Value | Note |
|---|---|---|
| **Primary Background** | `#0b0d12` | Near-black base |
| **Secondary Background** | `#131620` | Panels, headers |
| **Tertiary Background** | `#1b1f2c` | Active, popups |
| **Hover Background** | `#242938` | |
| **Borders** | `#262b3a` | |
| **Primary Text** | `#e4e6f0` | |
| **Secondary Text** | `#8b8fa3` | |
| **Muted Text** | `#5c6078` | |
| **Accent (Blue)** | `#7aa2ff` | Selection, links, focus rings, active controls (dimmed: `#5a82e8`) |
| **Success (Green)** | `#4fb98a` | ACK received, valid |
| **Warning (Amber)** | `#e5a54b` | Missing field |
| **Error (Red)** | `#ea6363` | Missing segment, NACK |
| **Reference (Violet)** | `#c4a7ff` | A draft's references and placeholders (`.ref-chip`, `.field-input.auto`); written out, not a variable |

### CSS Variables

```css
:root {
  --bg-primary:    #0b0d12;
  --bg-secondary:  #131620;
  --bg-tertiary:   #1b1f2c;
  --bg-hover:      #242938;
  --border:        #262b3a;

  --text-primary:  #e4e6f0;
  --text-secondary:#8b8fa3;
  --text-muted:    #5c6078;

  --accent:        #7aa2ff;
  --accent-dim:    #5a82e8;
  --success:       #4fb98a;
  --warning:       #e5a54b;
  --error:         #ea6363;
}
```

### Source Marker Palette

Used for the color-coded bars and chips of the sources (`SOURCE_PALETTE` in `static/util.js`):

```
hsl(210, 90%, 65%)  — Blue        hsl(150, 70%, 55%)  — Green
hsl(30,  85%, 60%)  — Orange      hsl(280, 75%, 65%)  — Purple
hsl(0,   80%, 62%)  — Red         hsl(180, 70%, 50%)  — Teal
hsl(50,  85%, 55%)  — Gold        hsl(330, 75%, 62%)  — Pink
hsl(200, 80%, 55%)  — Sky         hsl(100, 60%, 50%)  — Lime
hsl(260, 65%, 60%)  — Indigo      hsl(15,  90%, 58%)  — Coral
```

> **Note:** A source's colour is picked by a hash of its key (the sender's IP, or IP and port with *color by port*), so it stays the same across reloads; the order of the list means nothing. A send target (`→ target`) always uses `--accent`.

---

## 2. Typography

- **UI elements:** `Inter`, `-apple-system, system-ui, sans-serif`
- **Data / code:** `JetBrains Mono`, `SF Mono, Cascadia Code, Consolas, monospace`
- **Base font size:** 12–13 px for data-dense views
- **Headers:** Subtle uppercase with small letter-spacing for grid/table headers

---

## 3. Spacing & Density

- **List rows:** two lines, 61 px high (60 px content + 1 px border — `contain-intrinsic-size` in `static/style.css` depends on it, #216); content padding `11px 14px 11px 11px`
- **Panel gaps:** Use consistent CSS variable spacing
- **Borders:** 1 px solid `var(--border)` between sections

---

## 4. Animations & Transitions

- **Hover/focus transitions:** short, on background, colour and border only: `0.15s` on the header's buttons, `0.12s` on the smaller controls (list controls, source chips, tabs, action and copy buttons), `0.1s` on list rows
- **New message flash:** Row flashes `rgba(122, 162, 255, 0.15)` (the accent) and fades out over `0.6s`
- **Toast notifications:** Slide in, auto-dismiss after a short delay

---

## 5. Input Controls

- **No native browser checkboxes or radios** — use custom CSS toggle switches
- Toggle switches use `var(--bg-tertiary)` background and `var(--border)` for resting state
- Active toggles use `var(--accent)` for the knob, on an accent-tinted background (`rgba(122, 162, 255, 0.18)`)
- The toggle is a `<button class="switch" role="switch" aria-checked>` with a `data-action`, so it works with Tab, Enter and Space like every other control (#274)
- Search inputs are debounced at `300ms`

---

## 6. Frontend Architecture Rules

- **No frameworks:** Vanilla JavaScript only. No React, Vue, Svelte.
- **No build toolchain:** No npm, webpack, vite. Plain files only: `index.html`, `style.css` and the `<script>` files `state.js` → `util.js` → `ws.js` → `render.js` → `diff.js` → `send.js` → `compose.js` → `watch.js` → `app.js` (no module system, one global `state`).
- **No inline code:** The Content-Security-Policy forbids inline scripts, inline styles and `on…=` attributes. Interactive elements get a `data-action` and are handled by the delegation in `app.js`; values in attributes are escaped with `escAttr`, text with `esc`.
- **Embedded distribution:** Static files are baked into the binary via `rust-embed`. Changes require recompilation.
- **DOM batching:** Buffer incoming messages and flush to the DOM every `250ms` to prevent freeze at high throughput.
- **Client-side filtering:** Search filters the in-memory `messages[]` array (the loaded messages, at most 2,000 plus bookmarked/selected) directly. Debounce search input at `300ms`.
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
- **Title row, right (`#detail-actions`):** the **Resend → target** split button (#274) while sending is on — the left part sends to the target used last, the caret opens the target menu (`.send-menu`: the user's targets as radio rows with their last result, an add/edit form with toggle switches, Save to file / Load, Send). Empty while sending is off.
- **Tools row (`#detail-tools`):** tag chips, Add tag input, Bookmark and Pin diff, below the meta row
- **A draft (#277)** uses the same header: the badge reads `DRAFT` (`.detail-direction.draft`, warning colour, outlined), the title row's right side holds the target button (`.target-btn`, "to *name*", opens the same `.send-menu` without *Send as*) and **Send** with its `kbd.key` hint (`Ctrl ⏎`; *Send anyway* in the warning colour for text that does not parse), the meta row holds the encoding `<select>` (`.draft-charset`), and the tools row *Keep on this computer*, *Save as file*, *Delete*. The JSON tab is hidden for a draft; the ACK tab reads **Response**.
- **Drafts rail (`#draft-rail`):** under the source rail, only while drafts exist; `.draft-chip` = type (mono, accent) + title + a warning dot for unsent changes + ✕. The Raw tab's editor is a plain `<textarea class="draft-text">` (mono, no wrapping) with `.draft-status` below it; references are dashed violet `.ref-chip`s.
- **Editable segments (#278):** `.field-table.editable` (fixed layout: label 210 px, input, a 64 px quick-action column); `.field-input` (mono, accent focus ring; dashed violet `.auto` when it holds a placeholder; warning border on a `tr.warn`); read-only values as `.field-fixed` + `.field-note`; `.seg-tools` buttons (*show empty fields*, *duplicate*, *remove*) at the right of the segment header; addable checklist pills `.seg-pill.addable` (dashed, `+ NAME`).
- In the meta row the control ID is a link (`.meta-link`) that filters the list by it (trace); a sent message's description links its original (`.origin-link`)

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
- **Memory safeguards:** Explicit count and byte limits on the store, measured against what messages really cost on the heap. When either limit is hit, evict the oldest messages until both are at or below 90 %; bookmarks are protected only within their budget.
- **Configuration:** Every new setting goes into `src/config.rs` (validated, unknown keys rejected), `harux.toml` and `examples/harux-example.toml`.
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
cargo build
cargo test
node --test 'tests/js/*.test.mjs'
```

All must pass — this is the CI gate (`.github/workflows/ci.yml`). Zero tolerance for warnings.
