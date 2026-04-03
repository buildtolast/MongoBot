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
            var input = document.getElementById('query-input');
            var queryText = input.value.trim();
            if (!queryText || AppState.loading) return;

            var model = document.getElementById('model-select').value;

            AppState.currentQuery = queryText;
            showLoading(true);

            queryApi(queryText, model).then(function(response) {
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
                console.error('Query failed:', err);
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

        // Query inspector toggle
        var inspectorToggle = document.querySelector('.collapsible-toggle');
        if (inspectorToggle) {
            inspectorToggle.addEventListener('click', function() {
                var pre = document.getElementById('parsed-query-json');
                var isOpen = !pre.hidden;
                pre.hidden = isOpen;
                inspectorToggle.classList.toggle('open', !isOpen);
                inspectorToggle.innerHTML = isOpen ? 'Parsed Query &#9656;' : 'Parsed Query &#9662;';
            });
        }

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
