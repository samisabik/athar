// The editor's (editor.js) mouse, wheel and toolbar handling: selecting and dragging on the
// waveform, zooming, and the toolbar's menus.

import { TOOLS } from './tools.js';

/** @typedef {import('./editor.js').Span} Span */
/** @typedef {import('./tools.js').FadeSpan} FadeSpan */

// ---- input
/** @param {import('./editor.js').Editor} ed */
export function bind(ed) {
  ed.root.addEventListener('change', (e) => { if (ed.loaded && /** @type {Element} */ (e.target).closest('[data-f=rate], [data-f=bits]')) ed.updateInfo(); });
  ed.root.addEventListener('click', (e) => {
    const b = /** @type {Element} */ (e.target).closest('[data-a]'); if (!b) return;
    const a = /** @type {string} */ (b.dataset.a);
    if (TOOLS[a] && ed.loaded) ed.fx.openTool(a); else ed.action(a);
  });
  bindMenus(ed.root, () => !!ed.loaded);
  /** @type {number | null} */ let dragFrom = null;
  let moved = false;
  /** @type {FadeSpan | null} */ let fadeDrag = null;
  ed.canvas.addEventListener('mousedown', (e) => {
    if (!ed.loaded) return;
    const x = e.offsetX;
    const edge = ed.fadeEdgeAt(x);
    if (edge) { fadeDrag = edge; return; }
    dragFrom = ed.snap(ed.xToSample(x)); moved = false;
    if (e.shiftKey && ed.sel) { ed.sel = [Math.min(ed.sel[0], dragFrom), Math.max(ed.sel[1], dragFrom)]; dragFrom = null; ed.draw(); }
  });
  window.addEventListener('mousemove', (e) => {
    if (fadeDrag) return ed.dragFade(ed.xToSample(e.clientX - ed.canvas.getBoundingClientRect().left), fadeDrag.fadeIn);
    if (dragFrom == null) return;
    const r = ed.canvas.getBoundingClientRect();
    const s = ed.snap(ed.xToSample(e.clientX - r.left));
    if (Math.abs(s - dragFrom) / ed.spp > 2) moved = true;
    if (moved) { ed.sel = [Math.min(dragFrom, s), Math.max(dragFrom, s)]; ed.draw(); }
  });
  window.addEventListener('mouseup', () => {
    if (fadeDrag) { fadeDrag = null; return; }
    if (dragFrom == null) return;
    if (!moved) { ed.sel = null; ed.cursor = dragFrom; if (ed.playing) ed.play(); }
    else ed.cursor = /** @type {Span} */ (ed.sel)[0];
    dragFrom = null; ed.draw();
  });
  ed.canvas.addEventListener('dblclick', () => { ed.sel = [0, ed.length]; ed.draw(); });
  ed.canvas.addEventListener('mousemove', (e) => { ed.canvas.style.cursor = ed.fadeEdgeAt(e.offsetX) ? 'ew-resize' : ''; if (ed.mode !== 'spec' || !ed.loaded) return; ed.hover = { x: e.offsetX, y: e.offsetY }; ed.draw(); });
  ed.canvas.addEventListener('mouseleave', () => { if (ed.hover) { ed.hover = null; ed.draw(); } });
  ed.canvas.addEventListener('wheel', (e) => {
    if (!ed.loaded) return;
    e.preventDefault();
    if (e.ctrlKey || e.metaKey) ed.zoom(Math.exp(e.deltaY * 0.01), e.offsetX);
    else { ed.start += (Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY) * ed.spp; ed.clampView(); ed.draw(); }
  }, { passive: false });
}

// The toolbar's menus: one open at a time, opened by a click or, once one is open, by moving
// onto another; the arrow keys move through and between them. They stay shut until a sound is open.
/** @param {HTMLElement} root @param {() => boolean} isLoaded */
function bindMenus(root, isLoaded) {
  const menus = /** @type {HTMLElement[]} */ ([...root.querySelectorAll('.tmenu')]);
  const parts = (/** @type {Element} */ m) => /** @type {[HTMLElement, HTMLElement]} */ ([m.querySelector('.tmenu-btn'), m.querySelector('.tmenu-list')]);
  const isOpen = (/** @type {Element} */ m) => !parts(m)[1].hidden;
  const close = (/** @type {Element} */ m) => { const [btn, list] = parts(m); list.hidden = true; btn.setAttribute('aria-expanded', 'false'); };
  const open = (/** @type {Element} */ m) => {
    menus.forEach(o => o !== m && close(o));
    const [btn, list] = parts(m);
    list.hidden = false; btn.setAttribute('aria-expanded', 'true');
    // Keep the list inside the window when its menu sits near the right edge.
    list.style.left = '0';
    const over = list.getBoundingClientRect().right - (window.innerWidth - 8);
    if (over > 0) list.style.left = -over + 'px';
  };
  const step = (/** @type {Element} */ m, /** @type {number} */ dir) => {
    const items = [...parts(m)[1].querySelectorAll('button:not([hidden])')], i = items.indexOf(/** @type {Element} */ (document.activeElement));
    items[(i + dir + items.length) % items.length]?.focus();
  };
  for (const m of menus) {
    const [btn, list] = parts(m);
    btn.addEventListener('click', () => { if (!isLoaded()) return; isOpen(m) ? close(m) : open(m); });
    btn.addEventListener('mouseenter', () => { if (menus.some(isOpen) && !isOpen(m)) open(m); });
    list.addEventListener('click', (e) => { if (/** @type {Element} */ (e.target).closest('[role=menuitem]')) close(m); });
    m.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && isOpen(m)) { e.preventDefault(); e.stopPropagation(); close(m); btn.focus(); }
      else if (/** @type {Element} */ (e.target).matches('input, select')) return;
      else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault(); e.stopPropagation();
        if (!isOpen(m)) { if (!isLoaded()) return; open(m); }
        step(m, e.key === 'ArrowDown' ? 1 : -1);
      } else if ((e.key === 'ArrowRight' || e.key === 'ArrowLeft') && isOpen(m)) {
        e.preventDefault(); e.stopPropagation();
        const next = menus[(menus.indexOf(m) + (e.key === 'ArrowRight' ? 1 : -1) + menus.length) % menus.length];
        open(next); parts(next)[0].focus();
      }
    });
  }
  document.addEventListener('mousedown', (e) => { if (/** @type {Element} */ (e.target).closest?.('.dd-menu')) return; for (const m of menus) if (isOpen(m) && !m.contains(/** @type {Node} */ (e.target))) close(m); });
}
