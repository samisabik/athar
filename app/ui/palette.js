// The command palette: Cmd+K, type what you want, Enter.

import { $, el } from './ui.js';

/** @typedef {{ label: string, kind: string, run: () => unknown }} Command */

/** @param {Command[]} cmds */
function openPalette(cmds) {
  if ($('#palette')) return;
  const input = el('input', { placeholder: 'Type a command', spellcheck: 'false', autocomplete: 'off', 'aria-label': 'Command' });
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

/** Opens the palette on Cmd+K with the commands `commands` gives at that moment. @param {() => Command[]} commands */
export function bindPalette(commands) {
  document.addEventListener('keydown', (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k' && $('#modal').hidden) { e.preventDefault(); e.stopPropagation(); openPalette(commands()); }
  }, true);
}
