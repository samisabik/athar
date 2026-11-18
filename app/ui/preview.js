// The Library's preview: one sound at a time, played from wherever it is listed (the results,
// the card, a project's rows and pads, the Arranger's browser, the Map).

import { $, ib, notice } from './ui.js';
import { Player } from './player.js';
import { state } from './store.js';
import { SETTINGS } from './settings.js';
import { registerPlayer, stopOthers } from './players.js';

/** @typedef {import('./search.js').Item} Item */

export const player = new Player();
export const isPlaying = (/** @type {{ path: string } | null | undefined} */ it) => !!it && state.playingItem?.path === it.path;

/** Told after the previewing sound starts or stops. @type {(() => void) | null} */
let changed = null;
/** @param {() => void} fn */
export function onPreviewChange(fn) { changed = fn; }

/** Plays `it`, or stops it when it is the one playing (unless `forceStart`). @param {Item} it */
export function togglePreview(it, forceStart = false) {
  if (isPlaying(it) && !forceStart) { stopPreview(); return; }
  stopPreview();
  player.loop = state.libLoop;
  stopOthers('preview');
  state.playingItem = it;
  player.src = it.path;
  player.onerror = () => { if (state.playingItem === it) { stopPreview(); notice(`Could not play ${it.name}`); } };
  player.oncapped = () => {
    if (state.playingItem === it) notice(`Preview plays the first ${Math.round(SETTINGS._preview_max_s / 60)} minutes of ${it.name}. Open it in the editor to hear all of it.`, 7000);
  };
  player.play().catch(() => { });
  markPlaying();
  changed?.();
}
export function stopPreview() {
  player.pause();
  const was = state.playingItem;
  state.playingItem = null;
  if (was) markPlaying();
  changed?.();
}
/** @param {boolean} on */
export function setPreviewLoop(on) { state.libLoop = on; player.loop = on; }

// Every row, pad and play button of the sound that plays shows it.
function markPlaying() {
  $('#results').querySelectorAll('.qitem[data-i]').forEach((/** @type {HTMLElement} */ r) => {
    r.classList.toggle('playing', isPlaying(state.items[Number(r.dataset.i)]));
  });
  document.querySelectorAll('.pad').forEach((c) => c.classList.toggle('playing', !!c.padPath && state.playingItem?.path === c.padPath));
  document.querySelectorAll('.pplay').forEach((b) => {
    const on = state.playingItem?.path === b.playPath, tip = on ? 'Stop' : 'Play';
    /** @type {Element} */ (b.querySelector('.i')).textContent = on ? 'stop' : 'play_arrow';
    b.dataset.tip = tip; b.setAttribute('aria-label', tip);
  });
}

/** The play button of a sound's row in a list; markPlaying keeps it in step with what plays. @param {Item} it */
export function previewButton(it) {
  const on = isPlaying(it);
  return Object.assign(ib(on ? 'stop' : 'play_arrow', on ? 'Stop' : 'Play', (e) => { e.stopPropagation(); togglePreview(it); }, 'pplay'), { playPath: it.path });
}

player.onended = () => stopPreview();
registerPlayer('preview', stopPreview);
// The preview goes on between the Library and the Map, and stops on any other page.
window.addEventListener('athar:view', () => { if (state.currentView !== 'map' && state.currentView !== 'sounds') stopPreview(); });
