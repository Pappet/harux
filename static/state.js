// All mutable runtime state lives in one place. Other module files reference
// fields through `state.X`; the object is also exposed on `window.state` for
// devtools inspection. This is loaded first so every subsequent script sees
// a fully initialised `state`.

const state = {
    // Message buffer + selection
    messages: [],
    selectedId: null,
    selectedMessage: null,

    // Detail panel
    activeTab: 'parsed',
    collapsedSegments: new Set(),

    // List UI
    autoscroll: true,
    searchQuery: '',
    paused: false,
    pendingMessages: [],
    renderScheduled: false,
    showBookmarkedOnly: false,
    validationFilter: 0, // 0: All, 1: Warnings, 2: Errors Only
    searchDebounceTimer: null,

    // WebSocket
    ws: null,
    wsReconnectDelay: 1000, // initial; ws.js caps via WS_RECONNECT_MAX

    // Server-derived counters
    totalMessagesCount: 0,
    // Incrementally-maintained totals so updateHeaderCounters never iterates messages[].
    // Reset to 0 on clear/reload; recomputed from fresh batch in loadMessages().
    totalValidationCount: 0,
    totalBookmarkCount: 0,

    // Health-pill state: rolling 60-second window of message timestamps.
    rateWindow: [], // Date.now() timestamps within last 60 s
    lastMessageReceivedAt: null, // Date.now() of most recent addMessage

    // Source colouring
    seenSources: new Set(),
    colorByPort: false,
    highlightedSource: null,
    // Per-source message counts maintained incrementally so renderSourceLegend
    // does not iterate `messages[]` on every flush. Recomputed only when
    // labelling changes (toggleColorByPort) or the buffer is replaced.
    sourceCounts: new Map(),

    // Segment diff
    diffPinnedMessage: null,
    diffIgnoreDynamic: false,
};

window.state = state;
