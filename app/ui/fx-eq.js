// The EQ section of the panel (fx.js): its graph with a handle per band over the live spectrum,
// and the controls of the selected band. The bands are heard live while the sound plays.

import { hzLabel } from './units.js';
import { rate } from './engine.js';
import { effects } from './native.js';
import { fitCanvas, palette, monoFont } from './canvas.js';
import { FMIN, FMAX, freqX, xFreq, gainY, yGain, clampGain } from './fx-controls.js';

/** @typedef {import('./fx.js').Band} Band */

// The EQ's band types, by the name the native side knows them by. `gain` says whether the band
// has a gain of its own; a cut or a notch only has a frequency and a width.
/** @type {Record<string, { label: string, gain: boolean }>} */
const BAND_TYPES = {
  bell: { label: 'Bell', gain: true },
  lowshelf: { label: 'Low shelf', gain: true },
  highshelf: { label: 'High shelf', gain: true },
  lowcut: { label: 'Low cut', gain: false },
  highcut: { label: 'High cut', gain: false },
  notch: { label: 'Notch', gain: false },
};

const BAND_COLORS = ['--c1', '--c3', '--c2', '--c5', '--c4'];

/** @param {import('./fx.js').FxPanel} panel */
export function buildEq(panel) {
  panel.body.innerHTML = `<canvas class="eq-graph"></canvas><div class="eq-band"></div>`;
  panel.eqCanvas = /** @type {HTMLCanvasElement} */ (panel.body.querySelector('canvas'));
  panel.bandBox = /** @type {HTMLElement} */ (panel.body.querySelector('.eq-band'));
  bindEq(panel);
  renderBandControls(panel);
  drawEq(panel);
  panel.observe(panel.eqCanvas, () => drawEq(panel));
  // The spectrum moves for as long as the sound plays.
  const { signal } = panel.buildAbort;
  const loop = () => { if (signal.aborted) return; if (panel.ed.playing) drawEq(panel); requestAnimationFrame(loop); };
  requestAnimationFrame(loop);
}

// Where a band's handle sits on the graph.
/** @param {Band} b @param {number} W @param {number} H */
export function bandXY(b, W, H) {
  const gain = BAND_TYPES[b.type].gain ? b.g : 0;
  return [freqX(b.f, W), gainY(gain, H)];
}

/** @param {import('./fx.js').FxPanel} panel */
export function drawEq(panel) {
  if (!panel.ed.playing) panel.ed.draw();
  const fit = panel.eqCanvas && fitCanvas(panel.eqCanvas); if (!fit) return;
  const { g, W, H } = fit, css = palette();
  g.fillStyle = css('--wave-bg'); g.fillRect(0, 0, W, H);
  g.font = monoFont(10);
  g.strokeStyle = css('--line'); g.fillStyle = css('--muted'); g.lineWidth = 1;
  for (const f of [50, 100, 200, 500, 1000, 2000, 5000, 10000]) {
    const x = Math.round(freqX(f, W)) + .5;
    g.beginPath(); g.moveTo(x, 0); g.lineTo(x, H); g.stroke();
    g.fillText(hzLabel(f), x + 3, H - 4);
  }
  for (const d of [-18, -12, -6, 0, 6, 12, 18]) {
    const y = Math.round(gainY(d, H)) + .5;
    g.globalAlpha = d === 0 ? 1 : .6;
    g.beginPath(); g.moveTo(0, y); g.lineTo(W, y); g.stroke();
    g.globalAlpha = 1; g.fillText((d > 0 ? '+' : '') + d, 4, y - 3);
  }

  if (panel.ed.playing && panel.eqLive) {
    const data = panel.spectrum, sr = rate;
    g.beginPath(); g.moveTo(0, H);
    for (let x = 0; x <= W; x += 2) {
      const k = Math.min(data.length - 1, Math.round(xFreq(x, W) / (sr / 2) * data.length));
      const v = Math.max(0, (data[k] + 100) / 90);
      g.lineTo(x, H - v * H);
    }
    g.lineTo(W, H); g.closePath();
    g.fillStyle = css('--c4'); g.globalAlpha = .55; g.fill(); g.globalAlpha = 1;
  }

  const N = Math.ceil(W / 2) + 1;
  const total = new Float32Array(N).fill(0);
  const resp = response(panel, N, W);
  panel.bands.forEach((b, i) => {
    if (!b.on || !resp?.[i]) return;
    const mag = resp[i];
    for (let k = 0; k < N; k++) total[k] += mag[k];
    if (i === panel.selBand) {
      g.beginPath();
      for (let k = 0; k < N; k++) { const y = gainY(clampGain(mag[k]), H); k ? g.lineTo(k * 2, y) : g.moveTo(0, y); }
      g.lineTo(W, gainY(0, H)); g.lineTo(0, gainY(0, H)); g.closePath();
      g.fillStyle = css(BAND_COLORS[i % BAND_COLORS.length]); g.globalAlpha = .45; g.fill(); g.globalAlpha = 1;
    }
  });
  g.beginPath();
  for (let k = 0; k < N; k++) { const y = gainY(clampGain(total[k]), H); k ? g.lineTo(k * 2, y) : g.moveTo(0, y); }
  g.strokeStyle = panel.bypass ? css('--muted') : css('--accent'); g.lineWidth = 2; g.stroke(); g.lineWidth = 1;

  panel.bands.forEach((b, i) => {
    const [x, y] = bandXY(b, W, H);
    g.beginPath(); g.arc(x, y, i === panel.selBand ? 9 : 7, 0, Math.PI * 2);
    g.fillStyle = b.on ? css(BAND_COLORS[i % BAND_COLORS.length]) : css('--line'); g.fill();
    g.strokeStyle = css('--ink'); g.lineWidth = i === panel.selBand ? 2 : 1; g.stroke(); g.lineWidth = 1;
    g.fillStyle = css('--ink'); g.fillText(String(i + 1), x - 3, y + 3.5);
  });
  if (!panel.bands.length) {
    g.fillStyle = css('--muted'); g.font = monoFont(13);
    g.fillText('Double-click to add a band. Drag to move, scroll to change width.', 40, 26);
  }
}

// The EQ curve of each band, in dB at N points across the width, computed natively from the
// same filters that play. Null until it arrives; the panel redraws then.
/** @param {import('./fx.js').FxPanel} panel @param {number} N @param {number} W */
export function response(panel, N, W) {
  const key = JSON.stringify([N, W, rate, panel.bands]);
  if (panel.curves?.key === key) return panel.curves.data;
  if (panel.curvesWant !== key) {
    panel.curvesWant = key;
    const freqs = Array.from({ length: N }, (_, i) => xFreq(i * 2, W));
    effects.eqResponse(panel.bands, freqs).then((data) => {
      if (panel.curvesWant !== key) return;
      panel.curves = { key, data }; drawEq(panel);
    }).catch(() => { });
  }
  return panel.curves?.data?.length === panel.bands.length ? panel.curves.data : null;
}

/** @param {import('./fx.js').FxPanel} panel @param {number} x @param {number} y */
export function hitBand(panel, x, y) {
  const cv = /** @type {HTMLCanvasElement} */ (panel.eqCanvas), W = cv.clientWidth, H = cv.clientHeight;
  let best = -1, bd = 14;
  panel.bands.forEach((b, i) => { const [bx, by] = bandXY(b, W, H); const d = Math.hypot(bx - x, by - y); if (d < bd) { bd = d; best = i; } });
  return best;
}

/** @param {import('./fx.js').FxPanel} panel */
export function bindEq(panel) {
  const cv = /** @type {HTMLCanvasElement} */ (panel.eqCanvas), { signal } = panel.buildAbort;
  let drag = -1;
  const pos = (/** @type {MouseEvent} */ e) => { const r = cv.getBoundingClientRect(); return [e.clientX - r.left, e.clientY - r.top]; };
  cv.addEventListener('mousedown', (e) => {
    const [x, y] = pos(e); drag = hitBand(panel, x, y);
    if (drag >= 0) { panel.selBand = drag; renderBandControls(panel); drawEq(panel); }
  });
  window.addEventListener('mousemove', (e) => {
    if (drag < 0) return;
    const [x, y] = pos(e), W = cv.clientWidth, H = cv.clientHeight, b = panel.bands[drag];
    b.f = Math.max(FMIN, Math.min(FMAX, xFreq(x, W)));
    if (BAND_TYPES[b.type].gain) b.g = clampGain(yGain(y, H));
    bandChanged(panel);
  }, { signal });
  window.addEventListener('mouseup', () => { drag = -1; }, { signal });
  cv.addEventListener('dblclick', (e) => {
    const [x, y] = pos(e), W = cv.clientWidth, H = cv.clientHeight;
    const hit = hitBand(panel, x, y);
    if (hit >= 0) { panel.bands.splice(hit, 1); panel.selBand = -1; return bandChanged(panel); }
    const f = xFreq(x, W);
    const type = f < 40 ? 'lowcut' : f > 14000 ? 'highcut' : 'bell';
    panel.bands.push({ type, f, g: type === 'bell' ? clampGain(Math.round(yGain(y, H) * 2) / 2) : 0, q: type === 'bell' ? 1 : 0.71, on: true });
    panel.selBand = panel.bands.length - 1;
    bandChanged(panel);
  });
  cv.addEventListener('wheel', (e) => {
    const [x, y] = pos(e), hit = hitBand(panel, x, y), i = hit >= 0 ? hit : panel.selBand;
    if (i < 0) return;
    e.preventDefault();
    const b = panel.bands[i];
    b.q = Math.max(0.1, Math.min(18, b.q * Math.exp(-e.deltaY * 0.004)));
    panel.selBand = i; bandChanged(panel);
  }, { passive: false });
}

// A band was added, removed or moved: the live EQ, the band's controls and the graph follow.
/** @param {import('./fx.js').FxPanel} panel */
export function bandChanged(panel) {
  panel.touch();
  if (panel.eqLive) panel.syncEq();
  renderBandControls(panel);
  drawEq(panel);
}

/** @param {import('./fx.js').FxPanel} panel */
export function renderBandControls(panel) {
  const box = panel.bandBox; if (!box) return;
  const b = panel.bands[panel.selBand];
  if (!b) {
    box.innerHTML = `<span class="muted">${panel.bands.length} band${panel.bands.length === 1 ? '' : 's'}</span>`;
    return;
  }
  const opts = Object.entries(BAND_TYPES).map(([k, t]) => `<option value="${k}" ${k === b.type ? 'selected' : ''}>${t.label}</option>`).join('');
  box.innerHTML = `<span class="band-dot" style="background:var(${BAND_COLORS[panel.selBand % BAND_COLORS.length]})">${panel.selBand + 1}</span>
    <select data-k="type">${opts}</select>
    <label>Hz <input data-k="f" type="number" min="20" max="20000" step="1" value="${Math.round(b.f)}"></label>
    <label ${BAND_TYPES[b.type].gain ? '' : 'hidden'}>dB <input data-k="g" type="number" min="-24" max="24" step="0.5" value="${b.g.toFixed(1)}"></label>
    <label>Q <input data-k="q" type="number" min="0.1" max="18" step="0.1" value="${b.q.toFixed(2)}"></label>
    <button class="ib toggle ${b.on ? 'on' : ''}" data-k="on" data-tip="Band on or off"><span class="i">power_settings_new</span></button>
    <button class="ib" data-k="del" data-tip="Remove band (or double-click it)"><span class="i">delete_outline</span></button>`;
  /** @type {NodeListOf<HTMLInputElement>} */ (box.querySelectorAll('input, select')).forEach(inp => inp.onchange = () => {
    const k = /** @type {'type' | 'f' | 'g' | 'q'} */ (inp.dataset.k);
    if (k === 'type') { b.type = inp.value; b.q = BAND_TYPES[b.type].gain ? 1 : 0.71; }
    else {
      const v = Number(inp.value);
      if (inp.value === '' || !isFinite(v)) { inp.value = String(b[k]); return; }
      b[k] = Math.max(Number(inp.min), Math.min(Number(inp.max), v));
    }
    bandChanged(panel);
  });
  /** @type {Element} */ (box.querySelector('[data-k=on]')).onclick = () => { b.on = !b.on; bandChanged(panel); };
  /** @type {Element} */ (box.querySelector('[data-k=del]')).onclick = () => { panel.bands.splice(panel.selBand, 1); panel.selBand = -1; bandChanged(panel); };
}
