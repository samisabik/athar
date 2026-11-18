// Settings: kept by the app, applied everywhere, and the page that changes them.

import { $, el, ib, fail, notice, confirmModal, pickFolder } from './ui.js';
import { lib, app } from './native.js';
import { setCatSkin } from './cat.js';
import { CAT_SKINS } from './cats.js';

/** The settings as last loaded; changed in place, so every importer sees the current ones. @type {Record<string, any>} */
export const SETTINGS = /** @type {Record<string, any>} */ ({});

// Other parts of the window follow the settings they depend on, without this file knowing them.
/** @type {{ keys: string[], fn: () => void }[]} */
const watchers = [];
/** Calls `fn` once the settings have loaded and again whenever one of `keys` changes. @param {string[]} keys @param {() => void} fn */
export function onSetting(keys, fn) { watchers.push({ keys, fn }); }
/** Called after a backup has been restored: the tags, ratings and projects on the pages are stale. @type {(() => void) | null} */
let afterRestore = null;
/** @param {() => void} fn */
export function onRestore(fn) { afterRestore = fn; }

/** @type {[string, string][]} */
const ACCENTS = [
  ['#7a3dff', 'Ultraviolet'], ['#bc13fe', 'Neon purple'], ['#1f51ff', 'Neon blue'], ['#00f0ff', 'Neon cyan'],
  ['#39ff14', 'Neon green'], ['#e7ff00', 'Neon yellow'], ['#ff6a00', 'Neon orange'], ['#ff073a', 'Neon red'],
  ['#ff10f0', 'Neon pink'],
];
/** @typedef {{ options: [string | number, string][] }} SelectOpts */
/** @typedef {{ options: [string, string][] }} SwatchOpts */
/** @typedef {{ min: number, max: number, step: number, unit: string, onRelease?: boolean }} NumberOpts */
/** @typedef {{ hint?: string }} TextOpts */
/** One setting on the page: its key, its label, the control that changes it, and that control's options.
    @typedef {[string, string, 'toggle']
      | [string, string, 'select', SelectOpts]
      | [string, string, 'swatch', SwatchOpts]
      | [string, string, 'range' | 'number', NumberOpts]
      | [string, string, 'text' | 'folder', TextOpts?]} SettingRow */
/** @typedef {SettingRow[2]} SettingKind */
/** @typedef {SelectOpts | SwatchOpts | NumberOpts | TextOpts} SettingOpts */
/** @type {[string, SettingRow[]][]} */
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
    ['name_pattern', 'Project file names', 'text', { hint: 'Use {project}, {category}, {name} and {n:03d} for a three-digit counter' }],
  ]],
  ['SP-404MKII', [
    ['sp404_mono', 'Convert to mono (saves sample memory)', 'toggle'],
  ]],
];
// Settings that seed the editor's own fields; written only when they change so per-file choices survive.
/** @type {Record<string, string>} */
const EDITOR_FIELDS = { export_rate: 'rate', export_bits: 'bits', export_format: 'format', prep_target_db: 'norm', silence_db: 'thresh', slice_gap_ms: 'gap', loudness_target: 'lufs' };

// One accent colour, used at full strength everywhere; soft backgrounds are neutral grey.
/** @param {string} hex */
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
  const mix = (/** @type {number} */ k, /** @type {number} */ to) => '#' + c.map(v => Math.round(v + (to - v) * k).toString(16).padStart(2, '0')).join('');
  r.setProperty('--accent-text', dark ? (light ? hex : mix(.45, 255)) : (light ? mix(.45, 0) : hex));
}
// Share Tech Mono draws smaller than its nominal size, so the Text size setting is scaled up to match.
const TYPE_SCALE = 1.1;
const darkQuery = window.matchMedia('(prefers-color-scheme: dark)');
darkQuery.addEventListener?.('change', () => { if ((SETTINGS.theme || 'system') === 'system') applySetting('theme'); });

// What each setting changes on the page itself. The accent's text shade depends on the theme.
/** @type {Record<string, () => void>} */
const APPLY = {
  theme: () => {
    document.documentElement.dataset.theme = SETTINGS.theme === 'dark' || (SETTINGS.theme !== 'light' && darkQuery.matches) ? 'dark' : 'light';
    applyAccent(SETTINGS.accent);
  },
  text_size: () => document.documentElement.style.setProperty('--fs', SETTINGS.text_size * TYPE_SCALE + 'px'),
  accent: () => applyAccent(SETTINGS.accent),
  show_cat: () => { $('#cat').hidden = !SETTINGS.show_cat; },
  cat_skin: () => setCatSkin(SETTINGS.cat_skin),
};
/** @param {string} key */
function applySetting(key) {
  APPLY[key]?.();
  for (const w of watchers) if (w.keys.includes(key)) w.fn();
}
/** Applies every setting; called once they have loaded. */
export function applySettings() {
  for (const apply of Object.values(APPLY)) apply();
  for (const w of watchers) w.fn();
}
export function applyEditorFields(keys = Object.keys(EDITOR_FIELDS)) {
  for (const k of keys) {
    const n = $(`#editor [data-f=${EDITOR_FIELDS[k]}]`);
    if (!n || SETTINGS[k] == null) continue;
    n.value = String(SETTINGS[k]);
    // Opening another sound puts every tool field back to its default, which is this setting.
    if (n instanceof HTMLInputElement) n.defaultValue = n.value;
  }
}

// Changes are applied at once and stored together a moment after they stop.
/** @type {ReturnType<typeof setTimeout> | undefined} */
let saveTimer;
/** @type {Record<string, any>} */
let pending = {};
/** @param {string} key @param {any} value */
export function saveSetting(key, value) {
  SETTINGS[key] = value;
  pending[key] = value;
  applySetting(key);
  if (key in EDITOR_FIELDS) applyEditorFields([key]);
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    const batch = pending; pending = {};
    lib.setSettings(batch).catch(fail);
  }, 250);
}

/** @param {string} key @param {string} label @param {[string, string][]} options */
function swatchControl(key, label, options) {
  const buttons = options.map(([v, l]) => el('button', { type: 'button', class: 'swatch', style: `--sw: ${v}`, 'data-v': v, 'data-tip': l, 'aria-label': l, role: 'radio' }));
  // Any colour at all: the system colour panel behind a rainbow dot, which shows the pick once made.
  const picker = el('input', { type: 'color', 'aria-label': 'Pick any colour' });
  const pick = el('label', { class: 'swatch-pick', 'data-tip': 'Pick any colour' }, picker);
  const mark = (/** @type {string} */ v) => {
    const any = buttons.some(b => b.dataset.v === v);
    pick.classList.toggle('on', !any && /^#[0-9a-f]{6}$/.test(v));
    if (!any && /^#[0-9a-f]{6}$/.test(v)) { pick.style.setProperty('--sw', v); picker.value = v; }
    buttons.forEach((b, k) => {
      const on = b.dataset.v === v;
      b.classList.toggle('on', on); b.setAttribute('aria-checked', String(on));
      b.tabIndex = on || (!any && k === 0) ? 0 : -1;
    });
  };
  // A colour dragged in the system panel is saved once the dragging rests.
  /** @type {number | undefined} */
  let pickTimer;
  const choose = (/** @type {HTMLButtonElement} */ b) => { clearTimeout(pickTimer); mark(/** @type {string} */ (b.dataset.v)); saveSetting(key, b.dataset.v); };
  buttons.forEach((b, k) => {
    b.onclick = () => choose(b);
    b.onkeydown = (e) => {
      const d = /** @type {Record<string, number>} */ ({ ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 })[e.key];
      if (!d) return;
      e.preventDefault();
      const n = buttons[(k + d + buttons.length) % buttons.length];
      n.focus(); choose(n);
    };
  });
  picker.oninput = () => {
    const v = picker.value.toLowerCase();
    mark(v); applyAccent(v);
    clearTimeout(pickTimer); pickTimer = setTimeout(() => saveSetting(key, v), 250);
  };
  mark(String(SETTINGS[key] || '').toLowerCase());
  return el('span', { class: 'swatches', role: 'radiogroup', 'aria-label': label }, buttons, pick);
}

/** @param {string} key @param {string} id @param {SettingKind} kind @param {SettingOpts} o */
function settingControl(key, id, kind, o) {
  if (kind === 'toggle') {
    const c = el('input', { type: 'checkbox', id, class: 'switch' });
    c.checked = !!SETTINGS[key];
    c.onchange = () => saveSetting(key, c.checked);
    return c;
  }
  if (kind === 'select') {
    const { options } = /** @type {SelectOpts} */ (o);
    const s = el('select', { id }, options.map(([v, l]) => el('option', { value: v }, l)));
    s.value = String(SETTINGS[key]);
    s.onchange = () => saveSetting(key, typeof options[0][0] === 'number' ? Number(s.value) : s.value);
    return s;
  }
  if (kind === 'range') {
    const { min, max, step, unit, onRelease } = /** @type {NumberOpts} */ (o);
    const out = el('output', {}, `${SETTINGS[key]}${unit}`);
    const r = el('input', { type: 'range', id, min, max, step });
    r.value = SETTINGS[key];
    // A setting that resizes the page is applied when the ruler is let go, so the ruler does not move under the pointer.
    r.oninput = () => { out.textContent = `${r.value}${unit}`; if (!onRelease) saveSetting(key, Number(r.value)); };
    if (onRelease) r.onchange = () => saveSetting(key, Number(r.value));
    return el('span', { class: 'rangewrap' }, r, out);
  }
  if (kind === 'number') {
    const { min, max, step, unit } = /** @type {NumberOpts} */ (o);
    const n = el('input', { type: 'number', id, min, max, step });
    n.value = SETTINGS[key];
    n.onchange = () => {
      const raw = Number(n.value);
      if (n.value === '' || !Number.isFinite(raw)) { n.value = SETTINGS[key]; return; }
      const v = Math.min(max, Math.max(min, raw));
      n.value = String(v); saveSetting(key, v);
    };
    return el('span', { class: 'numwrap' }, n, el('span', { class: 'unit' }, unit));
  }
  const t = el('input', { id, spellcheck: 'false', placeholder: /** @type {TextOpts} */ (o).hint || '' });
  t.value = SETTINGS[key] ?? '';
  t.onchange = () => saveSetting(key, t.value);
  return t;
}

export function renderSettings() {
  const box = $('#settingsForm'); box.innerHTML = '';
  for (const [group, rows] of SETTINGS_SCHEMA) {
    const sec = el('div', { class: 'sgroup' }, el('h2', {}, group));
    for (const [key, label, kind, o = {}] of rows) {
      const id = 'set-' + key;
      const { hint } = /** @type {TextOpts} */ (o);
      const name = el('span', { class: 'slabel' }, label, hint ? el('small', {}, hint) : null);
      sec.append(kind === 'folder'
        ? el('div', { class: 'srow' }, name, el('span', { class: 'sfolder' }, el('span', { class: 'mono', 'data-tip': SETTINGS[key] }, SETTINGS[key]),
          el('button', { class: 'ghost', onclick: async () => { const d = await pickFolder(label); if (d) { saveSetting(key, d); renderSettings(); } } }, 'Change')))
        : kind === 'swatch'
        ? el('div', { class: 'srow' }, name, swatchControl(key, label, /** @type {SwatchOpts} */ (o).options))
        : el('label', { class: 'srow', for: id }, name, settingControl(key, id, kind, o)));
    }
    box.append(sec);
  }
  box.append(el('div', { class: 'sgroup' }, el('h2', {}, 'Backup'),
    el('div', { class: 'srow' }, el('span', { class: 'slabel' }, 'Your tags, ratings, notes, regions and projects', el('small', {}, 'One file you can keep somewhere safe, or carry to another Mac')),
      el('span', { class: 'sfolder' },
        el('button', { class: 'ghost', onclick: async () => {
          const dir = await pickFolder('Save the backup into'); if (!dir) return;
          try { notice(`Backup saved: ${(await lib.backup({ path: dir })).path}`, 8000); } catch (e) { fail(e); }
        } }, 'Back up'),
        el('button', { class: 'ghost', onclick: async () => {
          const file = await app.pickFile();
          if (!file) return;
          if (!await confirmModal('Restore this backup?', 'Tags, ratings and notes in the file replace the current ones for the sounds it names. Projects in the file are added.', 'Restore')) return;
          try { const r = await lib.restore({ path: file }); notice(`Restored ${r.sounds} sounds and ${r.projects} projects`); afterRestore?.(); } catch (e) { fail(e); }
        } }, 'Restore')))));
  box.append(el('div', { class: 'sgroup' }, el('h2', {}, 'About'),
    el('div', { class: 'srow' }, el('span', { class: 'slabel' }, 'Version'), el('span', { class: 'mono' }, SETTINGS._version || '')),
    el('div', { class: 'srow' }, el('span', { class: 'slabel' }, el('span', { id: 'statusRight' }), el('small', {}, `Library and settings: ${SETTINGS._data_dir || ''}`)),
      ib('folder_open', 'Show the library data in Finder', () => lib.revealData().catch(fail)))));
}
