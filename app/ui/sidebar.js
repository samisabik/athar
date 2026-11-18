// The Library's folders, the kinds of sound, and the projects with their sounds and pads.

import { $, el, ib, icon, fail, notice, modal, confirmModal, focusable, pickFolder, remember, recall } from './ui.js';
import { fmtTime } from './time.js';
import { lib } from './native.js';
import { state } from './store.js';
import { SETTINGS } from './settings.js';
import { baseOf, stemOf, plural } from './names.js';
import { isPlaying, togglePreview, previewButton } from './preview.js';
import { showView } from './views.js';
import { batch } from './batch.js';
import { openPath } from './editing.js';
import { dropZone, miniWave, search, syncIdleSearch } from './search.js';

/** @typedef {import('./search.js').Item} Item */
/** @typedef {import('./search.js').Project} Project */
/** @typedef {import('./search.js').Folder} Folder */
/** @typedef {import('./search.js').Sub} Sub */
/** Shows a folder's sounds in the Library, or only those under `under`. @typedef {(f: Folder, under?: string | null) => void} Show */

export function refreshSide() { loadFolders().catch(fail); loadCats().catch(fail); loadProjects().catch(fail); }
// A file that was just saved or copied is analysed in the background. Its folder's and its
// project's counts are right once that is done, so they are read again a moment later.
const INDEX_WAIT_MS = 2500;
export function refreshSideSoon() { setTimeout(refreshSide, INDEX_WAIT_MS); }

// ---------- Library folders: the list picks a folder, which opens to show what is inside.
/** The Library's folders as last read. @type {Folder[]} */
let folders = [];
export function hasFolders() { return folders.length > 0; }
/** @type {string | null} */
let folderSel = null, folderShut = false;
/** Numbers each reading of the folders; one that finishes after a newer one was shown is dropped. */
let folderSeq = 0, folderShown = 0;
export async function loadFolders() {
  const seq = ++folderSeq;
  /** @type {Folder[]} */
  const fs = await lib.folders();
  // Any answer newer than the one shown is taken: waiting for the very latest would show
  // nothing while readings are asked for faster than they come back.
  if (seq < folderShown) return;
  folderShown = seq;
  folders = fs;
  if (state.folder && !fs.some(f => f.path === state.folder)) { state.folder = null; state.under = null; }
  if (!fs.some(f => f.path === folderSel)) folderSel = (fs.find(f => f.path === state.folder) || fs[0])?.path ?? null;
  document.querySelectorAll('[data-folders=list]').forEach(b => { b.textContent = fs.length ? 'Library folders' : 'No Library folders yet'; });
  syncIdleSearch();
  // The list itself asks for every open level of the tree, so it is built only while it shows.
  if (state.currentView === 'folders') await renderFolders(seq);
}
/** @param {number} seq */
async function renderFolders(seq) {
  const box = document.createDocumentFragment();
  if (!folders.length) box.append(dropZone());
  /** @type {Show} */
  const show = (f, under = null) => { state.folder = f.path; state.under = under; loadFolders().catch(fail); showView('sounds'); search(); };
  for (const f of folders) {
    // A click opens a folder to show what is inside; a click on the open one closes it again.
    const open = folderSel === f.path && !folderShut;
    const pick = () => { folderShut = folderSel === f.path && !folderShut; folderSel = f.path; loadFolders().catch(fail); };
    box.append(focusable(el('div', {
      class: 'item' + (open ? ' on' : '') + (f.offline ? ' offline' : ''),
      'data-tip': f.offline ? `${f.path} was not found, or its drive is not connected. Its sounds, tags and notes are kept.` : f.failed ? failedTip(f) : null,
      onclick: pick,
    },
    el('span', { class: 'label' }, baseOf(f.path)),
    el('span', { class: 'count' }, [folderState(f), plural(f.files, 'sound'), diskSize(f.bytes)].filter(Boolean).join(', ')),
    open ? folderActions(f, show) : null), pick));
    if (open && !f.offline) await appendTree(box, f, f.path, 1, show);
  }
  if (seq !== folderSeq) return;
  // The list is rebuilt whole, also every second while sounds are analysed: the row that had the keyboard keeps it.
  const list = $('#folders'), focused = [...list.children].indexOf(document.activeElement);
  list.replaceChildren(box);
  if (focused >= 0) list.children[focused]?.focus({ preventScroll: true });
}

// The subfolders of the picked folder, as an indented tree. A level opens when clicked.
/** @type {Set<string>} */
const treeOpen = new Set();
/** @param {DocumentFragment} box @param {Folder} f @param {string} path @param {number} depth @param {Show} show */
async function appendTree(box, f, path, depth, show) {
  /** @type {Sub[]} */
  let subs = [];
  try { subs = await lib.subfolders({ path }); } catch { return; }
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
    el('span', { class: 'count' }, plural(sf.files, 'sound')),
    ib('queue_music', 'Show its sounds in the Library', (e) => { e.stopPropagation(); show(f, sf.path); }),
    ib('folder_open', 'Show in Finder', (e) => { e.stopPropagation(); revealFolder(sf.path); })), toggle));
    if (open && sf.nested) await appendTree(box, f, sf.path, depth + 1, show);
  }
}

// What a folder is doing, and for the picked one the two things you can do with it.
/** @param {Folder} f */
function folderState(f) {
  const waiting = f.files - f.indexed - f.failed;
  return f.offline ? 'not found' : waiting > 0 ? `analysing ${waiting} more` : f.failed ? `${f.failed} could not be read` : '';
}
/** Names the folder's files that could not be read, with the reason for each. @param {Folder} f */
function failedTip(f) {
  // The decoder's reasons are chains ("reading the container: ...: no suitable format reader found"); the last link says it.
  const named = f.failed_files.map(x => `${baseOf(x.path)} (${x.error.split(': ').pop()})`), more = f.failed - named.length;
  return `Could not be read: ${named.join(', ')}${more > 0 ? ` and ${more} more` : ''}`;
}
const revealFolder = (/** @type {string} */ path) => lib.revealFolder({ path }).catch(fail);
/** @param {number} n */
function diskSize(n) { return !n ? '' : n < 1e6 ? `${Math.max(1, Math.round(n / 1e3))} KB` : n < 1e9 ? `${(n / 1e6).toFixed(n < 1e7 ? 1 : 0)} MB` : `${(n / 1e9).toFixed(1)} GB`; }
/** @param {Folder} f @param {Show} show */
function folderActions(f, show) {
  const name = baseOf(f.path);
  return [
    ib('queue_music', 'Show its sounds in the Library', (e) => { e?.stopPropagation(); show(f); }),
    ib('folder_open', 'Show in Finder', (e) => { e?.stopPropagation(); revealFolder(f.path); }),
    ib('close', 'Remove from Athar. The files stay on disk', async (e) => {
      e?.stopPropagation();
      if (!await confirmModal(`Remove ${name}?`, 'The folder leaves your library. Its files stay on disk.', 'Remove')) return;
      try { await lib.removeFolder({ path: f.path }); }
      catch (err) { return fail(err); }
      if (state.folder === f.path) { state.folder = null; state.under = null; }
      refreshSide(); search();
    }),
  ];
}

export async function loadCats() {
  /** @type {{ name: string, count: number }[]} */
  const cs = await lib.categories();
  const sel = $('#catFilter');
  sel.innerHTML = '';
  sel.append(el('option', { value: '' }, 'any'));
  cs.filter(c => c.count || c.name === state.category).sort((a, b) => a.name.localeCompare(b.name))
    .forEach(c => sel.append(el('option', { value: c.name }, `${c.name} (${c.count})`)));
  sel.value = state.category || '';
}

// ---------- projects
/** @type {number | null} */
let projSel = null, projShut = false, projSeq = 0;
/** Opens the project `id` in the list, as after making it. @param {number} id */
export function selectProject(id) { projSel = id; projShut = false; }
/** Reads the projects again. 'athar:projects' goes out on the window once they are in the store. */
export async function loadProjects() {
  state.projects = await lib.projects();
  if (!state.projects.some(p => p.id === projSel)) projSel = state.projects[0]?.id ?? null;
  if (state.currentView === 'projects') renderProjectCard().catch(fail);
  window.dispatchEvent(new Event('athar:projects'));
}

// Every project is a row. The picked one carries its actions and opens to show its sounds.
async function renderProjectCard() {
  const seq = ++projSeq, box = $('#projList');
  if (!state.projects.length) {
    box.replaceChildren(el('div', { class: 'empty' }, el('h3', {}, 'No projects yet'), el('p', { class: 'muted' }, 'A project is a folder of its own for what you are making. Create one, then add sounds from the Library: each is copied in, and the original stays where it is.')));
    return;
  }
  const p = projShut ? null : state.projects.find(x => x.id === projSel);
  /** @type {Item[]} */
  const items = p?.n ? await lib.search({ project: p.id, sort: 'project', limit: 5000 }) : [];
  const pads = padView && p && items.length ? await lib.projectPads(p.id) : {};
  if (seq !== projSeq) return;
  const rename = async (/** @type {Project} */ p) => {
    const v = (await modal(`Rename ${p.name}`, [{ label: 'Name', value: p.name }]))?.[0]?.trim();
    if (!v || v === p.name) return;
    try { await lib.renameProject(p.id, { name: v }); loadProjects().catch(fail); }
    catch (e) { fail(e); }
  };
  const stop = (/** @type {() => unknown} */ fn) => (/** @type {MouseEvent} */ e) => { e?.stopPropagation(); return fn(); };
  const actions = (/** @type {Project} */ p) => [
    ib('ios_share', 'Export: copy the files, or make an SP-404MKII project', stop(() => exportProject(p))),
    ib('grid_view', 'Pads: lay the sounds out on the SP-404MKII pads', stop(() => {
      padView = !padView;
      remember(PAD_VIEW_KEY, padView ? '1' : '');
      renderProjectCard().catch(fail);
    }), padView ? 'on' : ''),
    ib('dynamic_feed', 'Batch: prep or convert every sound in this project', stop(() => batch(items))),
    ib('edit', 'Rename project', stop(() => rename(p))),
    ib('queue_music', 'Show its sounds in the Library', stop(() => { state.project = p.id; showView('sounds'); search(); })),
    p.folder ? ib('folder_open', 'Show in Finder', stop(() => revealFolder(/** @type {string} */ (p.folder)))) : null,
    ib('close', 'Remove from Athar. The files stay on disk', stop(async () => {
      if (!await confirmModal(`Remove ${p.name}?`, 'The project leaves Athar. Its folder and files stay on disk.', 'Remove')) return;
      try { await lib.deleteProject(p.id); }
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
      el('span', { class: 'count' }, [plural(on ? items.length : x.n || 0, 'sound'), diskSize(x.bytes)].filter(Boolean).join(', ')),
      on && p ? actions(p) : null), pick));
    if (!on || !p) continue;
    if (padView && items.length) { out.push(padsArea(p, items, pads)); continue; }
    out.push(...(items.length ? items.map(it => projectRow(p, it, items))
      : [el('div', { class: 'prow-empty muted' }, 'No sounds yet. Add some from the Library with "Add to project". Each one is copied into this project.')]));
  }
  box.replaceChildren(...out);
}

/** @param {Project} p */
export async function chooseProjectFolder(p) {
  const dir = await pickFolder(`Folder for ${p.name}`);
  if (!dir) return false;
  try { await lib.projectFolder(p.id, { path: dir }); }
  catch (e) { fail(e); return false; }
  notice(`Sounds added to ${p.name} are now copied into its folder`);
  refreshSide(); search();
  refreshSideSoon();
  return true;
}

// Rows of a project can be dragged into the order the pads should have.
/** @type {string | null} */
let dragPath = null;
/** @type {number | null} */
let dragPad = null;
/** @param {Project} p @param {Item[]} items @param {string} from @param {string} to @param {boolean} after */
async function moveInProject(p, items, from, to, after) {
  const paths = items.map(x => x.path).filter(x => x !== from);
  paths.splice(paths.indexOf(to) + (after ? 1 : 0), 0, from);
  try { await lib.setProjectOrder(p.id, { paths }); } catch (e) { return fail(e); }
  renderProjectCard().catch(fail);
}
/** @param {Project} p @param {Item} it @param {Item[]} [items] */
function projectRow(p, it, items = []) {
  const over = (/** @type {DragEvent} */ e) => { const r = /** @type {HTMLElement} */ (e.currentTarget).getBoundingClientRect(); return e.clientY > r.top + r.height / 2; };
  return focusable(el('div', { class: 'prow', draggable: 'true', ondblclick: () => openPath(it), 'data-tip': 'Double-click to edit. Drag to reorder.',
    ondragstart: (/** @type {DragEvent} */ e) => { dragPath = it.path; e.dataTransfer?.setData('text/plain', it.name); if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'; },
    ondragend: () => { dragPath = null; document.querySelectorAll('.prow.drop-before, .prow.drop-after').forEach(n => n.classList.remove('drop-before', 'drop-after')); },
    ondragover: (/** @type {DragEvent} */ e) => {
      if (!dragPath || dragPath === it.path || dragPad != null) return;
      e.preventDefault(); const a = over(e), n = /** @type {HTMLElement} */ (e.currentTarget); n.classList.toggle('drop-after', a); n.classList.toggle('drop-before', !a);
    },
    ondragleave: (/** @type {DragEvent} */ e) => /** @type {HTMLElement} */ (e.currentTarget).classList.remove('drop-before', 'drop-after'),
    ondrop: (/** @type {DragEvent} */ e) => { if (!dragPath || dragPath === it.path || dragPad != null) return; e.preventDefault(); e.stopPropagation(); const from = dragPath; dragPath = null; moveInProject(p, items, from, it.path, over(e)); } },
    previewButton(it),
    miniWave(it),
    el('span', { class: 'qn' }, it.name),
    el('span', { class: 'qd' }, [it.category, fmtTime(it.duration)].filter(Boolean).join(' · ')),
    ib('close', 'Remove from project', async (e) => {
      e.stopPropagation();
      try { await lib.projectItem(p.id, { path: it.path, remove: true }); }
      catch (err) { return fail(err); }
      loadProjects().catch(fail);
    })), () => openPath(it));
}

// The SP-404MKII has ten banks of sixteen pads. Pad n is bank n / 16, pad n % 16, and a
// bank's pads read like text: 1 to 4 across the top row, 13 to 16 across the bottom.
const BANKS = 'ABCDEFGHIJ', PADS = 160;
const PAD_VIEW_KEY = 'athar.padView';
let padView = recall(PAD_VIEW_KEY) === '1', padBank = 0;
const padName = (/** @type {number} */ n) => `${BANKS[Math.floor(n / 16)]}${n % 16 + 1}`;

/** @param {Project} p @param {Item[]} items @param {Record<string, number>} pads */
function padsArea(p, items, pads) {
  const byPath = new Map(items.map(it => [it.path, it]));
  pads = Object.fromEntries(Object.entries(pads).filter(([path]) => byPath.has(path)));
  const byPad = new Map(Object.entries(pads).map(([path, n]) => /** @type {[number, Item]} */ ([n, byPath.get(path)])));
  const save = async (/** @type {Record<string, number>} */ next) => {
    try { await lib.setProjectPads(p.id, { pads: next }); } catch (e) { return fail(e); }
    renderProjectCard().catch(fail);
  };
  const place = (/** @type {string} */ path, /** @type {number} */ n) => {
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
    if (left) notice(`${plural(left, 'sound')} did not fit: the pads from ${BANKS[padBank]}1 to J16 are full`);
    save(next);
  };
  const clear = (/** @type {string} */ path) => { const next = { ...pads }; delete next[path]; save(next); };
  const endDrag = () => { dragPath = null; dragPad = null; document.querySelectorAll('.pad.over').forEach(x => x.classList.remove('over')); };
  const cell = (/** @type {number} */ n) => {
    const it = byPad.get(n);
    const c = el('div', {
      class: 'pad' + (it ? '' : ' empty') + (isPlaying(it) ? ' playing' : ''), draggable: it ? 'true' : 'false',
      onclick: () => { if (it) togglePreview(it); },
      ondblclick: () => { if (it) openPath(it); },
      ondragstart: (/** @type {DragEvent} */ e) => { if (!it) return void e.preventDefault(); const t = it; dragPath = t.path; dragPad = n; e.dataTransfer?.setData('text/plain', t.name); if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'; },
      ondragend: endDrag,
      ondragover: (/** @type {DragEvent} */ e) => { if (!dragPath) return; e.preventDefault(); /** @type {HTMLElement} */ (e.currentTarget).classList.add('over'); },
      ondragleave: (/** @type {DragEvent} */ e) => /** @type {HTMLElement} */ (e.currentTarget).classList.remove('over'),
      ondrop: (/** @type {DragEvent} */ e) => { if (!dragPath) return; e.preventDefault(); e.stopPropagation(); const path = dragPath; endDrag(); place(path, n); },
    },
      el('span', { class: 'pn' }, padName(n)),
      it ? el('span', { class: 'qn' }, stemOf(it.name)) : null,
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
  const letter = (/** @type {number} */ i) => el('span', { class: used.has(i) ? 'used' : '' }, BANKS[i]);
  const banks = el('div', { class: 'ab-switch banks', role: 'group', 'aria-label': 'Bank' },
    [0, 1, 2, 3, 4].map((k) => el('button', {
      class: padBank % 5 === k ? 'on' : '', 'aria-pressed': String(padBank % 5 === k),
      'aria-label': `Bank ${BANKS[padBank % 5 === k ? padBank : k + half]}`,
      onclick: () => { padBank = padBank % 5 === k ? (padBank + 5) % 10 : k + half; renderProjectCard().catch(fail); },
    }, padBank % 5 === k ? BANKS[padBank] : [letter(k), '/', letter(k + 5)])));
  const grid = el('div', { class: 'padgrid' }, Array.from({ length: 16 }, (_, k) => cell(padBank * 16 + k)));
  const pool = el('div', {
    class: 'padpool',
    ondragover: (/** @type {DragEvent} */ e) => { if (dragPad != null) e.preventDefault(); },
    ondrop: (/** @type {DragEvent} */ e) => {
      if (dragPad == null) return;
      e.preventDefault();
      const path = /** @type {string} */ (dragPath); endDrag(); clear(path);
    },
  }, items.filter(it => !(it.path in pads)).map(it => {
    const row = projectRow(p, it, items);
    // Dragged toward the grid, the row looks like the pad it is about to become.
    row.addEventListener('dragstart', (e) => {
      if (!e.dataTransfer?.setDragImage) return;
      const w = /** @type {HTMLElement | null} */ (grid.querySelector('.pad'))?.offsetWidth || 180;
      const ghost = el('div', { class: 'pad padghost', style: `width:${w}px` },
        el('span', { class: 'pn' }, '\u00a0'), el('span', { class: 'qn' }, stemOf(it.name)), miniWave(it));
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

/** @param {Project} p */
async function exportProject(p) {
  const v = await modal(`Export ${p.name}`, [
    { label: 'Format', options: [['copy', 'Copy original files'], ['sp404', 'SP-404MKII project']] },
    { label: 'File names. Use {project}, {category}, {name} and {n:03d} for a three-digit counter', value: SETTINGS.name_pattern || '{project}_{name}', when: (f) => f !== 'sp404' },
  ]);
  if (!v) return;
  const card = v[0] === 'sp404' ? await lib.sp404Card().catch(() => ({})) : {};
  const dest = await pickFolder(v[0] === 'sp404' ? 'Choose the SD card' : 'Export to folder', card.import || '');
  if (!dest) return;
  try {
    const r = await lib.exportProject(p.id, { dest, pattern: v[1], format: v[0] });
    if (v[0] === 'sp404') notice(`Saved ${baseOf(r.folder)}`);
  } catch (e) { fail(e); }
}
