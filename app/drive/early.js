// Start-up: the page loads without errors, and the library answers.
// Errors from the page's own start-up are kept by the window; the harness runs after load.
const { lib } = await import('/static/native.js');
log({ startErrors: window.__startErrors || [], folders: (await lib.folders()).length, projects: (await lib.projects()).length });
log('DONE');
