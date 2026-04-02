/* =============================================
   Export Functions
   ============================================= */

function downloadCsv() {
    if (!AppState.currentQuery) {
        showError('No query to export');
        return;
    }
    exportData('csv');
}

function downloadJson() {
    if (!AppState.currentQuery) {
        showError('No query to export');
        return;
    }
    exportData('json');
}
