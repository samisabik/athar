import { fmtTime } from './time.js';
import { Player } from './player.js';
import { Editor } from './editor.js';
import { FxPanel } from './fx.js';
import { Discover } from './discover.js';
import { SoundMap } from './soundmap.js';
import { Recorder } from './recorder.js';
import { Arranger } from './arranger.js';
import { invoke, listen, setVolume as engineVolume } from './engine.js';
import { lib, app } from './native.js';
import { state } from './store.js';
import { host as searchHost, search, scopeText, renderList, select, row, removeFromProject, dropZone, miniWave, syncIdleSearch, setIdleSearch, filters, render, renderChips, renderSubs, emptyState, hasQuery, hasFilter, updateFilterBadge, SORTS, folderCrumbs, tableHead, subLabel, paintMarks, rowClick } from './search.js';
import { host as sideHost, loadFolders, revealFolder, loadCats, loadProjects, renderProjectCard, chooseProjectFolder, projectRow, padsArea, exportProject, selectProject, BANKS, padName, folderActions, diskSize, folderState, appendTree, treeOpen, moveInProject } from './sidebar.js';
import { host as cardHost, extOf, stemOf, loadTags, drawCardWave, saveCatalog, renameCard, setRating, renderCard, tagSuggest, playStop, renderCardPlayState, soundsKey, SHORTCUTS, showShortcuts } from './card.js';
import { toast, notice, busy, fail, el, icon, ib, focusable, NEEDS_LABEL, labelFromTip, labelControls, dialog, modal, confirmModal, choose , $, pickFolder } from './ui.js';
import './dropdown.js';
import './range.js';
import { where, setCatSkin, catContext, reduceMotion } from './cat.js';
import { SETTINGS, host as settingsHost, applySettings, applyEditorFields, saveSetting, renderSettings } from './settings.js';

/** @typedef {import('./search.js').Item} Item */
/** @typedef {import('./search.js').Project} Project */
/** @typedef {import('./editor.js').SaveTarget} SaveTarget */
/** @typedef {{ label: string, kind: string, run: () => unknown }} Command */
/** Settings are keyed by name and hold values of any kind. @type {Record<string, any>} */
const prefs = SETTINGS;



/** @param {Item | null} it */
async function trashSound(it) {
  if (!it) return;
  if (!await confirmModal('Delete file?', `This deletes ${it.name} itself, not just its place in Athar.`, 'Delete')) return;
  try {
    if (isPlaying(it)) stopPreview();
    await lib.trash({ path: it.path });
    const i = state.items.findIndex(x => x.path === it.path);
    if (i >= 0) state.items.splice(i, 1);
    state.cardItem = null;
    state.selected = Math.min(Math.max(0, i), state.items.length - 1);
    $('#scope').textContent = scopeText();
    renderList();
    if (state.items.length) select(state.selected); else renderCard();
    loadFolders().catch(() => { }); loadCats().catch(() => { }); loadProjects().catch(() => { }); loadTags().catch(() => { });
  } catch (e) { fail(e); }
}


/** @param {string} p */
function dirOf(p) { return p.slice(0, p.lastIndexOf('/')); }
const baseOf = (/** @type {string} */ p) => /** @type {string} */ (p.split('/').pop());

// Where a sound lives. Every project owns a folder, so a sound inside one belongs to that project;
// any other sound is in the Library only.
/** @param {string | null | undefined} path @returns {Project | null} */
const projectOf = (path) => (path && state.projects.find(p => p.folder && path.startsWith(p.folder + '/'))) || null;
/** @param {Item} it */
function placeChip(it) {
  const p = projectOf(it.path);
  return el('span', { class: 'place' + (p ? ' proj' : ''), 'data-tip': p ? `This file is in the folder of the project ${p.name}` : 'This file is in a Library folder. Athar never changes it' }, p ? `Project: ${p.name}` : 'Library');
}
const originChip = (/** @type {Item} */ it) => it.origin ? el('span', { class: 'place from', 'data-tip': `Made from ${it.origin}. That file is unchanged` }, `From ${baseOf(it.origin)}`) : null;

// The editor always saves a new file. This asks for its name and where it goes: a project,
// next to the sound it was made from, or another folder.
/** @type {string | null} */
let lastSaveTarget = null;
/** @param {string} [title] @returns {Promise<SaveTarget | null>} */
async function whereToSave(title) {
  try { state.projects = await lib.projects(); } catch { }
  const home = projectOf(editor.path), nameIn = editor.$('[data-f=name]');
  const beside = editor.dir && editor.dir !== home?.folder;
  /** @type {[string, string][]} */
  const options = [
    ...state.projects.map((/** @type {Project} */ p) => /** @type {[string, string]} */ ([`p:${p.id}`, `Project: ${p.name}`])),
    ...(beside ? /** @type {[string, string][]} */ ([['beside', `Library: next to the original, in ${baseOf(/** @type {string} */ (editor.dir))}`]]) : []),
    ['folder', 'Another folder…'],
  ];
  const want = home ? `p:${home.id}` : lastSaveTarget;
  const value = /** @type {string} */ (options.some(o => o[0] === want) ? want : beside ? 'beside' : options[0][0]);
  const v = await modal(title || 'Save as a new file', [
    { label: 'Name', value: nameIn.value },
    { label: editor.path ? 'Save into. The original file stays as it is' : 'Save into', options, value },
  ]);
  if (!v || !v[0]) return null;
  nameIn.value = v[0];
  if (v[1] === 'folder') { const dir = await pickFolder('Save into'); return dir ? { dir, where: baseOf(dir) } : null; }
  lastSaveTarget = v[1];
  if (v[1] === 'beside') return { dir: /** @type {string} */ (editor.dir), where: baseOf(/** @type {string} */ (editor.dir)) };
  const p = /** @type {Project} */ (state.projects.find(x => `p:${x.id}` === v[1]));
  return { dir: p.folder || editor.dir || '.', project: p.id, where: `the project ${p.name}` };
}

// ---------- editor
/** @type {Editor} */
const editor = new Editor($('#editor'), {
  toast, notice, pickFolder,
  ask: async (title, value) => (await modal(title, [{ label: 'Name', value }]))?.[0],
  // p says how to render the editor's sound; the native side renders and writes it.
  onSave: async (p, dir, name, opts = {}) => {
    const meta = { dest_dir: dir, name, sp404: !!opts.sp404, format: opts.sp404 ? 'wav' : opts.format || 'wav', source: opts.source || '', project: opts.project || 0 };
    try {
      const { path } = await invoke('ed_save', { p, meta });
      if (opts.project) { loadProjects().catch(() => { }); setTimeout(() => refreshSide(), 2500); }
      return path;
    }
    catch (e) { fail(e); return null; }
  },
  whereToSave: (title) => whereToSave(title),
  place: () => projectOf(editor.path)?.name || 'Library',
});
editor.fx = new FxPanel($('#fx'), editor, toast, notice);
editor.fx.choose = choose;
editor.fx.onSection = () => catContext();
editor.onAudition = () => { stopPreview(); arranger.stop(); };
window.athar = { editor };
// The window asks before closing while the editor holds unsaved edits.
// The app asks before quitting while the editor has unsaved edits or the recorder holds unsaved takes.
const syncUnsaved = () => app.setDirty(editor.dirty || recorder.hasUnsaved()).catch(() => { });
editor.onDirtyChange = () => syncUnsaved();
// Asked by the app when it is told to quit or close while the editor has unsaved edits.
let quitAsking = false;
window.atharConfirmQuit = async () => {
  if (quitAsking) return;
  quitAsking = true;
  const what = [editor.dirty && 'edits in the editor', recorder.hasUnsaved() && 'recordings'].filter(Boolean).join(' and ') || 'work';
  const ok = await confirmModal('Quit with unsaved work?', `You have unsaved ${what}. Quitting loses them.`, 'Quit');
  quitAsking = false;
  if (ok) app.quitNow();
};
window.addEventListener('beforeunload', (e) => { if (editor.dirty) { e.preventDefault(); e.returnValue = ''; } });
{
  const play = editor.play.bind(editor);
  editor.play = (/** @type {Parameters<typeof play>} */ ...args) => { stopPreview(); arranger.stop(); return play(...args); };
}
for (const type of ['mousedown', 'click', 'dblclick', 'change', 'keydown']) {
  $('#editor').addEventListener(type, (/** @type {Event} */ e) => {
    if ($('#editor').classList.contains('blank') && !/** @type {HTMLInputElement} */ (e.target).closest('#wave')) { e.preventDefault(); e.stopPropagation(); }
  }, true);
}
document.querySelectorAll('[data-fx-open]').forEach(b => b.onclick = () => editor.fx.open(/** @type {string} */ (b.dataset.fxOpen)));

let openSeq = 0;
// The editor holds a whole sound in memory as 32-bit samples, with further copies for playback and undo.
const EDITOR_ASK_BYTES = 1.2e9;
/** @param {Item} it */
async function openPath(it) {
  const bytes = (it.duration || 0) * (it.sr || 48000) * (it.channels || 2) * 4;
  if (bytes > EDITOR_ASK_BYTES && !await confirmModal('Open a long recording?', `${it.name} is ${fmtTime(it.duration)} long and needs about ${(bytes * 3 / 1e9).toFixed(1)} GB of memory in the editor.`, 'Open')) return;
  if (editor.dirty && !await confirmModal('Discard unsaved edits?', `${editor.name} has edits that are not saved. Opening ${it.name} discards them.`, 'Discard')) return;
  const seq = ++openSeq;
  stopPreview();
  try {
    if (seq !== openSeq) return;
    editor.fx.clearPreview();
    showView('editor');
    await editor.load({ source: { path: it.path }, name: it.name, dir: dirOf(it.path), path: it.path, tempo: it.tempo });
  } catch (e) { if (seq === openSeq) fail(new Error(`Could not open ${it.name}: ${/** @type {Error} */ (e).message}`)); }
}

// ---------- tooltips: any element with data-tip
const tip = $('#tip');
/** @type {number | undefined} */
let tipTimer;
/** @type {HTMLElement | null} */
let tipFor = null, tipAt = 0;
document.addEventListener('mousemove', (e) => { tipAt = e.clientX; }, { passive: true });
document.addEventListener('mouseover', (e) => {
  const t = /** @type {HTMLElement | null} */ (/** @type {HTMLInputElement} */ (e.target).closest('[data-tip]'));
  if (t === tipFor) return;
  tipFor = t; clearTimeout(tipTimer); tip.hidden = true;
  if (!t) return;
  tipTimer = setTimeout(() => {
    if (!t.isConnected) return;
    tip.textContent = t.dataset.tip; tip.hidden = false;
    const r = t.getBoundingClientRect(), w = tip.offsetWidth, h = tip.offsetHeight;
    // A tip stays inside the page: it does not spill over the strip on the left. On something
    // as wide as a row it sits by the pointer, since the middle of the row can be far away.
    const edge = t.closest('#activity') ? 8 : $('main').getBoundingClientRect().left + 8;
    const mid = r.width > 360 ? Math.min(r.right, Math.max(r.left, tipAt)) : r.left + r.width / 2;
    let x = Math.min(window.innerWidth - w - 8, Math.max(edge, mid - w / 2));
    let y = r.bottom + 8; if (y + h > window.innerHeight - 8) y = r.top - h - 8;
    if (t.closest('#activity')) { x = r.right + 8; y = r.top + r.height / 2 - h / 2; }
    tip.style.left = x + 'px'; tip.style.top = y + 'px';
  }, 350);
});
const hideTip = () => { clearTimeout(tipTimer); tip.hidden = true; tipFor = null; };
document.addEventListener('mousedown', hideTip, true);
document.addEventListener('scroll', hideTip, true);

// ---------- batch: prep or convert many sounds, each saved as a new file
/** @type {{ stop: boolean } | null} */
let batchJob = null;
/** @param {Item[]} items */
async function batch(items) {
  if (batchJob) { batchJob.stop = true; return; }
  if (!items.length) return notice('There are no sounds listed to process');
  const v = await modal(`Batch: ${items.length} sound${items.length === 1 ? '' : 's'}`, [
    { label: 'Do', options: [['prep', 'Sampler prep: trim silence, normalize, edge fades'], ['convert', 'Convert the format only']] },
    { label: 'Save as', options: [['wav', 'WAV'], ['flac', 'FLAC'], ['mp3', 'MP3']], value: prefs.export_format },
    { label: 'Where', options: [['beside', 'Next to each original'], ['folder', 'Into one folder']] },
  ]);
  if (!v) return;
  const dir = v[2] === 'folder' ? await pickFolder('Save the batch into') : null;
  if (v[2] === 'folder' && !dir) return;
  const prep = v[0] === 'prep' ? { silenceDb: prefs.silence_db, targetDb: prefs.prep_target_db, fadeInMs: prefs.prep_fade_in_ms, fadeOutMs: prefs.prep_fade_out_ms } : null;
  const job = batchJob = { stop: false };
  let saved = 0, failed = 0;
  for (const [i, it] of items.entries()) {
    if (job.stop) break;
    notice(`Batch: ${i + 1} of ${items.length}, ${it.name}. Press the batch button again to stop.`, 600000);
    try {
      const stem = stemOf(it.name) + (dir ? '' : (prefs.edit_suffix || '_edit'));
      await lib.saveBatch(it.path, { rate: prefs.export_rate || 0, bits: prefs.export_bits, mono: !!prefs.export_mono, prep }, { dest_dir: dir || dirOf(it.path), name: stem, format: v[1], source: it.path });
      saved++;
    } catch { failed++; }
  }
  batchJob = null;
  notice(`Batch ${job.stop ? 'stopped' : 'done'}: ${saved} saved${failed ? `, ${failed} failed` : ''}.`, 8000);
}
$('#batchBtn').onclick = () => batch(state.items);

function refreshSide() { loadFolders().catch(fail); loadCats().catch(fail); loadProjects().catch(fail); }
// Projects made or deleted in Finder show up when the window comes back.
window.addEventListener('focus', () => loadProjects().catch(() => { }));

// ---------- preview player
const player = new Player();
const isPlaying = (/** @type {Item | null | undefined} */ it) => !!it && state.playingItem?.path === it.path;
/** @param {Item} it */
function togglePreview(it, forceStart = false) {
  if (isPlaying(it) && !forceStart) { stopPreview(); return; }
  stopPreview();
  player.loop = state.libLoop;
  if (editor.playing) editor.stopPlay();
  editor.stopAudition(); arranger.stop();
  state.playingItem = it;
  player.src = it.path;
  player.onerror = () => { if (state.playingItem === it) { stopPreview(); notice(`Could not play ${it.name}`); } };
  player.oncapped = () => {
    if (state.playingItem === it) notice(`Preview plays the first ${Math.round(prefs._preview_max_s / 60)} minutes of ${it.name}. Open it in the editor to hear all of it.`, 7000);
  };
  player.play().catch(() => { });
  markPlaying();
  renderCardPlayState();
}
function stopPreview() {
  player.pause();
  const was = state.playingItem;
  state.playingItem = null;
  if (was) markPlaying();
  renderCardPlayState();
  drawCardWave();
}
function markPlaying() {
  $('#results').querySelectorAll('.qitem[data-i]').forEach((/** @type {HTMLElement} */ r) => {
    r.classList.toggle('playing', isPlaying(state.items[Number(r.dataset.i)]));
  });
  document.querySelectorAll('.pad').forEach((c) => c.classList.toggle('playing', !!c.padPath && state.playingItem?.path === c.padPath));
  // The play buttons of a project's sounds show stop while their sound plays.
  document.querySelectorAll('.pplay').forEach((b) => {
    const on = state.playingItem?.path === b.playPath, tip = on ? 'Stop' : 'Play';
    /** @type {Element} */ (b.querySelector('.i')).textContent = on ? 'stop' : 'play_arrow';
    b.dataset.tip = tip; b.setAttribute('aria-label', tip);
  });
}
player.onended = () => stopPreview();
(function tick() {
  if (player.duration && isPlaying(state.cardItem)) drawCardWave();
  requestAnimationFrame(tick);
})();

// ---------- volume: the same ruler as the editor's tools
const volume = () => prefs.preview_volume ?? 80;
/** @param {number} v */
function setVolume(v, save = true) {
  v = Math.round(Math.max(0, Math.min(100, v)));
  // The tile lifts off its shadow as the volume rises, and sits flat on the page at zero.
  const tile = $('#knob'), lift = v / 100;
  tile.textContent = v; tile.setAttribute('aria-valuenow', v);
  tile.style.boxShadow = v ? `${(lift * 14).toFixed(1)}px ${(lift * 14).toFixed(1)}px 0 var(--shade)` : 'none';
  tile.style.transform = `translate(${(-lift * 7).toFixed(1)}px, ${(-lift * 7).toFixed(1)}px)`;
  tile.dataset.tip = 'Volume';
  engineVolume(v / 100);
  if (save && prefs.preview_volume !== v) {
    saveSetting('preview_volume', v);
    const s = $('#set-preview_volume'); if (s) { s.value = v; s.nextElementSibling.textContent = `${v}%`; }
  }
}
{
  const tile = $('#knob');
  // The tile sits in the top right corner, so the drag runs toward the open side of the window:
  // pulling it left or up, away from its shadow, is louder; pushing it down or right is quieter.
  tile.addEventListener('pointerdown', (/** @type {PointerEvent} */ e) => {
    if (e.button !== 0) return;
    e.preventDefault(); tile.setPointerCapture(e.pointerId); tile.focus();
    const start = volume(), x0 = e.clientX, y0 = e.clientY;
    const move = (/** @type {PointerEvent} */ ev) => setVolume(start - ((ev.clientX - x0) + (ev.clientY - y0)));
    const up = () => { for (const t of ['pointerup', 'pointercancel', 'lostpointercapture']) tile.removeEventListener(t, up); tile.removeEventListener('pointermove', move); };
    tile.addEventListener('pointermove', move); for (const t of ['pointerup', 'pointercancel', 'lostpointercapture']) tile.addEventListener(t, up);
  });
  tile.addEventListener('keydown', (/** @type {KeyboardEvent} */ e) => {
    const d = /** @type {Record<string, number>} */ ({ ArrowLeft: 2, ArrowUp: 2, ArrowRight: -2, ArrowDown: -2, Home: -100, End: 100 })[e.key];
    if (d) { e.preventDefault(); e.stopPropagation(); setVolume(volume() + d); }
  });
  tile.addEventListener('wheel', (/** @type {WheelEvent} */ e) => { e.preventDefault(); setVolume(volume() - Math.sign(e.deltaY) * 4); }, { passive: false });
  tile.addEventListener('dblclick', () => setVolume(80));
}

// ---------- status: only problems are shown
let lastJobs = 0;
/** @type {string | null} */
let lastModel = null;
// A standing problem takes the place of the progress in the bottom bar until it is gone.
let problem = '';
/** @param {string} text */
function showProblem(text) {
  problem = text;
  if (!text) return;
  $('#indexing').hidden = false; document.body.classList.add('indexing');
  $('#indexingText').textContent = text; $('#indexingBar').hidden = true; $('#indexing .spin').hidden = true;
}
async function poll() {
  try {
    const s = await lib.status();
    showProblem(s.model === 'error' ? `Model failed to load: ${s.error}` : '');
    const right = $('#statusRight');
    if (right) right.textContent = `${s.indexed} sounds indexed${s.device ? ', running on ' + (s.device === 'mps' ? 'the Apple GPU' : 'the CPU') : ''}`;
    const busy = (s.queued && s.done < s.queued) || s.pending_jobs;
    // Analysis goes on in the background on every page; this says how far along it is.
    const left = s.queued && s.done < s.queued ? s.queued - s.done : 0;
    if (!problem) {
      $('#indexing').hidden = !(busy || s.model === 'loading');
      document.body.classList.toggle('indexing', !$('#indexing').hidden);
      $('#indexingText').textContent = s.model === 'loading' ? 'Starting up'
        : left ? `Analysing ${s.folder ? s.folder.split('/').pop() : 'new sounds'}` : 'Looking for new sounds';
      const bar = $('#indexingBar'), pct = left ? Math.round(s.done / s.queued * 100) : 0;
      bar.hidden = !left || s.model === 'loading';
      bar.firstElementChild.style.width = `${pct}%`;
      bar.setAttribute('aria-valuenow', pct);
      $('#indexing .spin').hidden = !bar.hidden;
    }
    if (s.model !== lastModel) { lastModel = s.model; loadCats().catch(() => { }); }
    if (s.indexed !== state.indexed || (!busy && lastJobs)) {
      const first = state.indexed <= 0;
      state.indexed = s.indexed;
      loadFolders().catch(() => { }); loadCats().catch(() => { });
      if (first || !busy) search();
    }
    lastJobs = busy ? 1 : 0;
  } catch { showProblem('Lost contact with the listening engine; restart the app'); }
  setTimeout(poll, document.hidden ? 4000 : 1000);
}

// ---------- wiring
/** @type {number | undefined} */
let qTimer;
function runQuery() {
  clearTimeout(qTimer);
  state.query = $('#q').value.trim();
  if (state.query) { state.sort = 'score'; $('#sort').value = 'score'; }
  search();
}
$('#q').addEventListener('input', () => { clearTimeout(qTimer); qTimer = setTimeout(runQuery, 400); });
$('#q').addEventListener('keydown', (/** @type {KeyboardEvent} */ e) => { if (e.key === 'Enter') runQuery(); });
$('#filterBtn').onclick = () => {
  const f = $('#filters'), b = $('#filterBtn');
  f.hidden = !f.hidden;
  b.classList.toggle('on', !f.hidden); b.setAttribute('aria-expanded', String(!f.hidden));
};
$('#tagFilter').addEventListener('change', (/** @type {Event} */ e) => { state.tag = /** @type {HTMLSelectElement} */ (e.target).value || null; search(); });
$('#ratingFilter').addEventListener('change', (/** @type {Event} */ e) => { state.minRating = Number(/** @type {HTMLSelectElement} */ (e.target).value) || null; search(); });
$('#catFilter').addEventListener('change', (/** @type {Event} */ e) => { state.category = /** @type {HTMLSelectElement} */ (e.target).value || null; loadCats(); search(); });
$('#sort').addEventListener('change', (/** @type {Event} */ e) => { state.sort = /** @type {HTMLSelectElement} */ (e.target).value; search(); });

$('#addFolder').onclick = async () => {
  const p = await app.pickFolder();
  if (p) addFolder(p);
};
/** @param {string} p */
async function addFolder(p) {
  try {
    await lib.addFolder({ path: p });
    loadFolders();
  } catch (e) { fail(e); }
}
$('#rescan').onclick = async () => {
  try { await lib.rescan(); } catch (e) { return fail(e); }
  toast('Looking for new and changed files');
  loadFolders().catch(fail);
  setTimeout(() => loadFolders().catch(() => { }), 2500);
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
$('#projectInput').onkeydown = (/** @type {KeyboardEvent} */ e) => { if (e.key === 'Enter') $('#projectOk').click(); else if (e.key === 'Escape') { e.stopPropagation(); $('#projectInput').value = ''; projectForm(false); } };

window.addEventListener('dragover', (e) => e.preventDefault());
window.addEventListener('drop', (e) => e.preventDefault());
// The web view's own right-click menu (Reload, Inspect) does not belong in the app; text
// fields keep theirs, for copy and paste.
document.addEventListener('contextmenu', (e) => {
  const t = /** @type {HTMLElement} */ (e.target);
  if (!t.closest?.('input, textarea, [contenteditable="true"]')) e.preventDefault();
});
// Folders dropped from Finder are added natively; this says what happened.
listen('dropped', ({ added, problems }) => {
  if (added.length) { refreshSide(); notice(`Added ${added.join(', ')} to your library`); }
  if (problems.length) notice(problems.join(' '), 7000);
});

// ---------- command palette: Cmd+K, type what you want, Enter
/** @returns {Command[]} */
function paletteCommands() {
  const page = (/** @type {string} */ v, /** @type {string} */ label) => ({ label: `Go to ${label}`, kind: 'page', run: () => showView(v) });
  /** @type {Command[]} */
  const cmds = [page('sounds', 'Library'), page('discover', 'Discover'), page('folders', 'Library folders'), page('projects', 'Projects'), page('arranger', 'Arranger'), page('editor', 'Editor'), page('recorder', 'Recorder'), page('map', 'Map'), page('settings', 'Settings'),
    { label: 'Add a folder', kind: 'library', run: () => { showView('folders'); $('#addFolder').click(); } },
    { label: 'New project', kind: 'projects', run: () => { showView('projects'); $('#newProject').click(); } },
    { label: 'Rescan every folder', kind: 'library', run: () => lib.rescan().then(() => toast('Looking for new and changed files')).catch(fail) },
    { label: 'Show keyboard shortcuts', kind: 'help', run: showShortcuts },
    { label: document.documentElement.dataset.theme === 'dark' ? 'Switch to light' : 'Switch to dark', kind: 'look', run: () => { saveSetting('theme', document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark'); renderSettings(); } },
    { label: 'Search the Library', kind: 'library', run: () => { showView('sounds'); $('#q').focus(); } },
  ];
  if (editor.loaded) for (const b of document.querySelectorAll('#editor .tmenu-list [data-a], #editor .tmenu-list [data-fx-open]')) {
    const label = b.querySelector('.tm-label')?.textContent.replace('…', '').trim();
    if (label && !b.hidden && !b.disabled) cmds.push({ label, kind: 'editor', run: () => { showView('editor'); b.click(); } });
  }
  return cmds;
}
function openPalette() {
  if ($('#palette')) return;
  const cmds = paletteCommands(), input = el('input', { placeholder: 'Type a command', spellcheck: 'false', autocomplete: 'off', 'aria-label': 'Command' });
  const list = el('div', { class: 'pal-list', role: 'listbox' }), box = el('div', { id: 'palette' }, el('div', { class: 'pal-card' }, input, list));
  const back = document.activeElement;
  /** @type {Command[]} */
  let shown = [], at = 0;
  const close = () => { box.remove(); if (back?.isConnected) back.focus(); };
  const run = (/** @type {Command | undefined} */ c) => { close(); c?.run(); };
  const draw = () => {
    const words = input.value.toLowerCase().split(/\s+/).filter(Boolean);
    shown = cmds.filter(c => words.every(w => `${c.label} ${c.kind}`.toLowerCase().includes(w)));
    at = Math.min(at, Math.max(0, shown.length - 1));
    list.replaceChildren(...(shown.length ? shown.map((c, i) => el('div', { class: 'pal-item' + (i === at ? ' on' : ''), role: 'option', onclick: () => run(c), onmousemove: () => { if (at !== i) { at = i; draw(); } } }, el('span', {}, c.label), el('small', {}, c.kind)))
      : [el('div', { class: 'pal-empty' }, 'No command matches')]));
    list.querySelector('.on')?.scrollIntoView({ block: 'nearest' });
  };
  input.oninput = () => { at = 0; draw(); };
  input.onkeydown = (e) => {
    e.stopPropagation();
    if (e.key === 'ArrowDown') { e.preventDefault(); at = Math.min(shown.length - 1, at + 1); draw(); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); at = Math.max(0, at - 1); draw(); }
    else if (e.key === 'Enter') { e.preventDefault(); run(shown[at]); }
    else if (e.key === 'Escape') { e.preventDefault(); close(); }
  };
  box.onmousedown = (e) => { if (e.target === box) close(); else if (e.target !== input) { e.preventDefault(); input.focus(); } };
  document.body.append(box); draw(); input.focus();
}
document.addEventListener('keydown', (e) => {
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k' && $('#modal').hidden) { e.preventDefault(); e.stopPropagation(); openPalette(); }
}, true);

// one view at a time, chosen from the strip
/** @param {string} name */
function showView(name) {
  state.currentView = name;
  document.querySelectorAll('main > .view').forEach(v => v.hidden = v.dataset.view !== name);
  document.querySelectorAll('.ab[data-view]').forEach(b => {
    const on = b.dataset.view === (name === 'folders' ? 'sounds' : name);
    b.classList.toggle('on', on);
    if (on) b.setAttribute('aria-current', 'page'); else b.removeAttribute('aria-current');
  });
  hideTip();
  if (name !== 'editor') { editor.stopAudition(); if (editor.playing) editor.stopPlay(); }
  if (name !== 'discover') discover.reset();
  if (name === 'discover') discover.show().catch(fail);
  if (name === 'map') soundmap.show().catch(fail);
  if (name === 'recorder') recorder.show().catch(fail); else recorder.stopPlay();
  if (name === 'arranger') arranger.show().catch(fail); else arranger.leave();
  if (name !== 'map' && name !== 'sounds') stopPreview();
  window.dispatchEvent(new Event('athar:view'));
  if (name === 'editor') requestAnimationFrame(() => editor.draw());
  if (name === 'folders') loadFolders().catch(() => { });
  if (name === 'projects') loadProjects().catch(() => { });
}
const discover = new Discover($('#discover'), {
  el, ib, icon, notice, fail, modal, focusable,
  projectFolder: (/** @type {Parameters<typeof chooseProjectFolder>[0]} */ p) => chooseProjectFolder(p),
  volume: () => volume() / 100,
  stopOthers: () => { stopPreview(); arranger.stop(); if (editor.playing) editor.stopPlay(); editor.stopAudition(); },
  onAdd: () => { refreshSide(); },
});
// A point on the Map carries the path, name, length and rate that preview and the editor read.
const mapItem = (/** @type {import('./soundmap.js').Point} */ p) => /** @type {Item} */ (/** @type {unknown} */ (p));
const soundmap = new SoundMap($('#soundmap'), {
  el, ib, icon, fail, busy,
  preview: (p) => togglePreview(mapItem(p), true), stop: () => stopPreview(),
  edit: (p) => { openPath(mapItem(p)); },
  similar: (it) => { showView('sounds'); $('#q').value = it.name.replace(/\.[^.]+$/, ''); $('#q').dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' })); },
});
const recorder = new Recorder($('#recorder'), {
  el, ib, icon, notice, toast, fail, modal, confirmModal,
  volume: () => volume() / 100,
  stopOthers: () => { stopPreview(); discover.stop(); arranger.stop(); if (editor.playing) editor.stopPlay(); editor.stopAudition(); },
  onSave: () => refreshSide(),
  onChange: () => syncUnsaved(),
  edit: async (/** @type {{ key: string, name: string, saved?: string | null }} */ t) => {
    if (editor.dirty && !await confirmModal('Discard unsaved edits?', `${editor.name} has edits that are not saved. Opening ${t.name} discards them.`, 'Discard')) return;
    const seq = ++openSeq;
    const dir = t.saved ? t.saved.slice(0, t.saved.lastIndexOf('/')) : (await lib.folders()).find((/** @type {{ path: string, offline?: boolean }} */ f) => !f.offline)?.path || '';
    if (seq !== openSeq) return;
    showView('editor');
    await editor.load({ source: { key: t.key }, name: `${t.name}.wav`, dir, path: t.saved || null });
  },
});
const arranger = new Arranger($('#arranger'), {
  el, ib, icon, notice, toast, fail, modal, confirmModal, miniWave, focusable, isPlaying,
  preview: (/** @type {Item} */ it) => togglePreview(it),
  settings: () => SETTINGS,
  go: (/** @type {string} */ view) => showView(view),
  edit: (/** @type {Item} */ it) => openPath(it),
  stopOthers: () => { stopPreview(); discover.stop(); recorder.stopPlay(); if (editor.playing) editor.stopPlay(); editor.stopAudition(); },
  onSave: () => { refreshSide(); setTimeout(() => refreshSide(), 2500); },
});
window.athar.discover = discover; window.athar.soundmap = soundmap; window.athar.recorder = recorder; window.athar.arranger = arranger;
document.querySelectorAll('.ab[data-view]').forEach(b => b.onclick = () => showView(/** @type {string} */ (b.dataset.view)));
document.querySelectorAll('[data-folders]').forEach(b => b.onclick = () => { showView('folders'); if (b.dataset.folders === 'add') $('#addFolder').click(); });
$('#foldersBack').onclick = () => showView('sounds');
showView('sounds');

document.addEventListener('keydown', (e) => {
  if (!$('#modal').hidden || $('#palette')) return;
  const tag = /** @type {HTMLInputElement} */ (e.target).tagName;
  // A ruler keeps its own arrow keys; every other shortcut still works while it has focus.
  const ruler = tag === 'INPUT' && /** @type {HTMLInputElement} */ (e.target).type === 'range';
  if (ruler && /^(Arrow|Home|End|Page)/.test(e.key)) return;
  if (!ruler && (tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA')) { if (e.key === 'Escape') /** @type {HTMLInputElement} */ (e.target).blur(); return; }
  if (tag === 'BUTTON' && (e.key === ' ' || e.key === 'Enter') && /** @type {HTMLInputElement} */ (e.target).matches(':focus-visible')) return;
  if (e.key === '?') { e.preventDefault(); return void showShortcuts(); }
  if (state.currentView === 'editor') editor.handleKey(e);
  else if (state.currentView === 'discover') discover.handleKey(e);
  else if (state.currentView === 'map') soundmap.handleKey(e);
  else if (state.currentView === 'recorder') recorder.handleKey(e);
  else if (state.currentView === 'arranger') arranger.handleKey(e);
  else if (state.currentView === 'sounds') soundsKey(e);
});

lib.settings().then(st => { Object.assign(SETTINGS, st); applySettings(); applyEditorFields(); renderSettings(); })
  .catch(fail)
  .finally(() => { refreshSide(); poll(); loadTags().catch(() => { }); });

where.view = () => state.currentView;
where.section = () => editor.fx.section();

Object.assign(settingsHost, {
  editor, search, refreshSide, loadTags, volume, setVolume, pickFolder,
  redraw: () => { editor.layerKey = null; editor.draw(); editor.fx?.redraw?.(); soundmap.draw(); recorder.drawWave(recorder.rec?.peaks || recorder.takes[0]?.peaks || []); },
});

Object.assign(cardHost, { player, isPlaying, loadProjects, openPath, originChip, placeChip, refreshSide, removeFromProject, renderList, row, search, select, stopPreview, togglePreview, trashSound });
Object.assign(sideHost, { editor, batch, dropZone, isPlaying, miniWave, openPath, refreshSide, search, showView, syncIdleSearch, togglePreview });
Object.assign(searchHost, { isPlaying, openPath, placeChip, projectOf, refreshSide, stopPreview, togglePreview });
