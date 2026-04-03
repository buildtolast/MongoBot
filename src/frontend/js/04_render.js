/* =============================================
   Rendering Functions
   ============================================= */

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
        mongoEl.textContent = response.mongo_query;
        mongoEl.hidden = false;
        var mongoToggle = document.querySelector('.mongo-toggle');
        if (mongoToggle) {
            mongoToggle.classList.add('open');
            mongoToggle.innerHTML = 'MongoDB Query &#9662;';
        }
    }

    // Update parsed query inspector
    var parsedEl = document.getElementById('parsed-query-json');
    if (response.parsed) {
        parsedEl.textContent = JSON.stringify(response.parsed, null, 2);
    } else {
        parsedEl.textContent = '(no parsed query available)';
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
        thead.innerHTML = '<tr><th>Operation</th><th>Collection</th><th>Result</th></tr>';
        tbody.innerHTML = '<tr><td>' + operation + '</td><td>' + collection +
            '</td><td style="font-weight:600;color:var(--accent)">' + response.results.count + '</td></tr>';
        document.getElementById('pagination').innerHTML = '';
    } else if (docs && docs.length > 0) {
        renderTable(docs);
    } else {
        // Empty result set
        var tbody2 = document.getElementById('results-tbody');
        var thead2 = document.getElementById('results-thead');
        thead2.innerHTML = '';
        tbody2.innerHTML = '<tr><td style="padding:20px;color:var(--text-secondary)">No documents found matching your query</td></tr>';
        document.getElementById('pagination').innerHTML = '';
    }
}

function renderTable(documents) {
    var thead = document.getElementById('results-thead');
    var tbody = document.getElementById('results-tbody');

    if (!documents || documents.length === 0) {
        thead.innerHTML = '';
        tbody.innerHTML = '<tr><td style="padding:20px;color:var(--text-muted)">No documents</td></tr>';
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
            renderTable(AppState.lastResults.documents);
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

function flattenDocument(doc) {
    var flat = {};
    function recurse(obj, prefix) {
        if (obj === null || obj === undefined || typeof obj !== 'object' || Array.isArray(obj)) {
            flat[prefix] = obj;
            return;
        }
        // Handle special MongoDB types
        if (obj.$oid) { flat[prefix] = obj.$oid; return; }
        if (obj.$date) { flat[prefix] = obj.$date; return; }
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

function renderSchemas(schemas) {
    var container = document.getElementById('schema-list');
    container.innerHTML = '';

    if (!schemas || schemas.length === 0) {
        container.innerHTML = '<div class="history-empty">No collections found</div>';
        return;
    }

    schemas.forEach(function(schema) {
        var card = document.createElement('div');
        card.className = 'collection-card';

        var header = document.createElement('div');
        header.className = 'collection-card-header';

        var nameSpan = document.createElement('span');
        nameSpan.className = 'collection-name';
        nameSpan.textContent = schema.name || schema.collection;

        var countSpan = document.createElement('span');
        countSpan.className = 'collection-count';
        if (schema.count != null) {
            countSpan.textContent = schema.count + ' docs';
        }

        var expandSpan = document.createElement('span');
        expandSpan.className = 'collection-expand';
        expandSpan.textContent = '\u25B6';

        header.appendChild(nameSpan);
        header.appendChild(countSpan);
        header.appendChild(expandSpan);

        header.addEventListener('click', function() {
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
        renderTable(AppState.lastResults.documents);
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
        renderTable(AppState.lastResults.documents);
    });
    container.appendChild(nextBtn);
}

function createPageBtn(num, currentPage) {
    var btn = document.createElement('button');
    btn.textContent = num;
    if (num === currentPage) btn.className = 'active';
    btn.addEventListener('click', function() {
        AppState.currentPage = num;
        renderTable(AppState.lastResults.documents);
    });
    return btn;
}

function showError(message) {
    var toast = document.getElementById('error-toast');
    toast.textContent = message;
    toast.hidden = false;
    toast.className = 'toast';
    clearTimeout(toast._timer);
    toast._timer = setTimeout(function() {
        toast.hidden = true;
    }, 5000);
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
