// The window's start: builds the pages, wires the controls that belong to no single page, and
// routes the keyboard to the page in view.

import './dropdown.js';
import './range.js';
import './tooltip.js';
import './volume.js';
import { $, toast, notice, fail, confirmModal, pickFolder } from './ui.js';
import { listen } from './engine.js';
import { lib, app } from './native.js';
import { state } from './store.js';
import { stemOf } from './names.js';
import { registerPage, showView } from './views.js';
import { bindPalette } from './palette.js';
import { watchStatus } from './status.js';
import { batch } from './batch.js';
import { editor } from './editing.js';
import { search } from './search.js';
import { loadFolders, loadCats, loadProjects, selectProject, refreshSide } from './sidebar.js';
import { loadTags, soundsKey, showShortcuts } from './card.js';
import { where } from './cat.js';
import { SETTINGS, applySettings, applyEditorFields, saveSetting, renderSettings, onSetting, onRestore } from './settings.js';
import { Discover } from './discover.js';
import { SoundMap } from './soundmap.js';
import { Recorder } from './recorder.js';
import { Arranger } from './arranger.js';

/** @typedef {import('./palette.js').Command} Command */

// ---------- the Library's search box and filters
/** @type {number | undefined} */
let queryTimer;
const QUERY_PAUSE_MS = 400;
function runQuery() {
  clearTimeout(queryTimer);
  state.query = $('#q').value.trim();
  if (state.query) state.sort = 'score';
  search();
}
$('#q').addEventListener('input', () => { clearTimeout(queryTimer); queryTimer = setTimeout(runQuery, QUERY_PAUSE_MS); });
$('#q').addEventListener('keydown', (/** @type {KeyboardEvent} */ e) => { if (e.key === 'Enter') runQuery(); });
$('#filterBtn').onclick = () => {
  const f = $('#filters'), b = $('#filterBtn');
  f.hidden = !f.hidden;
  b.classList.toggle('on', !f.hidden); b.setAttribute('aria-expanded', String(!f.hidden));
};
const picked = (/** @type {Event} */ e) => /** @type {HTMLSelectElement} */ (e.target).value;
$('#tagFilter').addEventListener('change', (/** @type {Event} */ e) => { state.tag = picked(e) || null; search(); });
$('#ratingFilter').addEventListener('change', (/** @type {Event} */ e) => { state.minRating = Number(picked(e)) || null; search(); });
$('#catFilter').addEventListener('change', (/** @type {Event} */ e) => { state.category = picked(e) || null; loadCats().catch(fail); search(); });
$('#batchBtn').onclick = () => batch(state.items);

// ---------- the pages
const discover = new Discover($('#discover'));
const soundmap = new SoundMap($('#soundmap'), (p) => { showView('sounds'); $('#q').value = stemOf(p.name); runQuery(); });
// The app asks before quitting while the editor has unsaved edits or the recorder holds unsaved takes.
const syncUnsaved = () => app.setDirty(editor.dirty || recorder.hasUnsaved()).catch(() => { });
const recorder = new Recorder($('#recorder'), syncUnsaved);
const arranger = new Arranger($('#arranger'));
editor.onDirtyChange = syncUnsaved;
window.athar = { editor, discover, soundmap, recorder, arranger };

// Discover opens fresh each time, so leaving it takes it back to its landing.
registerPage('discover', { show: () => discover.show().catch(fail), leave: () => discover.reset() });
registerPage('map', { show: () => soundmap.show().catch(fail) });
registerPage('recorder', { show: () => recorder.show().catch(fail), leave: () => recorder.stopPlay() });
registerPage('arranger', { show: () => arranger.show().catch(fail), leave: () => arranger.leave() });
registerPage('folders', { show: () => loadFolders().catch(fail) });
registerPage('projects', { show: () => loadProjects().catch(fail) });
showView('sounds');

// Canvases are drawn in the theme's colours, so they are drawn again when those change.
onSetting(['theme', 'accent'], () => { soundmap.draw(); recorder.drawWave(recorder.rec?.peaks || recorder.takes[0]?.peaks || []); });
// The list is sorted on the page from this setting, so it is sorted again when it changes.
onSetting(['default_sort'], search);
onRestore(() => { refreshSide(); search(); loadTags().catch(() => { }); });

// Asked by the native side when the app is told to quit or its window to close with unsaved work.
let quitAsking = false;
window.atharConfirmQuit = async () => {
  if (quitAsking) return;
  quitAsking = true;
  const what = [editor.dirty && 'edits in the editor', recorder.hasUnsaved() && 'recordings'].filter(Boolean).join(' and ') || 'work';
  const ok = await confirmModal('Quit with unsaved work?', `You have unsaved ${what}. Quitting loses them.`, 'Quit');
  quitAsking = false;
  if (ok) app.quitNow();
};

// ---------- Library folders and projects: the forms above their lists
$('#addFolder').onclick = async () => {
  const path = await pickFolder('Add a folder to the Library');
  if (!path) return;
  try { await lib.addFolder({ path }); loadFolders().catch(fail); }
  catch (e) { fail(e); }
};
$('#rescan').onclick = async () => {
  try { await lib.rescan(); } catch (e) { return fail(e); }
  toast('Looking for new and changed files');
  loadFolders().catch(fail);
};

const projectForm = (/** @type {boolean} */ open) => { $('#projectAdd').hidden = !open; if (open) $('#projectInput').focus(); };
$('#newProject').onclick = () => projectForm($('#projectAdd').hidden);
$('#projectOk').onclick = async () => {
  const name = $('#projectInput').value.trim(); if (!name) return;
  try {
    const p = await lib.createProject({ name });
    $('#projectInput').value = ''; projectForm(false);
    selectProject(p.id);
    await loadProjects();
    if (p.folder) notice(`${p.name} keeps its sounds in ${p.folder}`, 6000);
    else if (p.folder_error) notice(`${p.name} has no folder yet: ${p.folder_error}`, 8000);
    refreshSide();
  } catch (e) { fail(e); }
};
$('#projectInput').onkeydown = (/** @type {KeyboardEvent} */ e) => {
  if (e.key === 'Enter') $('#projectOk').click();
  else if (e.key === 'Escape') { e.stopPropagation(); $('#projectInput').value = ''; projectForm(false); }
};
// Projects made or deleted in Finder show up when the window comes back.
window.addEventListener('focus', () => loadProjects().catch(() => { }));

// ---------- the window
// A file dropped on the page must not replace it. Folders dropped from Finder are added natively;
// this says what happened.
window.addEventListener('dragover', (e) => e.preventDefault());
window.addEventListener('drop', (e) => e.preventDefault());
listen('dropped', (/** @type {{ added: string[], problems: string[] }} */ { added, problems }) => {
  if (added.length) { refreshSide(); notice(`Added ${added.join(', ')} to your library`); }
  if (problems.length) notice(problems.join(' '), 7000);
});
// The web view's own right-click menu (Reload, Inspect) does not belong in the app; text
// fields keep theirs, for copy and paste.
document.addEventListener('contextmenu', (e) => {
  const t = /** @type {HTMLElement} */ (e.target);
  if (!t.closest?.('input, textarea, [contenteditable="true"]')) e.preventDefault();
});

bindPalette(() => {
  const page = (/** @type {string} */ view, /** @type {string} */ label) => ({ label: `Go to ${label}`, kind: 'page', run: () => showView(view) });
  const dark = document.documentElement.dataset.theme === 'dark';
  /** @type {Command[]} */
  const cmds = [page('sounds', 'Library'), page('discover', 'Discover'), page('folders', 'Library folders'), page('projects', 'Projects'), page('arranger', 'Arranger'), page('editor', 'Editor'), page('recorder', 'Recorder'), page('map', 'Map'), page('settings', 'Settings'),
    { label: 'Add a folder', kind: 'library', run: () => { showView('folders'); $('#addFolder').click(); } },
    { label: 'New project', kind: 'projects', run: () => { showView('projects'); $('#newProject').click(); } },
    { label: 'Rescan every folder', kind: 'library', run: () => $('#rescan').click() },
    { label: 'Show keyboard shortcuts', kind: 'help', run: showShortcuts },
    { label: dark ? 'Switch to light' : 'Switch to dark', kind: 'look', run: () => { saveSetting('theme', dark ? 'light' : 'dark'); renderSettings(); } },
    { label: 'Search the Library', kind: 'library', run: () => { showView('sounds'); $('#q').focus(); } },
  ];
  // Every entry of the editor's menus that can be used right now is a command too.
  if (editor.loaded) for (const b of document.querySelectorAll('#editor .tmenu-list [data-a], #editor .tmenu-list [data-fx-open]')) {
    const label = b.querySelector('.tm-label')?.textContent.replace('…', '').trim();
    if (label && !b.hidden && !b.disabled) cmds.push({ label, kind: 'editor', run: () => { showView('editor'); b.click(); } });
  }
  return cmds;
});

// Keys go to the page in view, unless a dialog, the palette or a text field has them.
/** @type {Record<string, (e: KeyboardEvent) => unknown>} */
const pageKeys = {
  editor: (e) => editor.handleKey(e),
  discover: (e) => discover.handleKey(e),
  map: (e) => soundmap.handleKey(e),
  recorder: (e) => recorder.handleKey(e),
  arranger: (e) => arranger.handleKey(e),
  sounds: soundsKey,
};
document.addEventListener('keydown', (e) => {
  if (!$('#modal').hidden || $('#palette')) return;
  const target = /** @type {HTMLInputElement} */ (e.target), tag = target.tagName;
  // A ruler keeps its own arrow keys; every other shortcut still works while it has focus.
  const ruler = tag === 'INPUT' && target.type === 'range';
  if (ruler && /^(Arrow|Home|End|Page)/.test(e.key)) return;
  if (!ruler && (tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA')) { if (e.key === 'Escape') target.blur(); return; }
  if (tag === 'BUTTON' && (e.key === ' ' || e.key === 'Enter') && target.matches(':focus-visible')) return;
  if (e.key === '?') { e.preventDefault(); return void showShortcuts(); }
  pageKeys[state.currentView]?.(e);
});

// What the cat's tips are about.
where.view = () => state.currentView;
where.section = () => editor.fx.section();

lib.settings().then(st => { Object.assign(SETTINGS, st); applySettings(); applyEditorFields(); renderSettings(); })
  .catch(fail)
  .finally(() => { refreshSide(); watchStatus(); loadTags().catch(() => { }); });
