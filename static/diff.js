// Segment-by-segment diff view. Compares the currently selected message
// against `state.diffPinnedMessage`. Split into focused builders so that
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
function buildDiffFieldRow(segName, idx, fA, fB, ignoreDynamic, stats) {
    const fieldKey = `${segName}-${idx}`;
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
            <td class="diff-field-name" title="${escAttr(desc)}">${esc(segName)}-${idx}${descHtml}</td>
            <td class="diff-val diff-val-a ${changed ? 'diff-changed' : ''}">${esc(vA) || '<span class="field-empty">empty</span>'}</td>
            <td class="diff-val diff-val-b ${changed ? 'diff-changed' : ''}">${esc(vB) || '<span class="field-empty">empty</span>'}</td>
        </tr>`;
}

function buildDiffSegmentBlock(segName, segA, segB, ignoreDynamic, stats) {
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
        const rowHtml = buildDiffFieldRow(segName, idx, fA, fB, ignoreDynamic, segStats);
        if (rowHtml !== null) rows += rowHtml;
    }

    stats.totalDiffs += segStats.totalDiffs;
    stats.hiddenDynamic += segStats.hiddenDynamic;

    const onlyInCurrent = missingA ? '<span class="diff-missing-badge">only in current</span>' : '';
    const onlyInRef = missingB ? '<span class="diff-missing-badge">only in reference</span>' : '';

    return `
        <div class="diff-segment-block${segStats.segHasDiff ? ' has-diff' : ''}">
            <div class="diff-segment-name ${segClass}">
                ${esc(segName)}
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

function buildDiffSummary(totalDiffs, hiddenDynamic) {
    const hiddenNote = hiddenDynamic > 0 ? ` (${hiddenDynamic} dynamic hidden)` : '';
    return totalDiffs === 0
        ? `<div class="diff-summary same">&#10003; Messages are identical${hiddenNote}</div>`
        : `<div class="diff-summary changed">&#9651; ${totalDiffs} field difference${totalDiffs > 1 ? 's' : ''} found${hiddenNote}</div>`;
}

function buildDiffOptionsBar() {
    return `
        <div class="diff-options-bar">
            <label class="theme-toggle" style="cursor:pointer; display:flex; align-items:center; gap:8px;">
                <input type="checkbox" onchange="toggleDiffIgnoreDynamic(event)" style="display:none" ${state.diffIgnoreDynamic ? 'checked' : ''}>
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

    const segsA = msgA.segments || [];
    const segsB = msgB.segments || [];

    // Collect all segment names (union, preserving order: A first, then B-only).
    // Diff is pairing-by-first-occurrence — multi-segment messages compare
    // only the first instance of each name.
    const allSegNames = [];
    const seen = new Set();
    [...segsA, ...segsB].forEach(s => {
        if (!seen.has(s.name)) {
            seen.add(s.name);
            allSegNames.push(s.name);
        }
    });

    const findFirstSeg = (segs, name) => segs.find(s => s.name === name);

    const stats = { totalDiffs: 0, hiddenDynamic: 0 };
    let segmentsHtml = '';
    for (const segName of allSegNames) {
        const segA = findFirstSeg(segsA, segName);
        const segB = findFirstSeg(segsB, segName);
        if (!segA && !segB) continue;
        segmentsHtml += buildDiffSegmentBlock(segName, segA, segB, state.diffIgnoreDynamic, stats);
    }

    container.innerHTML =
        buildDiffHeader(msgA, msgB) +
        buildDiffSummary(stats.totalDiffs, stats.hiddenDynamic) +
        buildDiffOptionsBar() +
        segmentsHtml;
}

async function toggleDiffPin(id, event) {
    if (event) event.stopPropagation();
    if (state.diffPinnedMessage && state.diffPinnedMessage.id === id) {
        state.diffPinnedMessage = null;
        renderMessageList();
        renderDetail();
        return;
    }
    try {
        const resp = await fetch(`/api/messages/${id}`);
        if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
        state.diffPinnedMessage = await resp.json();
    } catch (e) {
        console.error('Failed to fetch pinned message:', e);
        return;
    }
    renderMessageList();
    renderDetail();
}

function toggleDiffIgnoreDynamic(e) {
    state.diffIgnoreDynamic = e.target.checked;
    renderTab();
    saveSession();
}
