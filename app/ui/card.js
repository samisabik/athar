// The sound card: the top of the Library, for the sound selected.

import { $, el, ib, icon, fail, notice, dialog } from './ui.js';
import { fmtTime } from './time.js';
import { lib } from './native.js';
import { state } from './store.js';

/** @typedef {import('./search.js').Item} Item */
/** @typedef {import('./search.js').Main} Main */
/** @typedef {typeof import('./search.js')} Search */


// What the card uses from the rest of the Library page; main.js fills these in.
/** @typedef {Pick<Main, 'isPlaying' | 'openPath' | 'originChip' | 'placeChip' | 'refreshSide' | 'stopPreview' | 'togglePreview' | 'trashSound'>
      & Pick<Search, 'removeFromProject' | 'renderList' | 'row' | 'search' | 'select'>
      & { player: import('./player.js').Player, loadProjects: typeof import('./sidebar.js').loadProjects }} CardHost */
export const host = /** @type {CardHost} */ (/** @type {unknown} */ ({ player: null }));
// ---------- the sound card (top of the Library)
export const extOf = (/** @type {string} */ n) => (n.match(/\.[^.]+$/) || [''])[0];
export const stemOf = (/** @type {string} */ n) => n.replace(/\.[^.]+$/, '');

export async function loadTags() {
  state.allTags = await lib.tags();
  const gone = state.tag && !state.allTags.some(t => t.tag === state.tag);
  if (gone) state.tag = null;
  const sel = $('#tagFilter');
  sel.innerHTML = '';
  sel.append(el('option', { value: '' }, 'any'));
  state.allTags.forEach(t => sel.append(el('option', { value: t.tag }, `${t.tag} (${t.count})`)));
  sel.value = state.tag || '';
  if (gone) host.search();
}

export function drawCardWave() {
  const cv = $('#card canvas.big'); if (!cv || !state.cardItem) return;
  const g = cv.getContext('2d'), W = cv.width, H = cv.height;
  const css = getComputedStyle(document.documentElement);
  const p = host.isPlaying(state.cardItem) && host.player.duration ? host.player.currentTime / host.player.duration : 0;
  g.clearRect(0, 0, W, H);
  const peaks = state.cardItem.peaks || [], n = peaks.length || 1, bw = W / n;
  for (let i = 0; i < n; i++) {
    const h = Math.max(2, peaks[i] / 255 * (H - 16));
    g.fillStyle = i / n < p ? css.getPropertyValue('--hi') : css.getPropertyValue('--wave');
    g.fillRect(i * bw + 1, (H - h) / 2, Math.max(1, bw - 2), h);
  }
}

// Resolves to true when the saved item is still on the card, false when it moved on, null on failure.
/** @param {{ tags?: string[], memo?: string, rating?: number }} changes @param {string} [path] */
export async function saveCatalog(changes, path = state.cardItem?.path) {
  if (!path) return false;
  try {
    const r = await lib.catalog({ path, ...changes });
    const fresh = { tags: r.tags, memo: r.memo, rating: r.rating };
    const i = state.items.findIndex(x => x.path === path);
    if (i >= 0) {
      Object.assign(state.items[i], fresh);
      const n = $('#results').querySelector(`.qitem[data-i="${i}"]`);
      if (n) {
        const fresher = host.row(state.items[i], i, n.querySelector('.qmatch')?.style.width);
        const focused = n === document.activeElement;
        n.replaceWith(fresher);
        if (focused) fresher.focus({ preventScroll: true });
      }
    }
    if (state.cardItem?.path === path) Object.assign(state.cardItem, fresh);
    if ('tags' in changes) loadTags().catch(() => { });
    return state.cardItem?.path === path;
  } catch (e) { fail(e); return null; }
}

/** @param {string} stem */
export async function renameCard(stem) {
  const target = state.cardItem;
  if (!target || !stem.trim() || stem === stemOf(target.name)) return;
  const old = target.path;
  try {
    const { path } = await lib.rename({ path: old, name: stem.trim() });
    const name = path.split('/').pop();
    for (const x of [target, ...state.items]) if (x.path === old) Object.assign(x, { path, name });
    host.renderList();
    if (state.cardItem === target) renderCard();
  } catch (e) { fail(e); if (state.cardItem === target) renderCard(); }
}

/** @param {number} k */
export function setRating(k) {
  const it = state.cardItem;
  if (!it) return;
  saveCatalog({ rating: k }, it.path).then(here => { if (here) renderCard(); });
}

/** @type {number | undefined} */
let memoTimer;
/** @type {(() => void) | null} */
let memoFlush = null;
/** @type {ReturnType<typeof tagSuggest> | null} */
let cardSuggest = null;
export function renderCard() {
  memoFlush?.();
  cardSuggest?.close(); cardSuggest = null;
  const box = $('#card'); box.innerHTML = '';
  const it = state.cardItem;
  box.hidden = !it;
  if (!it) return;
  const meta = el('div', { class: 'meta-line' },
    el('span', {}, fmtTime(it.duration)),
    el('span', {}, `${it.sr ? (it.sr / 1000) + ' kHz' : ''} ${it.channels === 1 ? 'mono' : 'stereo'}`),
    el('span', {}, extOf(it.name).replace('.', '').toUpperCase()),
    host.placeChip(it), host.originChip(it));
  const cv = el('canvas', { class: 'big', width: 1200, height: 240, 'data-tip': 'Click to play from that point' });
  cv.onclick = (e) => {
    const r = cv.getBoundingClientRect(), f = (e.clientX - r.left) / r.width;
    const seek = () => { if (host.player.duration) host.player.currentTime = f * host.player.duration; };
    if (!host.isPlaying(it)) { host.togglePreview(it, true); host.player.addEventListener('loadedmetadata', seek); } else seek();
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
  const setTags = (/** @type {string[]} */ tags, refocus = false) => {
    const prev = it.tags;
    it.tags = tags;
    saveCatalog({ tags }, it.path).then((here) => {
      if (here === null) { it.tags = prev; if (state.cardItem === it) renderCard(); }
      else if (here) { renderCard(); if (refocus) $('#cardTags')?.focus(); }
    });
  };
  const tagbox = el('div', { class: 'tagbox', onclick: () => tagIn.focus() },
    (it.tags || []).map(t => el('span', { class: 'tchip' }, t, el('button', { type: 'button', 'data-tip': `Remove ${t}`, 'aria-label': `Remove ${t}`,
      onclick: (/** @type {MouseEvent} */ e) => { e.stopPropagation(); setTags(it.tags.filter(x => x !== t)); } }, icon('close')))), tagIn);
  const addTags = () => {
    const parts = tagIn.value.split(',').map(x => x.trim()).filter(Boolean);
    if (!parts.length) return;
    tagIn.value = '';
    setTags([...(it.tags || []), ...parts], true);
  };
  const sug = tagSuggest(tagIn, () => state.allTags.map(t => t.tag).filter(t => !(it.tags || []).includes(t)), (t) => { tagIn.value = t; addTags(); });
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
      await lib.projectItem(pid, { path: it.path });
      host.loadProjects().catch(() => { });
      const p = state.projects.find(x => String(x.id) === String(pid));
      notice(`Copied into the project ${p?.name || ''}. The original stays in the Library.`);
      setTimeout(() => host.refreshSide(), 2500);
    }
    catch (e) { fail(e); }
  };

  const actions = el('div', { class: 'actions' },
    ib(host.isPlaying(it) ? 'stop' : 'play_arrow', 'Play', () => playStop(it), 'strong lplay'),
    ib('repeat', 'Loop', () => { state.libLoop = !state.libLoop; host.player.loop = state.libLoop; renderCardPlayState(); }, 'toggle lloop' + (state.libLoop ? ' on' : '')),
    proj,
    el('span', { class: 'spacer' }),
    state.project ? ib('playlist_remove', 'Remove from this project', () => host.removeFromProject(it)) : null,
    ib('folder_open', 'Show in Finder', () => lib.reveal({ path: it.path }).catch(fail)),
    ib('delete', 'Delete file (Backspace)', () => host.trashSound(it)),
    el('button', { class: 'primary', onclick: () => host.openPath(it), 'data-tip': 'Open in the editor (E, or double-click in the list)' }, icon('edit'), ' Edit'));

  box.append(
    el('div', { class: 'stage-main' }, el('div', { class: 'stage-head' }, name, rating), meta, cv, actions),
    el('div', { class: 'stage-side' }, tagbox, memo));
  renderCardPlayState();
  drawCardWave();
}

// Tag suggestions in the app's own menu style instead of the browser's datalist.
/** @param {HTMLInputElement} input @param {() => string[]} options @param {(t: string) => void} pick */
export function tagSuggest(input, options, pick) {
  /** @type {HTMLElement | null} */
  let menu = null;
  /** @type {string[]} */
  let items = [];
  let active = -1;
  const close = () => { menu?.remove(); menu = null; active = -1; };
  const mark = (/** @type {number} */ i) => {
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
      onmousedown: (/** @type {MouseEvent} */ e) => { e.preventDefault(); close(); pick(t); } }, t)));
    document.body.append(menu);
    const r = input.closest('.tagbox')?.getBoundingClientRect() || input.getBoundingClientRect();
    menu.style.left = r.left + 'px'; menu.style.top = (r.bottom + 6) + 'px'; menu.style.minWidth = r.width + 'px';
  };
  input.addEventListener('input', show);
  input.addEventListener('focus', show);
  return {
    close,
    /** @param {KeyboardEvent} e */
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

/** @param {Item} it */
export function playStop(it) {
  if (host.isPlaying(it)) host.stopPreview(); else host.togglePreview(it, true);
}
export function renderCardPlayState() {
  const b = $('#card .actions .ib.lplay'), l = $('#card .actions .ib.lloop');
  if (b) {
    const here = host.isPlaying(state.cardItem);
    b.querySelector('.i').textContent = here ? 'stop' : 'play_arrow';
    const t = here ? 'Stop' : 'Play';
    b.dataset.tip = t; b.setAttribute('aria-label', t);
  }
  if (l) {
    l.classList.toggle('on', state.libLoop);
    const t = state.libLoop ? 'Loop is on' : 'Loop';
    l.dataset.tip = t; l.setAttribute('aria-label', t); l.setAttribute('aria-pressed', String(state.libLoop));
  }
}

/** @param {KeyboardEvent} e */
export function soundsKey(e) {
  if (e.ctrlKey || e.altKey || (e.metaKey && e.key !== 'Backspace')) return;
  const k = e.key.toLowerCase(), it = state.cardItem;
  if (k === 'j' || e.key === 'ArrowDown') { e.preventDefault(); host.select(Math.min(state.items.length - 1, state.selected + 1)); }
  else if (k === 'k' || e.key === 'ArrowUp') { e.preventDefault(); host.select(Math.max(0, state.selected - 1)); }
  else if (e.key === '/') { e.preventDefault(); $('#q').focus(); }
  else if (e.key === 'Escape') host.stopPreview();
  else if (!it) return;
  else if (k === ' ') { e.preventDefault(); host.togglePreview(it); }
  else if (/^[0-5]$/.test(k)) setRating(Number(k));
  else if (k === 'r') { e.preventDefault(); $('#cardName')?.select(); }
  else if (k === 't') { e.preventDefault(); $('#cardTags')?.focus(); }
  else if (k === 'n') { e.preventDefault(); $('#cardMemo')?.focus(); }
  else if (k === 'e') host.openPath(it);
  else if (e.key === 'Backspace' || e.key === 'Delete') { e.preventDefault(); host.trashSound(it); }
}

/** @type {Record<string, [string, [string, string][]]>} */
export const SHORTCUTS = {
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
export function showShortcuts() {
  const [name, keys] = SHORTCUTS[state.currentView] || SHORTCUTS.sounds;
  const rows = [...keys, ['Cmd K', 'Command palette'], ['?', 'This list']].map(([k, what]) => el('div', { class: 'keyrow' }, el('kbd', {}, k), el('span', {}, what)));
  dialog(`${name} shortcuts`, [el('div', { class: 'keys' }, rows)], null, 'Close', () => true);
}
