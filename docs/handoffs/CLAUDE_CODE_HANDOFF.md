# Harux UI Redesign — Implementation Brief for Claude Code

**Reference mockup:** `harux-redesigned.html` (in this project, sibling file).
**Target files:** `static/index.html`, `static/style.css`, `static/app.js`.
**Constraint:** Keep all existing behavior — WebSocket, `/api/*` endpoints, session persistence, tags, bookmarks, pin-for-diff, validation, source coloring. This is a **visual + IA refactor**, not a feature rewrite.

Open `harux-redesigned.html` in a browser to see the target. Annotations are in the bottom-right "9 ideas" drawer.

---

## 1. Top bar — replace stats-bar dots with health pills

Current: `.stats-bar` with dots + numbers. Replace with a row of pill-shaped status indicators.

Required pills (left to right), built from existing data:
| Pill | Source | Notes |
|---|---|---|
| `listening :PORT` | `stats.mllp_port` | Green pulsing dot when WS connected; red static dot on disconnect. Replaces current `ws-status`. |
| `conns N / MAX` | `stats.active_connections` / `stats.max_connections` | |
| `rate X.X/min` + sparkline | **NEW** — derive client-side from rolling 60s of `addMessage` timestamps | 60×18 inline SVG polyline, green stroke |
| `last Ns ago` | **NEW** — track `Date.now() - lastMessageReceivedAt`, refresh every 1s | Format: `2s ago`, `1m ago`, `12m ago`, `1h ago` |
| `errors N` | `stats.parse_errors` | Red value when > 0 |

Hide pills with no signal (e.g. don't show `last` until first message).

CSS class names from mockup: `.health`, `.health-pill`, `.health-pill.live`, `.dot`, `.spark`.

## 2. Throughput band — above the message list

New row between source rail and message list. Renders 60 bars (one per second of last minute) plus four counters: `total`, `per min`, `warnings`, `errors`.

- Maintain a `rateBuckets = new Array(60).fill(0)` ring buffer.
- Increment current bucket on every `addMessage`.
- `setInterval(rotateBuckets, 1000)` shifts the ring.
- Render with the same SVG bar pattern as the mockup (`<rect>` per bar, opacity ramp from 0.4 → 1.0 left to right).

CSS classes: `.rate-band`, `.stat`, `.spark-wrap`, `svg.bars`.

## 3. Source rail — always visible, with counts

Replace `#source-legend` (currently `display:none` until toggled). Always render. Each chip shows:
- Colored dot (existing `getSourceColor`)
- Source label (host, or host:port when `colorByPort`)
- Per-source message count (count from `messages.filter(m => srcKey(m) === label).length`)

Click toggles `highlightedSource` (existing logic). Move the "Color by Port" toggle from inside the legend into a small overflow menu — it clutters the rail.

CSS classes: `.source-rail`, `.source-chip`, `.source-chip.active`, `.source-chip .num`.

## 4. Message rows — two-line layout, drop the 9-col grid

Current: `display: grid; grid-template-columns: 12px 100px 90px 1fr 140px 60px 40px 24px 24px;` — replace entirely.

New row structure (see mockup `.msg`):
```
[3px source bar] [body: row1 + row2] [actions]
```

**Row 1** (primary, ~13px):
- Type badge (`.msg-type`, monospace, accent color)
- Patient name (`.msg-patient`, white, ellipsis)
- Validation badge if any (`.msg-warning.warn` / `.err`)
- Tags inline (`.tag` pills, max 2-3 visible)
- ACK chip (`.msg-ack.aa/.ae/.ar/.none`, colored background)

**Row 2** (secondary, 11px monospace, fg-2):
- Facility · source_addr · `N segs` · time-or-relative-time

**Actions** (right edge, opacity 0 unless hover/selected/bookmarked):
- Bookmark star only. **Remove the pin-for-diff icon from the row** — pin lives in the detail panel only.

Selected: `background: var(--bg-2)` + 2px accent left border.
New flash: keep the existing `flash` keyframe animation.
Dimmed (when `highlightedSource` mismatch): `opacity: 0.35`.

## 5. Time grouping — sticky headers

Group messages into buckets when rendering:
- **Live** — last 30 seconds
- **Last few minutes** — 30s to 5min
- **Earlier today** — same calendar day, > 5min ago
- **Yesterday**, **MMM D** — older days

Render `.group-header` between groups (sticky position, semi-transparent bg).

`renderMessageList` is the only place that needs to change. Walk the filtered array and emit a header element when the bucket transitions.

## 6. ACK chip — colored background, not just colored text

Currently inline-styled `color`. Convert to chip:
- AA → `.msg-ack.aa` (green bg/text)
- AE → `.msg-ack.ae` (red)
- AR → `.msg-ack.ar` (amber)
- (none) → `.msg-ack.none` (muted)

## 7. Detail header — restructure

Replace the cramped `.detail-header` with:

```
┌─────────────────────────────────────────────────────────────┐
│  [ADT^A01]  Admit/Visit Notification                  [★] [📌 Pin diff] [⋯]
│  A new patient has been admitted — patient identification…
│  patient Romero, Marcus · MRN MRN-78421 · control MSG042185 [📋] · v 2.5.1 · received 14:03:21
└─────────────────────────────────────────────────────────────┘
```

- Title chip (`.detail-type`) + human title (`.detail-title`) on row 1
- Description (`msg.message_type_description`) on row 2 if present
- Metadata row (`.detail-meta`) with `key value` pairs separated by `·`, and a clickable copy icon next to `control` value (uses existing `copyToClipboard`).
- Actions on the right: Bookmark, Pin-for-diff, overflow menu. **All three become text+icon buttons** (move pin out of the row, into here).

## 8. Tabs — visually larger, badge counts

Existing `.detail-tab` styling is fine but bump padding to `11px 14px`, add badge for segment count on Segments tab. Move `Diff` tab to `margin-left: auto` (right-aligned) so it visually separates from the "view this message" tabs.

## 9. Typical segments — pill checklist

Existing `.typical-segments-bar` is close. Refinements:
- Wrap in a card (`.seg-checklist`) with border + radius
- Pills add explicit symbol: `MSH ✓` / `PD1 ⚠` / `OBX ✕` / `DG1` (no symbol = absent)
- Move above the validation banner

## 10. Validation summary banner — name the fields

Replace the bulleted `validation-warnings-panel` with a one-line summary banner at top of segments view:

> ⚠ **2 validation warnings** · required field missing in `PD1-3`, expected segment not sent: `OBX`

Build the summary by aggregating `validation_warnings` by code, then list the top 3-4 segment/field labels inline. Keep the full bulleted list available — collapsed by default, click to expand.

## 11. Field rows — show description inline

Currently descriptions are tooltips on `.field-idx`. Promote to a sub-line under the field index in the new `.field-table .idx`:

```
PID-5
Patient name      | Romero^Marcus^J^^Mr.
```

Highlight `.field-table tr.warn` rows (yellow tint) for fields that triggered a `MISSING_FIELD` warning, with `⚠ required` suffix on the value cell.

## 12. Empty state — config + sample CLI

Replace the lightning-bolt SVG empty state with:

```
┌──────────────────────────────────────┐
│            [icon]                    │
│   Listening on :2575                 │
│   No messages received yet.          │
│                                      │
│   $ echo "MSH|^~\&|TEST..." \        │
│       | mllp-send localhost 2575     │  [📋]
└──────────────────────────────────────┘
```

Pull port from `stats.mllp_port`. Include a copy-to-clipboard button on the CLI snippet.

## 13. Typography — adopt Inter + JetBrains Mono

Add to `<head>`:
```html
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;500;600&family=Inter:wght@400;500;600;700&display=swap" rel="stylesheet">
```

Update `--font-sans` and `--font-mono`. Body font-size stays 13px.

## 14. Color refinements

Update CSS variables in `:root`:
```
--bg-primary:  #0b0d12   (was #0f1117)
--bg-secondary:#131620   (was #1a1d27)
--bg-tertiary: #1b1f2c   (was #242736)
--bg-hover:    #242938
--border:      #262b3a   (was #2e3247)
--accent:      #7aa2ff   (was #6c8cff) — slightly cooler
--success:     #4fb98a
--warning:     #e5a54b
--error:       #ea6363
```

Higher contrast against the deeper bg, accent reads better on dark.

---

## What NOT to change

- WebSocket message protocol (`init`, `new_message`, `tags_updated`, `bookmark_toggled`, `lagged`, `cleared`)
- API surface (`/api/messages`, `/api/messages/:id`, `/api/stats`, `/api/clear`, tags, bookmark)
- Session persistence keys (`harux_session`, `_state`)
- Diff-vs-pinned logic — only the pin entry-point moves (out of the row, into detail header)
- Source color palette / `getSourceColor` algorithm
- Keyboard shortcuts (preserve any that exist; add a `⌘K` hint for the search)

## Suggested order

1. Variables + typography swap (quick win, baseline)
2. Top-bar health pills
3. Message row two-line layout + ACK chip + time grouping
4. Source rail always-on
5. Throughput band
6. Detail header restructure + typical-segments checklist
7. Validation summary banner
8. Empty state with CLI hint

Ship 1-3 first; that's most of the perceived improvement.
