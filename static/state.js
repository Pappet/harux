// All mutable runtime state lives in one place. Other module files reference
// fields through `state.X`; the object is also exposed on `window.state` for
// devtools inspection. This is loaded first so every subsequent script sees
// a fully initialised `state`.

const state = {
    // Message buffer + selection. One selection (#277): `{ kind: 'message'
    // | 'draft', id }` or null. `selectedId` (below) is the message part of
    // it, so clearing a message selection never closes an open draft.
    messages: [],
    selection: null,
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
    // Total messages held by the *server* — the single source for the header
    // `total` pill (#182). Writers: WS `init` (authoritative), addMessage (+1),
    // removeEvictedMessages (−n), `cleared` (0), pollStats every 3 s
    // (authoritative resync) and loadMessages (never below what is loaded).
    // It is deliberately NOT the client buffer length — that is capped at
    // CLIENT_MESSAGE_CAP (#177).
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

    // List (re)load (#239): while `loadMessages` waits for the server's list,
    // WebSocket events are queued in `loadQueue` and replayed after the
    // snapshot is applied, so nothing that arrives meanwhile is lost.
    // `loadSeq` lets a newer load supersede an older one.
    loadInProgress: false,
    loadQueue: [],
    loadSeq: 0,

    // Older history (#198, #197): pages fetched by "Load older" / "Search
    // older on the server" raise the client cap by `olderLoaded`, so new
    // live messages don't trim them away at once. `serverSearch` continues a
    // server search below its last hit while the query stays the same.
    // All reset on a (re)load and on clear.
    olderLoaded: 0,
    olderLoading: false,
    serverSearch: { query: '', cursor: null, done: false },

    // Async request sequencing (#183): each selectMessage / toggleDiffPin
    // call takes a new number; a response is applied only if no newer call
    // started meanwhile, so a slow response can't overwrite a newer choice.
    selectRequestSeq: 0,
    diffPinRequestSeq: 0,

    // Segment diff
    diffPinnedMessage: null,
    diffIgnoreDynamic: false,

    // Sending (#272, #274). `sendConfig` is /api/send/config, or null while
    // sending is off — then nothing below is shown.
    sendConfig: null,
    targets: [],          // the user's own targets (send.js, localStorage)
    targetResults: {},    // last result per target id, this browser only
    lastTargetId: null,   // the one-click Resend target
    sendAs: 'unchanged',  // how Resend sends (#275): see SEND_AS in send.js
    sendMenu: null,       // { pickedId, form } while the Resend menu is open
    sending: false,
    targetStorageWarned: false,
    directionFilter: 0,   // 0: In + Out, 1: Received, 2: Sent

    // Drafts (#277, compose.js): text kept in this browser tab
    // (sessionStorage; "Keep on this computer" moves one to localStorage).
    // The preview is the server's parse of the open draft's text.
    drafts: [],
    draftPreview: null,   // { draftId, raw, filled, data, error }
    draftPreviewSeq: 0,
    draftPreviewTimer: null,
    draftSendAnyway: null, // id of a broken draft armed for "Send anyway"
    draftStorageWarned: false,
    newMenuOpen: false,
    // Field-by-field editing (#278): segments showing their empty fields
    // ("PID[1]"), and the dictionary's field lists by segment name.
    draftShowEmpty: new Set(),
    dictionaryCache: new Map(),
    // Templates (#279): the server's listing, fetched when the menu opens.
    templates: [],
};

// The selected stored message's ID, or null — a view of `selection`.
// Setting it selects that message; setting null clears only a message
// selection, never an open draft (#277: `cleared`, `evicted` and a failed
// fetch set it to null).
Object.defineProperty(state, 'selectedId', {
    get() { return this.selection && this.selection.kind === 'message' ? this.selection.id : null; },
    set(id) {
        if (id == null) {
            if (this.selection && this.selection.kind === 'message') this.selection = null;
        } else {
            this.selection = { kind: 'message', id };
        }
    },
    enumerable: true,
});

// The open draft's ID, or null.
Object.defineProperty(state, 'selectedDraftId', {
    get() { return this.selection && this.selection.kind === 'draft' ? this.selection.id : null; },
    enumerable: true,
});

window.state = state;
