// Search, the results list, and the Library before any search.

import { $, el, ib, icon, fail, toast, modal, confirmModal, focusable } from './ui.js';
import { fmtTime } from './time.js';
import { lib } from './native.js';
import { state } from './store.js';
import { SETTINGS } from './settings.js';
import { reduceMotion } from './cat.js';
import { loadCats, loadFolders, loadProjects } from './sidebar.js';
import { loadTags, renderCard } from './card.js';

/** A sound of the Library, as a search returns it.
    @typedef {{ path: string, name: string, folder: string, sub: string, duration: number | null, sr: number | null, channels: number | null,
      tags: string[], memo: string, rating: number, origin: string | null, recorded: string | null, category: string | null,
      score: number, text_match: number, peaks: number[], tempo: number | null }} Item */
/** @typedef {{ id: number, name: string, folder: string | null, n: number, bytes: number }} Project */
/** A Library folder. @typedef {{ path: string, files: number, indexed: number, bytes: number, offline: boolean }} Folder */
/** A folder inside a Library folder. @typedef {{ path: string, name: string, files: number, nested: boolean }} Sub */
/** The state the library pages share (store.js).
    @typedef {{ query: string, category: string | null, folder: string | null, under: string | null, project: number | null,
      tag: string | null, minRating: number | string | null, sort: string, items: Item[], selected: number, projects: Project[],
      currentView: string, cardItem: Item | null, allTags: { tag: string, count: number }[], libLoop: boolean,
      subs: Sub[], indexed: number, playingItem: Item | null }} LibState */
/** What main.js provides to these pages.
    @typedef {{ isPlaying: (it: Item | null | undefined) => boolean, openPath: (it: Item) => Promise<void>,
      togglePreview: (it: Item, forceStart?: boolean) => void, stopPreview: () => void, refreshSide: () => void,
      placeChip: (it: Item) => HTMLElement, originChip: (it: Item) => HTMLElement | null, projectOf: (path: string) => Project | null,
      trashSound: (it: Item) => Promise<void>, showView: (name: string) => void, batch: (items: Item[]) => Promise<void> }} Main */


// What search uses from the rest of the window; main.js fills these in.
export const host = /** @type {Pick<Main, 'isPlaying' | 'openPath' | 'placeChip' | 'projectOf' | 'refreshSide' | 'stopPreview' | 'togglePreview'>} */ ({});
// ---------- search
let searchSeq = 0;
export const SORTS = ['name', 'duration', 'category', 'rating', 'recorded'];
export const hasQuery = () => !!state.query;
export const hasFilter = () => !!(state.folder || state.project || state.category || state.tag || state.minRating);

export function filters() {
  const { default_sort } = SETTINGS;
  const defaultSort = SORTS.includes(default_sort) ? default_sort : 'name';
  return {
    category: state.category, folder: state.folder, under: state.folder ? state.under : null, project: state.project,
    min_dur: null,
    tag: state.tag || null, min_rating: state.minRating || null,
    sort: state.sort === 'score' && !hasQuery() ? defaultSort : state.sort,
    limit: 500,
  };
}

export function updateFilterBadge() {
  const n = [state.category, state.tag, state.minRating].filter(Boolean).length;
  const b = $('#filterCount'); b.hidden = !n; b.textContent = n;
  for (const id of ['catFilter', 'tagFilter', 'ratingFilter']) {
    const s = $('#' + id);
    s.closest('.f').classList.toggle('set', !!s.value);
  }
}

export async function search() {
  updateFilterBadge();
  const seq = ++searchSeq;
  if (!hasQuery() && !hasFilter()) {
    state.items = []; state.selected = -1; render();
    return;
  }
  try {
    /** @type {Item[]} */
    const items = await lib.search({ ...filters(), query: state.query || null });
    if (seq !== searchSeq) return;
    const keep = items.findIndex(x => x.path === state.cardItem?.path);
    state.items = items;
    state.selected = keep >= 0 ? keep : (items.length ? 0 : -1);
    render();
  } catch (e) { if (seq === searchSeq) fail(e); }
}

export function scopeText() {
  const n = state.items.length;
  if (!n) return '';
  return `${n} sound${n === 1 ? '' : 's'}` + (state.query ? ` · like "${state.query}"` : '');
}

export function renderChips() {
  const box = $('#chips'); box.innerHTML = '';
  const chips = /** @type {[string, string, () => Promise<void> | undefined][]} */ ([
    state.folder && ['folder', state.folder.split('/').pop(), () => { state.folder = null; state.under = null; return loadFolders(); }],
    state.category && ['category', state.category, () => { state.category = null; return loadCats(); }],
    state.tag && ['sell', state.tag, () => { state.tag = null; $('#tagFilter').value = ''; }],
    state.minRating && ['star', `${state.minRating}+`, () => { state.minRating = null; $('#ratingFilter').value = ''; }],
    state.project && ['category', state.projects.find(p => p.id === state.project)?.name || 'Project', () => { state.project = null; return loadProjects(); }],
  ].filter(Boolean));
  for (const [ic, label, clear] of chips) {
    const crumbs = ic === 'folder' ? folderCrumbs() : [label];
    box.append(el('span', { class: 'fchip' }, icon(ic), crumbs,
      ib('close', `Show all, not only ${label}`, async () => { await clear()?.catch(fail); search(); })));
  }
  box.hidden = !chips.length;
  renderSubs();
}

// The folder chip reads as a path: each part goes back up to that level.
export function folderCrumbs() {
  const root = /** @type {string} */ (state.folder), parts = state.under ? state.under.slice(root.length + 1).split('/') : [];
  const go = (/** @type {string | null} */ under) => () => { state.under = under; search(); };
  /** @type {HTMLElement[]} */
  const out = [el('button', { class: 'crumb', onclick: go(null) }, root.split('/').pop())];
  parts.forEach((p, i) => out.push(el('span', { class: 'crumb-sep' }, '/'),
    el('button', { class: 'crumb', onclick: go(`${root}/${parts.slice(0, i + 1).join('/')}`) }, p)));
  return out;
}

let subsSeq = 0;
export async function renderSubs() {
  const box = $('#subs'), at = state.folder && (state.under || state.folder), seq = ++subsSeq;
  /** @type {Sub[]} */
  let subs = [];
  if (at) { try { subs = await lib.subfolders({ path: at }); } catch { subs = []; } }
  if (seq !== subsSeq) return;
  box.innerHTML = '';
  for (const sf of subs) {
    box.append(el('button', { class: 'subchip', 'data-tip': `Only the sounds in ${sf.name}`, onclick: () => { state.under = sf.path; search(); } },
      icon(sf.nested ? 'folder_copy' : 'folder'), sf.name, el('span', { class: 'count' }, String(sf.files))));
  }
  box.hidden = !subs.length;
}

export function render() {
  renderChips();
  $('#scope').textContent = scopeText();
  renderList();
  if (state.items.length) select(Math.max(0, state.selected), { scroll: false });
  else { state.cardItem = null; renderCard(); }
}

export function renderList() {
  const box = $('#results'), lib = $('.lib');
  const had = box.contains(document.activeElement) ? (document.activeElement?.dataset.sort || 'row') : null;
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

export function dropZone() {
  return el('div', { class: 'dropzone' },
    el('div', { class: 'dz-box' }, el('h3', {}, 'Add a folder of recordings'),
      el('button', { class: 'primary', onclick: () => $('#addFolder').click() }, icon('create_new_folder'), ' Choose a folder')));
}

// ---------- the Library before any search: the search box sits in the middle of the page under a
// line picked at random, and its placeholder types out things to look for.
export const HELLOS = ['What does it sound like?', 'Say it in words.', 'What are you hearing?', 'Name that noise.', 'Go on, describe it.', 'Looking for a noise?', 'What should it sound like?', 'Hum it in words.'];
export const TRY_PHRASES = [
  'rain on a tin roof', 'lonely metallic ping', 'fat sub kick', 'dusty vinyl crackle', 'crowd in a big hall', 'wooden door creak',
  'birds at dawn', 'glass breaking', 'warm tape hiss', 'tight snare with a ring', 'distant thunder', 'footsteps on gravel',
  'metal scrape', 'soft synth pad', 'train passing', 'water dripping in a cave', 'something angry', 'something tiny', 'underwater',
  'a room with nobody in it', 'engine starting', 'bell far away', 'paper tearing', 'low hum',
];
export const PLACEHOLDER = 'Describe a sound';
let typing = 0;
// The search box stays in the middle until a search finds something.
export function syncIdleSearch() { setIdleSearch(!state.items.length && !hasFilter()); }
/** @param {boolean} idle */
export function setIdleSearch(idle) {
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

export function emptyState() {
  if (!hasQuery() && !hasFilter()) return el('div');
  return el('div', { class: 'empty' }, el('h3', {}, 'Nothing matches'));
}

export const MINI_BARS = 48, MINI_H = 24;

// The sound's shape as one SVG path.
/** @param {Item} it */
export function miniWave(it) {
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
/** @type {Set<string>} */
export const marked = new Set();
/** @param {{ metaKey?: boolean, ctrlKey?: boolean, shiftKey?: boolean }} e @param {number} i */
export function rowClick(e, i) {
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
export function paintMarks() {
  for (const p of [...marked]) if (!state.items.some(x => x.path === p)) marked.delete(p);
  $('#results').querySelectorAll('.qitem[data-i]').forEach((/** @type {HTMLElement} */ r) => r.classList.toggle('marked', marked.has(state.items[Number(r.dataset.i)]?.path)));
  const bar = $('#markBar'), n = marked.size;
  bar.hidden = !n;
  if (!n) return bar.replaceChildren();
  const items = () => state.items.filter(x => marked.has(x.path));
  // Runs one request per marked sound. A failure stops the run: what was done loses its mark,
  // what was not keeps it, and the message counts only what was done.
  /** @param {(it: Item) => Promise<unknown>} fn @param {(k: number) => string} done */
  const each = async (fn, done) => {
    let ok = 0;
    for (const it of items()) { try { await fn(it); marked.delete(it.path); ok++; } catch (e) { fail(e); break; } }
    await search();
    if (state.cardItem) renderCard();
    if (ok) toast(done(ok));
  };
  const proj = el('select', { 'aria-label': 'Add the marked sounds to a project' }, el('option', { value: '' }, state.projects.length ? 'Add to project' : 'No projects yet'), state.projects.map(p => el('option', { value: p.id }, p.name)));
  proj.onchange = () => { const pid = proj.value; if (pid) each(it => lib.projectItem(pid, { path: it.path }), k => `Copied ${k} sound${k === 1 ? '' : 's'} into the project`).then(() => loadProjects().catch(() => { })); };
  const rate = el('select', { 'aria-label': 'Rate the marked sounds' }, el('option', { value: '' }, 'Rate'), [1, 2, 3, 4, 5, 0].map(k => el('option', { value: k }, k ? `${k}` : 'Clear rating')));
  rate.onchange = () => { if (rate.value !== '') each(it => lib.catalog({ path: it.path, rating: +rate.value }), k => `Rated ${k} sound${k === 1 ? '' : 's'}`); };
  bar.replaceChildren(el('span', { class: 'markn' }, `${n} marked`), proj, rate,
    ib('sell', 'Add tags to the marked sounds', async () => {
      const v = (await modal(`Tag ${n} sounds`, [{ label: 'Tags to add, separated by commas', placeholder: 'dusty, tape' }]))?.[0];
      const add = (v || '').split(',').map(t => t.trim()).filter(Boolean);
      if (add.length) each(it => lib.catalog({ path: it.path, tags: [...new Set([...(it.tags || []), ...add])] }), k => `Tagged ${k} sound${k === 1 ? '' : 's'}`).then(() => loadTags().catch(() => { }));
    }),
    ib('delete', 'Move the marked files to the Trash', async () => {
      if (!await confirmModal(`Delete ${n} files?`, 'This deletes the files themselves, not just their place in Athar. They go to the Trash.', 'Delete')) return;
      host.stopPreview(); each(it => lib.trash({ path: it.path }), k => `Moved ${k} file${k === 1 ? '' : 's'} to the Trash`).then(() => host.refreshSide());
    }),
    ib('close', 'Clear the marks', () => { marked.clear(); paintMarks(); }));
}

/** @param {Item} it @param {number} i @param {string} [width] */
export function row(it, i, width) {
  /** @param {MouseEvent} [e] */
  const activate = (e) => rowClick(e || {}, i);
  const r = el('div', {
    class: 'qitem trow' + (i === state.selected ? ' on' : '') + (host.isPlaying(it) ? ' playing' : '') + (marked.has(it.path) ? ' marked' : ''),
    'data-i': i, tabindex: i === state.selected ? '0' : '-1', 'data-tip': 'Double-click to edit',
    onclick: activate, ondblclick: () => host.openPath(it),
    onkeydown: (/** @type {KeyboardEvent} */ e) => { if (e.key === 'Enter' && e.target === e.currentTarget) { e.preventDefault(); e.stopPropagation(); activate(); } },
  },
  miniWave(it),
  el('span', { class: 'qn' }, it.name, host.projectOf(it.path) ? null : subLabel(it)),
  el('span', { class: 'rtags' }, host.projectOf(it.path) && !state.project ? host.placeChip(it) : null, (it.tags || []).map(t => el('span', { class: 'rtag' }, t))),
  el('span', { class: 'tc' }, it.category || ''),
  el('span', { class: 'tc num' }, fmtTime(it.duration)),
  el('span', { class: 'tc num' }, (it.recorded || '').slice(0, 10).replace(/:/g, '-')),
  el('span', { class: 'rowrate' }, Array.from({ length: it.rating || 0 }, () => el('i'))));
  if (hasQuery()) r.append(el('span', { class: 'qmatch', style: `width:${width || '10%'}` }));
  return r;
}

// The subfolders a sound sits in, below whatever level is being shown.
/** @param {Item} it */
export function subLabel(it) {
  if (!it.sub) return null;
  const shown = state.folder === it.folder && state.under ? state.under.slice(it.folder.length + 1) : '';
  const rest = shown && (it.sub === shown || it.sub.startsWith(shown + '/')) ? it.sub.slice(shown.length + 1) : it.sub;
  return rest ? el('span', { class: 'qsub' }, rest) : null;
}

export function tableHead() {
  /** @param {string} label @param {string} [key] */
  const col = (label, key) => {
    if (!key) return el('span', { class: 'th' }, label);
    const on = filters().sort === key;
    const sort = () => { state.sort = on && hasQuery() ? 'score' : key; $('#sort').value = state.sort; search(); };
    const tip = on && hasQuery() ? 'Back to best match first' : `Sort by ${label}`;
    return focusable(el('span', { class: 'th sortable' + (on ? ' on' : ''), role: 'button', 'data-sort': key, onclick: sort, 'data-tip': tip }, label), sort);
  };
  return el('div', { class: 'trow thead' }, el('span'), col('name', 'name'), col('tags'), col('type', 'category'), col('length', 'duration'), col('recorded', 'recorded'), col('rating', 'rating'));
}

/** @param {number} i */
export function select(i, { scroll = true, play = false } = {}) {
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
  const next = state.items[i], cur = state.cardItem, changed = cur?.path !== next.path;
  if (changed) { state.cardItem = next; renderCard(); }
  else if (cur !== next) { Object.assign(cur, next); state.items[i] = cur; }
  if (play || (scroll && changed && SETTINGS.autoplay)) host.togglePreview(/** @type {Item} */ (state.cardItem), true);
}

/** @param {Item} it */
export async function removeFromProject(it) {
  const pid = state.project;
  if (!pid) return;
  try { await lib.projectItem(pid, { path: it.path, remove: true }); }
  catch (e) { return fail(e); }
  loadProjects().catch(() => { });
  search();
}
