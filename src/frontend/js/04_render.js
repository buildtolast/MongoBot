/* =============================================
   Rendering Functions
   ============================================= */

function renderConfig(config) {
    var body = document.getElementById('settings-body');
    var html = '';

    function section(title, items) {
        html += '<div class="settings-section"><h4>' + title + '</h4><div class="settings-grid">';
        items.forEach(function(item) {
            var valClass = 'settings-value';
            var val = item[1];
            if (val === '***set***') { valClass += ' secret'; val = '***set***'; }
            else if (val === null || val === undefined) { valClass += ' not-set'; val = 'not set'; }
            else { val = String(val); }
            html += '<div class="settings-item"><span class="settings-key">' + item[0] + '</span><span class="' + valClass + '">' + val + '</span></div>';
        });
        html += '</div></div>';
    }

    var llm = config.llm || {};
    section('LLM Provider', [
        ['Provider', llm.provider],
        ['Ollama URL', llm.ollama_url],
        ['Ollama Model', llm.ollama_model],
        ['LM Studio URL', llm.lmstudio_url],
        ['LM Studio Model', llm.lmstudio_model],
        ['llama.cpp URL', llm.llamacpp_url],
        ['llama.cpp Model', llm.llamacpp_model],
        ['Anthropic URL', llm.anthropic_api_url],
        ['OpenAI URL', llm.openai_api_url],
        ['Cloud Model', llm.cloud_llm_model],
        ['Cloud API Key', llm.cloud_llm_api_key],
    ]);

    var db = config.mongodb || {};
    section('MongoDB', [
        ['URI', db.uri],
        ['Database', db.database],
    ]);

    var srv = config.server || {};
    section('Server', [
        ['Host', srv.host],
        ['Port', srv.port],
        ['Dashboard Auth', srv.dashboard_auth ? 'Enabled' : 'Disabled'],
    ]);

    var safety = config.safety || {};
    section('Safety', [
        ['Max Result Rows', safety.max_result_rows],
        ['Allowed Operations', (safety.allowed_operations || []).join(', ')],
    ]);

    var slack = config.slack || {};
    section('Slack', [
        ['Bot Token', slack.bot_token],
        ['Signing Secret', slack.signing_secret],
    ]);

    var fb = config.feedback || {};
    section('Feedback Loop', [
        ['Few-shot Retrieval', fb.enable_retrieval ? 'Enabled' : 'Disabled'],
        ['Top-K Examples', fb.top_k],
        ['Embedding Model', fb.embedding_model],
        ['Embedding URL', fb.embedding_url],
    ]);

    body.innerHTML = html;
}

function renderResults(response) {
    // API returns: { parsed: {...}, results: [...] or {count: N}, count: N, execution_ms: N }
    var section = document.getElementById('results-section');
    var emptyState = document.getElementById('empty-state');

    // Normalize: results can be an array of docs, or an object like {count: N}
    var docs = Array.isArray(response.results) ? response.results : null;
    var isCountResult = !docs && response.results && response.results.count !== undefined;
    var collection = response.parsed ? response.parsed.collection : '';
    var operation = response.parsed ? response.parsed.operation : '';

    // Show results section
    emptyState.style.display = 'none';
    section.hidden = false;

    // Always show feedback after every query response
    var feedbackRow = document.getElementById('feedback-row');
    var noteContainer = document.getElementById('feedback-note-container');
    if (feedbackRow) {
        feedbackRow.hidden = false;
        if (noteContainer) noteContainer.hidden = true;
    }

    // Update stats
    var countEl = document.getElementById('result-count');
    var timeEl = document.getElementById('result-time');
    var collEl = document.getElementById('result-collection');

    if (isCountResult) {
        countEl.textContent = 'Count: ' + response.results.count;
    } else if (docs) {
        countEl.textContent = docs.length + ' docs';
    } else {
        countEl.textContent = response.count + ' results';
    }
    timeEl.textContent = response.execution_ms != null ? response.execution_ms + 'ms' : '';
    collEl.textContent = collection;

    // Update mongo query panel
    var mongoEl = document.getElementById('mongo-query-cmd');
    if (response.mongo_query) {
        var queryText = response.mongo_query;

        // Append index analysis
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

        // Append query advice
        if (response.advice && response.advice.length > 0) {
            queryText += '\n\n/* Query Advice */';
            response.advice.forEach(function(a) {
                var icon = a.severity === 'error' ? 'ERROR' : a.severity === 'warning' ? 'WARN' : 'INFO';
                queryText += '\n// [' + icon + '] ' + a.message;
            });
        }

        mongoEl.textContent = queryText;
        mongoEl.hidden = false;
        var mongoToggle = document.querySelector('.mongo-toggle');
        if (mongoToggle) {
            mongoToggle.classList.add('open');
            mongoToggle.innerHTML = 'MongoDB Query &#9656;';
        }

        // Color the toggle based on index usage
        if (response.index_analysis && !response.index_analysis.index_used) {
            mongoToggle.classList.add('index-warning');
        } else {
            mongoToggle.classList.remove('index-warning');
        }
    }

    // Update parsed query inspector
    var parsedEl = document.getElementById('parsed-query-json');
    if (response.parsed) {
        parsedEl.textContent = JSON.stringify(response.parsed, null, 2);
    } else {
        parsedEl.textContent = '(no parsed query available)';
    }

    // Update raw LLM response panel
    var rawEl = document.getElementById('raw-llm-response');
    var rawToggle = document.querySelector('.raw-llm-toggle');
    if (response.raw_llm_response) {
        try {
            var rawJson = JSON.parse(response.raw_llm_response);
            rawEl.textContent = JSON.stringify(rawJson, null, 2);
        } catch (e) {
            rawEl.textContent = response.raw_llm_response;
        }
    } else {
        rawEl.textContent = '';
    }

    // Store response
    AppState.lastResults = response;
    AppState.currentPage = 1;
    AppState.sortColumn = null;
    AppState.sortDirection = 'asc';

    if (isCountResult) {
        // For count queries, show a simple result card instead of a table
        var tbody = document.getElementById('results-tbody');
        var thead = document.getElementById('results-thead');
        thead.innerHTML = '<tr class="no-sort"><th class="no-sort">Operation</th><th class="no-sort">Collection</th><th class="no-sort">Result</th></tr>';
        tbody.innerHTML = '<tr><td colspan="3" style="text-align:center;padding:20px;"><div style="font-size:1.2rem;font-weight:600;">' + response.results.count + ' documents found</div></td></tr>';
        document.getElementById('pagination').innerHTML = '';
    } else if (docs && docs.length > 0) {
        renderTable(docs);
    } else {
        // Empty result set
        var tbody2 = document.getElementById('results-tbody');
        var thead2 = document.getElementById('results-thead');
        thead2.innerHTML = '';
        tbody2.innerHTML = '<tr><td style="padding:20px;color:var(--text-muted);text-align:center;">No documents found matching your query</td></tr>';
        document.getElementById('pagination').innerHTML = '';
    }
}

function renderTable(documents) {
    var thead = document.getElementById('results-thead');
    var tbody = document.getElementById('results-tbody');

    // Store for re-use by sort/pagination handlers
    if (documents && documents.length > 0) {
        AppState.lastDocs = documents;
    }

    if (!documents || documents.length === 0) {
        thead.innerHTML = '';
        tbody.innerHTML = '<tr><td style="padding:20px;color:var(--text-muted);text-align:center;">No documents</td></tr>';
        document.getElementById('pagination').innerHTML = '';
        return;
    }

    // Flatten documents and collect all keys
    var flatDocs = documents.map(flattenDocument);
    var keySet = {};
    flatDocs.forEach(function(doc) {
        Object.keys(doc).forEach(function(k) { keySet[k] = true; });
    });
    var columns = Object.keys(keySet);

    // Move _id to front if present
    var idIdx = columns.indexOf('_id');
    if (idIdx > 0) {
        columns.splice(idIdx, 1);
        columns.unshift('_id');
    }

    // Sort if needed
    if (AppState.sortColumn && columns.indexOf(AppState.sortColumn) !== -1) {
        var col = AppState.sortColumn;
        var dir = AppState.sortDirection === 'asc' ? 1 : -1;
        flatDocs.sort(function(a, b) {
            var va = a[col], vb = b[col];
            if (va == null) return 1;
            if (vb == null) return -1;
            if (typeof va === 'number' && typeof vb === 'number') return (va - vb) * dir;
            return String(va).localeCompare(String(vb)) * dir;
        });
    }

    // Paginate
    var total = flatDocs.length;
    var start = (AppState.currentPage - 1) * AppState.pageSize;
    var end = Math.min(start + AppState.pageSize, total);
    var pageDocs = flatDocs.slice(start, end);

    // Render header
    thead.innerHTML = '';
    var headerRow = document.createElement('tr');
    columns.forEach(function(col) {
        var th = document.createElement('th');
        th.textContent = col;
        if (AppState.sortColumn === col) {
            th.className = AppState.sortDirection === 'asc' ? 'sorted-asc' : 'sorted-desc';
        }
        th.addEventListener('click', function() {
            if (AppState.sortColumn === col) {
                AppState.sortDirection = AppState.sortDirection === 'asc' ? 'desc' : 'asc';
            } else {
                AppState.sortColumn = col;
                AppState.sortDirection = 'asc';
            }
            renderTable(AppState.lastDocs);
        });
        headerRow.appendChild(th);
    });
    thead.appendChild(headerRow);

    // Render body
    tbody.innerHTML = '';
    pageDocs.forEach(function(doc) {
        var tr = document.createElement('tr');
        columns.forEach(function(col) {
            var td = document.createElement('td');
            var val = doc[col];
            if (val === null || val === undefined) {
                td.textContent = 'null';
                td.className = 'cell-null';
            } else if (col === '_id') {
                td.textContent = String(val);
                td.className = 'cell-id';
            } else if (typeof val === 'boolean') {
                td.textContent = String(val);
                td.className = val ? 'cell-bool-true' : 'cell-bool-false';
            } else if (typeof val === 'number') {
                td.textContent = String(val);
                td.className = 'cell-number';
            } else if (typeof val === 'object') {
                td.textContent = JSON.stringify(val);
                td.title = JSON.stringify(val, null, 2);
            } else {
                td.textContent = String(val);
                td.title = String(val);
            }
            tr.appendChild(td);
        });
        tbody.appendChild(tr);
    });

    renderPagination(total, AppState.currentPage, AppState.pageSize);
}

function formatBsonDate(dateVal) {
    // Handle {"$numberLong": "946684800000"} or raw milliseconds number or ISO string
    var ms;
    if (typeof dateVal === 'object' && dateVal !== null && dateVal.$numberLong) {
        ms = parseInt(dateVal.$numberLong, 10);
    } else if (typeof dateVal === 'number') {
        ms = dateVal;
    } else if (typeof dateVal === 'string') {
        // Could be ISO string or milliseconds string
        var parsed = Date.parse(dateVal);
        if (!isNaN(parsed)) return new Date(parsed).toISOString().replace('T', ' ').replace('.000Z', ' UTC');
        ms = parseInt(dateVal, 10);
    } else {
        return String(dateVal);
    }
    if (isNaN(ms)) return String(dateVal);
    return new Date(ms).toISOString().replace('T', ' ').replace('.000Z', ' UTC');
}

function flattenDocument(doc) {
    var flat = {};
    function recurse(obj, prefix) {
        if (obj === null || obj === undefined || typeof obj !== 'object' || Array.isArray(obj)) {
            flat[prefix] = obj;
            return;
        }
        // Handle special MongoDB types
        if (obj.$oid) { flat[prefix] = obj.$oid; return; }
        if (obj.$date) { flat[prefix] = formatBsonDate(obj.$date); return; }
        if (obj.$numberLong) { flat[prefix] = Number(obj.$numberLong); return; }
        if (obj.$numberInt) { flat[prefix] = Number(obj.$numberInt); return; }
        if (obj.$numberDouble) { flat[prefix] = Number(obj.$numberDouble); return; }

        var keys = Object.keys(obj);
        if (keys.length === 0) {
            flat[prefix] = obj;
            return;
        }
        keys.forEach(function(key) {
            var path = prefix ? prefix + '.' + key : key;
            var val = obj[key];
            if (val && typeof val === 'object' && !Array.isArray(val)) {
                // Only flatten one level deep for readability
                if (prefix) {
                    flat[path] = val;
                } else {
                    recurse(val, path);
                }
            } else {
                flat[path] = val;
            }
        });
    }
    recurse(doc, '');
    return flat;
}

function renderWriteResult(response) {
    var section = document.getElementById('results-section');
    var emptyState = document.getElementById('empty-state');
    emptyState.style.display = 'none';
    section.hidden = false;

    // Update stats
    document.getElementById('result-count').textContent =
        (response.affected_count || 0) + ' affected';
    document.getElementById('result-time').textContent =
        response.execution_ms ? response.execution_ms + 'ms' : '';
    document.getElementById('result-collection').textContent =
        response.parsed ? response.parsed.collection : '';

    // Update mongo query panel
    var mongoEl = document.getElementById('mongo-query-cmd');
    if (response.mongo_query) {
        mongoEl.textContent = response.mongo_query;
        mongoEl.hidden = false;
    }

    // Show write result in table
    var thead = document.getElementById('results-thead');
    var tbody = document.getElementById('results-tbody');
    thead.innerHTML = '<tr class="no-sort"><th class="no-sort">Operation</th><th class="no-sort">Collection</th><th class="no-sort">Matched</th><th class="no-sort">Modified</th><th class="no-sort">Status</th></tr>';

    var wr = response.results || {};
    tbody.innerHTML = '<tr><td colspan="5" style="text-align:center;padding:20px;"><div style="font-size:1.2rem;font-weight:600;">Write Operation Successful</div></td></tr>';
}

function ensureIndexTooltip() {
    var tip = document.getElementById('schema-idx-tooltip');
    if (!tip) {
        tip = document.createElement('div');
        tip.id = 'schema-idx-tooltip';
        document.body.appendChild(tip);
    }
    return tip;
}

function showIndexTooltip(badge, html) {
    var tip = ensureIndexTooltip();
    tip.innerHTML = html;
    tip.classList.add('visible');

    var rect = badge.getBoundingClientRect();
    var tipW = 300;
    var left = rect.right + 6;
    if (left + tipW > window.innerWidth) {
        left = rect.left - tipW - 6;
    }
    tip.style.left = Math.max(4, left) + 'px';
    tip.style.top = Math.max(4, rect.top) + 'px';
}

function hideIndexTooltip() {
    var tip = document.getElementById('schema-idx-tooltip');
    if (tip) tip.classList.remove('visible');
}

function buildGreenTooltip(indexes) {
    var nonDefault = indexes.filter(function(idx) { return idx.name !== '_id_'; });
    var html = '<div class="tip-title green">Existing indexes (' + indexes.length + ')</div>';
    indexes.forEach(function(idx) {
        html += '<div class="tip-index">';
        html += '<span class="tip-name">' + escHtml(idx.name) + '</span>';
        if (idx.keys && idx.keys.length) {
            var keyStr = idx.keys.map(function(k) {
                return k.field + ': ' + (k.direction === 1 ? 'asc' : k.direction === -1 ? 'desc' : k.direction);
            }).join(', ');
            html += '\n<span class="tip-keys">{ ' + escHtml(keyStr) + ' }</span>';
        }
        if (idx.unique) {
            html += '\n<span class="tip-unique">UNIQUE</span>';
        }
        html += '</div>';
    });
    return html;
}

function buildRedTooltip(missingIssues) {
    var html = '<div class="tip-title red">Missing indexes (' + missingIssues.length + ')</div>';
    missingIssues.forEach(function(issue) {
        html += '<div class="tip-index">';
        html += '<span class="tip-name">' + escHtml(issue.field) + '</span>';
        if (issue.fix_command) {
            html += '\n<span class="tip-cmd">' + escHtml(issue.fix_command) + '</span>';
        }
        html += '</div>';
    });
    return html;
}

function escHtml(str) {
    return String(str)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;');
}

function renderSchemas(schemas) {
    var container = document.getElementById('schema-list');
    container.innerHTML = '';

    if (!schemas || schemas.length === 0) {
        container.innerHTML = '<div class="history-empty">No collections found</div>';
        return;
    }

    // Build per-collection missing index map from AppState.issues
    var missingByCollection = {};
    (AppState.issues || []).forEach(function(issue) {
        if (!missingByCollection[issue.collection]) {
            missingByCollection[issue.collection] = [];
        }
        missingByCollection[issue.collection].push(issue);
    });

    schemas.forEach(function(schema) {
        var collName = schema.name || schema.collection;
        var card = document.createElement('div');
        card.className = 'collection-card';

        var header = document.createElement('div');
        header.className = 'collection-card-header';

        var nameSpan = document.createElement('span');
        nameSpan.className = 'collection-name';
        nameSpan.textContent = collName;

        // Index badges
        var badgeGroup = document.createElement('div');
        badgeGroup.className = 'idx-badges';

        var existingIndexes = schema.indexes || [];
        var missingIssues = missingByCollection[collName] || [];

        // Green badge \u2014 existing indexes
        if (existingIndexes.length > 0) {
            var greenBadge = document.createElement('span');
            greenBadge.className = 'idx-badge idx-badge-green';
            greenBadge.textContent = existingIndexes.length;
            var greenHtml = buildGreenTooltip(existingIndexes);
            greenBadge.addEventListener('mouseenter', function() {
                showIndexTooltip(greenBadge, greenHtml);
            });
            greenBadge.addEventListener('mouseleave', hideIndexTooltip);
            badgeGroup.appendChild(greenBadge);
        }

        // Red badge \u2014 missing indexes
        if (missingIssues.length > 0) {
            var redBadge = document.createElement('span');
            redBadge.className = 'idx-badge idx-badge-red';
            redBadge.textContent = missingIssues.length;
            var redHtml = buildRedTooltip(missingIssues);
            redBadge.addEventListener('mouseenter', function() {
                showIndexTooltip(redBadge, redHtml);
            });
            redBadge.addEventListener('mouseleave', hideIndexTooltip);
            badgeGroup.appendChild(redBadge);
        }

        var countSpan = document.createElement('span');
        countSpan.className = 'collection-count';
        if (schema.description) {
            countSpan.textContent = schema.description;
        } else if (schema.count != null) {
            countSpan.textContent = schema.count + ' docs';
        }

        var expandSpan = document.createElement('span');
        expandSpan.className = 'collection-expand';
        expandSpan.textContent = '\u25B6';

        header.appendChild(nameSpan);
        header.appendChild(badgeGroup);
        header.appendChild(countSpan);
        header.appendChild(expandSpan);

        // Expand click \u2014 but not on the badge area
        header.addEventListener('click', function(e) {
            if (e.target.classList.contains('idx-badge')) return;
            card.classList.toggle('expanded');
        });

        var fields = document.createElement('div');
        fields.className = 'collection-fields';

        if (schema.fields && schema.fields.length > 0) {
            schema.fields.forEach(function(field) {
                var item = document.createElement('div');
                item.className = 'field-item';

                var fname = document.createElement('span');
                fname.className = 'field-name';
                fname.textContent = field.name || field;

                var ftype = document.createElement('span');
                var typeName = (field.field_type || field.type || field.bson_type || 'unknown').toLowerCase();
                ftype.className = 'field-type field-type-' + typeName;
                ftype.textContent = typeName;

                item.appendChild(fname);
                item.appendChild(ftype);
                fields.appendChild(item);
            });
        }

        card.appendChild(header);
        card.appendChild(fields);
        container.appendChild(card);
    });
}

function renderIssues(issues) {
    var list = document.getElementById('issues-list');
    var badge = document.getElementById('issues-count');
    if (!list) return;

    if (!issues || issues.length === 0) {
        list.innerHTML = '<div class="issues-empty">No issues detected</div>';
        if (badge) badge.hidden = true;
        return;
    }

    // Group by collection
    var byCollection = {};
    issues.forEach(function(issue) {
        if (!byCollection[issue.collection]) {
            byCollection[issue.collection] = [];
        }
        byCollection[issue.collection].push(issue);
    });

    // Update badge
    if (badge) {
        badge.textContent = issues.length;
        badge.hidden = false;
    }

    list.innerHTML = '';
    Object.keys(byCollection).sort().forEach(function(collection) {
        var fields = byCollection[collection];
        var card = document.createElement('div');
        card.className = 'issue-card';

        var header = document.createElement('div');
        header.className = 'issue-card-header';
        header.innerHTML =
            '<span class="issue-collection">' + collection + '</span>' +
            '<span class="issue-field-count">' + fields.length + ' field' + (fields.length !== 1 ? 's' : '') + '</span>' +
            '<span class="issue-expand">&#9656;</span>';
        header.addEventListener('click', function() {
            card.classList.toggle('expanded');
        });

        var fieldList = document.createElement('div');
        fieldList.className = 'issue-fields';

        fields.forEach(function(issue) {
            var row = document.createElement('div');
            row.className = 'issue-row';
            row.dataset.collection = issue.collection;
            row.dataset.field = issue.field;

            var nameEl = document.createElement('span');
            nameEl.className = 'issue-field-name';
            nameEl.textContent = issue.field;
            nameEl.title = issue.message || '';

            var btn = document.createElement('button');
            btn.className = 'issue-fix-btn';
            btn.textContent = 'Create Index';
            btn.addEventListener('click', function() {
                btn.disabled = true;
                btn.classList.add('fixing');
                btn.textContent = 'Creating...';
                fixIndex(issue.collection, issue.field).then(function() {
                    btn.classList.remove('fixing');
                    btn.classList.add('fixed');
                    btn.textContent = 'Created';
                    // Remove from local issues and re-render after a brief pause
                    setTimeout(function() {
                        fetchIssues().then(function(updated) {
                            AppState.issues = updated || [];
                            renderIssues(updated);
                            if (AppState.schemas && AppState.schemas.length > 0) {
                                renderSchemas(AppState.schemas);
                            }
                        });
                    }, 800);
                }).catch(function(err) {
                    btn.disabled = false;
                    btn.classList.remove('fixing');
                    btn.textContent = 'Create Index';
                    showToast('Failed to create index: ' + err.message, 'error');
                });
            });

            row.appendChild(nameEl);
            row.appendChild(btn);
            fieldList.appendChild(row);
        });

        card.appendChild(header);
        card.appendChild(fieldList);
        list.appendChild(card);
    });
}

function renderHistory(history) {
    var container = document.getElementById('history-list');
    container.innerHTML = '';

    if (!history || history.length === 0) {
        container.innerHTML = '<div class="history-empty">No queries yet</div>';
        return;
    }

    history.slice(0, 20).forEach(function(entry) {
        var item = document.createElement('div');
        item.className = 'history-item';

        var text = document.createTextNode(entry.query);
        item.appendChild(text);

        var time = document.createElement('span');
        time.className = 'history-item-time';
        time.textContent = formatTimeAgo(entry.timestamp);
        if (entry.resultCount != null) {
            time.textContent += ' \u00B7 ' + entry.resultCount + ' docs';
        }
        item.appendChild(time);

        item.addEventListener('click', function() {
            document.getElementById('query-input').value = entry.query;
            document.getElementById('query-form').dispatchEvent(new Event('submit'));
        });

        container.appendChild(item);
    });
}

function renderPagination(total, page, pageSize) {
    var container = document.getElementById('pagination');
    container.innerHTML = '';

    var totalPages = Math.ceil(total / pageSize);
    if (totalPages <= 1) return;

    // Prev button
    var prevBtn = document.createElement('button');
    prevBtn.textContent = '\u2190';
    prevBtn.disabled = page <= 1;
    prevBtn.addEventListener('click', function() {
        AppState.currentPage = Math.max(1, page - 1);
        renderTable(AppState.lastDocs);
    });
    container.appendChild(prevBtn);

    // Page numbers
    var startPage = Math.max(1, page - 2);
    var endPage = Math.min(totalPages, page + 2);

    if (startPage > 1) {
        container.appendChild(createPageBtn(1, page));
        if (startPage > 2) {
            var dots = document.createElement('span');
            dots.textContent = '\u2026';
            dots.style.color = 'var(--text-muted)';
            dots.style.padding = '0 4px';
            container.appendChild(dots);
        }
    }

    for (var i = startPage; i <= endPage; i++) {
        container.appendChild(createPageBtn(i, page));
    }

    if (endPage < totalPages) {
        if (endPage < totalPages - 1) {
            var dots2 = document.createElement('span');
            dots2.textContent = '\u2026';
            dots2.style.color = 'var(--text-muted)';
            dots2.style.padding = '0 4px';
            container.appendChild(dots2);
        }
        container.appendChild(createPageBtn(totalPages, page));
    }

    // Next button
    var nextBtn = document.createElement('button');
    nextBtn.textContent = '\u2192';
    nextBtn.disabled = page >= totalPages;
    nextBtn.addEventListener('click', function() {
        AppState.currentPage = Math.min(totalPages, page + 1);
        renderTable(AppState.lastDocs);
    });
    container.appendChild(nextBtn);
}

function createPageBtn(num, currentPage) {
    var btn = document.createElement('button');
    btn.textContent = num;
    if (num === currentPage) btn.className = 'active';
    btn.addEventListener('click', function() {
        AppState.currentPage = num;
        renderTable(AppState.lastDocs);
    });
    return btn;
}

function showToast(msg, type) {
    var container = document.getElementById('toast-container');
    if (!container) {
        container = document.createElement('div');
        container.id = 'toast-container';
        document.body.appendChild(container);
    }

    var toast = document.createElement('div');
    toast.className = 'toast ' + type;
    toast.textContent = msg;

    container.appendChild(toast);

    setTimeout(function() {
        toast.classList.add('fade-out');
        setTimeout(function() {
            toast.remove();
        }, 300);
    }, 3000);
}

function showLoading(isLoading) {
    AppState.loading = isLoading;
    var submitBtn = document.getElementById('query-submit');
    var btnText = submitBtn.querySelector('.btn-text');
    var btnSpinner = submitBtn.querySelector('.btn-spinner');

    if (isLoading) {
        btnText.textContent = 'Abort';
        btnText.hidden = false;
        btnSpinner.hidden = false;
        submitBtn.disabled = false;
        submitBtn.classList.add('abort-mode');
    } else {
        btnText.textContent = 'Query';
        btnText.hidden = false;
        btnSpinner.hidden = true;
        submitBtn.disabled = false;
        submitBtn.classList.remove('abort-mode');
    }
}

function updateStatusBadge(id, connected) {
    var badge = document.getElementById(id);
    if (!badge) return;
    badge.className = 'status-badge ' + (connected ? 'connected' : 'disconnected');
}

function formatTimeAgo(timestamp) {
    var now = Date.now();
    var diff = now - timestamp;
    if (diff < 60000) return 'just now';
    if (diff < 3600000) return Math.floor(diff / 60000) + 'm ago';
    if (diff < 86400000) return Math.floor(diff / 3600000) + 'h ago';
    return Math.floor(diff / 86400000) + 'd ago';
}
