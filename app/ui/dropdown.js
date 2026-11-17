/** The open menu, and the select it belongs to. @type {HTMLElement | null} */
let menu = null;
/** @type {HTMLSelectElement | null} */
let owner = null;
let active = -1;

function close(refocus = true) {
  if (!menu || !owner) return;
  menu.remove(); menu = null;
  owner.setAttribute('aria-expanded', 'false');
  owner.removeAttribute('aria-activedescendant');
  if (refocus) owner.focus();
  owner = null; active = -1;
}

/** @param {number} i */
function choose(i) {
  const sel = owner; if (!sel) return;
  const opt = sel.options[i];
  if (!sel.isConnected) return close(false);
  close();
  if (!opt || opt.disabled || sel.selectedIndex === i) return;
  sel.selectedIndex = i;
  sel.dispatchEvent(new Event('input', { bubbles: true }));
  sel.dispatchEvent(new Event('change', { bubbles: true }));
}

/** @param {number} i */
function mark(i) {
  if (!menu || !owner) return;
  const items = menu.querySelectorAll('.dd-item');
  items[active]?.classList.remove('active');
  active = Math.max(0, Math.min(items.length - 1, i));
  items[active]?.classList.add('active');
  items[active]?.scrollIntoView({ block: 'nearest' });
  if (items[active]) owner.setAttribute('aria-activedescendant', items[active].id);
}

// Moves from `from` in direction `dir`, skipping disabled options.
/** @param {number} from @param {number} dir */
function move(from, dir) {
  if (!owner) return;
  const o = owner.options;
  for (let i = from; i >= 0 && i < o.length; i += dir) if (!o[i].disabled) return mark(i);
}

/** @param {string} ch */
function typeahead(ch) {
  if (!owner) return;
  const o = [...owner.options], n = o.length;
  for (let k = 1; k <= n; k++) {
    const i = (active + k + n) % n;
    if (!o[i].disabled && (o[i].textContent || '').trim().toLowerCase().startsWith(ch)) return mark(i);
  }
}

/** @param {HTMLSelectElement} sel */
function open(sel, keyboard = false) {
  if (sel.options.length < 2) return;
  close(false);
  owner = sel;
  const m = menu = document.createElement('div');
  menu.className = 'dd-menu';
  menu.id = 'dd-menu';
  menu.setAttribute('role', 'listbox');
  [...sel.options].forEach((o, i) => {
    const it = document.createElement('div');
    it.className = 'dd-item' + (i === sel.selectedIndex ? ' on' : '') + (o.disabled ? ' off' : '');
    it.id = `dd-opt-${i}`;
    it.setAttribute('role', 'option');
    it.setAttribute('aria-selected', String(i === sel.selectedIndex));
    if (o.disabled) it.setAttribute('aria-disabled', 'true');
    it.textContent = o.textContent;
    it.addEventListener('mousedown', (e) => { e.preventDefault(); e.stopPropagation(); });
    it.addEventListener('click', (e) => { e.stopPropagation(); choose(i); });
    it.addEventListener('mousemove', () => { if (active !== i) mark(i); });
    m.append(it);
  });
  document.body.append(menu);
  sel.setAttribute('aria-expanded', 'true');
  sel.setAttribute('aria-controls', menu.id);
  const r = sel.getBoundingClientRect(), vh = window.innerHeight;
  menu.style.minWidth = r.width + 'px';
  const h = menu.offsetHeight, w = menu.offsetWidth;
  const below = vh - r.bottom - 12, down = h <= below || r.top < below;
  const maxH = Math.min(vh - 16, Math.max(160, down ? below : r.top - 12));
  const shown = Math.min(h, maxH);
  menu.style.maxHeight = maxH + 'px';
  menu.style.top = (down ? Math.min(r.bottom + 6, vh - 8 - shown) : Math.max(8, r.top - shown - 6)) + 'px';
  menu.style.left = Math.max(8, Math.min(r.left, window.innerWidth - w - 8)) + 'px';
  if (keyboard) mark(Math.max(0, sel.selectedIndex));
  else active = -1;
}

/** @param {EventTarget | null} t @returns {t is HTMLSelectElement} */
const usable = (t) => t instanceof HTMLSelectElement && !t.multiple && !t.disabled && t.size <= 1;

document.addEventListener('mousedown', (e) => {
  if (menu && !menu.contains(/** @type {Node} */ (e.target))) {
    const same = e.target === owner;
    close(false);
    if (same) { e.preventDefault(); return; }
  }
  if (!usable(e.target) || e.button !== 0) return;
  e.preventDefault();
  e.target.focus();
  open(e.target);
});

document.addEventListener('keydown', (e) => {
  if (menu) {
    const n = menu.children.length;
    if (e.key === 'Tab') { close(false); return; }
    if (e.key === 'ArrowDown') move(active + 1, 1);
    else if (e.key === 'ArrowUp') move(Math.max(0, active - 1), -1);
    else if (e.key === 'Home') move(0, 1);
    else if (e.key === 'End') move(n - 1, -1);
    else if (e.key === 'Enter' || e.key === ' ') { if (active >= 0) choose(active); else close(); }
    else if (e.key === 'Escape') close();
    else if (e.key.length === 1 && !e.metaKey && !e.ctrlKey && !e.altKey) typeahead(e.key.toLowerCase());
    else return;
    e.preventDefault(); e.stopPropagation();
    return;
  }
  if (usable(e.target) && (e.key === ' ' || e.key === 'Enter' || ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && e.altKey))) {
    e.preventDefault(); e.stopPropagation();
    open(e.target, true);
  }
}, true);

window.addEventListener('resize', () => close(false));
document.addEventListener('scroll', (e) => { if (menu && !menu.contains(/** @type {Node} */ (e.target))) close(false); }, true);
