// The window's shared pieces: the message bar, building elements, and dialogs.

import { app } from './native.js';

/** An element of the page that is always there, by its selector. @param {string} s @returns {any} */
export const $ = (s) => document.querySelector(s);
/** Asks for a folder with a picker titled `title` (what the folder is for), starting in `start`;
    resolves its path, or null when cancelled.
    @param {string} [title] @param {string} [start] @returns {Promise<string | null>} */
export const pickFolder = (title = '', start = '') => app.pickFolder(title, start);

export const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

// Small choices the window keeps between launches (a panel left open, the last project picked).
/** @param {string} key @param {string | null} value null forgets it */
export function remember(key, value) { try { value == null ? localStorage.removeItem(key) : localStorage.setItem(key, value); } catch { } }
/** @param {string} key @returns {string | null} */
export function recall(key) { try { return localStorage.getItem(key); } catch { return null; } }

// Every message goes to the one bar at the bottom of the window: short confirmations, notices
// that stay a little longer, and (below them, for as long as it lasts) progress or a problem.
export const toast = (/** @type {string} */ msg) => { if (msg) notice(msg, 3500); };
/** @type {number | undefined} */
let noticeTimer;
/** @param {string} msg */
export function notice(msg, ms = 4000) {
  const t = $('#toast'); t.textContent = msg; t.classList.add('show');
  clearTimeout(noticeTimer); noticeTimer = setTimeout(() => t.classList.remove('show'), ms);
}
// A message that stays in the bottom bar while something is being worked out; '' takes it away.
let busyText = '';
/** @param {string} text */
export function busy(text) {
  const t = $('#toast');
  if (text) { busyText = text; notice(text, 600000); }
  else if (busyText && t.textContent === busyText) { busyText = ''; clearTimeout(noticeTimer); t.classList.remove('show'); }
}
export const fail = (/** @type {unknown} */ e) => notice((e instanceof Error ? e.message : '') || String(e), 5000);

/** What an element can hold: other nodes, text, or nothing (skipped). @typedef {Node | string | number | null | undefined | false} Kid */
/** Makes an element: `class`, `on…` handlers and other attributes, then its children.
    @template {keyof HTMLElementTagNameMap} K
    @param {K} tag @param {Record<string, any>} [attrs] @param {...(Kid | Kid[])} kids
    @returns {HTMLElementTagNameMap[K]} */
export function el(tag, attrs = {}, ...kids) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'class') n.className = v;
    else if (k.startsWith('on')) n.addEventListener(k.slice(2), v);
    else if (v != null) n.setAttribute(k, v);
  }
  for (const k of kids.flat()) if (k != null && k !== false) n.append(typeof k === 'object' ? k : document.createTextNode(String(k)));
  return n;
}

export const icon = (/** @type {string} */ name) => el('span', { class: 'i' }, name);
/** An icon button. @param {string} name @param {string} tip @param {(e: MouseEvent) => void} onclick */
export function ib(name, tip, onclick, cls = '') {
  return el('button', { class: `ib ${cls}`, 'data-tip': tip, 'aria-label': tip, onclick }, icon(name));
}

// Makes a non-button element reachable with Tab and activatable with Enter or Space.
/** @template {HTMLElement} T @param {T} n @param {() => void} activate @returns {T} */
export function focusable(n, activate) {
  n.tabIndex = 0;
  n.addEventListener('keydown', (e) => {
    if (e.target !== n || (e.key !== 'Enter' && e.key !== ' ')) return;
    e.preventDefault(); e.stopPropagation();
    activate();
  });
  return n;
}

// Types phrases into an empty search box as its placeholder: one is typed out, held, wiped, then
// the next. `next` gives each phrase. Returns what stops the typing.
/** @param {HTMLInputElement} input @param {() => string} next @returns {() => void} */
export function typePlaceholder(input, next) {
  /** @type {number | undefined} */
  let timer;
  let phrase = '', n = 0, wiping = false;
  const step = () => {
    if (!wiping && n === 0) phrase = next();
    n += wiping ? -2 : 1;
    if (n >= phrase.length) { n = phrase.length; wiping = true; input.placeholder = phrase; timer = setTimeout(step, 1900); return; }
    if (n <= 0) { n = 0; wiping = false; }
    input.placeholder = phrase.slice(0, n) || ' ';
    timer = setTimeout(step, wiping ? 18 : 55 + Math.random() * 50);
  };
  step();
  return () => clearTimeout(timer);
}

// A control that has a tooltip and no label of its own is named by its tooltip.
const NEEDS_LABEL = '[data-tip]:is(button, select, input):not([aria-label])';
const labelFromTip = (/** @type {Element} */ n) => n.setAttribute('aria-label', n.dataset.tip || '');
/** @param {Element | Document} root */
function labelControls(root) {
  if ('matches' in root && root.matches(NEEDS_LABEL)) labelFromTip(root);
  root.querySelectorAll(NEEDS_LABEL).forEach(labelFromTip);
}
labelControls(document);
new MutationObserver((muts) => {
  for (const m of muts) for (const n of m.addedNodes) if (n.nodeType === 1) labelControls(/** @type {Element} */ (n));
}).observe($('#editor'), { childList: true, subtree: true });

// There is one dialog. Opening another while it shows cancels the one showing first.
/** @type {(() => void) | null} */
let cancelOpenDialog = null;
/** Shows the dialog with `nodes`; resolves what `value()` gives on OK, 'alt' for the other
    button, or null.
    @template T @param {string} title @param {Node[]} nodes @param {HTMLElement | null} first @param {string} okLabel @param {() => T} value @param {string | null} [altLabel]
    @returns {Promise<T | 'alt' | null>} */
export function dialog(title, nodes, first, okLabel, value, altLabel = null) {
  cancelOpenDialog?.();
  return new Promise((resolve) => {
    const box = $('#modal'), card = box.querySelector('.modal-card'), ok = $('#modalOk'), cancel = $('#modalCancel'), alt = $('#modalAlt');
    alt.hidden = !altLabel; alt.textContent = altLabel || '';
    const back = /** @type {HTMLElement | null} */ (document.activeElement);
    $('#modalTitle').textContent = title;
    $('#modalBody').replaceChildren(...nodes);
    ok.textContent = okLabel;
    box.hidden = false;
    (first || ok).focus();
    const done = (/** @type {T | 'alt' | null} */ v) => {
      cancelOpenDialog = null;
      box.hidden = true; ok.onclick = cancel.onclick = alt.onclick = null;
      document.removeEventListener('keydown', key, true);
      if (back?.isConnected) back.focus();
      resolve(v);
    };
    const key = (/** @type {KeyboardEvent} */ e) => {
      if (e.defaultPrevented) return;
      if (e.key === 'Escape') done(null);
      else if (e.key === 'Enter' && /** @type {HTMLInputElement} */ (e.target).tagName === 'INPUT') done(value());
      else if (e.key === 'Tab') {
        const f = /** @type {HTMLElement[]} */ ([...card.querySelectorAll('input, select, textarea, button')]).filter(n => !n.disabled && !n.hidden);
        const i = f.indexOf(/** @type {HTMLElement} */ (document.activeElement));
        f[e.shiftKey ? (i <= 0 ? f.length - 1 : i - 1) : (i + 1) % f.length]?.focus();
      } else return;
      e.preventDefault(); e.stopPropagation();
    };
    document.addEventListener('keydown', key, true);
    cancelOpenDialog = () => done(null);
    ok.onclick = () => done(value());
    cancel.onclick = () => done(null);
    alt.onclick = () => done('alt');
  });
}

/** A field of a dialog: a text box, or with `options` ([value, label] pairs) a menu. `when`
    shows it only while the values pass it.
    @typedef {{ label: string, value?: string, placeholder?: string, options?: [string, string][], when?: (...values: string[]) => boolean }} Field */
/** Asks for the fields' values; resolves them in order, or null on Cancel.
    @param {string} title @param {Field[]} fields @returns {Promise<string[] | null>} */
export function modal(title, fields) {
  /** @type {(HTMLInputElement | HTMLSelectElement)[]} */
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
  return /** @type {Promise<string[] | null>} */ (dialog(title, rows, inputs[0], 'OK', () => inputs.map(i => i.value.trim())));
}

/** @param {string} title @param {string} text @param {string} okLabel */
export async function confirmModal(title, text, okLabel) {
  return await dialog(title, [el('p', { class: 'lede' }, text)], null, okLabel, () => true) === true;
}

// A question with two answers besides Cancel. Resolves 'ok', 'alt', or null for Cancel.
/** @param {string} title @param {string} text @param {string} okLabel @param {string} altLabel */
export async function choose(title, text, okLabel, altLabel) {
  return dialog(title, [el('p', { class: 'lede' }, text)], null, okLabel, () => 'ok', altLabel);
}
