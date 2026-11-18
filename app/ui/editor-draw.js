// How the editor (editor.js) paints its canvas: the ruler, the waveform or spectrogram of what
// is in view, regions, the selection, fades and the playhead.

import { doc } from './native.js';
import { fmtTime } from './time.js';
import { noteAt, hzLabel } from './units.js';
import { fitCanvas, palette, monoFont } from './canvas.js';

/** @typedef {Error} Failure What a native command rejects with. */
/** @typedef {import('./editor.js').SpecTile} SpecTile */

export const REGION_COLORS = ['--c1', '--c2', '--c3', '--c4', '--c5'];

// The height of the time ruler along the top of the waveform.
const RULER = 18;
const SPEC_BG = '#12101e';
const SPEC_TILE = 256, SPEC_TILES_KEPT = 160;

// ---- drawing
/** @param {import('./editor.js').Editor} ed */
export function draw(ed) {
  if (!ed.loaded) return drawEmpty(ed);
  const fit = fitCanvas(ed.canvas); if (!fit) return;
  const { g, W, H, dpr } = fit;
  // A view that showed the whole sound keeps showing the whole of whatever replaces it.
  const shown = ed.viewSrc().length;
  if (shown !== ed.shownLen) {
    const whole = ed.start === 0 && ed.spp * W >= (ed.shownLen || ed.length) * 0.999;
    ed.shownLen = shown;
    if (whole) { ed.start = 0; ed.spp = Math.max(1 / 16, shown / W); } else ed.clampView();
  }
  const col = palette();
  const secPerPx = ed.spp / ed.sr;
  const steps = [0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1, 2, 5, 10, 30, 60];
  const tick = steps.find(s => s / secPerPx > 80) || 60;
  const t0 = Math.ceil(ed.start / ed.sr / tick) * tick;
  if (ed.mode === 'spec') { g.fillStyle = col('--wave-bg'); g.fillRect(0, 0, W, H); drawSpec(ed, RULER, W, H - RULER); }
  else drawWave(ed, col, W, H, dpr, tick);

  ed.regions.forEach((r, i) => {
    const x0 = ed.sampleToX(r.a), x1 = ed.sampleToX(r.b);
    if (x1 < 0 || x0 > W) return;
    g.fillStyle = col(REGION_COLORS[i % REGION_COLORS.length]);
    g.globalAlpha = 0.18; g.fillRect(x0, RULER, x1 - x0, H - RULER);
    g.globalAlpha = 1; g.fillRect(x0, RULER, Math.max(2, x1 - x0), 14);
    g.fillStyle = col('--ink'); g.font = monoFont(11);
    g.fillText(r.name || String(i + 1).padStart(2, '0'), x0 + 4, RULER + 11);
  });

  if (ed.sel) {
    const x0 = ed.sampleToX(ed.sel[0]), x1 = ed.sampleToX(ed.sel[1]);
    g.fillStyle = col('--sel'); g.fillRect(x0, RULER, Math.max(1, x1 - x0), H - RULER);
  }
  // The open Fade tool draws the level the sound will have, as one shape: up from silence,
  // flat at full level, down to silence. The area under it is tinted, and a square marks the
  // end of each fade, which is where it can be dragged.
  const fades = ed.fx.toolMark();
  if (fades) {
    const [fin, fout] = fades, top = RULER + 8, bottom = H - 2, xa = ed.sampleToX(fin.a), xb = ed.sampleToX(fout.b);
    const level = (/** @type {number} */ s) => (fin.b > fin.a && s < fin.b ? Math.sin((s - fin.a) / (fin.b - fin.a) * Math.PI / 2) ** 2 : 1)
      * (fout.b > fout.a && s > fout.a ? Math.cos((s - fout.a) / (fout.b - fout.a) * Math.PI / 2) ** 2 : 1);
    const x0 = Math.max(0, xa), x1 = Math.min(W, xb), yOf = (/** @type {number} */ x) => bottom - level(Math.min(fout.b, Math.max(fin.a, ed.start + x * ed.spp))) * (bottom - top);
    if (x1 > x0 && (fin.b > fin.a || fout.b > fout.a)) {
      g.beginPath(); g.moveTo(x0, yOf(x0));
      for (let x = x0 + 1; x < x1; x++) g.lineTo(x, yOf(x));
      g.lineTo(x1, yOf(x1));
      g.strokeStyle = col('--accent'); g.lineWidth = 2; g.stroke();
      g.lineTo(x1, bottom); g.lineTo(x0, bottom); g.closePath();
      g.fillStyle = col('--accent'); g.globalAlpha = 0.1; g.fill(); g.globalAlpha = 1;
    }
    g.fillStyle = col('--accent');
    for (const x of [ed.sampleToX(fin.b), ed.sampleToX(fout.a)]) if (x >= -6 && x <= W + 6) g.fillRect(Math.round(x) - 5, Math.round(yOf(x)) - 5, 10, 10);
  }
  // Open on Trim silent ends, the parts it would cut are covered, with a line at each new edge.
  if (ed.fx.tab === 'tool' && ed.fx.toolName === 'trimsilence') {
    const r = ed.trimBounds() || [0, 0];
    g.fillStyle = col('--wave-bg'); g.globalAlpha = 0.78;
    const xa = Math.min(W, ed.sampleToX(r[0])), xb = Math.max(0, ed.sampleToX(r[1]));
    if (r[0] > 0 && xa > 0) g.fillRect(0, RULER, xa, H - RULER);
    if (r[1] < ed.length && xb < W) g.fillRect(xb, RULER, W - xb, H - RULER);
    g.globalAlpha = 1; g.fillStyle = col('--accent');
    if (r[0] > 0) g.fillRect(Math.round(xa) - 1, RULER, 2, H - RULER);
    if (r[1] < ed.length) g.fillRect(Math.round(xb) - 1, RULER, 2, H - RULER);
  }
  const cx = ed.sampleToX(ed.cursor);
  const px = ed.playing ? ed.sampleToX(ed.playPos()) : ed.auditioning ? ed.sampleToX(ed.auditionPos()) : null;
  const head = px ?? cx;
  g.fillStyle = col('--accent'); g.fillRect(Math.round(head) - 1, RULER, 2, H - RULER);

  g.fillStyle = col('--panel'); g.fillRect(0, 0, W, RULER);
  g.fillStyle = col('--muted'); g.font = monoFont(11);
  const beatNow = ed.beatLen();
  if (beatNow) {
    // Bar numbers, with beats as "3.2" once there is room for them.
    const every = ed.beatStride(beatNow, 46);
    for (let k = Math.ceil(ed.start / beatNow / every) * every; ed.sampleToX(k * beatNow) < W; k += every) {
      const x = ed.sampleToX(k * beatNow);
      if (x >= 1) g.fillRect(x, RULER - 5, 1, 5);
      g.fillText(k % 4 === 0 ? String(k / 4 + 1) : `${Math.floor(k / 4) + 1}.${k % 4 + 1}`, x + 3, 11);
    }
  } else for (let t = t0; ed.sampleToX(t * ed.sr) < W; t += tick) {
    const x = ed.sampleToX(t * ed.sr);
    if (x >= 1) g.fillRect(x, RULER - 5, 1, 5);
    g.fillText(tick < 0.01 ? t.toFixed(4) : tick < 1 ? t.toFixed(2) : fmtTime(t), x + 3, 11);
  }
  g.fillStyle = col('--accent');
  const m = Math.round(head);
  g.beginPath(); g.moveTo(m - 6, RULER - 8); g.lineTo(m + 6, RULER - 8); g.lineTo(m, RULER); g.closePath(); g.fill();
  ed.updateInfo();
}

// Background, time grid and waveform. These change only when the view or the sound does, so they
// are painted once into a layer; a playback frame then costs one image copy plus the playhead.
// The waveform's points come from the native side for exactly the pixels shown; until they
// arrive the last layer stays on screen.
/** @param {import('./editor.js').Editor} ed @param {(n: string) => string} col @param {number} W @param {number} H @param {number} dpr @param {number} tick */
export function drawWave(ed, col, W, H, dpr, tick) {
  const view = ed.viewSrc();
  const paint = [col('--wave-bg'), col('--line'), col('--wave'), col('--muted')];
  // Behind the result of an open effect or tool, the sound as it is now stays visible, fainter.
  const ghost = view.src !== 'doc' && view.length === ed.length;
  const beat = ed.beatLen();
  const key = [ed.start, ed.spp, W, H, dpr, ed.version, view.src === 'doc' ? '' : view.src, tick, beat, ...paint].join('|');
  if (ed.layerKey !== key && ed.peaksKey !== key) {
    ed.peaksKey = key;
    const want = ghost ? ['doc', view.src] : [view.src];
    Promise.all(want.map(src => doc.peaks(src, ed.start, ed.spp, Math.ceil(W))))
      .then((bufs) => {
        if (ed.peaksKey !== key) return;
        paintLayer(ed, key, bufs.map(b => new Float32Array(b)), ghost, paint, W, H, dpr, tick, beat);
        draw(ed);
      })
      .catch(() => { if (ed.peaksKey === key) ed.peaksKey = null; });
  }
  if (!ed.layer) return;
  ed.g.save(); ed.g.setTransform(1, 0, 0, 1, 0, 0);
  ed.g.drawImage(ed.layer, 0, 0);
  ed.g.restore();
}

/**
 * @param {string} key @param {Float32Array[]} sets @param {boolean} ghost @param {string[]} paint
 * @param {number} W @param {number} H @param {number} dpr @param {number} tick @param {number} beat
 * @param {import('./editor.js').Editor} ed
 */
export function paintLayer(ed, key, sets, ghost, paint, W, H, dpr, tick, beat) {
  const layer = ed.layer ||= document.createElement('canvas');
  layer.width = ed.canvas.width; layer.height = ed.canvas.height;
  const g = /** @type {CanvasRenderingContext2D} */ (layer.getContext('2d'));
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.fillStyle = paint[0]; g.fillRect(0, 0, W, H);
  // Time grid behind the waveform: a line per labelled tick, a fainter one halfway between.
  g.fillStyle = paint[1];
  const half = tick / 2;
  if (beat) {
    // A line per beat, heavier on each bar; beats thin out when they would crowd.
    const every = ed.beatStride(beat, 14);
    for (let k = Math.ceil(ed.start / beat / every) * every; ed.sampleToX(k * beat) < W; k += every) {
      g.globalAlpha = k % 4 === 0 ? 0.95 : 0.4;
      g.fillRect(Math.round(ed.sampleToX(k * beat)), RULER, 1, H - RULER);
    }
  } else for (let t = Math.ceil(ed.start / ed.sr / half) * half; ed.sampleToX(t * ed.sr) < W; t += half) {
    const major = Math.abs(t / tick - Math.round(t / tick)) < 1e-6;
    g.globalAlpha = major ? 0.9 : 0.4;
    g.fillRect(Math.round(ed.sampleToX(t * ed.sr)), RULER, 1, H - RULER);
  }
  g.globalAlpha = 1;
  const lanes = ed.nch, laneH = (H - RULER) / lanes, width = Math.ceil(W);
  /** @type {[Float32Array, string, number, boolean][]} */
  const layers = ghost ? [[sets[0], paint[3], 0.45, false], [sets[1], paint[2], 1, true]] : [[sets[0], paint[2], 1, true]];
  for (const [data, ink, alpha, main] of layers) for (let c = 0; c < lanes; c++) {
    const y0 = RULER + c * laneH, mid = y0 + laneH / 2, amp = laneH / 2 - 3;
    g.globalAlpha = 1;
    if (main) { g.strokeStyle = paint[1]; g.beginPath(); g.moveTo(0, mid + .5); g.lineTo(W, mid + .5); g.stroke(); }
    g.globalAlpha = alpha;
    g.fillStyle = ink;
    if (ed.spp <= 1) {
      // The samples themselves, from the first whole sample at or before the left edge.
      const per = data.length / lanes, d = data.subarray(c * per, (c + 1) * per), i0 = Math.floor(Math.max(0, ed.start));
      g.strokeStyle = ink; g.beginPath();
      for (let x = 0; x <= W; x++) {
        const i = Math.floor(ed.start + x * ed.spp) - i0;
        if (i >= d.length) break;
        const y = mid - d[i] * amp;
        x ? g.lineTo(x, y) : g.moveTo(x, y);
      }
      g.stroke();
      if (ed.spp < 0.12) {
        for (let i = Math.ceil(ed.start); i - i0 < d.length && i < ed.start + W * ed.spp; i++)
          g.fillRect(ed.sampleToX(i) - 1.5, mid - d[i - i0] * amp - 1.5, 3, 3);
      }
    } else {
      const lo = data.subarray(2 * c * width, (2 * c + 1) * width), hi = data.subarray((2 * c + 1) * width, (2 * c + 2) * width);
      for (let x = 0; x < width; x++) {
        const mn = lo[x], mx = hi[x];
        if (mn > mx) continue;
        g.fillRect(x, mid - mx * amp, 1, Math.max(1, (mx - mn) * amp));
      }
    }
  }
  g.globalAlpha = 1;
  ed.layerKey = key;
}

/** @param {import('./editor.js').Editor} ed */
export function drawEmpty(ed) {
  const fit = fitCanvas(ed.canvas); if (!fit) return;
  const { g, W, H } = fit, col = palette();
  const secs = 10, pps = W / secs;
  g.fillStyle = col('--wave-bg'); g.fillRect(0, 0, W, H);
  g.fillStyle = col('--line');
  for (let t = 0.5; t < secs; t += 0.5) {
    g.globalAlpha = t % 1 ? 0.4 : 0.9;
    g.fillRect(Math.round(t * pps), RULER, 1, H - RULER);
  }
  g.globalAlpha = 1;
  const laneH = (H - RULER) / 2;
  for (let c = 0; c < 2; c++) g.fillRect(0, Math.round(RULER + c * laneH + laneH / 2), W, 1);
  g.fillStyle = col('--panel'); g.fillRect(0, 0, W, RULER);
  g.fillStyle = col('--muted'); g.font = monoFont(11);
  for (let t = 0; t < secs; t++) { const x = t * pps; if (t) g.fillRect(x, RULER - 5, 1, 5); g.fillText(t + 's', x + 3, 11); }
  const msg = 'Double-click a sound, or use its Edit button';
  g.font = monoFont(12);
  const tw = g.measureText(msg).width, bx = (W - tw) / 2 - 12, by = RULER + (H - RULER) / 2 - 16;
  g.fillStyle = col('--wave-bg'); g.fillRect(bx, by, tw + 24, 32);
  g.fillStyle = col('--muted'); g.fillText(msg, bx + 12, by + 20);
}

// The picture is made of tiles SPEC_TILE device pixels wide, computed natively and kept per
// zoom level, so scrolling reuses them. While a zoom level is still filling in, tiles from other
// zoom levels are drawn stretched underneath.
/** @param {import('./editor.js').Editor} ed @param {number} top @param {number} W @param {number} H */
export function drawSpec(ed, top, W, H) {
  const view = ed.viewSrc(), dpr = window.devicePixelRatio || 1;
  const PW = Math.round(W * dpr), rows = Math.round(H * dpr), spd = ed.spp / dpr;
  const dataKey = `${ed.version}|${view.src}`;
  if (ed.tiles?.dataKey !== dataKey) ed.tiles = { dataKey, map: new Map() };
  const map = ed.tiles.map, zoom = `${spd}|${rows}`, x0 = ed.start / spd;
  const g0 = ed.g;
  g0.save(); g0.setTransform(1, 0, 0, 1, 0, 0);
  g0.beginPath(); g0.rect(0, top * dpr, PW, rows); g0.clip();
  g0.fillStyle = SPEC_BG; g0.fillRect(0, top * dpr, PW, rows);
  for (const t of map.values()) {
    if (t.zoom === zoom) continue;
    const x = t.k * SPEC_TILE * t.spd / spd - x0, w = SPEC_TILE * t.spd / spd;
    if (x < PW && x + w > 0) g0.drawImage(t.canvas, x, top * dpr, w, rows);
  }
  const missing = [];
  for (let k = Math.max(0, Math.floor(x0 / SPEC_TILE)); k * SPEC_TILE - x0 < PW && k * SPEC_TILE * spd < ed.length; k++) {
    const t = map.get(`${zoom}|${k}`);
    if (t) g0.drawImage(t.canvas, k * SPEC_TILE - x0, top * dpr); else missing.push(k);
  }
  g0.restore();
  ed.specWant = missing.length && ed.specFailed !== dataKey ? { dataKey, src: view.src, zoom, spd, rows, missing } : null;
  if (ed.specWant) specLoop(ed);
  // Frequency axis, plus a readout of frequency and note under the pointer.
  const g = ed.g, fmin = 20, fmax = ed.sr / 2;
  const yOf = (/** @type {number} */ f) => top + H * (1 - Math.log(f / fmin) / Math.log(fmax / fmin));
  g.font = monoFont(10);
  for (const f of [50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000]) {
    if (f >= fmax) break;
    const y = Math.round(yOf(f)) + .5;
    g.fillStyle = 'rgba(255,246,214,.18)'; g.fillRect(0, y, W, 1);
    g.fillStyle = 'rgba(255,246,214,.75)'; g.fillText(hzLabel(f), 4, y - 3);
  }
  if (ed.hover && ed.hover.y > top) {
    const f = fmin * Math.pow(fmax / fmin, 1 - (ed.hover.y - top) / H);
    const t = ed.xToSample(ed.hover.x) / ed.sr;
    const txt = `${f >= 1000 ? (f / 1000).toFixed(2) + ' kHz' : Math.round(f) + ' Hz'}  ${noteAt(f).name}  ${t.toFixed(2)}s`;
    const w = g.measureText(txt).width + 14, bx = Math.min(W - w - 4, ed.hover.x + 12), by = Math.max(top + 4, ed.hover.y - 26);
    g.fillStyle = 'rgba(18,16,30,.85)'; g.fillRect(bx, by, w, 20);
    g.fillStyle = '#fff6d6'; g.fillText(txt, bx + 7, by + 14);
    g.fillStyle = 'rgba(255,246,214,.5)'; g.fillRect(0, Math.round(ed.hover.y), W, 1);
  }
}

/** @param {import('./editor.js').Editor} ed */
export async function specLoop(ed) {
  if (ed.specBusy) return;
  ed.specBusy = true;
  try {
    for (let w; (w = ed.specWant) && w.missing.length;) {
      const k = /** @type {number} */ (w.missing.shift()), key = `${w.zoom}|${k}`, tiles = /** @type {{ dataKey: string, map: Map<string, SpecTile> }} */ (ed.tiles);
      if (tiles.dataKey !== w.dataKey || tiles.map.has(key)) continue;
      const pixels = new Uint8ClampedArray(await doc.spec(w.src, k * SPEC_TILE * w.spd, w.spd, SPEC_TILE, w.rows));
      const canvas = document.createElement('canvas');
      canvas.width = SPEC_TILE; canvas.height = w.rows;
      /** @type {CanvasRenderingContext2D} */ (canvas.getContext('2d')).putImageData(new ImageData(pixels, SPEC_TILE, w.rows), 0, 0);
      tiles.map.set(key, { canvas, k, zoom: w.zoom, spd: w.spd });
      // Oldest tiles go first; a Map keeps insertion order.
      while (tiles.map.size > SPEC_TILES_KEPT) tiles.map.delete(/** @type {string} */ (tiles.map.keys().next().value));
      draw(ed);
    }
  } catch (e) { ed.specFailed = ed.specWant?.dataKey; ed.specWant = null; ed.notice(`Spectrogram failed: ${/** @type {Failure} */ (e).message}`); }
  finally { ed.specBusy = false; }
}
