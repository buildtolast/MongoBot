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

        // Load theme
        var savedTheme = localStorage.getItem('mqb_theme') || 'dark';
        document.documentElement.setAttribute('data-theme', savedTheme);
        document.getElementById('theme-select').value = savedTheme;

        // Theme selector
        document.getElementById('theme-select').addEventListener('change', function() {
            var theme = this.value;
            document.documentElement.setAttribute('data-theme', theme);
            localStorage.setItem('mqb_theme', theme);
        });

        // Set version badge
        var versionBadge = document.getElementById('version-badge');
        if (versionBadge) {
            versionBadge.textContent = 'v0.3.0';
        }

        // Load schemas
        fetchSchemas().then(function(schemas) {
            if (schemas && schemas.length > 0) {
                AppState.schemas = schemas;
                renderSchemas(schemas);
            }
        });

        // Refresh schemas button
        document.getElementById('refresh-schemas').addEventListener('click', function() {
            var btn = this;
            btn.classList.add('spinning');
            btn.disabled = true;
            refreshSchemas().then(function(schemas) {
                if (schemas && schemas.length > 0) {
                    AppState.schemas = schemas;
                    renderSchemas(schemas);
                    showError('Schemas refreshed: ' + schemas.length + ' collections');
                }
                btn.classList.remove('spinning');
                btn.disabled = false;
            }).catch(function() {
                btn.classList.remove('spinning');
                btn.disabled = false;
            });
        });

        // Load available models — sort default first, select it
        fetchModels().then(function(models) {
            var select = document.getElementById('model-select');
            select.innerHTML = '';
            if (models && models.length > 0) {
                // Sort: default model first, then alphabetical
                models.sort(function(a, b) {
                    if (a.default && !b.default) return -1;
                    if (!a.default && b.default) return 1;
                    var na = (a.name || a).toString();
                    var nb = (b.name || b).toString();
                    return na.localeCompare(nb);
                });
                var hasDefault = false;
                models.forEach(function(model) {
                    var opt = document.createElement('option');
                    var name = model.name || model;
                    opt.value = name;
                    opt.textContent = name + (model.default ? ' (default)' : '');
                    if (model.default) { opt.selected = true; hasDefault = true; }
                    select.appendChild(opt);
                });
                // If no model marked default, select qwen3-coder:30b if present
                if (!hasDefault) {
                    for (var i = 0; i < select.options.length; i++) {
                        if (select.options[i].value === 'qwen3-coder:30b') {
                            select.selectedIndex = i;
                            break;
                        }
                    }
                }
            } else {
                select.innerHTML = '<option value="default">default</option>';
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

            // If approve mode is on, preview first instead of executing
            var queryFn = AppState.approveMode ? previewQuery : queryApi;

            queryFn(queryText, model).then(function(response) {
                var parsedOp = response.parsed ? response.parsed.operation : '';
                var isWriteOp = ['insert', 'update', 'delete'].indexOf(parsedOp) >= 0;

                // Write operations always go through confirmation modal with backup
                if (isWriteOp || response.requires_confirmation) {
                    showLoading(false);
                    showConfirmModal(response);
                    return;
                }
                // Read queries in approve mode show approval panel for review
                if (AppState.approveMode && !response.requires_confirmation) {
                    showLoading(false);
                    showApprovalPanel(response);
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

        // Settings modal
        document.getElementById('settings-btn').addEventListener('click', function() {
            var modal = document.getElementById('settings-modal');
            modal.hidden = false;
            fetchConfig().then(function(config) {
                if (!config) {
                    document.getElementById('settings-body').innerHTML = '<div class="settings-loading">Failed to load configuration</div>';
                    return;
                }
                renderConfig(config);
            });
        });
        document.getElementById('settings-close').addEventListener('click', function() {
            document.getElementById('settings-modal').hidden = true;
        });

        // Approve mode toggle
        document.getElementById('approve-toggle').addEventListener('change', function() {
            AppState.approveMode = this.checked;
        });

        // Approval buttons
        document.getElementById('approve-btn').addEventListener('click', executeApproved);
        document.getElementById('reject-btn').addEventListener('click', rejectQuery);

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
        AppState.pendingBackupData = response.results;
        AppState.backupDownloaded = false;

        var modal = document.getElementById('confirm-modal');
        var op = response.parsed.operation || 'write';
        var hasAffectedDocs = Array.isArray(response.results) && response.results.length > 0;
        var needsBackup = (op === 'update' || op === 'delete') && hasAffectedDocs;

        document.getElementById('confirm-title').textContent =
            'Confirm ' + op.charAt(0).toUpperCase() + op.slice(1) + ' Operation';

        var detailHtml = '<strong>Collection:</strong> ' + response.parsed.collection + '<br>';
        if (response.parsed.filters && response.parsed.filters.length > 0) {
            detailHtml += '<strong>Filter:</strong> ' + response.parsed.filters.length + ' conditions<br>';
        }
        if (response.affected_count != null) {
            detailHtml += '<strong>Affected documents:</strong> ' + response.affected_count + '<br>';
        }
        if (response.parsed.update_fields) {
            detailHtml += '<strong>Update:</strong> ' + JSON.stringify(response.parsed.update_fields) + '<br>';
        }
        if (response.parsed.document) {
            detailHtml += '<strong>Document:</strong> ' + JSON.stringify(response.parsed.document).substring(0, 200) + '<br>';
        }
        if (response.parsed.update_many) detailHtml += '<strong>Scope:</strong> ALL matching documents<br>';

        document.getElementById('confirm-operation').innerHTML = detailHtml;
        document.getElementById('confirm-query').textContent = response.mongo_query || '';

        // Backup section
        var previewEl = document.getElementById('confirm-preview');
        var execBtn = document.getElementById('confirm-execute');

        if (needsBackup) {
            previewEl.innerHTML =
                '<div class="backup-section">' +
                '<div class="backup-label">Download a backup of affected data before proceeding:</div>' +
                '<div class="backup-buttons">' +
                '<button id="backup-csv" class="modal-btn backup-btn">Download CSV Backup</button>' +
                '<button id="backup-json" class="modal-btn backup-btn">Download JSON Backup</button>' +
                '</div>' +
                '<div id="backup-status" class="backup-status"></div>' +
                '</div>';

            // Disable execute until backup downloaded
            execBtn.disabled = true;
            execBtn.title = 'Download a backup first';
            execBtn.classList.add('btn-disabled');

            // Backup button handlers
            document.getElementById('backup-csv').addEventListener('click', function() {
                downloadBackup('csv', response);
            });
            document.getElementById('backup-json').addEventListener('click', function() {
                downloadBackup('json', response);
            });

            document.getElementById('confirm-warning').textContent =
                'This will modify ' + response.affected_count + ' document(s). You must download a backup before executing.';
        } else if (op === 'insert') {
            previewEl.innerHTML = '';
            execBtn.disabled = false;
            execBtn.classList.remove('btn-disabled');
            document.getElementById('confirm-warning').textContent =
                'This will insert new data into your database.';
        } else {
            previewEl.innerHTML = '';
            execBtn.disabled = false;
            execBtn.classList.remove('btn-disabled');
        }

        execBtn.textContent = op.charAt(0).toUpperCase() + op.slice(1);
        modal.hidden = false;
    }

    function downloadBackup(format, response) {
        var data = AppState.pendingBackupData;
        if (!Array.isArray(data) || data.length === 0) {
            showError('No data to backup');
            return;
        }

        var content, mimeType, ext;
        if (format === 'csv') {
            // Build CSV from JSON docs
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

        var collection = response.parsed ? response.parsed.collection : 'backup';
        var timestamp = new Date().toISOString().replace(/[:.]/g, '-').substring(0, 19);
        var filename = collection + '_backup_' + timestamp + '.' + ext;

        var blob = new Blob([content], { type: mimeType });
        var url = URL.createObjectURL(blob);
        var a = document.createElement('a');
        a.href = url;
        a.download = filename;
        a.click();
        URL.revokeObjectURL(url);

        // Mark backup as downloaded, enable execute
        AppState.backupDownloaded = true;
        var statusEl = document.getElementById('backup-status');
        if (statusEl) {
            statusEl.textContent = 'Backup downloaded: ' + filename;
            statusEl.classList.add('backup-done');
        }
        var execBtn = document.getElementById('confirm-execute');
        execBtn.disabled = false;
        execBtn.classList.remove('btn-disabled');
        execBtn.title = '';
    }

    function hideConfirmModal() {
        document.getElementById('confirm-modal').hidden = true;
        AppState.pendingWrite = null;
    }

    // --- Query Approval Panel ---

    function showApprovalPanel(response) {
        AppState.pendingApproval = response;
        var section = document.getElementById('results-section');
        var emptyState = document.getElementById('empty-state');
        emptyState.style.display = 'none';
        section.hidden = false;

        // Show mongo query
        var mongoEl = document.getElementById('mongo-query-cmd');
        var queryText = response.mongo_query || '';
        if (response.index_analysis) {
            var ia = response.index_analysis;
            queryText += '\n\n/* Index Analysis */';
            if (ia.index_used) {
                queryText += '\n// Using index: ' + ia.matching_index;
            } else {
                queryText += '\n// WARNING: No matching index — collection scan required';
            }
            if (ia.unindexed_fields && ia.unindexed_fields.length > 0) {
                queryText += '\n// Unindexed fields: ' + ia.unindexed_fields.join(', ');
            }
            if (ia.suggestions && ia.suggestions.length > 0) {
                ia.suggestions.forEach(function(s) {
                    queryText += '\n// Suggestion: ' + s;
                });
            }
        }
        mongoEl.textContent = queryText;
        mongoEl.hidden = false;
        // Make mongo query editable in approval mode
        mongoEl.contentEditable = 'true';
        mongoEl.classList.add('editable');
        mongoEl.title = 'Edit the MongoDB query before approving';
        var mongoToggle = document.querySelector('.mongo-toggle');
        if (mongoToggle) {
            mongoToggle.classList.add('open');
            mongoToggle.innerHTML = 'MongoDB Query &#9662;';
            if (response.index_analysis && !response.index_analysis.index_used) {
                mongoToggle.classList.add('index-warning');
            } else {
                mongoToggle.classList.remove('index-warning');
            }
        }

        // Show parsed query (validated/fixed by app engine)
        var parsedEl = document.getElementById('parsed-query-json');
        parsedEl.textContent = JSON.stringify(response.parsed, null, 2);
        parsedEl.hidden = false;
        // Make parsed query editable in approval mode
        parsedEl.contentEditable = 'true';
        parsedEl.classList.add('editable');
        parsedEl.title = 'Edit the parsed query JSON before approving';
        var parsedToggle = document.querySelector('#query-inspector .collapsible-toggle');
        if (parsedToggle) {
            parsedToggle.classList.add('open');
            parsedToggle.innerHTML = 'Parsed JSON &#9662;';
        }

        // Show raw LLM response (before parsing/validation)
        var rawEl = document.getElementById('raw-llm-response');
        var rawToggle = document.querySelector('.raw-llm-toggle');
        if (response.raw_llm_response) {
            try {
                var rawJson = JSON.parse(response.raw_llm_response);
                rawEl.textContent = JSON.stringify(rawJson, null, 2);
            } catch (e) {
                rawEl.textContent = response.raw_llm_response;
            }
            rawEl.hidden = false;
            if (rawToggle) {
                rawToggle.classList.add('open');
                rawToggle.innerHTML = 'Raw LLM Response &#9662;';
            }
        } else {
            rawEl.hidden = true;
            if (rawToggle) {
                rawToggle.classList.remove('open');
                rawToggle.innerHTML = 'Raw LLM Response &#9656;';
            }
        }

        // Clear table area
        var thead = document.getElementById('results-thead');
        var tbody = document.getElementById('results-tbody');
        thead.innerHTML = '';
        tbody.innerHTML = '';
        document.getElementById('pagination').innerHTML = '';

        // Stats
        document.getElementById('result-count').textContent = 'Pending approval';
        document.getElementById('result-time').textContent = response.execution_ms + 'ms (parse only)';
        document.getElementById('result-collection').textContent = response.parsed ? response.parsed.collection : '';

        // Show approval buttons
        showApprovalButtons(true);
    }

    function showApprovalButtons(show) {
        var container = document.getElementById('approval-bar');
        if (!container) return;
        container.hidden = !show;
    }

    function executeApproved() {
        var response = AppState.pendingApproval;
        if (!response) return;

        // Read possibly-edited parsed query from the UI
        var parsedEl = document.getElementById('parsed-query-json');
        var parsedJson;
        try {
            parsedJson = JSON.parse(parsedEl.textContent);
        } catch (e) {
            showError('Invalid JSON in parsed query: ' + e.message);
            return;
        }

        AppState.pendingApproval = null;
        showApprovalButtons(false);

        // Reset editable state
        parsedEl.contentEditable = 'false';
        parsedEl.classList.remove('editable');
        var mongoEl = document.getElementById('mongo-query-cmd');
        mongoEl.contentEditable = 'false';
        mongoEl.classList.remove('editable');

        showLoading(true);

        var isWrite = ['insert', 'update', 'delete'].indexOf(parsedJson.operation) >= 0;

        if (isWrite) {
            // Route writes through the confirmation modal so backup is required
            showLoading(false);
            var writeResponse = {
                parsed: parsedJson,
                results: response.results || [],
                mongo_query: document.getElementById('mongo-query-cmd').textContent || '',
                affected_count: response.affected_count || null,
            };
            showConfirmModal(writeResponse);
            return;
        } else {
            // Execute the (possibly edited) parsed query directly via /api/execute
            executeWrite(parsedJson).then(function(result) {
                showLoading(false);
                renderResults(result);
                addToHistory(
                    AppState.currentQuery,
                    result.parsed ? result.parsed.collection : '',
                    result.count || 0,
                    result.execution_ms
                );
                renderHistory(AppState.queryHistory);
            }).catch(function() {
                showLoading(false);
            });
        }
    }

    function rejectQuery() {
        AppState.pendingApproval = null;
        showApprovalButtons(false);
        document.getElementById('results-section').hidden = true;
        document.getElementById('empty-state').style.display = '';
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
