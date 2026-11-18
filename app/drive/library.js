// The Library through its interface: search, the card, making a project, and every page opening
// without an error. Needs a library with a sound whose name has "tone" in it.
const { lib } = await import('/static/native.js');
const errs = []; window.addEventListener('error', e => errs.push(String(e.message))); window.addEventListener('unhandledrejection', e => errs.push('rej: ' + String(e.reason?.message || e.reason)));
for (let i = 0; i < 60 && (await lib.status()).model !== 'ready'; i++) await wait(500);
log({ status: Object.keys(await lib.status()).length, folders: (await lib.folders()).length, cats: (await lib.categories()).length, tags: (await lib.tags()).length });
const q = document.querySelector('#q'); q.value = 'tone'; q.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); await wait(1500);
log({ results: document.querySelectorAll('#results .qitem[data-i]').length });
document.querySelector('#results .qitem[data-i]')?.click(); await wait(800);
log({ card: document.querySelector('#card')?.textContent.slice(0, 60) });
const p = await lib.createProject({ name: 'SmokeTest' }); log({ project: p.name });
document.querySelector('.ab[data-view="projects"]').click(); await wait(1200);
log({ project_rows: document.querySelectorAll('#projList .prow').length });
await lib.deleteProject(p.id);
for (const v of ['discover', 'map', 'recorder', 'arranger', 'editor', 'settings', 'projects', 'sounds']) { document.querySelector(`.ab[data-view="${v}"]`)?.click(); await wait(900); }
const st = await lib.settings(); await lib.setSettings({ preview_volume: st.preview_volume });
log({ settings_rows: document.querySelectorAll('#settingsForm .srow').length });
log({ errors: errs }); log('DONE');
