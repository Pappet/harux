> **Historical learning log** written by the Jules agent. The current rules
> for agents are in [`AGENTS.md`](../AGENTS.md); where a note below
> contradicts the code or `AGENTS.md`, the code and `AGENTS.md` win (#246).

## 2025-02-28 - [Cross-Site Scripting via Improper HTML Attribute Escaping in JS context]
**Vulnerability:** XSS vulnerability where user input was correctly HTML-escaped with `escAttr`, but then rendered directly into `onclick="..."` HTML attributes. Because the browser HTML parser decodes HTML entities back to characters before passing the result to the JS parser, characters like `'` (which was escaped as `&#x27;`) were successfully injected into JS string literals breaking string context and running arbitrary JS payload.
**Learning:** You can't just escape HTML characters when inserting untrusted data into a JS event handler string in an HTML attribute. It first has to be escaped for JS execution (escaping backslashes, quotes) *then* HTML-escaped.
**Prevention (superseded):** At the time, the fix was a second escape step, `escAttr(escJS(input))`. That pattern is gone: inline handlers were removed (#137), the Content-Security-Policy forbids them, and `escJS` was deleted (#194). **Today:** never build `onclick="…"` or any inline handler; put the value in a `data-*` attribute escaped with `escAttr` (`static/util.js`) and handle it through the `data-action` delegation in `static/app.js`.
