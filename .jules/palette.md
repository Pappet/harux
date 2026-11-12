> **Historical learning log** written by the Jules agent. The current rules
> for agents are in [`AGENTS.md`](../AGENTS.md); where a note below
> contradicts the code or `AGENTS.md`, the code and `AGENTS.md` win (#246).

## 2026-05-08 - Added keyboard accessibility to custom list items
**Learning:** When using `div` elements as clickable rows in a list, setting `onclick` is not enough for keyboard accessibility. Screen reader and keyboard-only users cannot interact with them.
**Action:** Always add `tabIndex="0"`, `role="button"`, an `onkeydown` handler for 'Enter'/'Space', and a `:focus-visible` CSS pseudo-class to ensure custom interactive elements are fully accessible.

## 2026-05-09 - Added keyboard accessibility to standard UI components (spans/divs acting as buttons)
**Learning:** `div` and `span` elements that use `onclick` handlers inside app lists, headers, or custom tables don't automatically receive focus or trigger on keyboard interaction. Using click listeners and missing `aria-label` degrades experience for keyboard users.
**Action:** Consistently ensure that all custom interactive components (like `.segment-name`, `.copy-btn`, and `.field-val`) have `tabindex="0"`, `role="button"`, proper `aria-*` state, and either global or inline keyboard event listeners (like 'Enter'/'Space') to trigger their associated actions.

**Current state (2026-11):** keyboard activation is handled once, globally, in `static/app.js` (the keydown handler that activates `data-action` elements with Enter/Space). Give new interactive elements `tabindex="0"`, a `role`, an `aria-label` where needed and a `data-action` — no per-element `onclick`/`onkeydown` attributes, which the CSP blocks.
