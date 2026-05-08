## 2026-05-08 - Added keyboard accessibility to custom list items
**Learning:** When using `div` elements as clickable rows in a list, setting `onclick` is not enough for keyboard accessibility. Screen reader and keyboard-only users cannot interact with them.
**Action:** Always add `tabIndex="0"`, `role="button"`, an `onkeydown` handler for 'Enter'/'Space', and a `:focus-visible` CSS pseudo-class to ensure custom interactive elements are fully accessible.
