# Harux Chrome Cleanup — Implementation Handoff for Claude Code

**Reference mockup:** `harux-chrome-cleanup.html` (sibling file in this project).
**Target files:** `static/index.html`, `static/style.css`, `static/app.js`.
**Scope:** Visual + IA-only refactor. No protocol, API, or storage changes.

---

## Goals

1. Eliminate the redundant throughput band above the message list — its counters move into the header pill row, the rate sparkline already lives in the `rate` pill.
2. Relocate list-scoped controls (`Pause`, `Auto`, `Bookmarks`, validation cycle) out of the topbar and under the search bar, where they belong semantically.
3. Distinguish **parser errors** (MLLP-level) from **validation errors** (segment-level) in the header — both stay visible, with clearer naming.
4. Keep the existing 3-state validation filter button (`All → Warn → Error → All`) as-is. Only its position changes.

---

## 1. Topbar — promote pills, demote buttons

### 1a. Pills row — split into two clusters with a divider

Order, left to right:

```
[ listening :2575 ]  [ conns 0/100 ]  [ rate 12.4/min ▁▂▃▂▁ ]  [ last 9m ago ]
                              │
                              ▼ vertical divider
[ total 360 ]  [ validation 12 ]  [ parse errors 18 ]
```

- Group A (existing): listener health — `listening`, `conns`, `rate`, `last`
- 1px tall divider element (`.topbar-divider`, `width:1px; height:22px; background:var(--line); margin:0 4px`)
- Group B (new from band): message counters — `total`, `validation`, `parse errors`

**Naming**

| Pill label | Source | Color rule |
|---|---|---|
| `total` | `messages.length` | always neutral |
| `validation` | sum of `validation_warning_count` + `has_segment_errors` across all messages | amber when > 0 (`.health-pill.warn-pill`), neutral when 0 |
| `parse errors` | `stats.parse_errors` | red when > 0 (`.health-pill.err-pill`), neutral when 0 |

Important: the topbar's existing `errors` pill is renamed to **`parse errors`**. The new `validation` pill is the second one. They're not the same metric — keep both.

CSS: re-use the existing `.health-pill`, `.warn-pill`, `.err-pill` classes from the M1–M3 redesign. Add `.topbar-divider`.

### 1b. Topbar-right — keep only app-scoped actions

Remove from `.topbar-right`:
- `Pause`
- `Auto`
- `Bookmarks`
- The 3-state validation filter button

Keep in `.topbar-right`:
- `Export`
- `Clear` (danger style)

(If a settings/gear button exists, keep it too.)

---

## 2. Throughput band — delete

Remove the entire `.rate-band` block from `static/index.html`. Remove its CSS (`.rate-band`, `.rate-band .stat`, `.rate-band .spark-wrap`, `.rate-band svg.bars`) from `static/style.css`.

The 60-second ring buffer and rate calculation stay — they still feed the `rate` pill's sparkline. Don't touch:
- `rateBuckets` ring buffer
- `rotateBuckets` interval
- The polyline render in the `rate` pill

Only the standalone bar histogram between source rail and message list goes away.

---

## 3. Inbox controls — new row under the search

Insert a new row immediately below `.list-search`, above `.source-rail`:

```html
<div class="inbox-controls">
  <button class="ctl-btn" id="btn-pause">⏸ Pause</button>
  <button class="ctl-btn on ok-on" id="btn-autoscroll">↓ Auto</button>
  <div class="ctl-divider"></div>
  <button class="ctl-btn" id="btn-bookmarks">★ Bookmarks <span class="count">3</span></button>
  <button class="ctl-btn" id="btn-validation">⚠ All</button>
</div>
```

### CSS for the row

```css
.inbox-controls {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 14px;
  background: var(--bg-1);
  border-bottom: 1px solid var(--line);
}
.ctl-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 28px;
  padding: 0 10px;
  background: var(--bg-2);
  border: 1px solid var(--line);
  border-radius: 6px;
  color: var(--fg-1);
  font: inherit;
  font-size: 12px;
  cursor: pointer;
  transition: all .12s;
}
.ctl-btn:hover { background: var(--bg-3); color: var(--fg-0); }
.ctl-btn.on { background: var(--accent-bg); border-color: rgba(122,162,255,0.35); color: var(--accent); }
.ctl-btn.on.warn-on { background: rgba(229,165,75,0.10); border-color: rgba(229,165,75,0.35); color: var(--warn); }
.ctl-btn.on.err-on  { background: rgba(234,99,99,0.10); border-color: rgba(234,99,99,0.35); color: var(--err); }
.ctl-btn.on.ok-on   { background: rgba(79,185,138,0.10); border-color: rgba(79,185,138,0.35); color: var(--ok); }
.ctl-btn .count {
  font-family: var(--mono);
  font-size: 11px;
  color: var(--fg-2);
  background: var(--bg-1);
  padding: 1px 6px;
  border-radius: 3px;
}
.ctl-divider { width: 1px; height: 18px; background: var(--line); margin: 0 4px; }
```

### Group separation

The `.ctl-divider` between `Auto` and `Bookmarks` separates **stream controls** (Pause/Auto — affect live ingestion) from **filter controls** (Bookmarks/validation — affect what's displayed).

---

## 4. Validation filter button — keep the 3-state cycle

**Do not rename.** **Do not change behavior.** It cycles `All → Warn → Error → All` exactly as before.

Only change: it now sits in the new `.inbox-controls` row instead of the topbar, and uses the `.ctl-btn` class instead of the topbar's `.text-btn`.

The `syncValidationFilterUI()` function in `app.js` keeps working — just update the toggle classes:

```js
function syncValidationFilterUI() {
  const btn = document.getElementById('btn-validation');
  if (!btn) return;
  btn.classList.remove('on', 'warn-on', 'err-on');
  if (validationFilter === 0) {
    btn.innerHTML = '⚠ All';
  } else if (validationFilter === 1) {
    btn.innerHTML = '⚠ Warn';
    btn.classList.add('on', 'warn-on');
  } else if (validationFilter === 2) {
    btn.innerHTML = '⚠ Error';
    btn.classList.add('on', 'err-on');
  }
}
```

(Drop the inline `style.borderColor`/`style.color` writes — class-based styling is cleaner.)

The same conversion applies to:
- `togglePause()` — toggle `.on` (no color modifier)
- `toggleAutoscroll()` — toggle `.on.ok-on`
- `toggleBookmarkFilter()` — toggle `.on.warn-on` (gold star feel)

---

## 5. Source rail — minor tightening

Single-source case: when `seenSources.size === 1`, hide the `Color by Port` toggle behind a `⋯` overflow at the rail's right edge. It's only useful with multiple senders.

Implementation: render the toggle into a small popover triggered by an icon button (`.source-rail .more`). Existing `toggleColorByPort` handler unchanged — just relocate where it lives.

---

## 6. Wire-up summary (data flow unchanged)

| New pill | Existing source | Update on |
|---|---|---|
| `total` | `messages.length` (or server `totalMessagesCount`) | every `addMessage` / `cleared` |
| `validation` | `messages.reduce((n,m) => n + (m.validation_warning_count\|\|0) + (m.has_segment_errors?1:0), 0)` | every `flushAndRender` |
| `parse errors` | `stats.parse_errors` | `pollStats` (existing) |

If `validation` becomes a hot-loop bottleneck (very large inboxes), maintain a running counter that increments in `addMessage` and resets in `cleared`/`loadMessages`.

---

## 7. Concrete patch checklist

- [ ] `static/index.html`
  - [ ] Move `Pause`, `Auto`, `Bookmarks`, `btn-validation` buttons out of `.header-actions`
  - [ ] Add new `<div class="inbox-controls">` row inside `.list-panel`, between `.search-bar` and `.source-legend`
  - [ ] Re-insert those four buttons inside the new row, in order: Pause, Auto, divider, Bookmarks, btn-validation
  - [ ] Inside `.stats-bar` (or its replacement `.health` row), append: divider element + 3 new pills (`total`, `validation`, `parse errors`)
  - [ ] Rename the existing `errors` pill label to `parse errors`
  - [ ] Delete `.rate-band` block entirely

- [ ] `static/style.css`
  - [ ] Add `.inbox-controls`, `.ctl-btn`, `.ctl-btn.on`, `.ctl-btn.on.warn-on`, `.ctl-btn.on.err-on`, `.ctl-btn.on.ok-on`, `.ctl-btn .count`, `.ctl-divider`
  - [ ] Add `.topbar-divider`
  - [ ] Remove `.rate-band` and its descendants
  - [ ] Confirm `.health-pill.warn-pill` and `.health-pill.err-pill` exist (carry over from M1–M3 redesign if missing)

- [ ] `static/app.js`
  - [ ] Update `togglePause`, `toggleAutoscroll`, `toggleBookmarkFilter`, `syncValidationFilterUI` to use class toggles instead of inline styles
  - [ ] Remove rate-band specific render code (the bar histogram between `<g id="bars">` tags)
  - [ ] Keep the rate ring buffer and `rate` pill sparkline render
  - [ ] Add `updateHeaderCounters()` called from `flushAndRender`, `pollStats`, `cleared` handler — writes `total`, `validation`, `parse errors` pill values

---

## 8. What NOT to touch

- WebSocket protocol (`init`, `new_message`, `tags_updated`, `bookmark_toggled`, `lagged`, `cleared`)
- API endpoints
- Session persistence keys (`harux_session`, `_state`) and saved fields — `paused`, `autoscroll`, `showBookmarkedOnly`, `validationFilter` keep their meanings
- Source color palette / `getSourceColor`
- Diff-vs-pinned logic
- Rate ring buffer — the data feeds the `rate` pill sparkline, just no longer the deleted band

---

## 9. Suggested commit order

1. CSS additions (`.inbox-controls`, `.ctl-btn` family, `.topbar-divider`) — deployable without behavioral change
2. Move buttons out of header into new `.inbox-controls` row + class refactor in `app.js`
3. Add 3 new header pills + `updateHeaderCounters()`
4. Rename `errors` → `parse errors`
5. Delete `.rate-band` markup and CSS

Each step lands on its own and the app stays usable between commits.
