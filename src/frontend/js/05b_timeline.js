/* =============================================
   Execution Timeline — real-time step tracker
   ============================================= */

var TIMELINE_STEPS = [
    { id: 'submit',   label: 'Query submitted' },
    { id: 'llm',      label: 'LLM parsing (NL to JSON)' },
    { id: 'safety',   label: 'Safety validation' },
    { id: 'mongo',    label: 'MongoDB execution' },
    { id: 'response', label: 'Building response' }
];

var timelineState = {
    startTime: null,
    intervalId: null,
    currentStep: -1
};

/**
 * Show the timeline and start the live elapsed counter.
 * Called when a query is submitted.
 */
function showTimeline(modelName) {
    var el = document.getElementById('exec-timeline');
    el.classList.remove('hidden');
    timelineState.startTime = Date.now();
    timelineState.currentStep = 0;

    var html = '<div class="timeline-header">' +
        '<span class="timeline-title">Execution Pipeline' +
        (modelName ? ' <span class="model-badge">' + escapeHtml(modelName) + '</span>' : '') +
        '</span>' +
        '<button class="timeline-close" onclick="hideTimeline()" title="Close">&times;</button>' +
        '</div>' +
        '<div class="timeline-steps">';

    for (var i = 0; i < TIMELINE_STEPS.length; i++) {
        var step = TIMELINE_STEPS[i];
        var state = i === 0 ? 'active' : 'pending';
        html += '<div class="timeline-step ' + state + '" id="tl-step-' + step.id + '">' +
            '<div class="step-dot"></div>' +
            '<span class="step-label">' + step.label + '</span>' +
            '<span class="step-time" id="tl-time-' + step.id + '"></span>' +
            '</div>';
    }

    html += '</div>' +
        '<div class="timeline-total">' +
        '<span class="timeline-total-label">Total elapsed</span>' +
        '<span class="timeline-total-time" id="tl-total-time"><span class="elapsed-counter">0.0s</span></span>' +
        '</div>';

    el.innerHTML = html;

    // Mark submit as done immediately
    markStepDone('submit', 0);
    // Advance to LLM step
    activateStep('llm');

    // Start live elapsed counter
    if (timelineState.intervalId) clearInterval(timelineState.intervalId);
    timelineState.intervalId = setInterval(updateElapsed, 100);
}

/**
 * Update the timeline with actual server timing data.
 * Called when the response arrives.
 */
function updateTimelineWithTiming(timing) {
    if (!timing) {
        // No timing data — just mark everything done with total
        completeTimeline(null);
        return;
    }

    // Stop the live counter
    if (timelineState.intervalId) {
        clearInterval(timelineState.intervalId);
        timelineState.intervalId = null;
    }

    // Mark each step with actual server-side timing
    markStepDone('submit', 0);
    markStepDone('llm', timing.llm_parse_ms);
    markStepDone('safety', timing.safety_check_ms);
    markStepDone('mongo', timing.mongo_exec_ms);
    markStepDone('response', timing.response_build_ms);

    // Update model badge if we now know which model was used
    if (timing.model_used) {
        var badge = document.querySelector('.timeline-title .model-badge');
        if (badge) {
            badge.textContent = timing.model_used;
        }
    }

    // Set final total
    var totalEl = document.getElementById('tl-total-time');
    if (totalEl) {
        totalEl.innerHTML = formatMs(timing.total_ms);
    }
}

/**
 * Mark the timeline as errored (e.g., timeout or LLM failure).
 */
function errorTimeline(errorMsg) {
    if (timelineState.intervalId) {
        clearInterval(timelineState.intervalId);
        timelineState.intervalId = null;
    }

    // Find the currently active step and mark it as error
    for (var i = 0; i < TIMELINE_STEPS.length; i++) {
        var stepEl = document.getElementById('tl-step-' + TIMELINE_STEPS[i].id);
        if (stepEl && stepEl.classList.contains('active')) {
            stepEl.classList.remove('active');
            stepEl.classList.add('error');
            var timeEl = document.getElementById('tl-time-' + TIMELINE_STEPS[i].id);
            if (timeEl) {
                timeEl.textContent = 'FAILED';
            }
            var labelEl = stepEl.querySelector('.step-label');
            if (labelEl && errorMsg) {
                labelEl.textContent = TIMELINE_STEPS[i].label + ' — ' + truncate(errorMsg, 60);
            }
            break;
        }
    }

    // Set total with elapsed time
    var totalEl = document.getElementById('tl-total-time');
    if (totalEl && timelineState.startTime) {
        var elapsed = Date.now() - timelineState.startTime;
        totalEl.innerHTML = formatMs(elapsed) + ' <span style="color:var(--danger,#f85149)">(error)</span>';
    }
}

/**
 * Complete timeline without detailed timing (fallback).
 */
function completeTimeline(totalMs) {
    if (timelineState.intervalId) {
        clearInterval(timelineState.intervalId);
        timelineState.intervalId = null;
    }

    for (var i = 0; i < TIMELINE_STEPS.length; i++) {
        var stepEl = document.getElementById('tl-step-' + TIMELINE_STEPS[i].id);
        if (stepEl) {
            stepEl.classList.remove('active', 'pending');
            stepEl.classList.add('done');
        }
    }

    if (totalMs != null) {
        var totalEl = document.getElementById('tl-total-time');
        if (totalEl) totalEl.innerHTML = formatMs(totalMs);
    }
}

/**
 * Hide and reset the timeline.
 */
function hideTimeline() {
    var el = document.getElementById('exec-timeline');
    el.classList.add('hidden');
    if (timelineState.intervalId) {
        clearInterval(timelineState.intervalId);
        timelineState.intervalId = null;
    }
    timelineState.currentStep = -1;
}

// ── Internal helpers ──────────────────────────────────

function activateStep(stepId) {
    var stepEl = document.getElementById('tl-step-' + stepId);
    if (stepEl) {
        stepEl.classList.remove('pending');
        stepEl.classList.add('active');
    }
}

function markStepDone(stepId, ms) {
    var stepEl = document.getElementById('tl-step-' + stepId);
    var timeEl = document.getElementById('tl-time-' + stepId);
    if (stepEl) {
        stepEl.classList.remove('active', 'pending');
        stepEl.classList.add('done');
    }
    if (timeEl && ms != null) {
        timeEl.textContent = formatMs(ms);
    }
}

function updateElapsed() {
    if (!timelineState.startTime) return;
    var elapsed = Date.now() - timelineState.startTime;
    var totalEl = document.getElementById('tl-total-time');
    if (totalEl) {
        totalEl.innerHTML = '<span class="elapsed-counter">' + (elapsed / 1000).toFixed(1) + 's</span>';
    }
    // Update the active step's time too
    for (var i = 0; i < TIMELINE_STEPS.length; i++) {
        var stepEl = document.getElementById('tl-step-' + TIMELINE_STEPS[i].id);
        if (stepEl && stepEl.classList.contains('active')) {
            var timeEl = document.getElementById('tl-time-' + TIMELINE_STEPS[i].id);
            if (timeEl) {
                timeEl.innerHTML = '<span class="elapsed-counter">' + (elapsed / 1000).toFixed(1) + 's</span>';
            }
            break;
        }
    }
}

function formatMs(ms) {
    if (ms == null) return '';
    if (ms < 1) return '<1ms';
    if (ms < 1000) return ms + 'ms';
    return (ms / 1000).toFixed(2) + 's';
}

function truncate(str, max) {
    if (!str) return '';
    return str.length > max ? str.substring(0, max) + '...' : str;
}

function escapeHtml(s) {
    var d = document.createElement('div');
    d.textContent = s;
    return d.innerHTML;
}
