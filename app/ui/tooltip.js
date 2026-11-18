// Tooltips: any element with data-tip shows it after the pointer rests there.

import { $ } from './ui.js';

const SHOW_AFTER_MS = 350;
const tip = $('#tip');
/** @type {number | undefined} */
let tipTimer;
/** @type {HTMLElement | null} */
let tipFor = null, pointerX = 0;
document.addEventListener('mousemove', (e) => { pointerX = e.clientX; }, { passive: true });
document.addEventListener('mouseover', (e) => {
  const t = /** @type {HTMLElement | null} */ (/** @type {Element} */ (e.target).closest('[data-tip]'));
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
    const mid = r.width > 360 ? Math.min(r.right, Math.max(r.left, pointerX)) : r.left + r.width / 2;
    let x = Math.min(window.innerWidth - w - 8, Math.max(edge, mid - w / 2));
    let y = r.bottom + 8; if (y + h > window.innerHeight - 8) y = r.top - h - 8;
    // The strip's own tips sit to its right.
    if (t.closest('#activity')) { x = r.right + 8; y = r.top + r.height / 2 - h / 2; }
    tip.style.left = x + 'px'; tip.style.top = y + 'px';
  }, SHOW_AFTER_MS);
});
const hideTip = () => { clearTimeout(tipTimer); tip.hidden = true; tipFor = null; };
document.addEventListener('mousedown', hideTip, true);
document.addEventListener('scroll', hideTip, true);
window.addEventListener('athar:view', hideTip);
