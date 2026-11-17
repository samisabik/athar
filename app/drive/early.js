// Errors from the page's own start-up are kept by the window; the harness runs after load.
log({ startErrors: window.__startErrors || [], folders: document.querySelectorAll('#folders .item').length, projects: document.querySelectorAll('#projList .prow, #projects .item').length });
log('DONE');
