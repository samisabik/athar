import { ctx, out, decodeUrl, channelsOf, fmtTime } from './audio.js';
import { Editor } from './editor.js';
import { FxPanel } from './fx.js';
import { Discover } from './discover.js';
import { SoundMap } from './soundmap.js';
import { Recorder } from './recorder.js';
import { Arranger } from './arranger.js';
import { run } from './work.js';
import './dropdown.js';
import './range.js';
import { CAT_SKINS } from './cats.js';
import { SECTION_LESSONS, PAGE_LESSONS } from './lessons.js';

const $ = (s) => document.querySelector(s);

const state = {
  query: '', category: null, folder: null, under: null, subs: [], project: null, tag: null, minRating: null,
  sort: 'score', items: [], selected: -1, projects: [], indexed: -1,
};
let SETTINGS = {};
let currentView = 'sounds';
let cardItem = null, allTags = [];
let playingItem = null, libLoop = false;

// ---------- helpers
async function api(path, opts = {}) {
  const { json, ...init } = opts;
  if (json !== undefined) {
    init.method = init.method || 'POST';
    init.headers = { 'Content-Type': 'application/json' };
    init.body = JSON.stringify(json);
  }
  const r = await fetch(path, init);
  if (!r.ok) {
    let msg = r.statusText;
    try { msg = (await r.json()).detail || msg; } catch { }
    throw new Error(msg);
  }
  return r.json();
}

// Every message goes to the one bar at the bottom of the window: short confirmations, notices
// that stay a little longer, and (below them, for as long as it lasts) progress or a problem.
const toast = (msg) => { if (msg) notice(msg, 3500); };
let noticeTimer;
function notice(msg, ms = 4000) {
  const t = $('#toast'); t.textContent = msg; t.classList.add('show');
  clearTimeout(noticeTimer); noticeTimer = setTimeout(() => t.classList.remove('show'), ms);
}
// A message that stays in the bottom bar while something is being worked out; '' takes it away.
let busyText = '';
function busy(text) {
  const t = $('#toast');
  if (text) { busyText = text; notice(text, 600000); }
  else if (busyText && t.textContent === busyText) { busyText = ''; clearTimeout(noticeTimer); t.classList.remove('show'); }
}
const fail = (e) => notice(e.message || String(e), 5000);

function el(tag, attrs = {}, ...kids) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'class') n.className = v;
    else if (k.startsWith('on')) n.addEventListener(k.slice(2), v);
    else if (v != null) n.setAttribute(k, v);
  }
  for (const k of kids.flat()) if (k != null) n.append(k.nodeType ? k : document.createTextNode(k));
  return n;
}

const icon = (name) => el('span', { class: 'i' }, name);
function ib(name, tip, onclick, cls = '') {
  return el('button', { class: `ib ${cls}`, 'data-tip': tip, 'aria-label': tip, onclick }, icon(name));
}

// Makes a non-button element reachable with Tab and activatable with Enter or Space.
function focusable(n, activate) {
  n.tabIndex = 0;
  n.addEventListener('keydown', (e) => {
    if (e.target !== n || (e.key !== 'Enter' && e.key !== ' ')) return;
    e.preventDefault(); e.stopPropagation();
    activate();
  });
  return n;
}

const NEEDS_LABEL = '[data-tip]:is(button, select, input):not([aria-label])';
const labelFromTip = (n) => n.setAttribute('aria-label', n.dataset.tip);
function labelControls(root) {
  if (root.matches?.(NEEDS_LABEL)) labelFromTip(root);
  root.querySelectorAll(NEEDS_LABEL).forEach(labelFromTip);
}
labelControls(document);
new MutationObserver((muts) => {
  for (const m of muts) for (const n of m.addedNodes) if (n.nodeType === 1) labelControls(n);
}).observe($('#editor'), { childList: true, subtree: true });

function dialog(title, nodes, first, okLabel, value, altLabel = null) {
  return new Promise((resolve) => {
    const box = $('#modal'), card = box.querySelector('.modal-card'), ok = $('#modalOk'), cancel = $('#modalCancel'), alt = $('#modalAlt');
    alt.hidden = !altLabel; alt.textContent = altLabel || '';
    const back = document.activeElement;
    $('#modalTitle').textContent = title;
    $('#modalBody').replaceChildren(...nodes);
    ok.textContent = okLabel;
    box.hidden = false;
    (first || ok).focus();
    const done = (v) => {
      box.hidden = true; ok.onclick = cancel.onclick = alt.onclick = null;
      document.removeEventListener('keydown', key, true);
      if (back?.isConnected) back.focus();
      resolve(v);
    };
    const key = (e) => {
      if (e.defaultPrevented) return;
      if (e.key === 'Escape') done(null);
      else if (e.key === 'Enter' && e.target.tagName === 'INPUT') done(value());
      else if (e.key === 'Tab') {
        const f = [...card.querySelectorAll('input, select, textarea, button')].filter(n => !n.disabled && !n.hidden);
        const i = f.indexOf(document.activeElement);
        f[e.shiftKey ? (i <= 0 ? f.length - 1 : i - 1) : (i + 1) % f.length]?.focus();
      } else return;
      e.preventDefault(); e.stopPropagation();
    };
    document.addEventListener('keydown', key, true);
    ok.onclick = () => done(value());
    cancel.onclick = () => done(null);
    alt.onclick = () => done('alt');
  });
}

function modal(title, fields) {
  const inputs = fields.map(f => {
    if (!f.options) return el('input', { value: f.value || '', placeholder: f.placeholder || '', spellcheck: 'false' });
    const s = el('select', {}, f.options.map(([v, l]) => el('option', { value: v }, l)));
    s.value = f.value || f.options[0][0];
    return s;
  });
  const rows = fields.map((f, k) => el('label', {}, f.label, inputs[k]));
  // A field with `when` shows only while the values of the fields pass it.
  const sync = () => { const vals = inputs.map(i => i.value); fields.forEach((f, k) => { if (f.when) rows[k].hidden = !f.when(...vals); }); };
  inputs.forEach(i => i.addEventListener('change', sync)); sync();
  return dialog(title, rows, inputs[0], 'OK', () => inputs.map(i => i.value.trim()));
}

async function confirmModal(title, text, okLabel) {
  return await dialog(title, [el('p', { class: 'lede' }, text)], null, okLabel, () => true) === true;
}

// A question with two answers besides Cancel. Resolves 'ok', 'alt', or null for Cancel.
async function choose(title, text, okLabel, altLabel) {
  return dialog(title, [el('p', { class: 'lede' }, text)], null, okLabel, () => 'ok', altLabel);
}

async function trashSound(it) {
  if (!it) return;
  if (!await confirmModal('Delete file?', `This deletes ${it.name} itself, not just its place in Athar.`, 'Delete')) return;
  try {
    if (isPlaying(it)) stopPreview();
    await api('/api/trash', { json: { path: it.path } });
    const i = state.items.findIndex(x => x.path === it.path);
    if (i >= 0) state.items.splice(i, 1);
    cardItem = null;
    state.selected = Math.min(Math.max(0, i), state.items.length - 1);
    $('#scope').textContent = scopeText();
    renderList();
    if (state.items.length) select(state.selected); else renderCard();
    loadFolders().catch(() => { }); loadCats().catch(() => { }); loadProjects().catch(() => { }); loadTags().catch(() => { });
  } catch (e) { fail(e); }
}

async function pickFolder(title = 'Choose a folder', start = '') {
  if (window.pywebview?.api?.pick_folder) return window.pywebview.api.pick_folder(start);
  const v = await modal(title, [{ label: 'Folder path', placeholder: '/Users/you/Samples' }]);
  return v && v[0];
}

function dirOf(p) { return p.slice(0, p.lastIndexOf('/')); }
const baseOf = (p) => p.split('/').pop();

// Where a sound lives. Every project owns a folder, so a sound inside one belongs to that project;
// any other sound is in the Library only.
const projectOf = (path) => (path && state.projects.find(p => p.folder && path.startsWith(p.folder + '/'))) || null;
function placeChip(it) {
  const p = projectOf(it.path);
  return el('span', { class: 'place' + (p ? ' proj' : ''), 'data-tip': p ? `This file is in the folder of the project ${p.name}` : 'This file is in a Library folder. Athar never changes it' }, p ? `Project: ${p.name}` : 'Library');
}
const originChip = (it) => it.origin ? el('span', { class: 'place from', 'data-tip': `Made from ${it.origin}. That file is unchanged` }, `From ${baseOf(it.origin)}`) : null;

// The editor always saves a new file. This asks for its name and where it goes: a project,
// next to the sound it was made from, or another folder.
let lastSaveTarget = null;
async function whereToSave(title) {
  try { state.projects = await api('/api/projects'); } catch { }
  const home = projectOf(editor.path), nameIn = editor.$('[data-f=name]');
  const beside = editor.dir && editor.dir !== home?.folder;
  const options = [
    ...state.projects.map(p => [`p:${p.id}`, `Project: ${p.name}`]),
    ...(beside ? [['beside', `Library: next to the original, in ${baseOf(editor.dir)}`]] : []),
    ['folder', 'Another folder…'],
  ];
  const want = home ? `p:${home.id}` : lastSaveTarget;
  const value = options.some(o => o[0] === want) ? want : beside ? 'beside' : options[0][0];
  const v = await modal(title || 'Save as a new file', [
    { label: 'Name', value: nameIn.value },
    { label: editor.path ? 'Save into. The original file stays as it is' : 'Save into', options, value },
  ]);
  if (!v || !v[0]) return null;
  nameIn.value = v[0];
  if (v[1] === 'folder') { const dir = await pickFolder('Save into'); return dir ? { dir, where: baseOf(dir) } : null; }
  lastSaveTarget = v[1];
  if (v[1] === 'beside') return { dir: editor.dir, where: baseOf(editor.dir) };
  const p = state.projects.find(x => `p:${x.id}` === v[1]);
  return { dir: p.folder || editor.dir || '.', project: p.id, where: `the project ${p.name}` };
}

// ---------- editor
const editor = new Editor($('#editor'), {
  toast, notice, pickFolder,
  regionsApi: {
    get: (path) => api(`/api/regions?path=${encodeURIComponent(path)}`),
    set: (path, regions) => api('/api/regions', { json: { path, regions } }).catch(fail),
  },
  ask: async (title, value) => (await modal(title, [{ label: 'Name', value }]))?.[0],
  onSave: async (blob, dir, name, opts = {}) => {
    const fd = new FormData(); fd.append('file', blob, name); fd.append('dest_dir', dir); fd.append('name', name);
    if (opts.sp404) fd.append('sp404', 'true');
    else if (opts.format) fd.append('format', opts.format);
    if (opts.source) fd.append('source', opts.source);
    if (opts.project) fd.append('project', opts.project);
    try {
      const { path } = await api('/api/save', { method: 'POST', body: fd });
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
const syncUnsaved = () => window.pywebview?.api?.set_dirty?.(editor.dirty || recorder.hasUnsaved());
editor.onDirtyChange = () => syncUnsaved();
// Asked by the app when it is told to quit or close while the editor has unsaved edits.
let quitAsking = false;
window.atharConfirmQuit = async () => {
  if (quitAsking) return;
  quitAsking = true;
  const what = [editor.dirty && 'edits in the editor', recorder.hasUnsaved() && 'recordings'].filter(Boolean).join(' and ') || 'work';
  const ok = await confirmModal('Quit with unsaved work?', `You have unsaved ${what}. Quitting loses them.`, 'Quit');
  quitAsking = false;
  if (ok) window.pywebview?.api?.quit_now?.(); else window.pywebview?.api?.quit_cancelled?.();
};
window.addEventListener('beforeunload', (e) => { if (editor.dirty) { e.preventDefault(); e.returnValue = ''; } });
{
  const play = editor.play.bind(editor);
  editor.play = (...args) => { stopPreview(); arranger.stop(); return play(...args); };
}
for (const type of ['mousedown', 'click', 'dblclick', 'change', 'keydown']) {
  $('#editor').addEventListener(type, (e) => {
    if ($('#editor').classList.contains('blank') && !e.target.closest('#wave')) { e.preventDefault(); e.stopPropagation(); }
  }, true);
}
document.querySelectorAll('[data-fx-open]').forEach(b => b.onclick = () => editor.fx.open(b.dataset.fxOpen));

let openSeq = 0;
// The editor holds a whole sound in memory as 32-bit samples, with further copies for playback and undo.
const EDITOR_ASK_BYTES = 1.2e9;
async function openPath(it) {
  const bytes = (it.duration || 0) * (it.sr || 48000) * (it.channels || 2) * 4;
  if (bytes > EDITOR_ASK_BYTES && !await confirmModal('Open a long recording?', `${it.name} is ${fmtTime(it.duration)} long and needs about ${(bytes * 3 / 1e9).toFixed(1)} GB of memory in the editor.`, 'Open')) return;
  if (editor.dirty && !await confirmModal('Discard unsaved edits?', `${editor.name} has edits that are not saved. Opening ${it.name} discards them.`, 'Discard')) return;
  const seq = ++openSeq;
  stopPreview();
  try {
    const b = await decodeUrl(`/api/audio?path=${encodeURIComponent(it.path)}`, `/api/decoded?full=1&path=${encodeURIComponent(it.path)}`, it.sr || 0);
    if (seq !== openSeq) return;
    editor.fx.clearPreview();
    showView('editor');
    editor.load({ channels: channelsOf(b), sr: b.sampleRate, name: it.name, dir: dirOf(it.path), path: it.path, tempo: it.tempo });
  } catch (e) { if (seq === openSeq) fail(new Error(`Could not open ${it.name}: ${e.message}`)); }
}

// ---------- tooltips: any element with data-tip
const tip = $('#tip');
let tipTimer, tipFor = null, tipAt = 0;
document.addEventListener('mousemove', (e) => { tipAt = e.clientX; }, { passive: true });
document.addEventListener('mouseover', (e) => {
  const t = e.target.closest('[data-tip]');
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

// ---------- sidebar
// Library folders page: the list picks a folder, the panel above shows it.
let folderSel = null, folderShut = false;
async function loadFolders() {
  const fs = await api('/api/folders');
  const box = document.createDocumentFragment();
  if (state.folder && !fs.some(f => f.path === state.folder)) { state.folder = null; state.under = null; }
  if (!fs.some(f => f.path === folderSel)) folderSel = (fs.find(f => f.path === state.folder) || fs[0])?.path ?? null;
  if (!fs.length) box.append(dropZone());
  const show = (f, under = null) => { state.folder = f.path; state.under = under; loadFolders(); showView('sounds'); search(); };
  for (const f of fs) {
    // A click opens a folder to show what is inside; a click on the open one closes it again.
    const open = folderSel === f.path && !folderShut;
    const pick = () => { folderShut = folderSel === f.path && !folderShut; folderSel = f.path; loadFolders().catch(fail); };
    box.append(focusable(el('div', {
      class: 'item' + (open ? ' on' : '') + (f.offline ? ' offline' : ''),
      'data-tip': f.offline ? `${f.path} was not found, or its drive is not connected. Its sounds, tags and notes are kept.` : null,
      onclick: pick,
    },
    el('span', { class: 'label' }, f.path.split('/').pop()),
    el('span', { class: 'count' }, [folderState(f), `${f.files} sound${f.files === 1 ? '' : 's'}`, diskSize(f.bytes)].filter(Boolean).join(', ')),
    open ? folderActions(f, show) : null), pick));
    if (open && !f.offline) await appendTree(box, f, f.path, 1, show);
  }
  $('#folders').replaceChildren(box);
  document.querySelectorAll('[data-folders=list]').forEach(b => { b.textContent = fs.length ? 'Library folders' : 'No Library folders yet'; });
  syncIdleSearch();
}

// The subfolders of the picked folder, as an indented tree. A level opens when clicked.
const treeOpen = new Set();
async function appendTree(box, f, path, depth, show) {
  let subs = [];
  try { subs = await api('/api/subfolders', { json: { path } }); } catch { return; }
  for (const sf of subs) {
    const open = treeOpen.has(sf.path);
    // A folder with folders inside opens in place. One that holds only sounds has nothing more
    // to unfold, so a click shows its sounds in the Library.
    const toggle = () => { if (!sf.nested) return sf.files ? show(f, sf.path) : notice(`${sf.name} has no sounds in it yet`); open ? treeOpen.delete(sf.path) : treeOpen.add(sf.path); loadFolders().catch(fail); };
    box.append(focusable(el('div', {
      class: 'item sub', style: `padding-left:${12 + depth * 22}px`,
      onclick: toggle,
    },
    icon(sf.nested ? (open ? 'expand_more' : 'chevron_right') : 'folder'),
    el('span', { class: 'label' }, sf.name),
    el('span', { class: 'count' }, `${sf.files} sound${sf.files === 1 ? '' : 's'}`),
    ib('queue_music', 'Show its sounds in the Library', (e) => { e.stopPropagation(); show(f, sf.path); }),
    ib('folder_open', 'Show in Finder', (e) => { e.stopPropagation(); revealFolder(sf.path); })), toggle));
    if (open && sf.nested) await appendTree(box, f, sf.path, depth + 1, show);
  }
}

// What a folder is doing, and for the picked one the two things you can do with it.
function folderState(f) {
  const waiting = f.files - f.indexed;
  return f.offline ? 'not found' : waiting > 0 ? `analysing ${waiting} more` : '';
}
const revealFolder = (path) => api('/api/reveal-folder', { json: { path } }).catch(fail);
function diskSize(n) { return !n ? '' : n < 1e6 ? `${Math.max(1, Math.round(n / 1e3))} KB` : n < 1e9 ? `${(n / 1e6).toFixed(n < 1e7 ? 1 : 0)} MB` : `${(n / 1e9).toFixed(1)} GB`; }
function folderActions(f, show) {
  const name = f.path.split('/').pop();
  return [
    ib('queue_music', 'Show its sounds in the Library', (e) => { e?.stopPropagation(); show(f); }),
    ib('folder_open', 'Show in Finder', (e) => { e?.stopPropagation(); revealFolder(f.path); }),
    ib('close', 'Remove from Athar. The files stay on disk', async (e) => {
      e?.stopPropagation();
      if (!await confirmModal(`Remove ${name}?`, 'The folder leaves your library. Its files stay on disk.', 'Remove')) return;
      try { await api('/api/folders', { method: 'DELETE', json: { path: f.path } }); }
      catch (err) { return fail(err); }
      if (state.folder === f.path) { state.folder = null; state.under = null; }
      refreshSide(); search();
    }),
  ];
}

async function loadCats() {
  const cs = await api('/api/categories');
  const sel = $('#catFilter');
  sel.innerHTML = '';
  sel.append(el('option', { value: '' }, 'any'));
  cs.filter(c => c.count || c.name === state.category).sort((a, b) => a.name.localeCompare(b.name))
    .forEach(c => sel.append(el('option', { value: c.name }, `${c.name} (${c.count})`)));
  sel.value = state.category || '';
}

let projSel = null, projShut = false, projSeq = 0;
async function loadProjects() {
  state.projects = await api('/api/projects');
  if (!state.projects.some(p => p.id === projSel)) projSel = state.projects[0]?.id ?? null;
  if (currentView === 'projects') renderProjectCard().catch(fail);
  if (editor.channels) { editor.infoSig = null; editor.updateInfo(); }
}

// Every project is a row. The picked one carries its actions and opens to show its sounds.
async function renderProjectCard() {
  const seq = ++projSeq, box = $('#projList');
  if (!state.projects.length) {
    box.replaceChildren(el('div', { class: 'empty' }, el('h3', {}, 'No projects yet'), el('p', { class: 'muted' }, 'A project is a folder of its own for what you are making. Create one, then add sounds from the Library: each is copied in, and the original stays where it is.')));
    return;
  }
  const p = projShut ? null : state.projects.find(x => x.id === projSel);
  let items = p?.n ? await api('/api/search', { json: { project: p.id, sort: 'name', limit: 5000 } }) : [];
  if (items.length > 1) {
    const order = await api(`/api/projects/${p.id}/order`), at = new Map(order.map((path, k) => [path, k]));
    items.sort((a, b) => (at.get(a.path) ?? 1e9) - (at.get(b.path) ?? 1e9));
  }
  const pads = padView && items.length ? await api(`/api/projects/${p.id}/pads`) : {};
  if (seq !== projSeq) return;
  const count = (n) => `${n} sound${n === 1 ? '' : 's'}`;
  const rename = async () => {
    const v = (await modal(`Rename ${p.name}`, [{ label: 'Name', value: p.name }]))?.[0]?.trim();
    if (!v || v === p.name) return;
    try { await api(`/api/projects/${p.id}/rename`, { json: { name: v } }); loadProjects().catch(fail); }
    catch (e) { fail(e); }
  };
  const stop = (fn) => (e) => { e?.stopPropagation(); return fn(); };
  const actions = () => [
    ib('ios_share', 'Export: copy the files, or make an SP-404MKII project', stop(() => exportProject(p))),
    ib('grid_view', 'Pads: lay the sounds out on the SP-404MKII pads', stop(() => {
      padView = !padView;
      try { localStorage.setItem('athar.padView', padView ? '1' : ''); } catch { }
      renderProjectCard().catch(fail);
    }), padView ? 'on' : ''),
    ib('dynamic_feed', 'Batch: prep or convert every sound in this project', stop(() => batch(items))),
    ib('edit', 'Rename project', stop(rename)),
    ib('queue_music', 'Show its sounds in the Library', stop(() => { state.project = p.id; showView('sounds'); search(); })),
    p.folder ? ib('folder_open', 'Show in Finder', stop(() => revealFolder(p.folder))) : null,
    ib('close', 'Remove from Athar. The files stay on disk', stop(async () => {
      if (!await confirmModal(`Remove ${p.name}?`, 'The project leaves Athar. Its folder and files stay on disk.', 'Remove')) return;
      try { await api(`/api/projects/${p.id}`, { method: 'DELETE' }); }
      catch (e) { return fail(e); }
      if (state.project === p.id) { state.project = null; search(); }
      loadProjects().catch(fail);
    })),
  ];
  const out = [];
  for (const x of state.projects) {
    const on = !projShut && x.id === projSel, pick = () => { projShut = on; projSel = x.id; renderProjectCard().catch(fail); };
    out.push(focusable(el('div', { class: 'item' + (on ? ' on' : ''), onclick: pick },
      el('span', { class: 'label' }, x.name),
      el('span', { class: 'count' }, [count(on ? items.length : x.n || 0), diskSize(x.bytes)].filter(Boolean).join(', ')),
      on ? actions() : null), pick));
    if (!on) continue;
    if (padView && items.length) { out.push(padsArea(p, items, pads)); continue; }
    out.push(...(items.length ? items.map(it => projectRow(p, it, items))
      : [el('div', { class: 'prow-empty muted' }, 'No sounds yet. Add some from the Library with "Add to project". Each one is copied into this project.')]));
  }
  box.replaceChildren(...out);
}

async function chooseProjectFolder(p) {
  const dir = await pickFolder(`Folder for ${p.name}`);
  if (!dir) return false;
  try { await api(`/api/projects/${p.id}/folder`, { json: { path: dir } }); }
  catch (e) { fail(e); return false; }
  notice(`Sounds added to ${p.name} are now copied into its folder`);
  refreshSide(); search();
  setTimeout(() => loadProjects().catch(() => { }), 2500);
  return true;
}

// Rows of a project can be dragged into the order the pads should have.
let dragPath = null, dragPad = null;
async function moveInProject(p, items, from, to, after) {
  const paths = items.map(x => x.path).filter(x => x !== from);
  paths.splice(paths.indexOf(to) + (after ? 1 : 0), 0, from);
  try { await api(`/api/projects/${p.id}/order`, { json: { paths } }); } catch (e) { return fail(e); }
  renderProjectCard().catch(fail);
}
function projectRow(p, it, items = []) {
  const over = (e) => { const r = e.currentTarget.getBoundingClientRect(); return e.clientY > r.top + r.height / 2; };
  return focusable(el('div', { class: 'prow', draggable: 'true', ondblclick: () => openPath(it), 'data-tip': 'Double-click to edit. Drag to reorder.',
    ondragstart: (e) => { dragPath = it.path; e.dataTransfer?.setData('text/plain', it.name); if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'; },
    ondragend: () => { dragPath = null; document.querySelectorAll('.prow.drop-before, .prow.drop-after').forEach(n => n.classList.remove('drop-before', 'drop-after')); },
    ondragover: (e) => { if (!dragPath || dragPath === it.path || dragPad != null) return; e.preventDefault(); const a = over(e); e.currentTarget.classList.toggle('drop-after', a); e.currentTarget.classList.toggle('drop-before', !a); },
    ondragleave: (e) => e.currentTarget.classList.remove('drop-before', 'drop-after'),
    ondrop: (e) => { if (!dragPath || dragPath === it.path || dragPad != null) return; e.preventDefault(); e.stopPropagation(); const from = dragPath; dragPath = null; moveInProject(p, items, from, it.path, over(e)); } },
    Object.assign(ib(isPlaying(it) ? 'stop' : 'play_arrow', isPlaying(it) ? 'Stop' : 'Play', (e) => { e.stopPropagation(); togglePreview(it); }, 'pplay'), { playPath: it.path }),
    miniWave(it),
    el('span', { class: 'qn' }, it.name),
    el('span', { class: 'qd' }, [it.category, fmtTime(it.duration)].filter(Boolean).join(' · ')),
    ib('close', 'Remove from project', async (e) => {
      e.stopPropagation();
      try { await api(`/api/projects/${p.id}/items`, { json: { path: it.path, remove: true } }); }
      catch (err) { return fail(err); }
      loadProjects().catch(fail);
    })), () => openPath(it));
}

// The SP-404MKII has ten banks of sixteen pads. Pad n is bank n / 16, pad n % 16, and a
// bank's pads read like text: 1 to 4 across the top row, 13 to 16 across the bottom.
const BANKS = 'ABCDEFGHIJ', PADS = 160;
let padView = false, padBank = 0;
try { padView = localStorage.getItem('athar.padView') === '1'; } catch { }
const padName = (n) => `${BANKS[Math.floor(n / 16)]}${n % 16 + 1}`;

function padsArea(p, items, pads) {
  const byPath = new Map(items.map(it => [it.path, it]));
  pads = Object.fromEntries(Object.entries(pads).filter(([path]) => byPath.has(path)));
  const byPad = new Map(Object.entries(pads).map(([path, n]) => [n, byPath.get(path)]));
  const save = async (next) => {
    try { await api(`/api/projects/${p.id}/pads`, { json: { pads: next } }); } catch (e) { return fail(e); }
    renderProjectCard().catch(fail);
  };
  const place = (path, n) => {
    const next = { ...pads }, from = next[path], there = byPad.get(n);
    if (there && there.path !== path) { if (from != null) next[there.path] = from; else delete next[there.path]; }
    next[path] = n;
    save(next);
  };
  const fill = () => {
    const next = { ...pads }, taken = new Set(byPad.keys());
    let n = padBank * 16, left = 0;
    for (const it of items) {
      if (it.path in next) continue;
      while (n < PADS && taken.has(n)) n++;
      if (n >= PADS) { left++; continue; }
      next[it.path] = n; taken.add(n);
    }
    if (left) notice(`${left} sound${left === 1 ? '' : 's'} did not fit: the pads from ${BANKS[padBank]}1 to J16 are full`);
    save(next);
  };
  const clear = (path) => { const next = { ...pads }; delete next[path]; save(next); };
  const endDrag = () => { dragPath = null; dragPad = null; document.querySelectorAll('.pad.over').forEach(x => x.classList.remove('over')); };
  const cell = (n) => {
    const it = byPad.get(n);
    const c = el('div', {
      class: 'pad' + (it ? '' : ' empty') + (isPlaying(it) ? ' playing' : ''), draggable: it ? 'true' : 'false',
      onclick: () => { if (it) togglePreview(it); },
      ondblclick: () => { if (it) openPath(it); },
      ondragstart: (e) => { dragPath = it.path; dragPad = n; e.dataTransfer?.setData('text/plain', it.name); if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'; },
      ondragend: endDrag,
      ondragover: (e) => { if (!dragPath) return; e.preventDefault(); e.currentTarget.classList.add('over'); },
      ondragleave: (e) => e.currentTarget.classList.remove('over'),
      ondrop: (e) => { if (!dragPath) return; e.preventDefault(); e.stopPropagation(); const path = dragPath; endDrag(); place(path, n); },
    },
      el('span', { class: 'pn' }, padName(n)),
      it ? el('span', { class: 'qn' }, it.name.replace(/\.[^.]+$/, '')) : null,
      it ? miniWave(it) : null,
      it ? ib('close', 'Clear pad', (e) => { e.stopPropagation(); clear(it.path); }, 'padx') : null);
    if (it) {
      c.padPath = it.path;
      c.addEventListener('keydown', (e) => { if (e.target === c && (e.key === 'Delete' || e.key === 'Backspace')) { e.preventDefault(); clear(it.path); } });
    }
    return focusable(c, () => it && togglePreview(it));
  };
  const used = new Set([...byPad.keys()].map(n => Math.floor(n / 16)));
  // Five buttons like the sampler's: each holds a bank pair, and pressing the lit one again
  // flips between its two banks. Another button keeps the current half.
  const half = padBank >= 5 ? 5 : 0;
  const letter = (i) => el('span', { class: used.has(i) ? 'used' : '' }, BANKS[i]);
  const banks = el('div', { class: 'ab-switch banks', role: 'group', 'aria-label': 'Bank' },
    [0, 1, 2, 3, 4].map((k) => el('button', {
      class: padBank % 5 === k ? 'on' : '', 'aria-pressed': String(padBank % 5 === k),
      'aria-label': `Bank ${BANKS[padBank % 5 === k ? padBank : k + half]}`,
      onclick: () => { padBank = padBank % 5 === k ? (padBank + 5) % 10 : k + half; renderProjectCard().catch(fail); },
    }, padBank % 5 === k ? BANKS[padBank] : [letter(k), '/', letter(k + 5)])));
  const grid = el('div', { class: 'padgrid' }, Array.from({ length: 16 }, (_, k) => cell(padBank * 16 + k)));
  const pool = el('div', {
    class: 'padpool',
    ondragover: (e) => { if (dragPad != null) e.preventDefault(); },
    ondrop: (e) => {
      if (dragPad == null) return;
      e.preventDefault();
      const path = dragPath; endDrag(); clear(path);
    },
  }, items.filter(it => !(it.path in pads)).map(it => {
    const row = projectRow(p, it, items);
    // Dragged toward the grid, the row looks like the pad it is about to become.
    row.addEventListener('dragstart', (e) => {
      if (!e.dataTransfer?.setDragImage) return;
      const w = grid.querySelector('.pad')?.offsetWidth || 180;
      const ghost = el('div', { class: 'pad padghost', style: `width:${w}px` },
        el('span', { class: 'pn' }, '\u00a0'), el('span', { class: 'qn' }, it.name.replace(/\.[^.]+$/, '')), miniWave(it));
      document.body.append(ghost);
      e.dataTransfer.setDragImage(ghost, w / 2, 42);
      setTimeout(() => ghost.remove(), 0);
    });
    return row;
  }));
  return el('div', { class: 'pads' },
    el('div', { class: 'padbar' }, banks, ib('auto_awesome_mosaic', 'Fill the free pads in project order', fill)),
    grid, pool);
}

async function exportProject(p) {
  const v = await modal(`Export ${p.name}`, [
    { label: 'Format', options: [['copy', 'Copy original files'], ['sp404', 'SP-404MKII project']] },
    { label: 'Name pattern: {project} {category} {n:03d} {name}', value: SETTINGS.name_pattern || '{project}_{name}', when: (f) => f !== 'sp404' },
  ]);
  if (!v) return;
  const card = v[0] === 'sp404' ? await api('/api/sp404-card').catch(() => ({})) : {};
  const dest = await pickFolder(v[0] === 'sp404' ? 'Choose the SD card' : 'Export to folder', card.import || '');
  if (!dest) return;
  try {
    const r = await api(`/api/projects/${p.id}/export`, { json: { dest, pattern: v[1], format: v[0] } });
    if (v[0] === 'sp404') notice(`Saved ${r.folder.split('/').pop()}`);
  } catch (e) { fail(e); }
}

// ---------- batch: prep or convert many sounds, each saved as a new file
let batchJob = null;
async function batch(items) {
  if (batchJob) { batchJob.stop = true; return; }
  if (!items.length) return notice('There are no sounds listed to process');
  const v = await modal(`Batch: ${items.length} sound${items.length === 1 ? '' : 's'}`, [
    { label: 'Do', options: [['prep', 'Sampler prep: trim silence, normalize, edge fades'], ['convert', 'Convert the format only']] },
    { label: 'Save as', options: [['wav', 'WAV'], ['flac', 'FLAC'], ['mp3', 'MP3']], value: SETTINGS.export_format },
    { label: 'Where', options: [['beside', 'Next to each original'], ['folder', 'Into one folder']] },
  ]);
  if (!v) return;
  const dir = v[2] === 'folder' ? await pickFolder('Save the batch into') : null;
  if (v[2] === 'folder' && !dir) return;
  const prep = v[0] === 'prep' ? { silenceDb: SETTINGS.silence_db, targetDb: SETTINGS.prep_target_db, fadeInMs: SETTINGS.prep_fade_in_ms, fadeOutMs: SETTINGS.prep_fade_out_ms } : null;
  const job = batchJob = { stop: false };
  let saved = 0, failed = 0;
  for (const [i, it] of items.entries()) {
    if (job.stop) break;
    notice(`Batch: ${i + 1} of ${items.length}, ${it.name}. Press the batch button again to stop.`, 600000);
    try {
      const url = encodeURIComponent(it.path);
      const b = await decodeUrl(`/api/audio?path=${url}`, `/api/decoded?full=1&path=${url}`, it.sr || 0);
      const { wav } = await run('render', { chs: channelsOf(b), sr: b.sampleRate, rate: SETTINGS.export_rate || b.sampleRate, bits: SETTINGS.export_bits, mono: !!SETTINGS.export_mono, prep });
      const stem = stemOf(it.name) + (dir ? '' : (SETTINGS.edit_suffix || '_edit'));
      const fd = new FormData();
      fd.append('file', new Blob([wav], { type: 'audio/wav' }), stem + '.wav');
      fd.append('dest_dir', dir || dirOf(it.path)); fd.append('name', stem); fd.append('format', v[1]); fd.append('source', it.path);
      await api('/api/save', { method: 'POST', body: fd });
      saved++;
    } catch { failed++; }
  }
  batchJob = null;
  notice(`Batch ${job.stop ? 'stopped' : 'done'}: ${saved} saved${failed ? `, ${failed} failed` : ''}.`, 8000);
}
$('#batchBtn').onclick = () => batch(state.items);

function refreshSide() { loadFolders().catch(fail); loadCats().catch(fail); loadProjects().catch(fail); }

// ---------- search
let searchSeq = 0;
const SORTS = ['name', 'duration', 'category', 'rating', 'recorded'];
const hasQuery = () => !!state.query;
const hasFilter = () => !!(state.folder || state.project || state.category || state.tag || state.minRating);

function filters() {
  const defaultSort = SORTS.includes(SETTINGS.default_sort) ? SETTINGS.default_sort : 'name';
  return {
    category: state.category, folder: state.folder, under: state.folder ? state.under : null, project: state.project,
    min_dur: null,
    tag: state.tag || null, min_rating: state.minRating || null,
    sort: state.sort === 'score' && !hasQuery() ? defaultSort : state.sort,
    limit: 500,
  };
}

function updateFilterBadge() {
  const n = [state.category, state.tag, state.minRating].filter(Boolean).length;
  const b = $('#filterCount'); b.hidden = !n; b.textContent = n;
  for (const id of ['catFilter', 'tagFilter', 'ratingFilter']) {
    const s = $('#' + id);
    s.closest('.f').classList.toggle('set', !!s.value);
  }
}

async function search() {
  updateFilterBadge();
  const seq = ++searchSeq;
  if (!hasQuery() && !hasFilter()) {
    state.items = []; state.selected = -1; render();
    return;
  }
  try {
    const items = await api('/api/search', { json: { ...filters(), query: state.query || null } });
    if (seq !== searchSeq) return;
    const keep = items.findIndex(x => x.path === cardItem?.path);
    state.items = items;
    state.selected = keep >= 0 ? keep : (items.length ? 0 : -1);
    render();
  } catch (e) { if (seq === searchSeq) fail(e); }
}

function scopeText() {
  const n = state.items.length;
  if (!n) return '';
  return `${n} sound${n === 1 ? '' : 's'}` + (state.query ? ` · like "${state.query}"` : '');
}

function renderChips() {
  const box = $('#chips'); box.innerHTML = '';
  const chips = [
    state.folder && ['folder', state.folder.split('/').pop(), () => { state.folder = null; state.under = null; return loadFolders(); }],
    state.category && ['category', state.category, () => { state.category = null; return loadCats(); }],
    state.tag && ['sell', state.tag, () => { state.tag = null; $('#tagFilter').value = ''; }],
    state.minRating && ['star', `${state.minRating}+`, () => { state.minRating = null; $('#ratingFilter').value = ''; }],
    state.project && ['category', state.projects.find(p => p.id === state.project)?.name || 'Project', () => { state.project = null; return loadProjects(); }],
  ].filter(Boolean);
  for (const [ic, label, clear] of chips) {
    const crumbs = ic === 'folder' ? folderCrumbs() : [label];
    box.append(el('span', { class: 'fchip' }, icon(ic), crumbs,
      ib('close', `Show all, not only ${label}`, async () => { await clear()?.catch(fail); search(); })));
  }
  box.hidden = !chips.length;
  renderSubs();
}

// The folder chip reads as a path: each part goes back up to that level.
function folderCrumbs() {
  const root = state.folder, parts = state.under ? state.under.slice(root.length + 1).split('/') : [];
  const go = (under) => () => { state.under = under; search(); };
  const out = [el('button', { class: 'crumb', onclick: go(null) }, root.split('/').pop())];
  parts.forEach((p, i) => out.push(el('span', { class: 'crumb-sep' }, '/'),
    el('button', { class: 'crumb', onclick: go(`${root}/${parts.slice(0, i + 1).join('/')}`) }, p)));
  return out;
}

let subsSeq = 0;
async function renderSubs() {
  const box = $('#subs'), at = state.folder && (state.under || state.folder), seq = ++subsSeq;
  let subs = [];
  if (at) { try { subs = await api('/api/subfolders', { json: { path: at } }); } catch { subs = []; } }
  if (seq !== subsSeq) return;
  box.innerHTML = '';
  for (const sf of subs) {
    box.append(el('button', { class: 'subchip', 'data-tip': `Only the sounds in ${sf.name}`, onclick: () => { state.under = sf.path; search(); } },
      icon(sf.nested ? 'folder_copy' : 'folder'), sf.name, el('span', { class: 'count' }, String(sf.files))));
  }
  box.hidden = !subs.length;
}

function render() {
  renderChips();
  $('#scope').textContent = scopeText();
  renderList();
  if (state.items.length) select(Math.max(0, state.selected), { scroll: false });
  else { cardItem = null; renderCard(); }
}

function renderList() {
  const box = $('#results'), lib = $('.lib');
  const had = box.contains(document.activeElement) ? (document.activeElement.dataset.sort || 'row') : null;
  box.innerHTML = '';
  const noFolders = !$('#folders').querySelector('.item'), idle = !hasQuery() && !hasFilter();
  lib.classList.toggle('blank', noFolders);
  lib.classList.toggle('idle', idle);
  syncIdleSearch();
  $('.scope-row').hidden = noFolders || idle;
  if (!state.items.length) { box.append(emptyState()); return; }
  box.append(tableHead());
  const scores = state.items.map(i => i.score), max = Math.max(...scores), min = Math.min(...scores);
  state.items.forEach((it, i) => {
    const rel = it.text_match >= 1 ? 1 : (it.score - min) / ((max - min) || 1);
    box.append(row(it, i, `${Math.round(10 + rel * 90)}%`));
  });
  paintMarks();
  if (had === 'row') box.querySelector(`.qitem[data-i="${state.selected}"]`)?.focus({ preventScroll: true });
  else if (had) box.querySelector(`.th[data-sort="${had}"]`)?.focus();
}

function dropZone() {
  return el('div', { class: 'dropzone' },
    el('div', { class: 'dz-box' }, el('h3', {}, 'Add a folder of recordings'),
      el('button', { class: 'primary', onclick: () => $('#addFolder').click() }, icon('create_new_folder'), ' Choose a folder')));
}

// ---------- the Library before any search: the search box sits in the middle of the page under a
// line picked at random, and its placeholder types out things to look for.
const HELLOS = ['What does it sound like?', 'Say it in words.', 'What are you hearing?', 'Name that noise.', 'Go on, describe it.', 'Looking for a noise?', 'What should it sound like?', 'Hum it in words.'];
const TRY_PHRASES = [
  'rain on a tin roof', 'lonely metallic ping', 'fat sub kick', 'dusty vinyl crackle', 'crowd in a big hall', 'wooden door creak',
  'birds at dawn', 'glass breaking', 'warm tape hiss', 'tight snare with a ring', 'distant thunder', 'footsteps on gravel',
  'metal scrape', 'soft synth pad', 'train passing', 'water dripping in a cave', 'something angry', 'something tiny', 'underwater',
  'a room with nobody in it', 'engine starting', 'bell far away', 'paper tearing', 'low hum',
];
const PLACEHOLDER = 'Describe a sound';
let typing = 0;
// The search box stays in the middle until a search finds something.
function syncIdleSearch() { setIdleSearch(!state.items.length && !hasFilter()); }
function setIdleSearch(idle) {
  const view = $('.view[data-view=sounds]');
  if (view.classList.contains('idle') === idle) return;
  view.classList.toggle('idle', idle);
  clearTimeout(typing);
  const q = $('#q');
  if (!idle) { q.placeholder = PLACEHOLDER; return; }
  $('#hello').textContent = HELLOS[Math.floor(Math.random() * HELLOS.length)];
  if (reduceMotion) { q.placeholder = TRY_PHRASES[Math.floor(Math.random() * TRY_PHRASES.length)]; return; }
  // Type a phrase, hold it, wipe it, then the next one.
  let phrase = '', n = 0, wiping = false;
  const step = () => {
    if (!wiping && n === 0) phrase = TRY_PHRASES[Math.floor(Math.random() * TRY_PHRASES.length)];
    n += wiping ? -2 : 1;
    if (n >= phrase.length) { n = phrase.length; wiping = true; q.placeholder = phrase; typing = setTimeout(step, 1900); return; }
    if (n <= 0) { n = 0; wiping = false; }
    q.placeholder = phrase.slice(0, n) || ' ';
    typing = setTimeout(step, wiping ? 18 : 55 + Math.random() * 50);
  };
  step();
}

function emptyState() {
  if (!hasQuery() && !hasFilter()) return el('div');
  return el('div', { class: 'empty' }, el('h3', {}, 'Nothing matches'));
}

const MINI_BARS = 48, MINI_H = 24;

// The sound's shape as one SVG path.
function miniWave(it) {
  const peaks = it.peaks || [], w = el('span', { class: 'mini', 'aria-hidden': 'true' });
  if (!peaks.length) return w;
  let d = '';
  for (let i = 0; i < MINI_BARS; i++) {
    const a = Math.floor(i * peaks.length / MINI_BARS), b = Math.max(a + 1, Math.floor((i + 1) * peaks.length / MINI_BARS));
    let m = 0; for (let j = a; j < b; j++) m = Math.max(m, peaks[j]);
    const h = Math.max(1, m / 255 * MINI_H);
    d += `M${i + 0.5} ${((MINI_H - h) / 2).toFixed(1)}v${h.toFixed(1)}`;
  }
  w.innerHTML = `<svg viewBox="0 0 ${MINI_BARS} ${MINI_H}" preserveAspectRatio="none"><path d="${d}"/></svg>`;
  return w;
}

// Several sounds can be marked at once with Cmd-click and Shift-click; the bar above the list
// then acts on all of them.
const marked = new Set();
function rowClick(e, i) {
  const it = state.items[i];
  if (e.metaKey || e.ctrlKey) {
    if (!marked.size && state.items[state.selected] && state.selected !== i) marked.add(state.items[state.selected].path);
    marked.has(it.path) ? marked.delete(it.path) : marked.add(it.path);
    return paintMarks();
  }
  if (e.shiftKey && state.selected >= 0) {
    for (let k = Math.min(state.selected, i); k <= Math.max(state.selected, i); k++) marked.add(state.items[k].path);
    return paintMarks();
  }
  if (marked.size) { marked.clear(); paintMarks(); }
  select(i, { scroll: false, play: true });
}
function paintMarks() {
  for (const p of [...marked]) if (!state.items.some(x => x.path === p)) marked.delete(p);
  $('#results').querySelectorAll('.qitem[data-i]').forEach(r => r.classList.toggle('marked', marked.has(state.items[+r.dataset.i]?.path)));
  const bar = $('#markBar'), n = marked.size;
  bar.hidden = !n;
  if (!n) return bar.replaceChildren();
  const items = () => state.items.filter(x => marked.has(x.path));
  // Runs one request per marked sound. A failure stops the run: what was done loses its mark,
  // what was not keeps it, and the message counts only what was done.
  const each = async (fn, done) => {
    let ok = 0;
    for (const it of items()) { try { await fn(it); marked.delete(it.path); ok++; } catch (e) { fail(e); break; } }
    await search();
    if (cardItem) renderCard();
    if (ok) toast(done(ok));
  };
  const proj = el('select', { 'aria-label': 'Add the marked sounds to a project' }, el('option', { value: '' }, state.projects.length ? 'Add to project' : 'No projects yet'), state.projects.map(p => el('option', { value: p.id }, p.name)));
  proj.onchange = () => { const pid = proj.value; if (pid) each(it => api(`/api/projects/${pid}/items`, { json: { path: it.path } }), k => `Copied ${k} sound${k === 1 ? '' : 's'} into the project`).then(() => loadProjects().catch(() => { })); };
  const rate = el('select', { 'aria-label': 'Rate the marked sounds' }, el('option', { value: '' }, 'Rate'), [1, 2, 3, 4, 5, 0].map(k => el('option', { value: k }, k ? `${k}` : 'Clear rating')));
  rate.onchange = () => { if (rate.value !== '') each(it => api('/api/catalog', { json: { path: it.path, rating: +rate.value } }), k => `Rated ${k} sound${k === 1 ? '' : 's'}`); };
  bar.replaceChildren(el('span', { class: 'markn' }, `${n} marked`), proj, rate,
    ib('sell', 'Add tags to the marked sounds', async () => {
      const v = (await modal(`Tag ${n} sounds`, [{ label: 'Tags to add, separated by commas', placeholder: 'dusty, tape' }]))?.[0];
      const add = (v || '').split(',').map(t => t.trim()).filter(Boolean);
      if (add.length) each(it => api('/api/catalog', { json: { path: it.path, tags: [...new Set([...(it.tags || []), ...add])] } }), k => `Tagged ${k} sound${k === 1 ? '' : 's'}`).then(() => loadTags().catch(() => { }));
    }),
    ib('delete', 'Move the marked files to the Trash', async () => {
      if (!await confirmModal(`Delete ${n} files?`, 'This deletes the files themselves, not just their place in Athar. They go to the Trash.', 'Delete')) return;
      stopPreview(); each(it => api('/api/trash', { json: { path: it.path } }), k => `Moved ${k} file${k === 1 ? '' : 's'} to the Trash`).then(() => refreshSide());
    }),
    ib('close', 'Clear the marks', () => { marked.clear(); paintMarks(); }));
}

function row(it, i, width) {
  const activate = (e) => rowClick(e || {}, i);
  const r = el('div', {
    class: 'qitem trow' + (i === state.selected ? ' on' : '') + (isPlaying(it) ? ' playing' : '') + (marked.has(it.path) ? ' marked' : ''),
    'data-i': i, tabindex: i === state.selected ? '0' : '-1', 'data-tip': 'Double-click to edit',
    onclick: activate, ondblclick: () => openPath(it),
    onkeydown: (e) => { if (e.key === 'Enter' && e.target === e.currentTarget) { e.preventDefault(); e.stopPropagation(); activate(); } },
  },
  miniWave(it),
  el('span', { class: 'qn' }, it.name, projectOf(it.path) ? null : subLabel(it)),
  el('span', { class: 'rtags' }, projectOf(it.path) && !state.project ? placeChip(it) : null, (it.tags || []).map(t => el('span', { class: 'rtag' }, t))),
  el('span', { class: 'tc' }, it.category || ''),
  el('span', { class: 'tc num' }, fmtTime(it.duration)),
  el('span', { class: 'tc num' }, (it.recorded || '').slice(0, 10).replace(/:/g, '-')),
  el('span', { class: 'rowrate' }, Array.from({ length: it.rating || 0 }, () => el('i'))));
  if (hasQuery()) r.append(el('span', { class: 'qmatch', style: `width:${width || '10%'}` }));
  return r;
}

// The subfolders a sound sits in, below whatever level is being shown.
function subLabel(it) {
  if (!it.sub) return null;
  const shown = state.folder === it.folder && state.under ? state.under.slice(it.folder.length + 1) : '';
  const rest = shown && (it.sub === shown || it.sub.startsWith(shown + '/')) ? it.sub.slice(shown.length + 1) : it.sub;
  return rest ? el('span', { class: 'qsub' }, rest) : null;
}

function tableHead() {
  const col = (label, key) => {
    if (!key) return el('span', { class: 'th' }, label);
    const on = filters().sort === key;
    const sort = () => { state.sort = on && hasQuery() ? 'score' : key; $('#sort').value = state.sort; search(); };
    const tip = on && hasQuery() ? 'Back to best match first' : `Sort by ${label}`;
    return focusable(el('span', { class: 'th sortable' + (on ? ' on' : ''), role: 'button', 'data-sort': key, onclick: sort, 'data-tip': tip }, label), sort);
  };
  return el('div', { class: 'trow thead' }, el('span'), col('name', 'name'), col('tags'), col('type', 'category'), col('length', 'duration'), col('recorded', 'recorded'), col('rating', 'rating'));
}

function select(i, { scroll = true, play = false } = {}) {
  if (i < 0 || i >= state.items.length) return;
  const list = $('#results'), rows = list.querySelectorAll('.qitem[data-i]');
  const focused = list.contains(document.activeElement);
  const prev = rows[state.selected];
  prev?.classList.remove('on'); prev?.setAttribute('tabindex', '-1');
  state.selected = i;
  const r = rows[i];
  r?.classList.add('on'); r?.setAttribute('tabindex', '0');
  if (focused) r?.focus({ preventScroll: true });
  if (scroll) r?.scrollIntoView({ block: 'nearest' });
  const next = state.items[i], changed = cardItem?.path !== next.path;
  if (changed) { cardItem = next; renderCard(); }
  else if (cardItem !== next) { Object.assign(cardItem, next); state.items[i] = cardItem; }
  if (play || (scroll && changed && SETTINGS.autoplay)) togglePreview(cardItem, true);
}

async function removeFromProject(it) {
  const pid = state.project;
  if (!pid) return;
  try { await api(`/api/projects/${pid}/items`, { json: { path: it.path, remove: true } }); }
  catch (e) { return fail(e); }
  loadProjects().catch(() => { });
  search();
}

// ---------- preview player
const player = new Audio();
const isPlaying = (it) => !!it && playingItem?.path === it.path;
function togglePreview(it, forceStart = false) {
  ctx.resume();
  if (isPlaying(it) && !forceStart) { stopPreview(); return; }
  stopPreview();
  player.loop = libLoop;
  if (editor.playing) editor.stopPlay();
  editor.stopAudition(); arranger.stop();
  playingItem = it;
  const decoded = `/api/decoded?path=${encodeURIComponent(it.path)}`;
  player.src = `/api/audio?path=${encodeURIComponent(it.path)}`;
  player.onerror = () => {
    if (playingItem !== it) return;
    if (!player.src.includes('/api/decoded')) {
      player.src = decoded; player.play().catch(() => { });
      const cap = SETTINGS._preview_max_s;
      if (cap && it.duration > cap) notice(`Preview plays the first ${Math.round(cap / 60)} minutes of ${it.name}. Open it in the editor to hear all of it.`, 7000);
    }
    else { stopPreview(); notice(`Could not play ${it.name}`); }
  };
  player.play().catch(() => { });
  markPlaying();
  renderCardPlayState();
}
function stopPreview() {
  player.pause();
  const was = playingItem;
  playingItem = null;
  if (was) markPlaying();
  renderCardPlayState();
  drawCardWave();
}
function markPlaying() {
  $('#results').querySelectorAll('.qitem[data-i]').forEach((r) => {
    r.classList.toggle('playing', isPlaying(state.items[r.dataset.i]));
  });
  document.querySelectorAll('.pad').forEach((c) => c.classList.toggle('playing', !!c.padPath && playingItem?.path === c.padPath));
  // The play buttons of a project's sounds show stop while their sound plays.
  document.querySelectorAll('.pplay').forEach((b) => {
    const on = playingItem?.path === b.playPath, tip = on ? 'Stop' : 'Play';
    b.querySelector('.i').textContent = on ? 'stop' : 'play_arrow';
    b.dataset.tip = tip; b.setAttribute('aria-label', tip);
  });
}
player.onended = () => stopPreview();
(function tick() {
  if (player.duration && isPlaying(cardItem)) drawCardWave();
  requestAnimationFrame(tick);
})();

// ---------- volume: the same ruler as the editor's tools
const volume = () => SETTINGS.preview_volume ?? 80;
function setVolume(v, save = true) {
  v = Math.round(Math.max(0, Math.min(100, v)));
  // The tile lifts off its shadow as the volume rises, and sits flat on the page at zero.
  const tile = $('#knob'), lift = v / 100;
  tile.textContent = v; tile.setAttribute('aria-valuenow', v);
  tile.style.boxShadow = v ? `${(lift * 14).toFixed(1)}px ${(lift * 14).toFixed(1)}px 0 var(--shade)` : 'none';
  tile.style.transform = `translate(${(-lift * 7).toFixed(1)}px, ${(-lift * 7).toFixed(1)}px)`;
  tile.dataset.tip = 'Volume';
  player.volume = v / 100;
  out.gain.value = v / 100;
  discover?.setVolume(v / 100); recorder?.setVolume(v / 100);
  if (save && SETTINGS.preview_volume !== v) {
    saveSetting('preview_volume', v);
    const s = $('#set-preview_volume'); if (s) { s.value = v; s.nextElementSibling.textContent = `${v}%`; }
  }
}
{
  const tile = $('#knob');
  // The tile sits in the top right corner, so the drag runs toward the open side of the window:
  // pulling it left or up, away from its shadow, is louder; pushing it down or right is quieter.
  tile.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    e.preventDefault(); tile.setPointerCapture(e.pointerId); tile.focus();
    const start = volume(), x0 = e.clientX, y0 = e.clientY;
    const move = (ev) => setVolume(start - ((ev.clientX - x0) + (ev.clientY - y0)));
    const up = () => { for (const t of ['pointerup', 'pointercancel', 'lostpointercapture']) tile.removeEventListener(t, up); tile.removeEventListener('pointermove', move); };
    tile.addEventListener('pointermove', move); for (const t of ['pointerup', 'pointercancel', 'lostpointercapture']) tile.addEventListener(t, up);
  });
  tile.addEventListener('keydown', (e) => {
    const d = { ArrowLeft: 2, ArrowUp: 2, ArrowRight: -2, ArrowDown: -2, Home: -100, End: 100 }[e.key];
    if (d) { e.preventDefault(); e.stopPropagation(); setVolume(volume() + d); }
  });
  tile.addEventListener('wheel', (e) => { e.preventDefault(); setVolume(volume() - Math.sign(e.deltaY) * 4); }, { passive: false });
  tile.addEventListener('dblclick', () => setVolume(80));
}

// ---------- status: only problems are shown
let lastJobs = 0, lastModel = null;
// A standing problem takes the place of the progress in the bottom bar until it is gone.
let problem = '';
function showProblem(text) {
  problem = text;
  if (!text) return;
  $('#indexing').hidden = false; document.body.classList.add('indexing');
  $('#indexingText').textContent = text; $('#indexingBar').hidden = true; $('#indexing .spin').hidden = true;
}
async function poll() {
  try {
    const s = await api('/api/status');
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
let qTimer;
function runQuery() {
  clearTimeout(qTimer);
  state.query = $('#q').value.trim();
  if (state.query) { state.sort = 'score'; $('#sort').value = 'score'; }
  search();
}
$('#q').addEventListener('input', () => { clearTimeout(qTimer); qTimer = setTimeout(runQuery, 400); });
$('#q').addEventListener('keydown', (e) => { if (e.key === 'Enter') runQuery(); });
$('#filterBtn').onclick = () => {
  const f = $('#filters'), b = $('#filterBtn');
  f.hidden = !f.hidden;
  b.classList.toggle('on', !f.hidden); b.setAttribute('aria-expanded', String(!f.hidden));
};
$('#tagFilter').addEventListener('change', (e) => { state.tag = e.target.value || null; search(); });
$('#ratingFilter').addEventListener('change', (e) => { state.minRating = Number(e.target.value) || null; search(); });
$('#catFilter').addEventListener('change', (e) => { state.category = e.target.value || null; loadCats(); search(); });
$('#sort').addEventListener('change', (e) => { state.sort = e.target.value; search(); });

$('#addFolder').onclick = async () => {
  if (window.pywebview?.api?.pick_folder) {
    const p = await window.pywebview.api.pick_folder();
    if (p) addFolder(p);
  } else { $('#folderAdd').hidden = !$('#folderAdd').hidden; $('#folderInput').focus(); }
};
async function addFolder(p) {
  try {
    await api('/api/folders', { json: { path: p } });
    $('#folderAdd').hidden = true; $('#folderInput').value = '';
    loadFolders();
  } catch (e) { fail(e); }
}
$('#folderOk').onclick = () => addFolder($('#folderInput').value.trim());
$('#folderInput').onkeydown = (e) => { if (e.key === 'Enter') $('#folderOk').click(); };
$('#rescan').onclick = async () => {
  try { await api('/api/rescan', { method: 'POST' }); } catch (e) { return fail(e); }
  toast('Looking for new and changed files');
  loadFolders().catch(fail);
  setTimeout(() => loadFolders().catch(() => { }), 2500);
};

const projectForm = (open) => { $('#projectAdd').hidden = !open; if (open) $('#projectInput').focus(); };
$('#newProject').onclick = () => projectForm($('#projectAdd').hidden);
$('#projectOk').onclick = async () => {
  const name = $('#projectInput').value.trim(); if (!name) return;
  try {
    const p = await api('/api/projects', { json: { name } });
    $('#projectInput').value = ''; projectForm(false);
    projSel = p.id; projShut = false;
    await loadProjects();
    if (p.folder) notice(`${p.name} keeps its sounds in ${p.folder}`, 6000);
    else if (p.folder_error) notice(`${p.name} has no folder yet: ${p.folder_error}`, 8000);
    refreshSide();
  } catch (e) { fail(e); }
};
$('#projectInput').onkeydown = (e) => { if (e.key === 'Enter') $('#projectOk').click(); else if (e.key === 'Escape') { e.stopPropagation(); $('#projectInput').value = ''; projectForm(false); } };

window.atharDropped = ({ added, problems }) => {
  if (added.length) { refreshSide(); notice(`Added ${added.join(', ')} to your library`); }
  if (problems.length) notice(problems.join(' '), 7000);
};
window.addEventListener('dragover', (e) => e.preventDefault());
window.addEventListener('drop', (e) => e.preventDefault());

// ---------- settings: stored by the app, applied everywhere
const ACCENTS = [
  ['#7a3dff', 'Ultraviolet'], ['#bc13fe', 'Neon purple'], ['#1f51ff', 'Neon blue'], ['#00f0ff', 'Neon cyan'],
  ['#39ff14', 'Neon green'], ['#e7ff00', 'Neon yellow'], ['#ff6a00', 'Neon orange'], ['#ff073a', 'Neon red'],
  ['#ff10f0', 'Neon pink'],
];
const SETTINGS_SCHEMA = [
  ['Display', [
    ['theme', 'Appearance', 'select', { options: [['system', 'Match the Mac'], ['light', 'Light'], ['dark', 'Dark']] }],
    ['text_size', 'Text size', 'range', { min: 11, max: 20, step: 1, unit: 'px', onRelease: true }],
    ['show_cat', 'Show the cat at the top of the page', 'toggle'],
    ['cat_skin', 'Cat', 'select', { options: Object.entries(CAT_SKINS).map(([k, v]) => [k, v.name]) }],
    ['accent', 'Accent colour', 'swatch', { options: ACCENTS }],
  ]],
  ['Listening', [
    ['preview_volume', 'Volume', 'range', { min: 0, max: 100, step: 1, unit: '%' }],
    ['autoplay', 'Play each sound as you move with the arrow keys', 'toggle'],
  ]],
  ['Search', [
    ['default_sort', 'Sort when not searching', 'select', { options: [['name', 'Name'], ['duration', 'Length'], ['rating', 'Highest rated'], ['recorded', 'Newest recording']] }],
  ]],
  ['Library', [
    ['rescan_on_open', 'Look for new and changed files when Athar opens', 'toggle'],
  ]],
  ['Discover', [
    ['freesound_key', 'Freesound API key', 'text', { hint: 'Adds Freesound as an archive. Free key at freesound.org/apiv2/apply' }],
  ]],
  ['Projects', [
    ['projects_dir', 'New projects keep their sounds in', 'folder', { hint: 'Each new project gets its own folder inside this one' }],
  ]],
  ['Editor', [
    ['edit_suffix', 'Added to the name of saved edits', 'text'],
    ['smooth_edits', 'Smooth every cut and paste with a short crossfade', 'toggle'],
    ['edit_fade_ms', 'Length of that crossfade', 'number', { min: 1, max: 50, step: 1, unit: 'ms' }],
    ['loudness_target', 'Loudness target', 'number', { min: -36, max: -6, step: 0.5, unit: 'LUFS' }],
    ['silence_db', 'Silence is anything below', 'number', { min: -90, max: -10, step: 1, unit: 'dB' }],
    ['slice_gap_ms', 'Auto-slice joins sounds closer than', 'number', { min: 20, max: 3000, step: 10, unit: 'ms' }],
  ]],
  ['Sampler prep', [
    ['prep_target_db', 'Normalize to', 'number', { min: -12, max: 0, step: 0.5, unit: 'dB' }],
    ['prep_fade_in_ms', 'Fade in', 'number', { min: 0, max: 100, step: 1, unit: 'ms' }],
    ['prep_fade_out_ms', 'Fade out', 'number', { min: 0, max: 500, step: 1, unit: 'ms' }],
  ]],
  ['Export', [
    ['export_format', 'File format', 'select', { options: [['wav', 'WAV'], ['flac', 'FLAC'], ['mp3', 'MP3']] }],
    ['export_rate', 'Sample rate', 'select', { options: [[0, 'Same as original'], [48000, '48 kHz'], [44100, '44.1 kHz'], [32000, '32 kHz'], [24000, '24 kHz'], [22050, '22.05 kHz'], [16000, '16 kHz']] }],
    ['export_bits', 'Bit depth', 'select', { options: [[16, '16-bit'], [24, '24-bit'], [32, '32-bit float']] }],
    ['export_mono', 'Export as mono', 'toggle'],
    ['name_pattern', 'Project file names', 'text', { hint: '{project} {category} {n:03d} {name}' }],
  ]],
  ['SP-404MKII', [
    ['sp404_mono', 'Convert to mono (saves sample memory)', 'toggle'],
  ]],
];
// Settings that seed the editor's own fields; written only when they change so per-file choices survive.
const EDITOR_FIELDS = { export_rate: 'rate', export_bits: 'bits', export_format: 'format', prep_target_db: 'norm', silence_db: 'thresh', slice_gap_ms: 'gap', loudness_target: 'lufs' };

// One accent colour, used at full strength everywhere; soft backgrounds are neutral grey.
function applyAccent(hex) {
  if (!/^#[0-9a-f]{6}$/i.test(hex || '')) hex = ACCENTS[0][0];
  const c = [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16));
  const r = document.documentElement.style;
  for (const v of ['--accent', '--hi', '--shade']) r.setProperty(v, hex);
  // Bright neons need dark text on top and a darker shade when the accent itself is used for text.
  const lum = c.map(v => { v /= 255; return v <= .04045 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4; });
  const light = .2126 * lum[0] + .7152 * lum[1] + .0722 * lum[2] > .35;
  const dark = document.documentElement.dataset.theme === 'dark';
  r.setProperty('--on-accent', light ? '#24272b' : '#fbfbfa');
  // Accent used as text: darkened on paper when it is a bright neon, lightened on a dark page when it is deep.
  const mix = (k, to) => '#' + c.map(v => Math.round(v + (to - v) * k).toString(16).padStart(2, '0')).join('');
  r.setProperty('--accent-text', dark ? (light ? hex : mix(.45, 255)) : (light ? mix(.45, 0) : hex));
}
// Share Tech Mono draws smaller than its nominal size, so the Text size setting is scaled up to match.
const TYPE_SCALE = 1.1;
const darkQuery = window.matchMedia('(prefers-color-scheme: dark)');
darkQuery.addEventListener?.('change', () => { if ((SETTINGS.theme || 'system') === 'system') applySettings(); });
function applySettings() {
  const st = SETTINGS;
  const theme = st.theme === 'dark' || (st.theme !== 'light' && darkQuery.matches) ? 'dark' : 'light';
  if (document.documentElement.dataset.theme !== theme) { document.documentElement.dataset.theme = theme; requestAnimationFrame(() => { editor.layerKey = null; editor.draw(); editor.fx?.redraw?.(); soundmap.draw(); recorder.drawWave(recorder.rec?.peaks || recorder.takes[0]?.peaks || []); }); }
  if (st._version) document.title = `Athar ${st._version}`;
  document.documentElement.style.setProperty('--fs', st.text_size * TYPE_SCALE + 'px');
  setVolume(volume(), false);
  editor.settings = st;
  applyAccent(st.accent);
  $('#cat').hidden = !st.show_cat;
  setCatSkin(st.cat_skin);
  editor.draw();
}
function applyEditorFields(keys = Object.keys(EDITOR_FIELDS)) {
  for (const k of keys) {
    const n = $(`#editor [data-f=${EDITOR_FIELDS[k]}]`);
    if (n && SETTINGS[k] != null) n.value = String(SETTINGS[k]);
  }
}

let saveTimer, pending = {};
function saveSetting(key, value) {
  SETTINGS[key] = value;
  pending[key] = value;
  applySettings();
  if (key in EDITOR_FIELDS) applyEditorFields([key]);
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    const batch = pending; pending = {};
    api('/api/settings', { json: batch }).then(() => {
      if (['results_limit', 'default_sort', 'hide_shorter_ms'].some(k => k in batch)) search();
    }).catch(fail);
  }, 250);
}

function swatchControl(key, label, options) {
  const buttons = options.map(([v, l]) => el('button', { type: 'button', class: 'swatch', style: `--sw: ${v}`, 'data-v': v, 'data-tip': l, 'aria-label': l, role: 'radio' }));
  // Any colour at all: the system colour panel behind a rainbow dot, which shows the pick once made.
  const picker = el('input', { type: 'color', 'aria-label': 'Pick any colour' });
  const pick = el('label', { class: 'swatch-pick', 'data-tip': 'Pick any colour' }, picker);
  const mark = (v) => {
    const any = buttons.some(b => b.dataset.v === v);
    pick.classList.toggle('on', !any && /^#[0-9a-f]{6}$/.test(v));
    if (!any && /^#[0-9a-f]{6}$/.test(v)) { pick.style.setProperty('--sw', v); picker.value = v; }
    buttons.forEach((b, k) => {
      const on = b.dataset.v === v;
      b.classList.toggle('on', on); b.setAttribute('aria-checked', String(on));
      b.tabIndex = on || (!any && k === 0) ? 0 : -1;
    });
  };
  const choose = (b) => { clearTimeout(saveTimer); mark(b.dataset.v); saveSetting(key, b.dataset.v); };
  buttons.forEach((b, k) => {
    b.onclick = () => choose(b);
    b.onkeydown = (e) => {
      const d = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
      if (!d) return;
      e.preventDefault();
      const n = buttons[(k + d + buttons.length) % buttons.length];
      n.focus(); choose(n);
    };
  });
  let saveTimer = 0;
  picker.oninput = () => {
    const v = picker.value.toLowerCase();
    mark(v); applyAccent(v);
    clearTimeout(saveTimer); saveTimer = setTimeout(() => saveSetting(key, v), 250);
  };
  mark(String(SETTINGS[key] || '').toLowerCase());
  return el('span', { class: 'swatches', role: 'radiogroup', 'aria-label': label }, buttons, pick);
}

function settingControl(key, id, kind, o) {
  if (kind === 'toggle') {
    const c = el('input', { type: 'checkbox', id, class: 'switch' });
    c.checked = !!SETTINGS[key];
    c.onchange = () => saveSetting(key, c.checked);
    return c;
  }
  if (kind === 'select') {
    const s = el('select', { id }, o.options.map(([v, l]) => el('option', { value: v }, l)));
    s.value = String(SETTINGS[key]);
    s.onchange = () => saveSetting(key, typeof o.options[0][0] === 'number' ? Number(s.value) : s.value);
    return s;
  }
  if (kind === 'range') {
    const out = el('output', {}, `${SETTINGS[key]}${o.unit}`);
    const r = el('input', { type: 'range', id, min: o.min, max: o.max, step: o.step });
    r.value = SETTINGS[key];
    // A setting that resizes the page is applied when the ruler is let go, so the ruler does not move under the pointer.
    r.oninput = () => { out.textContent = `${r.value}${o.unit}`; if (!o.onRelease) saveSetting(key, Number(r.value)); };
    if (o.onRelease) r.onchange = () => saveSetting(key, Number(r.value));
    return el('span', { class: 'rangewrap' }, r, out);
  }
  if (kind === 'number') {
    const n = el('input', { type: 'number', id, min: o.min, max: o.max, step: o.step });
    n.value = SETTINGS[key];
    n.onchange = () => {
      const raw = Number(n.value);
      if (n.value === '' || !Number.isFinite(raw)) { n.value = SETTINGS[key]; return; }
      const v = Math.min(o.max, Math.max(o.min, raw));
      n.value = v; saveSetting(key, v);
    };
    return el('span', { class: 'numwrap' }, n, el('span', { class: 'unit' }, o.unit));
  }
  const t = el('input', { id, spellcheck: 'false', placeholder: o.hint || '' });
  t.value = SETTINGS[key] ?? '';
  t.onchange = () => saveSetting(key, t.value);
  return t;
}

function renderSettings() {
  const box = $('#settingsForm'); box.innerHTML = '';
  for (const [group, rows] of SETTINGS_SCHEMA) {
    const sec = el('div', { class: 'sgroup' }, el('h2', {}, group));
    for (const [key, label, kind, o = {}] of rows) {
      const id = 'set-' + key;
      const name = el('span', { class: 'slabel' }, label, o.hint ? el('small', {}, o.hint) : null);
      sec.append(kind === 'folder'
        ? el('div', { class: 'srow' }, name, el('span', { class: 'sfolder' }, el('span', { class: 'mono', 'data-tip': SETTINGS[key] }, SETTINGS[key]),
          el('button', { class: 'ghost', onclick: async () => { const d = await pickFolder(label); if (d) { saveSetting(key, d); renderSettings(); } } }, 'Change')))
        : kind === 'swatch'
        ? el('div', { class: 'srow' }, name, swatchControl(key, label, o.options))
        : el('label', { class: 'srow', for: id }, name, settingControl(key, id, kind, o)));
    }
    box.append(sec);
  }
  box.append(el('div', { class: 'sgroup' }, el('h2', {}, 'Backup'),
    el('div', { class: 'srow' }, el('span', { class: 'slabel' }, 'Your tags, ratings, notes, regions and projects', el('small', {}, 'One file you can keep somewhere safe, or carry to another Mac')),
      el('span', { class: 'sfolder' },
        el('button', { class: 'ghost', onclick: async () => {
          const dir = await pickFolder('Save the backup into'); if (!dir) return;
          try { notice(`Backup saved: ${(await api('/api/backup', { json: { path: dir } })).path}`, 8000); } catch (e) { fail(e); }
        } }, 'Back up'),
        el('button', { class: 'ghost', onclick: async () => {
          const file = window.pywebview?.api?.pick_file ? await window.pywebview.api.pick_file() : (await modal('Restore from a backup', [{ label: 'Backup file', placeholder: '/Users/you/Athar backup 2026-11-13.json' }]))?.[0];
          if (!file) return;
          if (!await confirmModal('Restore this backup?', 'Tags, ratings and notes in the file replace the current ones for the sounds it names. Projects in the file are added.', 'Restore')) return;
          try { const r = await api('/api/restore', { json: { path: file } }); notice(`Restored ${r.sounds} sounds and ${r.projects} projects`); refreshSide(); search(); loadTags().catch(() => { }); } catch (e) { fail(e); }
        } }, 'Restore')))));
  box.append(el('div', { class: 'sgroup' }, el('h2', {}, 'About'),
    el('div', { class: 'srow' }, el('span', { class: 'slabel' }, 'Version'), el('span', { class: 'mono' }, SETTINGS._version || '')),
    el('div', { class: 'srow' }, el('span', { class: 'slabel' }, el('span', { id: 'statusRight' }), el('small', {}, `Library and settings: ${SETTINGS._data_dir || ''}`)),
      ib('folder_open', 'Show the library data in Finder', () => api('/api/reveal-data', { method: 'POST' }).catch(fail)))));
}

// ---------- the sound card (top of the Library)
const extOf = (n) => (n.match(/\.[^.]+$/) || [''])[0];
const stemOf = (n) => n.replace(/\.[^.]+$/, '');

async function loadTags() {
  allTags = await api('/api/tags');
  const gone = state.tag && !allTags.some(t => t.tag === state.tag);
  if (gone) state.tag = null;
  const sel = $('#tagFilter');
  sel.innerHTML = '';
  sel.append(el('option', { value: '' }, 'any'));
  allTags.forEach(t => sel.append(el('option', { value: t.tag }, `${t.tag} (${t.count})`)));
  sel.value = state.tag || '';
  if (gone) search();
}

function drawCardWave() {
  const cv = $('#card canvas.big'); if (!cv || !cardItem) return;
  const g = cv.getContext('2d'), W = cv.width, H = cv.height;
  const css = getComputedStyle(document.documentElement);
  const p = isPlaying(cardItem) && player.duration ? player.currentTime / player.duration : 0;
  g.clearRect(0, 0, W, H);
  const peaks = cardItem.peaks || [], n = peaks.length || 1, bw = W / n;
  for (let i = 0; i < n; i++) {
    const h = Math.max(2, peaks[i] / 255 * (H - 16));
    g.fillStyle = i / n < p ? css.getPropertyValue('--hi') : css.getPropertyValue('--wave');
    g.fillRect(i * bw + 1, (H - h) / 2, Math.max(1, bw - 2), h);
  }
}

// Resolves to true when the saved item is still on the card, false when it moved on, null on failure.
async function saveCatalog(changes, path = cardItem?.path) {
  if (!path) return false;
  try {
    const r = await api('/api/catalog', { json: { path, ...changes } });
    const fresh = { tags: r.tags, memo: r.memo, rating: r.rating };
    const i = state.items.findIndex(x => x.path === path);
    if (i >= 0) {
      Object.assign(state.items[i], fresh);
      const n = $('#results').querySelector(`.qitem[data-i="${i}"]`);
      if (n) {
        const fresher = row(state.items[i], i, n.querySelector('.qmatch')?.style.width);
        const focused = n === document.activeElement;
        n.replaceWith(fresher);
        if (focused) fresher.focus({ preventScroll: true });
      }
    }
    if (cardItem?.path === path) Object.assign(cardItem, fresh);
    if ('tags' in changes) loadTags().catch(() => { });
    return cardItem?.path === path;
  } catch (e) { fail(e); return null; }
}

async function renameCard(stem) {
  const target = cardItem;
  if (!target || !stem.trim() || stem === stemOf(target.name)) return;
  const old = target.path;
  try {
    const { path } = await api('/api/rename', { json: { path: old, name: stem.trim() } });
    const name = path.split('/').pop();
    for (const x of [target, ...state.items]) if (x.path === old) Object.assign(x, { path, name });
    renderList();
    if (cardItem === target) renderCard();
  } catch (e) { fail(e); if (cardItem === target) renderCard(); }
}

function setRating(k) {
  const it = cardItem;
  if (!it) return;
  saveCatalog({ rating: k }, it.path).then(here => { if (here) renderCard(); });
}

let memoTimer, memoFlush = null, cardSuggest = null;
function renderCard() {
  memoFlush?.();
  cardSuggest?.close(); cardSuggest = null;
  const box = $('#card'); box.innerHTML = '';
  const it = cardItem;
  box.hidden = !it;
  if (!it) return;
  const meta = el('div', { class: 'meta-line' },
    el('span', {}, fmtTime(it.duration)),
    el('span', {}, `${it.sr ? (it.sr / 1000) + ' kHz' : ''} ${it.channels === 1 ? 'mono' : 'stereo'}`),
    el('span', {}, extOf(it.name).replace('.', '').toUpperCase()),
    placeChip(it), originChip(it));
  const cv = el('canvas', { class: 'big', width: 1200, height: 240, 'data-tip': 'Click to play from that point' });
  cv.onclick = (e) => {
    const r = cv.getBoundingClientRect(), f = (e.clientX - r.left) / r.width;
    const seek = () => { if (player.duration) player.currentTime = f * player.duration; };
    if (!isPlaying(it)) { togglePreview(it, true); player.addEventListener('loadedmetadata', seek, { once: true }); } else seek();
  };

  const nameIn = el('input', { id: 'cardName', spellcheck: 'false', 'aria-label': 'File name' });
  nameIn.value = stemOf(it.name);
  nameIn.onkeydown = (e) => {
    if (e.key === 'Enter') { e.preventDefault(); nameIn.blur(); }
    if (e.key === 'Escape') { nameIn.value = stemOf(it.name); nameIn.blur(); }
  };
  nameIn.onblur = () => renameCard(nameIn.value);
  const name = el('label', { class: 'bigname', 'data-tip': 'Click to rename the file' }, nameIn);

  const rating = el('div', { class: 'rating', role: 'group', 'aria-label': 'Rating' });
  for (let k = 1; k <= 5; k++) {
    rating.append(el('button', { type: 'button', class: k <= (it.rating || 0) ? 'on' : '', 'data-tip': `Rate ${k} (key ${k})`, 'aria-label': `Rate ${k}`,
      onclick: () => setRating(k === it.rating ? 0 : k) }));
  }

  const tagIn = el('input', { id: 'cardTags', autocomplete: 'off', placeholder: it.tags?.length ? '' : 'Tags: place, device, mood…', spellcheck: 'false', 'aria-label': 'Add tag' });
  const setTags = (tags, refocus = false) => {
    const prev = it.tags;
    it.tags = tags;
    saveCatalog({ tags }, it.path).then((here) => {
      if (here === null) { it.tags = prev; if (cardItem === it) renderCard(); }
      else if (here) { renderCard(); if (refocus) $('#cardTags')?.focus(); }
    });
  };
  const tagbox = el('div', { class: 'tagbox', onclick: () => tagIn.focus() },
    (it.tags || []).map(t => el('span', { class: 'tchip' }, t, el('button', { type: 'button', 'data-tip': `Remove ${t}`, 'aria-label': `Remove ${t}`,
      onclick: (e) => { e.stopPropagation(); setTags(it.tags.filter(x => x !== t)); } }, icon('close')))), tagIn);
  const addTags = () => {
    const parts = tagIn.value.split(',').map(x => x.trim()).filter(Boolean);
    if (!parts.length) return;
    tagIn.value = '';
    setTags([...(it.tags || []), ...parts], true);
  };
  const sug = tagSuggest(tagIn, () => allTags.map(t => t.tag).filter(t => !(it.tags || []).includes(t)), (t) => { tagIn.value = t; addTags(); });
  cardSuggest = sug;
  tagIn.onkeydown = (e) => {
    if (sug.key(e)) return;
    if (e.key === 'Enter' || e.key === ',') { e.preventDefault(); addTags(); }
    else if (e.key === 'Backspace' && !tagIn.value && it.tags?.length) setTags(it.tags.slice(0, -1), true);
    else if (e.key === 'Escape') tagIn.blur();
  };
  tagIn.onblur = () => { sug.close(); if (tagIn.value.trim()) addTags(); };

  const memo = el('textarea', { id: 'cardMemo', placeholder: 'Note: where, when, what you hear, ideas for it…', 'aria-label': 'Note' });
  memo.value = it.memo || '';
  memo.oninput = () => {
    const path = it.path, text = memo.value;
    clearTimeout(memoTimer);
    memoFlush = () => { clearTimeout(memoTimer); memoFlush = null; saveCatalog({ memo: text }, path); };
    memoTimer = setTimeout(memoFlush, 500);
  };
  memo.onblur = () => memoFlush?.();
  memo.onkeydown = (e) => { if (e.key === 'Escape') memo.blur(); };

  const proj = el('select', { id: 'cardProject', 'aria-label': 'Project' },
    el('option', { value: '' }, state.projects.length ? 'Add to project' : 'No projects yet'),
    state.projects.map(p => el('option', { value: p.id }, p.name)));
  proj.onchange = async () => {
    const pid = proj.value;
    if (!pid) return;
    proj.value = '';
    try {
      await api(`/api/projects/${pid}/items`, { json: { path: it.path } });
      loadProjects().catch(() => { });
      const p = state.projects.find(x => String(x.id) === String(pid));
      notice(`Copied into the project ${p?.name || ''}. The original stays in the Library.`);
      setTimeout(() => refreshSide(), 2500);
    }
    catch (e) { fail(e); }
  };

  const actions = el('div', { class: 'actions' },
    ib(isPlaying(it) ? 'stop' : 'play_arrow', 'Play', () => playStop(it), 'strong lplay'),
    ib('repeat', 'Loop', () => { libLoop = !libLoop; player.loop = libLoop; renderCardPlayState(); }, 'toggle lloop' + (libLoop ? ' on' : '')),
    proj,
    el('span', { class: 'spacer' }),
    state.project ? ib('playlist_remove', 'Remove from this project', () => removeFromProject(it)) : null,
    ib('folder_open', 'Show in Finder', () => api('/api/reveal', { json: { path: it.path } }).catch(fail)),
    ib('delete', 'Delete file (Backspace)', () => trashSound(it)),
    el('button', { class: 'primary', onclick: () => openPath(it), 'data-tip': 'Open in the editor (E, or double-click in the list)' }, icon('edit'), ' Edit'));

  box.append(
    el('div', { class: 'stage-main' }, el('div', { class: 'stage-head' }, name, rating), meta, cv, actions),
    el('div', { class: 'stage-side' }, tagbox, memo));
  renderCardPlayState();
  drawCardWave();
}

// Tag suggestions in the app's own menu style instead of the browser's datalist.
function tagSuggest(input, options, pick) {
  let menu = null, items = [], active = -1;
  const close = () => { menu?.remove(); menu = null; active = -1; };
  const mark = (i) => {
    if (!menu) return;
    const n = menu.children;
    n[active]?.classList.remove('active');
    active = (i + n.length) % n.length;
    n[active]?.classList.add('active'); n[active]?.scrollIntoView({ block: 'nearest' });
  };
  const show = () => {
    const q = input.value.trim().toLowerCase();
    items = options().filter(t => !q || t.toLowerCase().includes(q)).slice(0, 12);
    close();
    if (!items.length || (items.length === 1 && items[0].toLowerCase() === q)) return;
    menu = el('div', { class: 'dd-menu tags', role: 'listbox' }, items.map((t) => el('div', { class: 'dd-item', role: 'option',
      onmousedown: (e) => { e.preventDefault(); close(); pick(t); } }, t)));
    document.body.append(menu);
    const r = input.closest('.tagbox')?.getBoundingClientRect() || input.getBoundingClientRect();
    menu.style.left = r.left + 'px'; menu.style.top = (r.bottom + 6) + 'px'; menu.style.minWidth = r.width + 'px';
  };
  input.addEventListener('input', show);
  input.addEventListener('focus', show);
  return {
    close,
    key(e) {
      if (!menu) return false;
      if (e.key === 'ArrowDown') { e.preventDefault(); mark(active + 1); return true; }
      if (e.key === 'ArrowUp') { e.preventDefault(); mark(active - 1); return true; }
      if (e.key === 'Escape') { e.preventDefault(); close(); return true; }
      if (e.key === 'Enter' && active >= 0) { e.preventDefault(); const t = items[active]; close(); pick(t); return true; }
      return false;
    },
  };
}

function playStop(it) {
  if (isPlaying(it)) stopPreview(); else togglePreview(it, true);
}
function renderCardPlayState() {
  const b = $('#card .actions .ib.lplay'), l = $('#card .actions .ib.lloop');
  if (b) {
    const here = isPlaying(cardItem);
    b.querySelector('.i').textContent = here ? 'stop' : 'play_arrow';
    const t = here ? 'Stop' : 'Play';
    b.dataset.tip = t; b.setAttribute('aria-label', t);
  }
  if (l) {
    l.classList.toggle('on', libLoop);
    const t = libLoop ? 'Loop is on' : 'Loop';
    l.dataset.tip = t; l.setAttribute('aria-label', t); l.setAttribute('aria-pressed', String(libLoop));
  }
}

function soundsKey(e) {
  if (e.ctrlKey || e.altKey || (e.metaKey && e.key !== 'Backspace')) return;
  const k = e.key.toLowerCase(), it = cardItem;
  if (k === 'j' || e.key === 'ArrowDown') { e.preventDefault(); select(Math.min(state.items.length - 1, state.selected + 1)); }
  else if (k === 'k' || e.key === 'ArrowUp') { e.preventDefault(); select(Math.max(0, state.selected - 1)); }
  else if (e.key === '/') { e.preventDefault(); $('#q').focus(); }
  else if (e.key === 'Escape') stopPreview();
  else if (!it) return;
  else if (k === ' ') { e.preventDefault(); togglePreview(it); }
  else if (/^[0-5]$/.test(k)) setRating(Number(k));
  else if (k === 'r') { e.preventDefault(); $('#cardName')?.select(); }
  else if (k === 't') { e.preventDefault(); $('#cardTags')?.focus(); }
  else if (k === 'n') { e.preventDefault(); $('#cardMemo')?.focus(); }
  else if (k === 'e') openPath(it);
  else if (e.key === 'Backspace' || e.key === 'Delete') { e.preventDefault(); trashSound(it); }
}

const SHORTCUTS = {
  sounds: ['Library', [['J or ↓', 'Next sound'], ['K or ↑', 'Previous sound'], ['Space', 'Play or stop'], ['/', 'Jump to the search box'], ['1 to 5', 'Rate the sound, 0 clears'],
    ['E', 'Open in the editor'], ['R', 'Rename the file'], ['T', 'Edit tags'], ['N', 'Edit the note'], ['Backspace', 'Move to the Trash, after asking'], ['Escape', 'Stop playback']]],
  discover: ['Discover', [['/', 'Jump to the search box'], ['Tab', 'Take the idea being typed'], ['Enter', 'Search'], ['J or ↓', 'Next result'], ['K or ↑', 'Previous result'],
    ['Space', 'Play or stop, or open a collection'], ['A', 'Add to a project'], ['Escape', 'Stop playback']]],
  editor: ['Editor', [['Space', 'Play or stop'], ['L', 'Loop on or off'], ['Cmd Z', 'Undo'], ['Shift Cmd Z', 'Redo'], ['Cmd A', 'Select all'], ['Cmd C, X, V', 'Copy, cut, paste'],
    ['Cmd T', 'Trim to the selection'], ['Backspace', 'Delete the selection'], ['+ and -', 'Zoom in and out'], ['F', 'Fit the whole sound'], ['E', 'Zoom to the selection'],
    ['R', 'Add a region from the selection'], ['B', 'Switch between Original and Edited'], ['Cmd S', 'Save as a new file'], ['Home, End', 'Cursor to start or end'], ['Escape', 'Clear the selection']]],
  arranger: ['Arranger', [['Space', 'Play or stop'], ['L', 'Loop on or off'], ['B', 'Hide or show the browser'], ['Double-click', 'Edit the clip\'s sound'], ['Alt drag', 'Drag a copy of the clip'], ['Cmd drag', 'Move without catching on other clips'],
    ['Cmd C, X, V', 'Copy or cut the picked clip, paste it where playback starts'], ['D', 'Place the picked clip again, after itself'], ['← and →', 'Nudge the picked clip'], ['Backspace', 'Remove the picked clip'], ['M, S', 'Mute or solo its lane'],
    ['+ and -', 'Zoom in and out'], ['Home', 'Start from the beginning'], ['Escape', 'Stop playback']]],
  map: ['Map', [['Space', 'Play the picked sound'], ['+ and -', 'Zoom in and out'], ['Escape', 'Stop playback']]],
  recorder: ['Recorder', [['R', 'Start or stop recording']]],
  folders: ['Library folders', [['Tab', 'Move between folders'], ['Enter', 'Pick the folder, or open a subfolder']]],
  projects: ['Projects', [['Tab', 'Move between projects and sounds'], ['Enter', 'Open the project, or edit the sound']]],
  settings: ['Settings', []],
};
function showShortcuts() {
  const [name, keys] = SHORTCUTS[currentView] || SHORTCUTS.sounds;
  const rows = [...keys, ['Cmd K', 'Command palette'], ['?', 'This list']].map(([k, what]) => el('div', { class: 'keyrow' }, el('kbd', {}, k), el('span', {}, what)));
  dialog(`${name} shortcuts`, [el('div', { class: 'keys' }, rows)], null, 'Close', () => true);
}

// ---------- command palette: Cmd+K, type what you want, Enter
function paletteCommands() {
  const page = (v, label) => ({ label: `Go to ${label}`, kind: 'page', run: () => showView(v) });
  const cmds = [page('sounds', 'Library'), page('discover', 'Discover'), page('folders', 'Library folders'), page('projects', 'Projects'), page('arranger', 'Arranger'), page('editor', 'Editor'), page('recorder', 'Recorder'), page('map', 'Map'), page('settings', 'Settings'),
    { label: 'Add a folder', kind: 'library', run: () => { showView('folders'); $('#addFolder').click(); } },
    { label: 'New project', kind: 'projects', run: () => { showView('projects'); $('#newProject').click(); } },
    { label: 'Rescan every folder', kind: 'library', run: () => api('/api/rescan', { method: 'POST' }).then(() => toast('Looking for new and changed files')).catch(fail) },
    { label: 'Show keyboard shortcuts', kind: 'help', run: showShortcuts },
    { label: document.documentElement.dataset.theme === 'dark' ? 'Switch to light' : 'Switch to dark', kind: 'look', run: () => { saveSetting('theme', document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark'); renderSettings(); } },
    { label: 'Search the Library', kind: 'library', run: () => { showView('sounds'); $('#q').focus(); } },
  ];
  if (editor.channels) for (const b of document.querySelectorAll('#editor .tmenu-list [data-a], #editor .tmenu-list [data-fx-open]')) {
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
  let shown = [], at = 0;
  const close = () => { box.remove(); if (back?.isConnected) back.focus(); };
  const run = (c) => { close(); c?.run(); };
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
function showView(name) {
  currentView = name;
  document.querySelectorAll('main > .view').forEach(v => v.hidden = v.dataset.view !== name);
  document.querySelectorAll('.ab[data-view]').forEach(b => {
    const on = b.dataset.view === (name === 'folders' ? 'sounds' : name);
    b.classList.toggle('on', on);
    if (on) b.setAttribute('aria-current', 'page'); else b.removeAttribute('aria-current');
  });
  hideTip();
  if (name !== 'editor') editor.stopAudition();
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
  api, el, ib, icon, notice, fail, modal, focusable,
  projectFolder: (p) => chooseProjectFolder(p),
  volume: () => volume() / 100,
  stopOthers: () => { stopPreview(); arranger.stop(); if (editor.playing) editor.stopPlay(); editor.stopAudition(); },
  onAdd: () => { refreshSide(); },
});
const soundmap = new SoundMap($('#soundmap'), {
  api, el, ib, icon, fail, busy,
  preview: (it) => togglePreview(it, true), stop: () => stopPreview(),
  edit: (it) => openPath(it),
  similar: (it) => { showView('sounds'); $('#q').value = it.name.replace(/\.[^.]+$/, ''); $('#q').dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' })); },
});
const recorder = new Recorder($('#recorder'), {
  api, el, ib, icon, notice, toast, fail, modal, confirmModal,
  volume: () => volume() / 100,
  stopOthers: () => { stopPreview(); discover.stop(); arranger.stop(); if (editor.playing) editor.stopPlay(); editor.stopAudition(); },
  onSave: () => refreshSide(),
  onChange: () => syncUnsaved(),
  edit: async (t) => {
    if (editor.dirty && !await confirmModal('Discard unsaved edits?', `${editor.name} has edits that are not saved. Opening ${t.name} discards them.`, 'Discard')) return;
    const seq = ++openSeq;
    const dir = t.saved ? t.saved.slice(0, t.saved.lastIndexOf('/')) : (await api('/api/folders')).find(f => !f.offline)?.path || '';
    if (seq !== openSeq) return;
    showView('editor');
    editor.load({ channels: t.channels.map(d => d.slice()), sr: t.sr, name: `${t.name}.wav`, dir, path: t.saved || null });
  },
});
const arranger = new Arranger($('#arranger'), {
  api, el, ib, icon, notice, toast, fail, modal, confirmModal, miniWave, focusable, isPlaying,
  preview: (it) => togglePreview(it),
  settings: () => SETTINGS,
  go: (view) => showView(view),
  edit: (it) => openPath(it),
  stopOthers: () => { stopPreview(); discover.stop(); recorder.stopPlay(); if (editor.playing) editor.stopPlay(); editor.stopAudition(); },
  onSave: () => { refreshSide(); setTimeout(() => refreshSide(), 2500); },
});
window.athar.discover = discover; window.athar.soundmap = soundmap; window.athar.recorder = recorder; window.athar.arranger = arranger;
document.querySelectorAll('.ab[data-view]').forEach(b => b.onclick = () => showView(b.dataset.view));
document.querySelectorAll('[data-folders]').forEach(b => b.onclick = () => { showView('folders'); if (b.dataset.folders === 'add') $('#addFolder').click(); });
$('#foldersBack').onclick = () => showView('sounds');
showView('sounds');

document.addEventListener('keydown', (e) => {
  if (!$('#modal').hidden || $('#palette')) return;
  const tag = e.target.tagName;
  // A ruler keeps its own arrow keys; every other shortcut still works while it has focus.
  const ruler = tag === 'INPUT' && e.target.type === 'range';
  if (ruler && /^(Arrow|Home|End|Page)/.test(e.key)) return;
  if (!ruler && (tag === 'INPUT' || tag === 'SELECT' || tag === 'TEXTAREA')) { if (e.key === 'Escape') e.target.blur(); return; }
  if (tag === 'BUTTON' && (e.key === ' ' || e.key === 'Enter') && e.target.matches(':focus-visible')) return;
  if (e.key === '?') { e.preventDefault(); return void showShortcuts(); }
  if (currentView === 'editor') editor.handleKey(e);
  else if (currentView === 'discover') discover.handleKey(e);
  else if (currentView === 'map') soundmap.handleKey(e);
  else if (currentView === 'recorder') recorder.handleKey(e);
  else if (currentView === 'arranger') arranger.handleKey(e);
  else if (currentView === 'sounds') soundsKey(e);
});

api('/api/settings').then(st => { SETTINGS = st; applySettings(); applyEditorFields(); renderSettings(); })
  .catch(fail)
  .finally(() => { refreshSide(); poll(); loadTags().catch(() => { }); });

// ---------- the cat: tips for the page you are on
const CAT_TIPS = {
  sounds: [
    'Describe what you hear, like "rain on a tin roof". File names and tags count too.',
    'J and K move through the list. Space plays the sound.',
    'Press / to jump to the search box from anywhere on this page.',
    'Press 1 to 5 to rate the sound you are on. 0 clears it.',
    'Double-click a sound, or press E, to open it in the editor.',
    'Click the big name, or press R, to rename the file on disk.',
    'T jumps to tags, N to the note. Tags help you find field recordings later.',
    'Click name, length or rating at the top of the table to sort by it.',
    'Try searching for a feeling instead of a thing: "lonely metallic ping", "warm and dusty".',
    'Stuck on a beat? Search for something that should not fit, like "creaking door", and use it as percussion.',
    'The little waveform on each row shows the shape before you play it. Short spikes are hits, a long block is a texture.',
    'The stacked button above the list preps or converts every sound in the list in one go.',
    'Rate as you listen. Later, the filter button can show only your 4 and 5 dot sounds.',
    'Backspace moves the sound you are on to the Trash, after asking. Escape stops playback.',
    'The tile at the top right is the volume. Pull it left, off its shadow, for louder. Scroll works too.',
    'Write where and when in the note. In a year, "under the bridge, 6 am" will find it for you.',
  ],
  folders: [
    'Athar analyses every file in a folder once, then keeps watching it for new recordings.',
    'Tags, notes and ratings follow a file when you move or rename it in Finder.',
    'A folder that is missing, or on an SD card that is out, stays in the list, greyed until it is back.',
    'Add the folder your recorder imports into. New takes show up in the Library on their own.',
    'The refresh button rescans every folder, for when you changed files while Athar was closed.',
    'WAV, AIFF, FLAC, OGG, MP3 and M4A files are all read.',
    'Open a folder, then use its "Show its sounds in the Library" button to see only those sounds.',
    'Idea: keep one folder only for sounds you recorded yourself. It becomes your signature set.',
  ],
  discover: [
    'Search free archives here. No account needed.',
    'The empty search box keeps typing out ideas. Press Tab to take the one on screen, then Enter to search.',
    'The filter button in the search box picks the archive.',
    'Online search matches titles and tags only. Once a sound is in your library, you can find it by describing it.',
    'The plus button downloads a sound into a project. Its licence and source link are written into the note.',
    'Internet Archive results open to show the recordings inside. Click one to hear it.',
    'Old radio, wax cylinders and 78s on the Internet Archive are full of textures nobody else is using.',
    'Added sounds get the tag "discovered", so you can filter for them in the Library.',
    'Press / to jump to the search box.',
  ],
  map: [
    'The map is a landscape. Peaks are where many sounds that sound alike gather.',
    'Move over the land to see the sounds there. Zoom in and they all appear.',
    'Click a name on the map to light up that kind of sound and dim the rest. Click it again to clear.',
    'Rest the pointer on a dot to hear it. Sweep slowly to audition a whole neighbourhood.',
    'Scroll to zoom in on a crowd of dots. Drag to move around.',
    'Click a dot to keep it at the bottom, then send it to the editor.',
    'Look at the edges. Lonely dots far from any group are your strangest sounds.',
    'Two dots that sit together but come from different folders layer well.',
  ],
  recorder: [
    'Press R to start and stop a take.',
    'Takes stay in the list until you save or discard them. Nothing is written to disk before that.',
    'Watch the meter. If it fills to the end, the take clipped: move back or turn the input down.',
    'Open a take in the editor first to trim it, then save from there.',
    'Record the room with nobody in it. Room tone under a dry sample makes it sit in a mix.',
    'Tap on things. A table, a radiator and a mug are a drum kit nobody else has.',
    'Save straight into a project to fill it as you record.',
  ],
  arranger: [
    'The browser on the left is how sounds get here. Pick a project, then drag a sound onto a lane. Double-click a clip to edit its sound.',
    'A lane is a track you name: Drums, Vocal, Synth. Double-click its name to change it. Any sound can go in any lane.',
    'Drop a sound below the last lane and it starts a new lane. "Add a lane" makes an empty one to fill.',
    'Drag a clip sideways to move it in time, and up or down to move it to another lane. Hold alt to leave a copy behind.',
    'A moved clip catches on the start and end of the others, so two can sit end to end. Hold cmd to place it freely.',
    'Each clip has a small square at its top corners. Drag one inward to fade the clip in or out.',
    'For a crossfade, overlap two clips, then drag the fade out of the first and the fade in of the second across the overlap.',
    'Click a clip, press Cmd C, click the time ruler where you want it, then Cmd V.',
    'M mutes a lane. S solos it, so you hear it on its own.',
    'The ruler under each name is its level. Double-click it to go back to 0 dB.',
    'Pinch on the trackpad, or hold cmd and scroll, to zoom the lanes where the pointer is.',
    'Save mix writes what you hear as one new sound, into a project or a folder.',
    'Idea: layer a soft kick under a clicky one. Slide the soft one a few milliseconds later.',
    'Idea: put room tone under a dry hit, turn its lane down to -18 dB, and save the pair as one sound.',
    'Click the time ruler at the top to choose where playback starts.',
  ],
  projects: [
    'A project has its own folder. "Add to project" on a sound copies it in.',
    'Export makes an SP-404MKII project with the pads as laid out, ready for IMPORT PROJECT on the device.',
    'Double-click a sound in a project to open it in the editor.',
    'A sound can be added to several projects. Each gets its own copy, and the original stays in the Library.',
    'Exported files are named by the pattern in Settings, so they sort neatly on the sampler.',
    'The batch button on a project preps or converts every sound in it at once.',
    'Idea: build a project from one place. Eight sounds from the same kitchen already belong together.',
    'Idea: limit a project to 16 sounds, one per pad. Choosing is half the work.',
  ],
  editor: [
    'The magic wand preps a sample: trims silence, normalizes and adds tiny fades.',
    'Drag across the waveform to select, then R to add it as a region.',
    'Space plays and stops. L loops the selection.',
    'F fits the whole file. E zooms into the selection. Plus and minus zoom in and out.',
    'Hold cmd and scroll to zoom where the pointer is. Scroll alone moves along the sound.',
    'Shift-click extends the selection. Double-click the waveform selects everything. Escape clears it.',
    'Home and End jump to the start and the end.',
    'Fade has a ruler for each end. Select part of the waveform first and the fades sit at the ends of that part.',
    'Cmd T keeps only the selection. Cmd Z undoes it if you cut too close.',
    'Cmd S saves a new file and asks which project it goes into. The file you opened is never changed.',
    'Move any control and you hear the result. Turn Loop on first and it keeps playing while you adjust.',
    'The Original and Edited switch, or the B key, compares a change with the sound before it. Apply keeps the change.',
    'Auto-slice cuts a long recording at its silences. Each region saves as its own file.',
    'Slice at hits finds every drum hit in a loop. Then "Save each region" in File gives you one file per hit.',
    'Noise reduction works best if you first select a bit of pure background noise.',
    'Level, then "Normalize loudness", sets a LUFS target, so every sample in a project sits at the same level.',
    'Double-click a region\'s number to name it. Regions are remembered with the file.',
    'Stretch changes length without changing pitch, and pitch without changing length.',
    'File, then "Export for SP-404MKII", saves straight to the card in the right format.',
    'In the EQ, double-click to add a band, drag it to move, scroll on it to change its width.',
    'The spectrogram view shows pitch over time. Bird calls and squeaks are easy to spot there.',
    'Time, then "Tune to note", reads the pitch of an 808 or a one-shot and moves it onto the nearest note.',
    'Saturation makes a bass hit audible on small speakers. Push Drive, and the Ceiling keeps it from clipping.',
    'Shape has two rulers: Attack for more or less punch, Tail to shorten a ring or let it breathe.',
    'Lo-fi lowers bit depth and sample rate for old-sampler grit. Try 12-bit at 26 kHz on drums.',
    'Idea: set Speed to 50% on a field recording. Rain turns into a slow, low wash.',
    'Idea: reverse a cymbal or a door slam and end it right on a kick for a swell.',
    'Idea: select one second of room tone and use "Make it loop". You get a bed to sit under a beat.',
    'Idea: pitch a single hit up 12 semitones and down 12. That is three drums from one recording.',
    'Idea: stretch a half-second vocal to 400%. The smear is the point.',
  ],
  settings: [
    'Turn the cat off at the top of this page if I get in the way.',
    'Sampler prep settings change what the magic wand does in the editor.',
    'I have friends. Pick another cat from the list on this page.',
    'The accent colour changes every highlight and shadow in the app.',
    'Turn on play-as-you-move to hear each sound while you arrow through the Library.',
    'Smooth edits adds a tiny crossfade to every cut, so deletes do not click.',
    'Choose the format saved edits use by default: WAV, FLAC or MP3.',
    'The loudness target here is what "Normalize loudness" starts from in the editor.',
  ],
};
const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
const cat = { x: null, moving: false, facing: 1, stop: false, mode: 'idle', t0: 0 };
const catTipIndex = {};
let skin = CAT_SKINS.sukey;
const catImg = new Image();
// The cat lives in the band above the page: from its left edge to the volume ruler.
const CAT_MAX_SIZE = 72;
const catSize = () => Math.min(skin.size, CAT_MAX_SIZE);
const catMin = () => {
  const h = [...document.querySelectorAll('.vhead')].find(e => e.offsetParent);
  return h ? Math.round(h.getBoundingClientRect().left) + 28 : 90;
};
// The speech bubble reaches 86 px to the right of the cat, so the cat stops far enough from the
// volume tile for the bubble to clear it.
const catMax = () => Math.max(catMin(), Math.round($('.transport').getBoundingClientRect().left) - Math.max(catSize() + 12, 86 + 28));
const clampCat = (x) => Math.min(Math.max(x, catMin()), catMax());
function catPlace() { $('#cat').style.transform = `translateX(${cat.x}px)`; }

function catSay(text) {
  const b = $('#catBubble'), left = (cat.x ?? 999) < 320;
  $('#catText').textContent = text;
  b.style.left = left ? '20px' : '-200px';
  b.classList.toggle('flip', left);
  b.hidden = false;
}
// What the cat can say where the user is. With an editor section open it explains that section
// first, then carries on with the page; on a page, its tips and its short explanations are mixed.
function catLines() {
  const tips = CAT_TIPS[currentView] || CAT_TIPS.sounds, facts = PAGE_LESSONS[currentView]?.cards || [];
  const mixed = [];
  for (let t = 0, f = 0; t < tips.length || f < facts.length;) {
    mixed.push(...tips.slice(t, t + 2)); t += 2;
    if (f < facts.length) mixed.push(facts[f++]);
  }
  const section = currentView === 'editor' ? editor.fx.section() : null;
  return section && SECTION_LESSONS[section] ? { key: section, list: [...SECTION_LESSONS[section].cards, ...mixed] } : { key: `page:${currentView}`, list: mixed };
}
let catKey = null;
function catTip() {
  const { key, list } = catLines();
  const k = ((catTipIndex[key] ?? -1) + 1) % list.length;
  catTipIndex[key] = k; catKey = key;
  catSay(list[k]);
}
// Called when the open editor section changes: an open bubble starts on what that section does.
function catContext() {
  if ($('#catBubble').hidden) return;
  const { key } = catLines();
  if (key !== catKey) { if (key in SECTION_LESSONS) catTipIndex[key] = -1; catTip(); }
}
window.addEventListener('athar:view', () => { $('#catBubble').hidden = true; });
$('#catBtn').onclick = () => { cat.stop = true; if ($('#catBubble').hidden) catTip(); else $('#catBubble').hidden = true; };
$('#catNext').onclick = catTip;
$('#catClose').onclick = () => { $('#catBubble').hidden = true; };
catContext();

function setCatSkin(name) {
  const next = CAT_SKINS[name] || CAT_SKINS.sukey;
  if (next === skin && catImg.src) return;
  skin = next;
  const cv = $('#catCanvas');
  cv.width = cv.height = skin.px;
  cv.style.imageRendering = skin.smooth ? 'auto' : 'pixelated';
  $('#cat').style.setProperty('--cat-size', catSize() + 'px');
  catImg.onload = () => catShow('idle');
  catImg.src = skin.src;
  cat.mode = 'idle'; cat.t0 = performance.now();
  if (cat.x != null) { cat.x = clampCat(cat.x); catPlace(); }
}
// Skins with a dedicated left row draw it as-is; every other skin mirrors all its frames when facing left.
function catFrame(name, i) {
  const cv = $('#catCanvas');
  if (!cv || !catImg.complete || !catImg.naturalWidth) return;
  const a = skin.anim[name] || skin.anim.idle;
  const row = name === 'right' && cat.facing < 0 && skin.anim.left ? skin.anim.left.row : a.row;
  const f = a.seq[Math.min(i, a.seq.length - 1)];
  const g = cv.getContext('2d');
  g.clearRect(0, 0, skin.px, skin.px);
  g.drawImage(catImg, f * skin.px, row * skin.px, skin.px, skin.px, 0, 0, skin.px, skin.px);
  cv.style.transform = !skin.anim.left && cat.facing < 0 ? 'scaleX(-1)' : '';
}
function catShow(name) { cat.mode = name; cat.t0 = performance.now(); catFrame(name, 0); }
// Play a one-shot animation to its end, then return to idle.
function catOnce(name) {
  const a = skin.anim[name];
  if (!a) return Promise.resolve();
  cat.mode = 'once'; cat.t0 = performance.now();
  return new Promise((done) => {
    let i = 0;
    const step = () => {
      if (cat.moving || cat.mode !== 'once') return done();
      catFrame(name, i);
      if (++i < a.seq.length) setTimeout(step, a.ms); else { catShow('idle'); done(); }
    };
    step();
  });
}

let meowTimer;
function catMeow() {
  const m = $('#catMeow');
  m.classList.toggle('left', cat.facing < 0);
  m.hidden = false;
  clearTimeout(meowTimer);
  meowTimer = setTimeout(() => { m.hidden = true; }, 1400);
}

// The cat stays put most of the time: it sits, grooms or meows, then runs across to the other side.
function catMove(to) {
  return new Promise((done) => {
    const from = cat.x, ms = Math.abs(to - from) / skin.speed * 1000, t0 = performance.now(), fm = skin.anim.right.ms;
    cat.facing = to < from ? -1 : 1; cat.moving = true; cat.stop = false;
    $('#cat').classList.add('moving');
    $('#catMeow').hidden = true;
    const step = (t) => {
      const dt = Math.max(0, t - t0), k = ms ? Math.min(1, dt / ms) : 1;
      cat.x = clampCat(from + (to - from) * k); catPlace();
      catFrame('right', Math.floor(dt / fm) % skin.anim.right.seq.length);
      if (k < 1 && !cat.stop) { requestAnimationFrame(step); return; }
      cat.moving = false;
      $('#cat').classList.remove('moving');
      catOnce('sitdown').then(() => { if (cat.mode !== 'idle' && !cat.moving) catShow('idle'); });
      done();
    };
    requestAnimationFrame(step);
  });
}
async function catWander() {
  const busy = $('#cat').hidden || !$('#catBubble').hidden || reduceMotion;
  if (!busy) {
    if (Math.random() < .7) {
      const r = Math.random();
      if (r < .2) catMeow();
      else if (r < .4 && skin.anim.groom) await catOnce('groom');
      else if (r < .6 && !skin.anim.left) { cat.facing = -cat.facing; catShow('idle'); }
    } else {
      const lo = catMin(), hi = catMax(), mid = (lo + hi) / 2;
      const to = cat.x > mid ? lo + Math.random() * (mid - lo) * .8 : hi - Math.random() * (hi - mid) * .8;
      await catMove(to);
      if (Math.random() < .25 && !cat.stop) catMeow();
    }
  }
  setTimeout(catWander, 1200 + Math.random() * 2300);
}
setInterval(() => {
  if ($('#cat').hidden || cat.moving || cat.mode === 'once') return;
  const a = skin.anim[cat.mode] || skin.anim.idle;
  catFrame(cat.mode, Math.floor((performance.now() - cat.t0) / a.ms) % a.seq.length);
}, 60);
setCatSkin('sukey');
cat.x = catMax(); catPlace();
window.addEventListener('resize', () => { cat.x = clampCat(cat.x); catPlace(); });
window.addEventListener('athar:view', () => { if (!cat.moving) { cat.x = clampCat(cat.x); catPlace(); } });
setTimeout(catWander, 1500);
