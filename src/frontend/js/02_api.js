/* =============================================
   API Client
   ============================================= */

var API_BASE = '';

async function queryApi(queryText, model) {
    // Cancel any previous in-flight request
    if (currentAbortController) {
        currentAbortController.abort();
    }
    currentAbortController = new AbortController();

    var body = { query: queryText };
    if (model && model !== 'default') {
        body.model = model;
    }
    try {
        var resp = await fetch(API_BASE + '/api/query', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
            signal: currentAbortController.signal
        });
        currentAbortController = null;
        if (!resp.ok) {
            var errBody = await resp.text();
            var errMsg;
            try {
                errMsg = JSON.parse(errBody).error || errBody;
            } catch (_) {
                errMsg = errBody || ('HTTP ' + resp.status);
            }
            throw new Error(errMsg);
        }
        return await resp.json();
    } catch (err) {
        currentAbortController = null;
        if (err.name === 'AbortError') {
            throw err; // Don't show toast for user-initiated abort
        }
        showError(err.message || 'Query failed');
        throw err;
    }
}

async function previewQuery(queryText, model) {
    if (currentAbortController) {
        currentAbortController.abort();
    }
    currentAbortController = new AbortController();

    var body = { query: queryText, preview: true };
    if (model && model !== 'default') {
        body.model = model;
    }
    try {
        var resp = await fetch(API_BASE + '/api/query', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
            signal: currentAbortController.signal
        });
        currentAbortController = null;
        if (!resp.ok) {
            var errBody = await resp.text();
            var errMsg;
            try { errMsg = JSON.parse(errBody).error || errBody; } catch(_) { errMsg = errBody || ('HTTP ' + resp.status); }
            throw new Error(errMsg);
        }
        return await resp.json();
    } catch (err) {
        currentAbortController = null;
        if (err.name === 'AbortError') throw err;
        showError(err.message || 'Preview failed');
        throw err;
    }
}

function abortCurrentQuery() {
    if (currentAbortController) {
        currentAbortController.abort();
        currentAbortController = null;
    }
}

async function executeWrite(parsedQuery) {
    try {
        var resp = await fetch(API_BASE + '/api/execute', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ parsed: parsedQuery, confirm: true })
        });
        if (!resp.ok) {
            var errBody = await resp.text();
            var errMsg;
            try { errMsg = JSON.parse(errBody).error || errBody; } catch(_) { errMsg = errBody; }
            throw new Error(errMsg);
        }
        return await resp.json();
    } catch (err) {
        showError(err.message || 'Write operation failed');
        throw err;
    }
}

async function fetchConfig() {
    try {
        var resp = await fetch(API_BASE + '/api/config');
        if (!resp.ok) throw new Error('Failed to fetch config');
        return await resp.json();
    } catch (err) {
        console.warn('Config fetch failed:', err);
        return null;
    }
}

async function fetchSchemas() {
    try {
        var resp = await fetch(API_BASE + '/api/schemas');
        if (!resp.ok) {
            throw new Error('Failed to fetch schemas: HTTP ' + resp.status);
        }
        return await resp.json();
    } catch (err) {
        console.warn('Schema fetch failed:', err);
        return [];
    }
}

async function refreshSchemas() {
    try {
        var resp = await fetch(API_BASE + '/api/schemas/refresh', { method: 'POST' });
        if (!resp.ok) throw new Error('Refresh failed: HTTP ' + resp.status);
        var data = await resp.json();
        return data.schemas || [];
    } catch (err) {
        showError('Schema refresh failed: ' + err.message);
        return [];
    }
}

async function fetchModels() {
    try {
        var resp = await fetch(API_BASE + '/api/models');
        if (!resp.ok) return [];
        return await resp.json();
    } catch (err) {
        console.warn('Models fetch failed:', err);
        return [];
    }
}

function exportData(format) {
    var response = AppState.lastResults;
    if (!response || !response.results) {
        showError('No results to export');
        return;
    }

    var data = Array.isArray(response.results) ? response.results : [response.results];
    if (data.length === 0) {
        showError('No data to export');
        return;
    }

    var content, mimeType, ext;
    if (format === 'csv') {
        var allKeys = {};
        data.forEach(function(doc) {
            Object.keys(flattenDocument(doc)).forEach(function(k) { allKeys[k] = true; });
        });
        var headers = Object.keys(allKeys);
        var rows = [headers.join(',')];
        data.forEach(function(doc) {
            var flat = flattenDocument(doc);
            var row = headers.map(function(h) {
                var v = flat[h];
                if (v == null) return '';
                var s = String(v);
                if (s.indexOf(',') >= 0 || s.indexOf('"') >= 0 || s.indexOf('\n') >= 0) {
                    return '"' + s.replace(/"/g, '""') + '"';
                }
                return s;
            });
            rows.push(row.join(','));
        });
        content = rows.join('\n');
        mimeType = 'text/csv';
        ext = 'csv';
    } else {
        content = JSON.stringify(data, null, 2);
        mimeType = 'application/json';
        ext = 'json';
    }

    var collection = response.parsed ? response.parsed.collection : 'export';
    var timestamp = new Date().toISOString().replace(/[:.]/g, '-').substring(0, 19);
    var filename = collection + '_' + timestamp + '.' + ext;

    var blob = new Blob([content], { type: mimeType });
    var url = URL.createObjectURL(blob);
    var a = document.createElement('a');
    a.href = url;
    a.download = filename;
    a.click();
    URL.revokeObjectURL(url);
}

async function checkHealth() {
    try {
        var resp = await fetch(API_BASE + '/api/health');
        if (!resp.ok) throw new Error('Health check failed');
        var data = await resp.json();
        updateStatusBadge('mongo-status', data.mongo === true);
        updateStatusBadge('ollama-status', data.ollama === true);
        AppState.mongoConnected = data.mongo === true;
        AppState.ollamaConnected = data.ollama === true;
        return data;
    } catch (err) {
        updateStatusBadge('mongo-status', false);
        updateStatusBadge('ollama-status', false);
        AppState.mongoConnected = false;
        AppState.ollamaConnected = false;
        return null;
    }
}
