// The tool sections of the panel (fx.js): the rulers of a tool that takes values, bound to the
// fields the editor's actions read, and the note picker of Tune to note.

import { NOTE_NAMES, noteAt, midiName, midiHz, hzMidi } from './units.js';
import { doc } from './native.js';
import { slider, presetRow } from './fx-controls.js';
import { sideLayout } from './fx-effects.js';

/** @typedef {import('./fx-controls.js').Slider} Slider */

// ---------------- tools: the same rulers, bound to the values the editor's actions read
/** @param {import('./fx.js').FxPanel} panel */
export function buildTool(panel) {
  const name = panel.toolName, spec = panel.spec, e = panel.ed, { signal } = panel.buildAbort;
  const inputs = spec.fields.map(f => e.field(f.f));
  panel.toolRangeKey = e.rangeKey();
  if (name === 'fade') for (const inp of inputs) inp.value = String(Math.min(Number(inp.value) || 0, e.rangeMs()));
  /** @type {(() => void) | null} */
  let hint = null;
  const changed = () => {
    hint?.();
    // The value Tune sets by itself when it opens is not a change made by hand.
    if (!panel.settingUp) panel.touch();
    e.draw();
  };
  const rows = spec.fields.map((f, i) => {
    // The ruler reads and writes the field that the editor's actions read.
    const value = { get v() { return Number(inputs[i].value) || 0; }, set v(x) { inputs[i].value = String(x); } };
    // A fade can be as long as the sound or the selection, so its ruler runs to exactly that.
    const max = name === 'fade' ? Math.max(1, e.rangeMs()) : f.max;
    return slider(f.label, 'v', f.min, max, f.step, f.unit, value, () => { panel.lastField = i; changed(); });
  });
  const setFirst = (/** @type {number} */ v) => () => { inputs[0].value = String(v); rows[0].sync(); changed(); };
  const presets = spec.presets ? presetRow(spec.presets.map(([label, v]) => [label, setFirst(v)])) : null;
  sideLayout(panel, [...rows, presets], false);
  panel.toolHint = () => hint?.();
  // Called when the editor changes a tool's value itself, as when a fade is dragged on the waveform.
  panel.toolSync = () => { rows.forEach(r => r.sync()); changed(); };
  if (name === 'fade') {
    hint = () => {
      const [fin, fout] = e.fadeSpans(), ms = (/** @type {{ a: number, b: number }} */ m) => `${Math.round((m.b - m.a) / e.sr * 1000)} ms`, on = e.sel && e.sel[1] > e.sel[0] ? 'the selection' : 'the sound';
      const parts = [fin.b > fin.a && `fades in over the first ${ms(fin)}`, fout.b > fout.a && `fades out over the last ${ms(fout)}`].filter(Boolean);
      panel.setInfo(parts.length ? `${on[0].toUpperCase() + on.slice(1)} ${parts.join(' and ')}. Drag the end of a curve on the waveform to change it.`
        : `Set a length for either end of ${on}, or for both.`);
    };
    hint();
  } else if (name === 'trimsilence') {
    hint = () => {
      const r = e.trimBounds(), s = (/** @type {number} */ n) => `${(n / e.sr).toFixed(2)} s`;
      panel.setInfo(!r ? 'Everything is below panel level, so nothing would be left.'
        : r[0] === 0 && r[1] === e.length ? 'Nothing is cut yet. Raise the level until the quiet ends are covered on the waveform.'
          : `Cuts ${s(r[0])} from the start and ${s(e.length - r[1])} from the end.`);
    };
    hint();
  } else if (name === 'speed') {
    hint = () => { const p = Number(inputs[0].value) || 100, st = 12 * Math.log2(p / 100); panel.setInfo(`${st >= 0 ? '+' : '−'}${Math.abs(st).toFixed(1)} semitones, ${p > 100 ? 'shorter' : p < 100 ? 'longer' : 'same length'}`); };
    hint();
  } else if (name === 'tune') buildTune(panel, inputs[0], rows[0], changed, signal, (fn) => { hint = fn; });
}

// Reads the pitch of the range, offers the nearest notes, and says where the shift lands.
/**
 * @param {HTMLInputElement | HTMLSelectElement} input @param {Slider} row @param {() => void} changed @param {AbortSignal} signal
 * @param {(fn: () => void) => void} setHint
 * @param {import('./fx.js').FxPanel} panel
 */
export async function buildTune(panel, input, row, changed, signal, setHint) {
  const e = panel.ed, [a, b] = e.range();
  input.value = '0'; row.sync();
  panel.setInfo('Listening for the pitch…');
  let hz = null;
  try { hz = (await doc.analyse({ kind: 'pitch', a, b })).hz; } catch { }
  if (signal.aborted) return;
  if (!hz) return panel.setInfo('No clear pitch found here. Shift by ear; the sound gets shorter as it goes up.');
  const near = noteAt(hz), shiftTo = (/** @type {number} */ midi) => 12 * Math.log2(midiHz(midi) / hz);
  const describe = () => {
    const s = Number(input.value) || 0, land = noteAt(hz * Math.pow(2, s / 12));
    panel.setInfo(`Now ${near.name} ${near.cents >= 0 ? '+' : '−'}${Math.abs(near.cents)} cents (${hz.toFixed(1)} Hz). With panel shift: ${land.name} ${land.cents >= 0 ? '+' : '−'}${Math.abs(land.cents)} cents. Length changes with pitch.`);
  };
  setHint(describe);
  // The ruler steps through notes, so every stop is in tune. A key and scale narrow the stops
  // further; "Free" brings back the plain shift in semitones.
  /** @type {Record<string, [string, number[] | null]>} */
  const SCALES = { all: ['All notes', null], major: ['Major', [0, 2, 4, 5, 7, 9, 11]], minor: ['Minor', [0, 2, 3, 5, 7, 8, 10]],
    majpent: ['Major pentatonic', [0, 2, 4, 7, 9]], minpent: ['Minor pentatonic', [0, 3, 5, 7, 10]], free: ['Free, no snapping', null] };
  const st = panel.tuneScale ||= { scale: 'all', key: 0 };
  const noteRow = document.createElement('label');
  noteRow.className = 'sl';
  noteRow.innerHTML = '<span>Note</span><input type="range" min="0" step="1"><output></output>';
  const noteIn = /** @type {HTMLInputElement} */ (noteRow.querySelector('input')), noteOut = /** @type {HTMLOutputElement} */ (noteRow.querySelector('output'));
  /** @type {number[]} */
  let stops = [];
  /**
   * @param {[string | number, string][]} opts @param {string | number} value
   * @param {(v: string) => void} onchange @param {string} label
   */
  const select = (opts, value, onchange, label) => {
    const sel = document.createElement('select'); sel.setAttribute('aria-label', label);
    for (const [v, l] of opts) { const o = document.createElement('option'); o.value = String(v); o.textContent = l; sel.append(o); }
    sel.value = String(value); sel.onchange = () => onchange(sel.value);
    return sel;
  };
  const setNote = (/** @type {number} */ midi, /** @type {boolean} */ announce) => {
    input.value = shiftTo(midi).toFixed(2); row.sync();
    const s = Number(input.value);
    noteOut.textContent = `${midiName(midi)}  ${s >= 0 ? '+' : '−'}${Math.abs(s).toFixed(2)} st`;
    if (announce) changed(); else describe();
  };
  const layout = (/** @type {boolean} */ announce) => {
    const free = st.scale === 'free', steps = SCALES[st.scale][1];
    row.hidden = !free; noteRow.hidden = free; keySel.disabled = free || !steps;
    if (free) return announce ? changed() : describe();
    stops = [];
    for (let m = near.midi - 12; m <= near.midi + 12; m++) if (!steps || steps.includes((((m - st.key) % 12) + 12) % 12)) stops.push(m);
    // Stay on the note the current shift is closest to.
    const want = hzMidi(hz * Math.pow(2, (Number(input.value) || 0) / 12));
    let at = 0; stops.forEach((m, k) => { if (Math.abs(m - want) < Math.abs(stops[at] - want)) at = k; });
    noteIn.max = String(stops.length - 1); noteIn.value = String(at);
    setNote(stops[at], announce);
  };
  noteIn.oninput = () => setNote(stops[+noteIn.value], true);
  const keySel = select(NOTE_NAMES.map((n, k) => [k, n]), st.key, (v) => { st.key = +v; layout(true); }, 'Key');
  const scaleSel = select(Object.entries(SCALES).map(([k, v]) => [k, v[0]]), st.scale, (v) => { st.scale = v; layout(true); }, 'Scale');
  const scaleRow = document.createElement('div'); scaleRow.className = 'fx-presets fx-scale';
  const lab = (/** @type {string} */ text, /** @type {Node} */ node) => { const l = document.createElement('label'); l.className = 'f'; const sp = document.createElement('span'); sp.textContent = text; l.append(sp, node); return l; };
  scaleRow.append(lab('Snap to', scaleSel), lab('Key', keySel));
  row.after(noteRow, scaleRow);
  input.value = shiftTo(near.midi).toFixed(2); row.sync();
  // The shift is only known now that the pitch has been read: show and play the tuned sound, not the untouched one.
  panel.settingUp = true;
  try { layout(true); } finally { panel.settingUp = false; }
  e.draw();
  describe();
}
