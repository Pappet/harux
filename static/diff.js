// Segment-by-segment diff view. Compares the currently selected message
// against `state.diffPinnedMessage`, pairing segments by name and
// occurrence (OBX[1] with OBX[1], OBX[2] with OBX[2], … — #228). Split into focused builders so that
// adding more diff capabilities (issue #139 / F4 follow-ups) stays cheap.

function buildDiffHeader(msgA, msgB) {
    return `
        <div class="diff-header">
            <div class="diff-col-label diff-label-a">
                &#128204; Reference: <strong>${esc(msgA.message_type)}</strong>
                <span class="diff-meta">${esc(msgA.message_control_id)}</span>
            </div>
            <div class="diff-col-label diff-label-b">
                &#10145; Current: <strong>${esc(msgB.message_type)}</strong>
                <span class="diff-meta">${esc(msgB.message_control_id)}</span>
            </div>
        </div>
    `;
}

// Render one field comparison row. Returns either the HTML for the row or
// `null` if the row should be skipped (dynamic field hidden). The `stats`
// object is mutated in place to accumulate counts.
function buildDiffFieldRow(segName, label, idx, fA, fB, ignoreDynamic, stats) {
    const fieldKey = `${segName}-${idx}`; // dynamic-field lookup ignores the occurrence
    const vA = fA ? fA.value : '';
    const vB = fB ? fB.value : '';
    const changed = vA !== vB;

    if (ignoreDynamic && DYNAMIC_DIFF_FIELDS.has(fieldKey)) {
        if (changed) stats.hiddenDynamic++;
        return null;
    }

    const desc = (fA && fA.description) || (fB && fB.description) || '';
    if (changed) {
        stats.totalDiffs++;
        stats.segHasDiff = true;
    }
    const rowClass = changed ? 'diff-row changed' : 'diff-row same';
    const descHtml = desc ? ' <span class="diff-desc">' + esc(desc) + '</span>' : '';
    return `
        <tr class="${rowClass}">
            <td class="diff-field-name" title="${escAttr(desc)}">${esc(label)}-${idx}${descHtml}</td>
            <td class="diff-val diff-val-a ${changed ? 'diff-changed' : ''}">${diffValueHtml(vA)}</td>
            <td class="diff-val diff-val-b ${changed ? 'diff-changed' : ''}">${diffValueHtml(vB)}</td>
        </tr>`;
}

// A diff cell: the value (compared in full above), shown as a preview when
// it is a multi-MB attachment (#215).
function diffValueHtml(v) {
    if (!v) return '<span class="field-empty">empty</span>';
    const shown = previewText(v);
    if (!shown.truncated) return esc(v);
    return esc(shown.text) + `<span class="value-truncated">… ${esc(shown.size)} in total, shortened for display</span>`;
}

function buildDiffSegmentBlock(segName, label, segA, segB, ignoreDynamic, stats) {
    // Collect all field indices (union)
    const allIdxs = new Set();
    (segA ? segA.fields : []).forEach(f => allIdxs.add(f.index));
    (segB ? segB.fields : []).forEach(f => allIdxs.add(f.index));
    const sortedIdxs = Array.from(allIdxs).sort((a, b) => a - b);

    const missingA = !segA;
    const missingB = !segB;
    const segClass = (missingA || missingB) ? 'diff-segment-missing' : 'diff-segment';

    // Per-segment local stats — the segment header needs to know if any
    // changes were found inside it. Roll into the parent `stats` after.
    const segStats = { totalDiffs: 0, hiddenDynamic: 0, segHasDiff: missingA || missingB };

    let rows = '';
    for (const idx of sortedIdxs) {
        const fA = segA ? segA.fields.find(f => f.index === idx) : null;
        const fB = segB ? segB.fields.find(f => f.index === idx) : null;
        const rowHtml = buildDiffFieldRow(segName, label, idx, fA, fB, ignoreDynamic, segStats);
        if (rowHtml !== null) rows += rowHtml;
    }

    stats.totalDiffs += segStats.totalDiffs;
    stats.hiddenDynamic += segStats.hiddenDynamic;
    // A segment present on one side only is a difference in itself, even if
    // all its fields are empty (#228).
    if (missingA || missingB) stats.missingSegments++;

    const onlyInCurrent = missingA ? '<span class="diff-missing-badge">only in current</span>' : '';
    const onlyInRef = missingB ? '<span class="diff-missing-badge">only in reference</span>' : '';

    return `
        <div class="diff-segment-block${segStats.segHasDiff ? ' has-diff' : ''}">
            <div class="diff-segment-name ${segClass}">
                ${esc(label)}
                ${onlyInCurrent}
                ${onlyInRef}
            </div>
            <table class="diff-table">
                <thead><tr>
                    <th class="diff-field-col">Field</th>
                    <th>Reference value</th>
                    <th>Current value</th>
                </tr></thead>
                <tbody>${rows}</tbody>
            </table>
        </div>`;
}

function buildDiffSummary(totalDiffs, missingSegments, hiddenDynamic) {
    const hiddenNote = hiddenDynamic > 0 ? ` (${hiddenDynamic} dynamic hidden)` : '';
    if (totalDiffs === 0 && missingSegments === 0) {
        return `<div class="diff-summary same">&#10003; Messages are identical${hiddenNote}</div>`;
    }
    const parts = [];
    if (totalDiffs > 0) parts.push(`${totalDiffs} field difference${totalDiffs > 1 ? 's' : ''} found`);
    if (missingSegments > 0) parts.push(`${missingSegments} segment${missingSegments > 1 ? 's' : ''} only in one message`);
    return `<div class="diff-summary changed">&#9651; ${parts.join(', ')}${hiddenNote}</div>`;
}

// Pair segments by name *and occurrence* (#228): the n-th OBX of the
// reference with the n-th OBX of the current message. Pairs keep the
// reference's order; an occurrence only the current message has is placed
// after the last pair of the same name (or at the end). Repeating names are
// labelled `OBX[2]`, single ones keep their plain name.
function pairSegments(segsA, segsB) {
    const withOccurrence = (segs) => {
        const seen = new Map();
        return segs.map(seg => {
            const n = (seen.get(seg.name) || 0) + 1;
            seen.set(seg.name, n);
            return { seg, n, key: `${seg.name}#${n}` };
        });
    };
    const a = withOccurrence(segsA);
    const b = withOccurrence(segsB);
    const bByKey = new Map(b.map(x => [x.key, x]));
    const maxCount = new Map();
    for (const x of [...a, ...b]) {
        maxCount.set(x.seg.name, Math.max(maxCount.get(x.seg.name) || 0, x.n));
    }

    const pairs = a.map(x => ({
        name: x.seg.name,
        n: x.n,
        segA: x.seg,
        segB: bByKey.has(x.key) ? bByKey.get(x.key).seg : null,
    }));
    const inA = new Set(a.map(x => x.key));
    for (const y of b) {
        if (inA.has(y.key)) continue;
        const pair = { name: y.seg.name, n: y.n, segA: null, segB: y.seg };
        let at = -1;
        for (let i = pairs.length - 1; i >= 0; i--) {
            if (pairs[i].name === y.seg.name) { at = i; break; }
        }
        if (at >= 0) pairs.splice(at + 1, 0, pair);
        else pairs.push(pair);
    }
    for (const p of pairs) {
        p.label = maxCount.get(p.name) > 1 ? `${p.name}[${p.n}]` : p.name;
    }
    return pairs;
}

function buildDiffOptionsBar() {
    return `
        <div class="diff-options-bar">
            <label class="theme-toggle">
                <input type="checkbox" data-action="diff-ignore-dynamic" ${state.diffIgnoreDynamic ? 'checked' : ''}>
                <span class="toggle-slider"></span>
                Hide dynamic fields (MSH-7, MSH-10)
            </label>
        </div>`;
}

function renderDiffTab(container, msgB) {
    const msgA = state.diffPinnedMessage;
    if (!msgA) {
        container.innerHTML = '<div class="empty-state"><p>No reference message pinned.</p></div>';
        return;
    }

    const stats = { totalDiffs: 0, missingSegments: 0, hiddenDynamic: 0 };
    let segmentsHtml = '';
    for (const p of pairSegments(msgA.segments || [], msgB.segments || [])) {
        segmentsHtml += buildDiffSegmentBlock(p.name, p.label, p.segA, p.segB, state.diffIgnoreDynamic, stats);
    }

    container.innerHTML =
        buildDiffHeader(msgA, msgB) +
        buildDiffSummary(stats.totalDiffs, stats.missingSegments, stats.hiddenDynamic) +
        buildDiffOptionsBar() +
        segmentsHtml;
}

async function toggleDiffPin(id, event) {
    if (event) event.stopPropagation();
    // Every pin/unpin invalidates in-flight pin requests (#183), so a slow
    // response for an earlier pin can't override a newer pin or an unpin.
    const seq = ++state.diffPinRequestSeq;
    if (state.diffPinnedMessage && state.diffPinnedMessage.id === id) {
        state.diffPinnedMessage = null;
        saveSession();
        renderMessageList();
        renderDetail();
        return;
    }
    try {
        const resp = await fetch(`/api/messages/${id}`);
        if (seq !== state.diffPinRequestSeq) return;
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        const msg = await resp.json();
        if (seq !== state.diffPinRequestSeq) return;
        state.diffPinnedMessage = msg;
    } catch (e) {
        console.error('Failed to fetch pinned message:', e);
        return;
    }
    saveSession();
    renderMessageList();
    renderDetail();
}

function toggleDiffIgnoreDynamic(e) {
    state.diffIgnoreDynamic = e.target.checked;
    renderTab();
    saveSession();
}
