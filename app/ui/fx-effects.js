// The effect sections of the panel (fx.js): compressor, maximizer, noise reduction, saturation,
// shaping and lo-fi. Each is a column of rulers, most with a graph of what the effect does.

import { db, NR_N, hzLabel } from './units.js';
import { doc, effects } from './native.js';
import { fitCanvas, palette, monoFont } from './canvas.js';
import { freqX, slider, presetRow } from './fx-controls.js';

/** @typedef {Error} Failure What a native command rejects with. */

/** @param {import('./fx.js').FxPanel} panel @param {(Element | null)[]} controls */
export function sideLayout(panel, controls, graph = true) {
  panel.body.innerHTML = `<div class="fx-side${graph ? '' : ' plain'}"><div class="fx-controls"></div>${graph ? '<canvas class="fx-graph"></canvas>' : ''}</div><div class="fx-info muted"></div>`;
  const c = /** @type {Element} */ (panel.body.querySelector('.fx-controls'));
  controls.forEach(x => x && c.append(x));
  panel.sideCanvas = /** @type {HTMLCanvasElement | null} */ (panel.body.querySelector('.fx-graph'));
  if (!panel.sideCanvas) return;
  panel.observe(panel.sideCanvas, () => drawSide(panel));
  drawSide(panel);
}

/** @param {import('./fx.js').FxPanel} panel */
export function buildSat(panel) {
  const on = () => panel.paramsChanged(), p = panel.sat;
  sideLayout(panel, [
    slider('Drive', 'drive', 0, 24, 0.5, ' dB', p, on),
    slider('Ceiling', 'ceiling', -6, 0, 0.1, ' dB', p, on),
    slider('Mix', 'mix', 0, 100, 1, '%', p, on),
  ]);
  panel.setInfo('Drive pushes the sound into a soft clipper. At full mix it never passes the ceiling.');
}

/** @param {import('./fx.js').FxPanel} panel */
export function buildShape(panel) {
  const on = () => panel.paramsChanged(), p = panel.shape;
  sideLayout(panel, [
    slider('Attack', 'attack', -100, 100, 1, '%', p, on),
    slider('Tail', 'tail', -100, 100, 1, '%', p, on),
  ], false);
  panel.setInfo('Attack: more or less punch on each hit. Tail: shorter or longer ring after it.');
}

/** @param {import('./fx.js').FxPanel} panel */
export function buildLofi(panel) {
  const on = () => panel.paramsChanged(), p = panel.lofi;
  const bits = slider('Bit depth', 'bits', 4, 16, 1, '-bit', p, on), rate = slider('Sample rate', 'rate', 2, 48, 0.5, ' kHz', p, on);
  const set = (/** @type {number} */ b, /** @type {number} */ r) => () => { p.bits = b; p.rate = r; bits.sync(); rate.sync(); on(); };
  sideLayout(panel, [bits, rate, presetRow([['12-bit 26 kHz', set(12, 26)], ['12-bit 13 kHz', set(12, 13)], ['8-bit 11 kHz', set(8, 11)], ['8-bit 4 kHz', set(8, 4)]])], false);
}

/** @param {import('./fx.js').FxPanel} panel */
export function buildComp(panel) {
  const on = () => panel.paramsChanged(), p = panel.comp;
  const makeup = slider('Makeup', 'makeup', 0, 24, 0.5, ' dB', p, () => { p.autoMakeup = false; auto.classList.remove('on'); on(); });
  const auto = document.createElement('button');
  auto.className = 'ib toggle' + (p.autoMakeup ? ' on' : ''); auto.dataset.tip = 'Automatic makeup gain';
  auto.innerHTML = '<span class="i">auto_mode</span>';
  auto.onclick = () => { p.autoMakeup = !p.autoMakeup; auto.classList.toggle('on', p.autoMakeup); on(); };
  makeup.append(auto);
  sideLayout(panel, [
    slider('Threshold', 'threshold', -60, 0, 0.5, ' dB', p, on),
    slider('Ratio', 'ratio', 1, 20, 0.1, ':1', p, on),
    slider('Knee', 'knee', 0, 24, 0.5, ' dB', p, on),
    slider('Attack', 'attack', 0.1, 200, 0.1, ' ms', p, on),
    slider('Release', 'release', 5, 2000, 1, ' ms', p, on),
    makeup,
  ]);
}

/** @param {import('./fx.js').FxPanel} panel */
export function buildMax(panel) {
  const on = () => panel.paramsChanged(), p = panel.max;
  const gain = slider('Amplify', 'gain', 0, 30, 0.5, ' dB', p, on);
  const auto = document.createElement('button');
  auto.className = 'ib'; auto.dataset.tip = 'Auto: amplify until the sound reaches about -12 dB RMS, limited at the ceiling';
  auto.innerHTML = '<span class="i">auto_mode</span>';
  auto.onclick = async () => {
    const e = panel.ed, { signal } = panel.buildAbort, key = e.rangeKey(), [a, b] = e.range();
    let s;
    try { s = await doc.analyse({ kind: 'stats', a, b }); }
    catch (err) { return panel.notice(/** @type {Failure} */ (err).message); }
    // The answer is for the sound and range it was asked about, and for panel panel.
    if (signal.aborted || e.rangeKey() !== key) return;
    p.gain = Math.max(0, Math.min(30, Math.round((-12 - s.rms) * 2) / 2));
    gain.sync(); on();
    panel.setInfo(`input peak ${s.peak.toFixed(1)} dB, RMS ${s.rms.toFixed(1)} dB, amplify ${p.gain} dB`);
  };
  gain.append(auto);
  sideLayout(panel, [
    gain,
    slider('Ceiling', 'ceiling', -6, 0, 0.1, ' dB', p, on),
    slider('Release', 'release', 5, 500, 1, ' ms', p, on),
  ]);
}

/** @param {import('./fx.js').FxPanel} panel */
export function buildNr(panel) {
  const on = () => panel.paramsChanged(), p = panel.nr;
  const cap = document.createElement('div'); cap.className = 'row';
  cap.innerHTML = `<button class="ib" data-tip="Capture noise print from the selection (select a part with only noise)"><span class="i">noise_aware</span></button>
    <button class="ib" data-tip="Forget the noise print and estimate noise automatically"><span class="i">restart_alt</span></button>
    <span class="muted nr-state"></span>`;
  const [capBtn, clrBtn] = cap.querySelectorAll('button');
  const state = /** @type {Element} */ (cap.querySelector('.nr-state'));
  const showState = () => {
    state.textContent = panel.noise ? `Noise print: ${(panel.noise.len / panel.ed.sr).toFixed(2)} s` : 'Noise print: automatic';
  };
  capBtn.onclick = () => {
    const e = panel.ed;
    if (!e.sel || e.sel[1] - e.sel[0] < e.sr * 0.05) return panel.notice('Select at least 50 ms of noise only, then capture');
    doc.noise([e.sel[0], e.sel[1]]).catch(() => { });
    panel.noise = { len: e.sel[1] - e.sel[0] };
    panel.noiseId++;
    showState(); panel.toast('Noise print captured. Select what to clean (or nothing for the whole file).'); on();
  };
  clrBtn.onclick = () => { panel.noise = null; doc.noise(null).catch(() => { }); showState(); on(); };
  showState();
  sideLayout(panel, [
    cap,
    slider('Reduction', 'reduction', 0, 40, 1, ' dB', p, on),
    slider('Sensitivity', 'sensitivity', 0.5, 4, 0.05, '', p, on),
  ]);
}

/** @param {import('./fx.js').FxPanel} panel */
export function drawSide(panel) {
  const cv = panel.sideCanvas; if (!cv || !cv.isConnected) return;
  const fit = fitCanvas(cv); if (!fit) return;
  const { g, W, H } = fit, css = palette();
  g.fillStyle = css('--wave-bg'); g.fillRect(0, 0, W, H);
  g.strokeStyle = css('--line'); g.lineWidth = 1;
  g.font = monoFont(10); g.fillStyle = css('--muted');

  if (panel.tab === 'comp' || panel.tab === 'max' || panel.tab === 'sat') {
    const R = 60, X = (/** @type {number} */ d) => (d + R) / R * W, Y = (/** @type {number} */ d) => H - (d + R) / R * H;
    for (let d = -48; d < 0; d += 12) {
      g.beginPath(); g.moveTo(X(d), 0); g.lineTo(X(d), H); g.moveTo(0, Y(d)); g.lineTo(W, Y(d)); g.stroke();
      g.fillText(String(d), X(d) + 2, H - 3);
    }
    g.setLineDash([3, 3]); g.beginPath(); g.moveTo(0, H); g.lineTo(W, 0); g.stroke(); g.setLineDash([]);
    // The curve comes from the effect's own formulas, natively; the panel redraws when it arrives.
    const n = Math.ceil(W) + 1, tab = panel.tab, key = JSON.stringify([tab, panel.params(tab), n]);
    if (panel.sideCurve?.key === key) {
      const out = panel.sideCurve.out;
      g.beginPath();
      out.forEach((o, x) => { x ? g.lineTo(x, Y(o)) : g.moveTo(x, Y(o)); });
      g.strokeStyle = css('--accent'); g.lineWidth = 2; g.stroke();
    } else if (panel.sideWant !== key) {
      panel.sideWant = key;
      effects.curve(tab, panel.params(tab), -R, 0, n).then((out) => {
        if (panel.sideWant !== key) return;
        panel.sideCurve = { key, out }; drawSide(panel);
      }).catch(() => { });
    }
    g.fillStyle = css('--muted'); g.fillText('in →', W - 34, H - 14); g.fillText('out', 4, 12);
  } else if (panel.tab === 'nr') {
    const e = panel.ed; if (!e.loaded) return;
    const prof = panel.profileFor(.../** @type {[number, number]} */ (e.range()));
    if (!prof) return;
    const bins = prof.length, xOf = (/** @type {number} */ k) => freqX(Math.max(1, k * e.sr / NR_N), W);
    const yOf = (/** @type {number} */ m) => H - Math.max(0, (db(m / (NR_N / 4)) + 110) / 100) * H;
    for (const f of [100, 1000, 10000]) { const x = freqX(f, W); g.beginPath(); g.moveTo(x, 0); g.lineTo(x, H); g.stroke(); g.fillText(hzLabel(f), x + 2, H - 3); }
    const path = (/** @type {number} */ scale) => { g.beginPath(); for (let k = 1; k < bins; k++) { const x = xOf(k), y = yOf(prof[k] * scale); k > 1 ? g.lineTo(x, y) : g.moveTo(x, y); } };
    path(panel.nr.sensitivity); g.strokeStyle = css('--accent'); g.lineWidth = 2; g.stroke();
    path(1); g.lineTo(W, H); g.lineTo(0, H); g.closePath(); g.fillStyle = css('--c1'); g.globalAlpha = .6; g.fill(); g.globalAlpha = 1;
    g.fillStyle = css('--muted'); g.fillText('noise print (fill) and gate threshold (line)', 6, 12);
  }
}
