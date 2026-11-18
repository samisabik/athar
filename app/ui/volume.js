// The volume tile at the top right. The volume is a setting (preview_volume, 0 to 100).

import { $ } from './ui.js';
import { setVolume as setEngineVolume } from './engine.js';
import { SETTINGS, saveSetting, onSetting } from './settings.js';

const DEFAULT_VOLUME = 80;
const tile = $('#knob');
/** The volume the tile shows and the engine plays at, so that the same value is not sent twice. @type {number | null} */
let shown = null;

/** The volume as a setting holds it, 0 to 100. */
const volume = () => SETTINGS.preview_volume ?? DEFAULT_VOLUME;

function show() {
  const v = volume();
  if (v === shown) return;
  shown = v;
  // The tile lifts off its shadow as the volume rises, and sits flat on the page at zero.
  const lift = v / 100;
  tile.textContent = v; tile.setAttribute('aria-valuenow', v);
  tile.style.boxShadow = v ? `${(lift * 14).toFixed(1)}px ${(lift * 14).toFixed(1)}px 0 var(--shade)` : 'none';
  tile.style.transform = `translate(${(-lift * 7).toFixed(1)}px, ${(-lift * 7).toFixed(1)}px)`;
  setEngineVolume(lift);
}
// Also follows the volume ruler on the Settings page.
onSetting(['preview_volume'], show);

/** @param {number} v */
function setVolume(v) {
  v = Math.round(Math.max(0, Math.min(100, v)));
  if (v === SETTINGS.preview_volume) return;
  saveSetting('preview_volume', v);
  const s = $('#set-preview_volume'); if (s) { s.value = v; s.nextElementSibling.textContent = `${v}%`; }
}

// The tile sits in the top right corner, so the drag runs toward the open side of the window:
// pulling it left or up, away from its shadow, is louder; pushing it down or right is quieter.
tile.addEventListener('pointerdown', (/** @type {PointerEvent} */ e) => {
  if (e.button !== 0) return;
  e.preventDefault(); tile.setPointerCapture(e.pointerId); tile.focus();
  const start = volume(), x0 = e.clientX, y0 = e.clientY;
  const move = (/** @type {PointerEvent} */ ev) => setVolume(start - ((ev.clientX - x0) + (ev.clientY - y0)));
  const up = () => { for (const t of ['pointerup', 'pointercancel', 'lostpointercapture']) tile.removeEventListener(t, up); tile.removeEventListener('pointermove', move); };
  tile.addEventListener('pointermove', move); for (const t of ['pointerup', 'pointercancel', 'lostpointercapture']) tile.addEventListener(t, up);
});
tile.addEventListener('keydown', (/** @type {KeyboardEvent} */ e) => {
  const d = /** @type {Record<string, number>} */ ({ ArrowLeft: 2, ArrowUp: 2, ArrowRight: -2, ArrowDown: -2, Home: -100, End: 100 })[e.key];
  if (d) { e.preventDefault(); e.stopPropagation(); setVolume(volume() + d); }
});
tile.addEventListener('wheel', (/** @type {WheelEvent} */ e) => { e.preventDefault(); setVolume(volume() - Math.sign(e.deltaY) * 4); }, { passive: false });
tile.addEventListener('dblclick', () => setVolume(DEFAULT_VOLUME));
