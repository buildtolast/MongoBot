/* =============================================
   WebSocket Connection
   ============================================= */

var ws = null;
var wsReconnectDelay = 1000;
var wsMaxDelay = 30000;
var wsReconnectTimer = null;

function connectWebSocket() {
    if (ws && (ws.readyState === WebSocket.CONNECTING || ws.readyState === WebSocket.OPEN)) {
        return;
    }

    var proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    var url = proto + '//' + location.host + '/ws/live';

    try {
        ws = new WebSocket(url);
    } catch (err) {
        console.warn('WebSocket connection error:', err);
        scheduleReconnect();
        return;
    }

    ws.onopen = function() {
        console.log('[WS] Connected');
        AppState.wsConnected = true;
        updateStatusBadge('ws-status', true);
        wsReconnectDelay = 1000;
    };

    ws.onmessage = function(evt) {
        try {
            var msg = JSON.parse(evt.data);
            handleWsMessage(msg);
        } catch (err) {
            console.warn('[WS] Failed to parse message:', err);
        }
    };

    ws.onclose = function(evt) {
        console.log('[WS] Disconnected (code=' + evt.code + ')');
        AppState.wsConnected = false;
        updateStatusBadge('ws-status', false);
        ws = null;
        scheduleReconnect();
    };

    ws.onerror = function() {
        // onclose will fire after this
        AppState.wsConnected = false;
        updateStatusBadge('ws-status', false);
    };
}

function scheduleReconnect() {
    if (wsReconnectTimer) return;
    wsReconnectTimer = setTimeout(function() {
        wsReconnectTimer = null;
        connectWebSocket();
        // Exponential backoff
        wsReconnectDelay = Math.min(wsReconnectDelay * 2, wsMaxDelay);
    }, wsReconnectDelay);
}

function handleWsMessage(msg) {
    switch (msg.type) {
        case 'health':
            if (msg.mongodb !== undefined) {
                updateStatusBadge('mongo-status', msg.mongodb);
                AppState.mongoConnected = msg.mongodb;
            }
            if (msg.ollama !== undefined) {
                updateStatusBadge('ollama-status', msg.ollama);
                AppState.ollamaConnected = msg.ollama;
            }
            break;
        case 'progress':
            // Could show partial progress
            if (msg.message) {
                console.log('[WS] Progress:', msg.message);
            }
            break;
        case 'schema_update':
            if (msg.schemas) {
                AppState.schemas = msg.schemas;
                renderSchemas(msg.schemas);
            }
            break;
        default:
            console.log('[WS] Unknown message type:', msg.type);
    }
}
