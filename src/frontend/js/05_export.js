/* =============================================
   Export Functions
   ============================================= */

function downloadCsv() {
    if (!AppState.currentQuery) {
        showToast('No query to export', 'warning');
        return;
    }
    exportData('csv');
}

function downloadJson() {
    if (!AppState.currentQuery) {
        showToast('No query to export', 'warning');
        return;
    }
    exportData('json');
}
