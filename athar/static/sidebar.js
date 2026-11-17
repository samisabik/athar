// The sidebar: folders, categories, projects and their pads.

import { $, el, ib, icon, fail, notice, modal, confirmModal, focusable, pickFolder } from './ui.js';
import { fmtTime } from './time.js';
import { lib } from './native.js';
import { state } from './store.js';
import { SETTINGS } from './settings.js';

/** @typedef {import('./search.js').Item} Item */
/** @typedef {import('./search.js').Project} Project */
/** @typedef {import('./search.js').Folder} Folder */
/** @typedef {import('./search.js').Sub} Sub */
/** @typedef {import('./search.js').Main} Main */
/** @typedef {typeof import('./search.js')} Search */
/** Shows a folder's sounds in the Library, or only those under `under`. @typedef {(f: Folder, under?: string | null) => void} Show */


// What the sidebar uses from the rest of the window; main.js fills these in.
/** @typedef {Pick<Main, 'batch' | 'isPlaying' | 'openPath' | 'refreshSide' | 'showView' | 'togglePreview'>
      & Pick<Search, 'dropZone' | 'miniWave' | 'search' | 'syncIdleSearch'> & { editor: import('./editor.js').Editor }} SideHost */
export const host = /** @type {SideHost} */ (/** @type {unknown} */ ({ editor: null }));
// ---------- sidebar
// Library folders page: the list picks a folder, the panel above shows it.
/** @type {string | null} */
let folderSel = null, folderShut = false;
export async function loadFolders() {
  /** @type {Folder[]} */
  const fs = await lib.folders();
  const box = document.createDocumentFragment();
  if (state.folder && !fs.some(f => f.path === state.folder)) { state.folder = null; state.under = null; }
  if (!fs.some(f => f.path === folderSel)) folderSel = (fs.find(f => f.path === state.folder) || fs[0])?.path ?? null;
  if (!fs.length) box.append(host.dropZone());
  /** @type {Show} */
  const show = (f, under = null) => { state.folder = f.path; state.under = under; loadFolders(); host.showView('sounds'); host.search(); };
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
  host.syncIdleSearch();
}

// The subfolders of the picked folder, as an indented tree. A level opens when clicked.
/** @type {Set<string>} */
export const treeOpen = new Set();
/** @param {DocumentFragment} box @param {Folder} f @param {string} path @param {number} depth @param {Show} show */
export async function appendTree(box, f, path, depth, show) {
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
    el('span', { class: 'count' }, `${sf.files} sound${sf.files === 1 ? '' : 's'}`),
    ib('queue_music', 'Show its sounds in the Library', (e) => { e.stopPropagation(); show(f, sf.path); }),
    ib('folder_open', 'Show in Finder', (e) => { e.stopPropagation(); revealFolder(sf.path); })), toggle));
    if (open && sf.nested) await appendTree(box, f, sf.path, depth + 1, show);
  }
}

// What a folder is doing, and for the picked one the two things you can do with it.
/** @param {Folder} f */
export function folderState(f) {
  const waiting = f.files - f.indexed;
  return f.offline ? 'not found' : waiting > 0 ? `analysing ${waiting} more` : '';
}
export const revealFolder = (/** @type {string} */ path) => lib.revealFolder({ path }).catch(fail);
/** @param {number} n */
export function diskSize(n) { return !n ? '' : n < 1e6 ? `${Math.max(1, Math.round(n / 1e3))} KB` : n < 1e9 ? `${(n / 1e6).toFixed(n < 1e7 ? 1 : 0)} MB` : `${(n / 1e9).toFixed(1)} GB`; }
/** @param {Folder} f @param {Show} show */
export function folderActions(f, show) {
  const name = f.path.split('/').pop();
  return [
    ib('queue_music', 'Show its sounds in the Library', (e) => { e?.stopPropagation(); show(f); }),
    ib('folder_open', 'Show in Finder', (e) => { e?.stopPropagation(); revealFolder(f.path); }),
    ib('close', 'Remove from Athar. The files stay on disk', async (e) => {
      e?.stopPropagation();
      if (!await confirmModal(`Remove ${name}?`, 'The folder leaves your library. Its files stay on disk.', 'Remove')) return;
      try { await lib.removeFolder({ path: f.path }); }
      catch (err) { return fail(err); }
      if (state.folder === f.path) { state.folder = null; state.under = null; }
      host.refreshSide(); host.search();
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

/** @type {number | null} */
let projSel = null, projShut = false, projSeq = 0;
/** Opens the project `id` in the list, as after making it. @param {number} id */
export function selectProject(id) { projSel = id; projShut = false; }
export async function loadProjects() {
  state.projects = await lib.projects();
  if (!state.projects.some(p => p.id === projSel)) projSel = state.projects[0]?.id ?? null;
  if (state.currentView === 'projects') renderProjectCard().catch(fail);
  if (host.editor.loaded) { host.editor.infoSig = null; host.editor.updateInfo(); }
}

// Every project is a row. The picked one carries its actions and opens to show its sounds.
export async function renderProjectCard() {
  const seq = ++projSeq, box = $('#projList');
  if (!state.projects.length) {
    box.replaceChildren(el('div', { class: 'empty' }, el('h3', {}, 'No projects yet'), el('p', { class: 'muted' }, 'A project is a folder of its own for what you are making. Create one, then add sounds from the Library: each is copied in, and the original stays where it is.')));
    return;
  }
  const p = projShut ? null : state.projects.find(x => x.id === projSel);
  /** @type {Item[]} */
  let items = p?.n ? await lib.search({ project: p.id, sort: 'name', limit: 5000 }) : [];
  if (p && items.length > 1) {
    const order = /** @type {string[]} */ (await lib.projectOrder(p.id)), at = new Map(order.map((path, k) => [path, k]));
    items.sort((a, b) => (at.get(a.path) ?? 1e9) - (at.get(b.path) ?? 1e9));
  }
  const pads = padView && p && items.length ? await lib.projectPads(p.id) : {};
  if (seq !== projSeq) return;
  const count = (/** @type {number} */ n) => `${n} sound${n === 1 ? '' : 's'}`;
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
      try { localStorage.setItem('athar.padView', padView ? '1' : ''); } catch { }
      renderProjectCard().catch(fail);
    }), padView ? 'on' : ''),
    ib('dynamic_feed', 'Batch: prep or convert every sound in this project', stop(() => host.batch(items))),
    ib('edit', 'Rename project', stop(() => rename(p))),
    ib('queue_music', 'Show its sounds in the Library', stop(() => { state.project = p.id; host.showView('sounds'); host.search(); })),
    p.folder ? ib('folder_open', 'Show in Finder', stop(() => revealFolder(/** @type {string} */ (p.folder)))) : null,
    ib('close', 'Remove from Athar. The files stay on disk', stop(async () => {
      if (!await confirmModal(`Remove ${p.name}?`, 'The project leaves Athar. Its folder and files stay on disk.', 'Remove')) return;
      try { await lib.deleteProject(p.id); }
      catch (e) { return fail(e); }
      if (state.project === p.id) { state.project = null; host.search(); }
      loadProjects().catch(fail);
    })),
  ];
  const out = [];
  for (const x of state.projects) {
    const on = !projShut && x.id === projSel, pick = () => { projShut = on; projSel = x.id; renderProjectCard().catch(fail); };
    out.push(focusable(el('div', { class: 'item' + (on ? ' on' : ''), onclick: pick },
      el('span', { class: 'label' }, x.name),
      el('span', { class: 'count' }, [count(on ? items.length : x.n || 0), diskSize(x.bytes)].filter(Boolean).join(', ')),
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
  host.refreshSide(); host.search();
  setTimeout(() => loadProjects().catch(() => { }), 2500);
  return true;
}

// Rows of a project can be dragged into the order the pads should have.
/** @type {string | null} */
let dragPath = null;
/** @type {number | null} */
let dragPad = null;
/** @param {Project} p @param {Item[]} items @param {string} from @param {string} to @param {boolean} after */
export async function moveInProject(p, items, from, to, after) {
  const paths = items.map(x => x.path).filter(x => x !== from);
  paths.splice(paths.indexOf(to) + (after ? 1 : 0), 0, from);
  try { await lib.setProjectOrder(p.id, { paths }); } catch (e) { return fail(e); }
  renderProjectCard().catch(fail);
}
/** @param {Project} p @param {Item} it @param {Item[]} [items] */
export function projectRow(p, it, items = []) {
  const over = (/** @type {DragEvent} */ e) => { const r = /** @type {HTMLElement} */ (e.currentTarget).getBoundingClientRect(); return e.clientY > r.top + r.height / 2; };
  return focusable(el('div', { class: 'prow', draggable: 'true', ondblclick: () => host.openPath(it), 'data-tip': 'Double-click to edit. Drag to reorder.',
    ondragstart: (/** @type {DragEvent} */ e) => { dragPath = it.path; e.dataTransfer?.setData('text/plain', it.name); if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'; },
    ondragend: () => { dragPath = null; document.querySelectorAll('.prow.drop-before, .prow.drop-after').forEach(n => n.classList.remove('drop-before', 'drop-after')); },
    ondragover: (/** @type {DragEvent} */ e) => {
      if (!dragPath || dragPath === it.path || dragPad != null) return;
      e.preventDefault(); const a = over(e), n = /** @type {HTMLElement} */ (e.currentTarget); n.classList.toggle('drop-after', a); n.classList.toggle('drop-before', !a);
    },
    ondragleave: (/** @type {DragEvent} */ e) => /** @type {HTMLElement} */ (e.currentTarget).classList.remove('drop-before', 'drop-after'),
    ondrop: (/** @type {DragEvent} */ e) => { if (!dragPath || dragPath === it.path || dragPad != null) return; e.preventDefault(); e.stopPropagation(); const from = dragPath; dragPath = null; moveInProject(p, items, from, it.path, over(e)); } },
    Object.assign(ib(host.isPlaying(it) ? 'stop' : 'play_arrow', host.isPlaying(it) ? 'Stop' : 'Play', (e) => { e.stopPropagation(); host.togglePreview(it); }, 'pplay'), { playPath: it.path }),
    host.miniWave(it),
    el('span', { class: 'qn' }, it.name),
    el('span', { class: 'qd' }, [it.category, fmtTime(it.duration)].filter(Boolean).join(' · ')),
    ib('close', 'Remove from project', async (e) => {
      e.stopPropagation();
      try { await lib.projectItem(p.id, { path: it.path, remove: true }); }
      catch (err) { return fail(err); }
      loadProjects().catch(fail);
    })), () => host.openPath(it));
}

// The SP-404MKII has ten banks of sixteen pads. Pad n is bank n / 16, pad n % 16, and a
// bank's pads read like text: 1 to 4 across the top row, 13 to 16 across the bottom.
export const BANKS = 'ABCDEFGHIJ', PADS = 160;
let padView = false, padBank = 0;
try { padView = localStorage.getItem('athar.padView') === '1'; } catch { }
export const padName = (/** @type {number} */ n) => `${BANKS[Math.floor(n / 16)]}${n % 16 + 1}`;

/** @param {Project} p @param {Item[]} items @param {Record<string, number>} pads */
export function padsArea(p, items, pads) {
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
    if (left) notice(`${left} sound${left === 1 ? '' : 's'} did not fit: the pads from ${BANKS[padBank]}1 to J16 are full`);
    save(next);
  };
  const clear = (/** @type {string} */ path) => { const next = { ...pads }; delete next[path]; save(next); };
  const endDrag = () => { dragPath = null; dragPad = null; document.querySelectorAll('.pad.over').forEach(x => x.classList.remove('over')); };
  const cell = (/** @type {number} */ n) => {
    const it = byPad.get(n);
    const c = el('div', {
      class: 'pad' + (it ? '' : ' empty') + (host.isPlaying(it) ? ' playing' : ''), draggable: it ? 'true' : 'false',
      onclick: () => { if (it) host.togglePreview(it); },
      ondblclick: () => { if (it) host.openPath(it); },
      ondragstart: (/** @type {DragEvent} */ e) => { if (!it) return void e.preventDefault(); const t = it; dragPath = t.path; dragPad = n; e.dataTransfer?.setData('text/plain', t.name); if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'; },
      ondragend: endDrag,
      ondragover: (/** @type {DragEvent} */ e) => { if (!dragPath) return; e.preventDefault(); /** @type {HTMLElement} */ (e.currentTarget).classList.add('over'); },
      ondragleave: (/** @type {DragEvent} */ e) => /** @type {HTMLElement} */ (e.currentTarget).classList.remove('over'),
      ondrop: (/** @type {DragEvent} */ e) => { if (!dragPath) return; e.preventDefault(); e.stopPropagation(); const path = dragPath; endDrag(); place(path, n); },
    },
      el('span', { class: 'pn' }, padName(n)),
      it ? el('span', { class: 'qn' }, it.name.replace(/\.[^.]+$/, '')) : null,
      it ? host.miniWave(it) : null,
      it ? ib('close', 'Clear pad', (e) => { e.stopPropagation(); clear(it.path); }, 'padx') : null);
    if (it) {
      c.padPath = it.path;
      c.addEventListener('keydown', (e) => { if (e.target === c && (e.key === 'Delete' || e.key === 'Backspace')) { e.preventDefault(); clear(it.path); } });
    }
    return focusable(c, () => it && host.togglePreview(it));
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
        el('span', { class: 'pn' }, '\u00a0'), el('span', { class: 'qn' }, it.name.replace(/\.[^.]+$/, '')), host.miniWave(it));
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
export async function exportProject(p) {
  const v = await modal(`Export ${p.name}`, [
    { label: 'Format', options: [['copy', 'Copy original files'], ['sp404', 'SP-404MKII project']] },
    { label: 'Name pattern: {project} {category} {n:03d} {name}', value: SETTINGS.name_pattern || '{project}_{name}', when: (f) => f !== 'sp404' },
  ]);
  if (!v) return;
  const card = v[0] === 'sp404' ? await lib.sp404Card().catch(() => ({})) : {};
  const dest = await pickFolder(v[0] === 'sp404' ? 'Choose the SD card' : 'Export to folder', card.import || '');
  if (!dest) return;
  try {
    const r = await lib.exportProject(p.id, { dest, pattern: v[1], format: v[0] });
    if (v[0] === 'sp404') notice(`Saved ${r.folder.split('/').pop()}`);
  } catch (e) { fail(e); }
}
