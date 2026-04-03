/* =============================================
   Application Init
   ============================================= */

(function() {
    'use strict';

    document.addEventListener('DOMContentLoaded', function() {
        initApp();
    });

    function initApp() {
        // Load persisted state
        loadHistory();
        renderHistory(AppState.queryHistory);

        // Set version badge
        var versionBadge = document.getElementById('version-badge');
        if (versionBadge) {
            versionBadge.textContent = 'v0.1.0';
        }

        // Load schemas
        fetchSchemas().then(function(schemas) {
            if (schemas && schemas.length > 0) {
                AppState.schemas = schemas;
                renderSchemas(schemas);
            }
        });

        // Load available models
        fetchModels().then(function(models) {
            if (models && models.length > 0) {
                var select = document.getElementById('model-select');
                select.innerHTML = '<option value="default">default</option>';
                models.forEach(function(model) {
                    var opt = document.createElement('option');
                    opt.value = model.name || model;
                    opt.textContent = model.name || model;
                    select.appendChild(opt);
                });
            }
        });

        // Check health
        checkHealth();
        setInterval(checkHealth, 30000);

        // Connect WebSocket
        connectWebSocket();

        // --- Event Handlers ---

        // Query form submit
        var queryForm = document.getElementById('query-form');
        queryForm.addEventListener('submit', function(e) {
            e.preventDefault();

            // If currently loading, abort instead
            if (AppState.loading) {
                abortCurrentQuery();
                showLoading(false);
                showError('Query aborted');
                return;
            }

            var input = document.getElementById('query-input');
            var queryText = input.value.trim();
            if (!queryText) return;

            var model = document.getElementById('model-select').value;

            AppState.currentQuery = queryText;
            showLoading(true);

            queryApi(queryText, model).then(function(response) {
                if (response.requires_confirmation) {
                    showLoading(false);
                    showConfirmModal(response);
                    return;
                }
                showLoading(false);
                renderResults(response);
                addToHistory(
                    queryText,
                    response.parsed ? response.parsed.collection : '',
                    response.count || 0,
                    response.execution_ms
                );
                renderHistory(AppState.queryHistory);
            }).catch(function(err) {
                showLoading(false);
                if (err.name !== 'AbortError') {
                    console.error('Query failed:', err);
                }
            });
        });

        // Export buttons
        document.getElementById('export-csv').addEventListener('click', downloadCsv);
        document.getElementById('export-json').addEventListener('click', downloadJson);

        // Sidebar toggle
        var sidebar = document.getElementById('sidebar');
        var sidebarToggle = document.getElementById('sidebar-toggle');

        sidebarToggle.addEventListener('click', function() {
            if (window.innerWidth <= 768) {
                sidebar.classList.toggle('open');
                toggleOverlay(sidebar.classList.contains('open'));
            } else {
                sidebar.classList.toggle('collapsed');
                document.getElementById('app').style.gridTemplateColumns =
                    sidebar.classList.contains('collapsed') ? '1fr' : 'var(--sidebar-width) 1fr';
            }
        });

        // Query inspector toggles (mongo query + parsed JSON)
        document.querySelectorAll('.collapsible-toggle').forEach(function(toggle) {
            toggle.addEventListener('click', function() {
                var pre = toggle.nextElementSibling;
                if (!pre) return;
                var isOpen = !pre.hidden;
                pre.hidden = isOpen;
                toggle.classList.toggle('open', !isOpen);
                var label = toggle.textContent.replace(/[^a-zA-Z ]/g, '').trim();
                toggle.innerHTML = label + (isOpen ? ' &#9656;' : ' &#9662;');
            });
        });

        // Example query buttons
        var exampleBtns = document.querySelectorAll('.example-query');
        exampleBtns.forEach(function(btn) {
            btn.addEventListener('click', function() {
                document.getElementById('query-input').value = btn.textContent;
                queryForm.dispatchEvent(new Event('submit'));
            });
        });

        // Keyboard shortcut: Ctrl/Cmd+K to focus query input
        document.addEventListener('keydown', function(e) {
            if ((e.ctrlKey || e.metaKey) && e.key === 'k') {
                e.preventDefault();
                document.getElementById('query-input').focus();
                document.getElementById('query-input').select();
            }
        });

        // Confirmation modal handlers
        document.getElementById('confirm-close').addEventListener('click', hideConfirmModal);
        document.getElementById('confirm-cancel').addEventListener('click', hideConfirmModal);
        document.getElementById('confirm-execute').addEventListener('click', function() {
            var parsed = AppState.pendingWrite;
            if (!parsed) return;
            hideConfirmModal();
            showLoading(true);
            executeWrite(parsed).then(function(result) {
                showLoading(false);
                renderWriteResult(result);
            }).catch(function() {
                showLoading(false);
            });
        });
    }

    function showConfirmModal(response) {
        AppState.pendingWrite = response.parsed;
        var modal = document.getElementById('confirm-modal');
        var op = response.parsed.operation || 'write';

        document.getElementById('confirm-title').textContent =
            'Confirm ' + op.charAt(0).toUpperCase() + op.slice(1) + ' Operation';

        var detailHtml = '<strong>Collection:</strong> ' + response.parsed.collection + '<br>';
        if (response.parsed.filters && response.parsed.filters.length > 0) {
            detailHtml += '<strong>Filter:</strong> ' + response.parsed.filters.length + ' conditions<br>';
        }
        if (response.parsed.update_fields) {
            detailHtml += '<strong>Update:</strong> ' + JSON.stringify(response.parsed.update_fields) + '<br>';
        }
        if (response.parsed.document) {
            detailHtml += '<strong>Document:</strong> ' + JSON.stringify(response.parsed.document).substring(0, 200) + '<br>';
        }
        if (response.parsed.update_many) detailHtml += '<strong>Scope:</strong> ALL matching documents<br>';
        if (response.parsed.delete_many) detailHtml += '<strong>Scope:</strong> ALL matching documents<br>';

        document.getElementById('confirm-operation').innerHTML = detailHtml;
        document.getElementById('confirm-query').textContent = response.mongo_query || '';

        // Danger styling for deletes
        var execBtn = document.getElementById('confirm-execute');
        if (op === 'delete') {
            execBtn.textContent = 'Delete';
            document.getElementById('confirm-warning').textContent =
                'WARNING: This will permanently delete data. This cannot be undone.';
        } else if (op === 'update') {
            execBtn.textContent = 'Update';
            document.getElementById('confirm-warning').textContent =
                'This will modify existing data. Please review the changes carefully.';
        } else {
            execBtn.textContent = 'Insert';
            document.getElementById('confirm-warning').textContent =
                'This will insert new data into your database.';
        }

        modal.hidden = false;
    }

    function hideConfirmModal() {
        document.getElementById('confirm-modal').hidden = true;
        AppState.pendingWrite = null;
    }

    // --- Overlay for mobile sidebar ---

    function toggleOverlay(show) {
        var overlay = document.querySelector('.sidebar-overlay');
        if (show && !overlay) {
            overlay = document.createElement('div');
            overlay.className = 'sidebar-overlay visible';
            overlay.addEventListener('click', function() {
                document.getElementById('sidebar').classList.remove('open');
                toggleOverlay(false);
            });
            document.body.appendChild(overlay);
        } else if (!show && overlay) {
            overlay.remove();
        }
    }
})();
