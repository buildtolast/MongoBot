/* =============================================
   Application State
   ============================================= */

var currentAbortController = null;

var AppState = {
    currentQuery: '',
    lastResults: null,
    queryHistory: [],
    currentPage: 1,
    pageSize: 25,
    schemas: [],
    wsConnected: false,
    mongoConnected: false,
    ollamaConnected: false,
    loading: false,
    sortColumn: null,
    sortDirection: 'asc',
    pendingWrite: null
};

var HISTORY_KEY = 'mqb_query_history';
var HISTORY_MAX = 50;

function loadHistory() {
    try {
        var stored = localStorage.getItem(HISTORY_KEY);
        if (stored) {
            AppState.queryHistory = JSON.parse(stored);
        }
    } catch (e) {
        console.warn('Failed to load query history:', e);
        AppState.queryHistory = [];
    }
}

function saveHistory() {
    try {
        localStorage.setItem(HISTORY_KEY, JSON.stringify(AppState.queryHistory));
    } catch (e) {
        console.warn('Failed to save query history:', e);
    }
}

function addToHistory(queryText, collection, resultCount, durationMs) {
    var entry = {
        query: queryText,
        collection: collection || '',
        resultCount: resultCount || 0,
        durationMs: durationMs || 0,
        timestamp: Date.now()
    };
    // Remove duplicate if exists
    AppState.queryHistory = AppState.queryHistory.filter(function(h) {
        return h.query !== queryText;
    });
    AppState.queryHistory.unshift(entry);
    if (AppState.queryHistory.length > HISTORY_MAX) {
        AppState.queryHistory = AppState.queryHistory.slice(0, HISTORY_MAX);
    }
    saveHistory();
}
