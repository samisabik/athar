import { ctx, out, toBuffer, fmtTime, mixMono, resample, readyAudio, soundPlaying } from './audio.js';
import { run, runPerChannel } from './work.js';
import { loopCrossfade } from './dsp.js';
import { TOOLS } from './tools.js';

const REGION_COLORS = ['--c1', '--c2', '--c3', '--c4', '--c5'];

const UNDO_BYTES = 1.5 * 2 ** 30;
const SPEC_BG = '#12101e';
const SPEC_TILE = 256, SPEC_TILES_KEPT = 160;
const PYRAMID = 256;
const NOTE_NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
function noteOf(f) { const m = Math.round(69 + 12 * Math.log2(f / 440)); return NOTE_NAMES[((m % 12) + 12) % 12] + (Math.floor(m / 12) - 1); }

const AUDITION_S = 8, AUDITION_FADE_S = 0.02, AUDITION_FADE_IN_S = 0.002;

export class Editor {
  constructor(root, { onSave, toast, notice, pickFolder, regionsApi, ask, whereToSave, place }) {
    this.root = root;
    this.onSave = onSave; this.whereToSave = whereToSave; this.place = place;
    this.regionsApi = regionsApi; this.ask = ask;
    this.toast = toast; this.notice = notice || toast;
    this.pickFolder = pickFolder;
    this.canvas = root.querySelector('canvas');
    this.g = this.canvas.getContext('2d');
    this.mode = 'wave';
    this.version = 0;
    this.undo = []; this.redo = [];
    this.clip = null;
    this.loop = false;
    this.bind();
    new ResizeObserver(() => this.draw()).observe(this.canvas);
  }

  $(sel) { return this.root.querySelector(sel); }

  load({ channels, sr, name, dir, path = null, tempo = null }) {
    this.stopPlay(); this.stopAudition(); this.shownLen = 0;
    this.channels = channels; this.sr = sr; this.name = name; this.dir = dir; this.path = path;
    this.fileId = (this.fileId || 0) + 1;
    this.$('[data-f=bpm]').value = tempo || 120;
    this.version++; this.savedVersion = this.version; this.savedOnce = false;
    this.undo = []; this.redo = [];
    this.sel = null; this.cursor = 0; this.regions = []; this.hover = null;
    this.root.classList.remove('blank');
    this.fx?.fileChanged();
    this.renderRegions();
    this.$('[data-f=name]').value = name.replace(/\.[^.]+$/, '') + (this.settings?.edit_suffix ?? '_edit');
    this.$('[data-f=title]').textContent = name;
    this.fit();
    // Regions saved for this file come back, unless some were made in the meantime.
    const v = this.version;
    if (path) this.regionsApi?.get(path).then((list) => {
      if (this.version !== v || this.regions.length || !list?.length) return;
      this.regions = list.map(r => ({ a: Math.round(r.start * sr), b: Math.min(this.length, Math.round(r.end * sr)), name: r.name || '' })).filter(r => r.b > r.a);
      this.renderRegions(); this.draw();
    }).catch(() => { });
  }

  get length() { return this.channels ? this.channels[0].length : 0; }
  get dirty() { return !!this.channels && this.version !== this.savedVersion; }

  // ---- view
  // While a tool that changes the length is open, the waveform shows its result, which is longer
  // or shorter than the sound itself. Zoom and scroll follow what is shown.
  viewLen() { return this.shownLen || this.length; }
  fit() { this.start = 0; this.spp = Math.max(1 / 16, this.viewLen() / Math.max(1, this.width())); this.draw(); }
  width() { return this.canvas.clientWidth || 800; }
  xToSample(x) { return Math.round(Math.max(0, Math.min(this.length, this.start + x * this.spp))); }
  sampleToX(s) { return (s - this.start) / this.spp; }
  zoom(f, aroundX = this.width() / 2) {
    const s = this.start + aroundX * this.spp;
    this.spp = Math.max(1 / 16, Math.min(this.viewLen() / this.width() * 1.0001 || 1, this.spp * f));
    this.start = s - aroundX * this.spp;
    this.clampView(); this.draw();
  }
  zoomSel() {
    if (!this.sel) return;
    const [a, b] = this.range();
    this.spp = Math.max(1 / 16, (b - a) / this.width()); this.start = a; this.clampView(); this.draw();
  }
  // ---- bars and beats
  beatLen() { const bpm = Number(this.$('[data-f=bpm]').value); return this.bars && bpm > 0 ? this.sr * 60 / bpm : 0; }
  // With the grid on, a position within a few pixels of a quarter beat lands on it.
  snap(s) {
    const beat = this.beatLen(); if (!beat) return s;
    const step = beat / 4, near = Math.round(s / step) * step;
    return Math.abs(near - s) / this.spp <= 6 ? Math.round(Math.min(this.length, near)) : s;
  }
  clampView() { this.start = Math.max(0, Math.min(this.start, this.viewLen() - this.width() * this.spp)); }

  range() {
    if (this.sel && this.sel[1] - this.sel[0] > 0) return this.sel;
    return [0, this.length];
  }

  // ---- drawing
  draw() {
    if (!this.channels) return this.drawEmpty();
    const dpr = window.devicePixelRatio || 1;
    const W = this.canvas.clientWidth, H = this.canvas.clientHeight;
    if (!W || !H) return;
    if (this.canvas.width !== W * dpr || this.canvas.height !== H * dpr) {
      this.canvas.width = W * dpr; this.canvas.height = H * dpr;
    }
    const g = this.g;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    // A view that showed the whole sound keeps showing the whole of whatever replaces it.
    const shown = this.viewChannels()[0].length;
    if (shown !== this.shownLen) {
      const whole = this.start === 0 && this.spp * W >= (this.shownLen || this.length) * 0.999;
      this.shownLen = shown;
      if (whole) { this.start = 0; this.spp = Math.max(1 / 16, shown / W); } else this.clampView();
    }
    const css = getComputedStyle(document.documentElement);
    const col = (n) => css.getPropertyValue(n).trim();
    const RULER = 18;
    const secPerPx = this.spp / this.sr;
    const steps = [0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1, 2, 5, 10, 30, 60];
    const tick = steps.find(s => s / secPerPx > 80) || 60;
    const t0 = Math.ceil(this.start / this.sr / tick) * tick;
    if (this.mode === 'spec') { g.fillStyle = col('--wave-bg'); g.fillRect(0, 0, W, H); this.drawSpec(RULER, W, H - RULER); }
    else this.drawWave(col, RULER, W, H, dpr, tick);

    this.regions.forEach((r, i) => {
      const x0 = this.sampleToX(r.a), x1 = this.sampleToX(r.b);
      if (x1 < 0 || x0 > W) return;
      g.fillStyle = col(REGION_COLORS[i % REGION_COLORS.length]);
      g.globalAlpha = 0.18; g.fillRect(x0, RULER, x1 - x0, H - RULER);
      g.globalAlpha = 1; g.fillRect(x0, RULER, Math.max(2, x1 - x0), 14);
      g.fillStyle = col('--ink'); g.font = '11px "Share Tech Mono", "Martian Mono", ui-monospace, monospace';
      g.fillText(r.name || String(i + 1).padStart(2, '0'), x0 + 4, RULER + 11);
    });

    if (this.sel) {
      const x0 = this.sampleToX(this.sel[0]), x1 = this.sampleToX(this.sel[1]);
      g.fillStyle = col('--sel'); g.fillRect(x0, RULER, Math.max(1, x1 - x0), H - RULER);
    }
    // The open Fade tool draws the level the sound will have, as one shape: up from silence,
    // flat at full level, down to silence. The area under it is tinted, and a square marks the
    // end of each fade, which is where it can be dragged.
    const fades = this.fx?.toolMark();
    if (fades) {
      const [fin, fout] = fades, top = RULER + 8, bottom = H - 2, xa = this.sampleToX(fin.a), xb = this.sampleToX(fout.b);
      const level = (s) => (fin.b > fin.a && s < fin.b ? Math.sin((s - fin.a) / (fin.b - fin.a) * Math.PI / 2) ** 2 : 1)
        * (fout.b > fout.a && s > fout.a ? Math.cos((s - fout.a) / (fout.b - fout.a) * Math.PI / 2) ** 2 : 1);
      const x0 = Math.max(0, xa), x1 = Math.min(W, xb), yOf = (x) => bottom - level(Math.min(fout.b, Math.max(fin.a, this.start + x * this.spp))) * (bottom - top);
      if (x1 > x0 && (fin.b > fin.a || fout.b > fout.a)) {
        g.beginPath(); g.moveTo(x0, yOf(x0));
        for (let x = x0 + 1; x < x1; x++) g.lineTo(x, yOf(x));
        g.lineTo(x1, yOf(x1));
        g.strokeStyle = col('--accent'); g.lineWidth = 2; g.stroke();
        g.lineTo(x1, bottom); g.lineTo(x0, bottom); g.closePath();
        g.fillStyle = col('--accent'); g.globalAlpha = 0.1; g.fill(); g.globalAlpha = 1;
      }
      g.fillStyle = col('--accent');
      for (const x of [this.sampleToX(fin.b), this.sampleToX(fout.a)]) if (x >= -6 && x <= W + 6) g.fillRect(Math.round(x) - 5, Math.round(yOf(x)) - 5, 10, 10);
    }
    // Open on Trim silent ends, the parts it would cut are covered, with a line at each new edge.
    if (this.fx?.tab === 'tool' && this.fx.toolName === 'trimsilence') {
      const r = this.trimBounds() || [0, 0];
      g.fillStyle = col('--wave-bg'); g.globalAlpha = 0.78;
      const xa = Math.min(W, this.sampleToX(r[0])), xb = Math.max(0, this.sampleToX(r[1]));
      if (r[0] > 0 && xa > 0) g.fillRect(0, RULER, xa, H - RULER);
      if (r[1] < this.length && xb < W) g.fillRect(xb, RULER, W - xb, H - RULER);
      g.globalAlpha = 1; g.fillStyle = col('--accent');
      if (r[0] > 0) g.fillRect(Math.round(xa) - 1, RULER, 2, H - RULER);
      if (r[1] < this.length) g.fillRect(Math.round(xb) - 1, RULER, 2, H - RULER);
    }
    const cx = this.sampleToX(this.cursor);
    const px = this.playing ? this.sampleToX(this.playPos()) : this.auditionSrc ? this.sampleToX(this.auditionPos()) : null;
    const head = px ?? cx;
    g.fillStyle = col('--accent'); g.fillRect(Math.round(head) - 1, RULER, 2, H - RULER);

    g.fillStyle = col('--panel'); g.fillRect(0, 0, W, RULER);
    g.fillStyle = col('--muted'); g.font = '11px "Share Tech Mono", "Martian Mono", ui-monospace, monospace';
    const beatNow = this.beatLen();
    if (beatNow) {
      // Bar numbers, with beats as "3.2" once there is room for them.
      let every = 1; while (beatNow * every / this.spp < 46) every *= every < 4 ? 2 : every === 4 ? 4 : 2;
      for (let k = Math.ceil(this.start / beatNow / every) * every; this.sampleToX(k * beatNow) < W; k += every) {
        const x = this.sampleToX(k * beatNow);
        if (x >= 1) g.fillRect(x, RULER - 5, 1, 5);
        g.fillText(k % 4 === 0 ? String(k / 4 + 1) : `${Math.floor(k / 4) + 1}.${k % 4 + 1}`, x + 3, 11);
      }
    } else for (let t = t0; this.sampleToX(t * this.sr) < W; t += tick) {
      const x = this.sampleToX(t * this.sr);
      if (x >= 1) g.fillRect(x, RULER - 5, 1, 5);
      g.fillText(tick < 0.01 ? t.toFixed(4) : tick < 1 ? t.toFixed(2) : fmtTime(t), x + 3, 11);
    }
    g.fillStyle = col('--accent');
    const m = Math.round(head);
    g.beginPath(); g.moveTo(m - 6, RULER - 8); g.lineTo(m + 6, RULER - 8); g.lineTo(m, RULER); g.closePath(); g.fill();
    this.updateInfo();
  }

  // Background, time grid and waveform. These change only when the view or the sound does, so they
  // are painted once into a layer; a playback frame then costs one image copy plus the playhead.
  drawWave(col, RULER, W, H, dpr, tick) {
    const view = this.viewChannels();
    const paint = [col('--wave-bg'), col('--line'), col('--wave'), col('--muted')];
    // Behind the result of an open effect or tool, the sound as it is now stays visible, fainter.
    const ghost = view !== this.channels && view[0].length === this.length && view.length === this.channels.length ? this.channels : null;
    const beat = this.beatLen();
    const key = [this.start, this.spp, W, H, dpr, this.version, view === this.channels ? '' : this.fx?.view?.key, tick, beat, ...paint].join('|');
    if (this.layerKey !== key) {
      const layer = this.layer ||= document.createElement('canvas');
      layer.width = this.canvas.width; layer.height = this.canvas.height;
      const g = layer.getContext('2d');
      g.setTransform(dpr, 0, 0, dpr, 0, 0);
      g.fillStyle = paint[0]; g.fillRect(0, 0, W, H);
      // Time grid behind the waveform: a line per labelled tick, a fainter one halfway between.
      g.fillStyle = paint[1];
      const half = tick / 2;
      if (beat) {
        // A line per beat, heavier on each bar; beats thin out when they would crowd.
        let every = 1; while (beat * every / this.spp < 14) every *= every < 4 ? 2 : every === 4 ? 4 : 2;
        for (let k = Math.ceil(this.start / beat / every) * every; this.sampleToX(k * beat) < W; k += every) {
          g.globalAlpha = k % 4 === 0 ? 0.95 : 0.4;
          g.fillRect(Math.round(this.sampleToX(k * beat)), RULER, 1, H - RULER);
        }
      } else for (let t = Math.ceil(this.start / this.sr / half) * half; this.sampleToX(t * this.sr) < W; t += half) {
        const major = Math.abs(t / tick - Math.round(t / tick)) < 1e-6;
        g.globalAlpha = major ? 0.9 : 0.4;
        g.fillRect(Math.round(this.sampleToX(t * this.sr)), RULER, 1, H - RULER);
      }
      g.globalAlpha = 1;
      const lanes = view.length, laneH = (H - RULER) / lanes;
      for (const [chans, ink, alpha] of ghost ? [[ghost, paint[3], 0.45], [view, paint[2], 1]] : [[view, paint[2], 1]]) for (let c = 0; c < lanes; c++) {
        const d = chans[c], y0 = RULER + c * laneH, mid = y0 + laneH / 2, amp = laneH / 2 - 3;
        g.globalAlpha = 1;
        if (chans === view) { g.strokeStyle = paint[1]; g.beginPath(); g.moveTo(0, mid + .5); g.lineTo(W, mid + .5); g.stroke(); }
        g.globalAlpha = alpha;
        g.fillStyle = ink;
        if (this.spp <= 1) {
          g.strokeStyle = ink; g.beginPath();
          for (let x = 0; x <= W; x++) {
            const i = Math.floor(this.start + x * this.spp);
            if (i >= d.length) break;
            const y = mid - d[i] * amp;
            x ? g.lineTo(x, y) : g.moveTo(x, y);
          }
          g.stroke();
          if (this.spp < 0.12) {
            for (let i = Math.ceil(this.start); i < Math.min(d.length, this.start + W * this.spp); i++)
              g.fillRect(this.sampleToX(i) - 1.5, mid - d[i] * amp - 1.5, 3, 3);
          }
        } else {
          const pyr = this.spp >= PYRAMID * 2 ? this.pyramid(d) : null;
          for (let x = 0; x < W; x++) {
            const a = Math.floor(this.start + x * this.spp), b = Math.min(d.length, Math.floor(this.start + (x + 1) * this.spp));
            if (a >= d.length) break;
            let mn = 1, mx = -1, i = a;
            if (pyr) {
              // Whole blocks come from the pyramid; the few samples before and after are read directly.
              const k0 = Math.ceil(a / PYRAMID), k1 = Math.floor(b / PYRAMID);
              for (const e = Math.min(b, k0 * PYRAMID); i < e; i++) { const v = d[i]; if (v < mn) mn = v; if (v > mx) mx = v; }
              for (let k = k0; k < k1; k++) { if (pyr.mn[k] < mn) mn = pyr.mn[k]; if (pyr.mx[k] > mx) mx = pyr.mx[k]; }
              i = Math.max(i, k1 * PYRAMID);
            }
            for (; i < b; i++) { const v = d[i]; if (v < mn) mn = v; if (v > mx) mx = v; }
            if (mn > mx) continue;
            g.fillRect(x, mid - mx * amp, 1, Math.max(1, (mx - mn) * amp));
          }
        }
      }
      g.globalAlpha = 1;
      this.layerKey = key;
    }
    this.g.save(); this.g.setTransform(1, 0, 0, 1, 0, 0);
    this.g.drawImage(this.layer, 0, 0);
    this.g.restore();
  }

  // Lowest and highest sample of every PYRAMID-sample block of a channel, rebuilt when the sound
  // changes. It makes the zoomed-out waveform exact: no peak is skipped, however long the file.
  pyramid(d) {
    const cache = this.pyramids ||= new WeakMap();
    let p = cache.get(d);
    if (!p || p.version !== this.version) {
      const n = Math.floor(d.length / PYRAMID), mn = new Float32Array(n), mx = new Float32Array(n);
      for (let k = 0, i = 0; k < n; k++) {
        let lo = d[i], hi = lo;
        for (const e = i + PYRAMID; i < e; i++) { const v = d[i]; if (v < lo) lo = v; else if (v > hi) hi = v; }
        mn[k] = lo; mx[k] = hi;
      }
      cache.set(d, p = { version: this.version, mn, mx });
    }
    return p;
  }

  drawEmpty() {
    const dpr = window.devicePixelRatio || 1;
    const W = this.canvas.clientWidth, H = this.canvas.clientHeight;
    if (!W || !H) return;
    if (this.canvas.width !== W * dpr || this.canvas.height !== H * dpr) {
      this.canvas.width = W * dpr; this.canvas.height = H * dpr;
    }
    const g = this.g;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    const css = getComputedStyle(document.documentElement);
    const col = (n) => css.getPropertyValue(n).trim();
    const RULER = 18, secs = 10, pps = W / secs;
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
    g.fillStyle = col('--muted'); g.font = '11px "Share Tech Mono", "Martian Mono", ui-monospace, monospace';
    for (let t = 0; t < secs; t++) { const x = t * pps; if (t) g.fillRect(x, RULER - 5, 1, 5); g.fillText(t + 's', x + 3, 11); }
    const msg = 'Double-click a sound, or use its Edit button';
    g.font = '12px "Share Tech Mono", "Martian Mono", ui-monospace, monospace';
    const tw = g.measureText(msg).width, bx = (W - tw) / 2 - 12, by = RULER + (H - RULER) / 2 - 16;
    g.fillStyle = col('--wave-bg'); g.fillRect(bx, by, tw + 24, 32);
    g.fillStyle = col('--muted'); g.fillText(msg, bx + 12, by + 20);
  }

  // The picture is made of tiles SPEC_TILE device pixels wide, computed in the worker and kept per
  // zoom level, so scrolling reuses them. While a zoom level is still filling in, tiles from other
  // zoom levels are drawn stretched underneath.
  drawSpec(top, W, H) {
    const view = this.viewChannels(), dpr = window.devicePixelRatio || 1;
    const PW = Math.round(W * dpr), rows = Math.round(H * dpr), spd = this.spp / dpr;
    const dataKey = `${this.version}|${view === this.channels ? '' : this.fx?.view?.key}`;
    if (this.tiles?.dataKey !== dataKey) this.tiles = { dataKey, map: new Map() };
    const map = this.tiles.map, zoom = `${spd}|${rows}`, x0 = this.start / spd;
    const g0 = this.g;
    g0.save(); g0.setTransform(1, 0, 0, 1, 0, 0);
    g0.beginPath(); g0.rect(0, top * dpr, PW, rows); g0.clip();
    g0.fillStyle = SPEC_BG; g0.fillRect(0, top * dpr, PW, rows);
    for (const t of map.values()) {
      if (t.zoom === zoom) continue;
      const x = t.k * SPEC_TILE * t.spd / spd - x0, w = SPEC_TILE * t.spd / spd;
      if (x < PW && x + w > 0) g0.drawImage(t.canvas, x, top * dpr, w, rows);
    }
    const missing = [];
    for (let k = Math.max(0, Math.floor(x0 / SPEC_TILE)); k * SPEC_TILE - x0 < PW && k * SPEC_TILE * spd < this.length; k++) {
      const t = map.get(`${zoom}|${k}`);
      if (t) g0.drawImage(t.canvas, k * SPEC_TILE - x0, top * dpr); else missing.push(k);
    }
    g0.restore();
    this.specWant = missing.length && this.specFailed !== dataKey ? { dataKey, view, zoom, spd, rows, missing } : null;
    if (this.specWant) this.specLoop();
    // Frequency axis, plus a readout of frequency and note under the pointer.
    const g = this.g, fmin = 20, fmax = this.sr / 2;
    const yOf = (f) => top + H * (1 - Math.log(f / fmin) / Math.log(fmax / fmin));
    g.font = '10px "Share Tech Mono", "Martian Mono", ui-monospace, monospace';
    for (const f of [50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000]) {
      if (f >= fmax) break;
      const y = Math.round(yOf(f)) + .5;
      g.fillStyle = 'rgba(255,246,214,.18)'; g.fillRect(0, y, W, 1);
      g.fillStyle = 'rgba(255,246,214,.75)'; g.fillText(f >= 1000 ? f / 1000 + 'k' : String(f), 4, y - 3);
    }
    if (this.hover && this.hover.y > top) {
      const f = fmin * Math.pow(fmax / fmin, 1 - (this.hover.y - top) / H);
      const t = this.xToSample(this.hover.x) / this.sr;
      const txt = `${f >= 1000 ? (f / 1000).toFixed(2) + ' kHz' : Math.round(f) + ' Hz'}  ${noteOf(f)}  ${t.toFixed(2)}s`;
      const w = g.measureText(txt).width + 14, bx = Math.min(W - w - 4, this.hover.x + 12), by = Math.max(top + 4, this.hover.y - 26);
      g.fillStyle = 'rgba(18,16,30,.85)'; g.fillRect(bx, by, w, 20);
      g.fillStyle = '#fff6d6'; g.fillText(txt, bx + 7, by + 14);
      g.fillStyle = 'rgba(255,246,214,.5)'; g.fillRect(0, Math.round(this.hover.y), W, 1);
    }
  }

  async specLoop() {
    if (this.specBusy) return;
    this.specBusy = true;
    try {
      for (let w; (w = this.specWant) && w.missing.length;) {
        const k = w.missing.shift(), key = `${w.zoom}|${k}`, tiles = this.tiles;
        if (tiles.dataKey !== w.dataKey || tiles.map.has(key)) continue;
        if (this.specSent !== w.dataKey) { await run('specData', { data: mixMono(w.view), sr: this.sr }); this.specSent = w.dataKey; }
        const { pixels } = await run('spec', { sr: this.sr, start: k * SPEC_TILE * w.spd, spp: w.spd, PW: SPEC_TILE, rows: w.rows });
        const canvas = document.createElement('canvas');
        canvas.width = SPEC_TILE; canvas.height = w.rows;
        canvas.getContext('2d').putImageData(new ImageData(pixels, SPEC_TILE, w.rows), 0, 0);
        tiles.map.set(key, { canvas, k, zoom: w.zoom, spd: w.spd });
        // Oldest tiles go first; a Map keeps insertion order.
        while (tiles.map.size > SPEC_TILES_KEPT) tiles.map.delete(tiles.map.keys().next().value);
        this.draw();
      }
    } catch (e) { this.specFailed = this.specWant?.dataKey; this.specWant = null; this.notice(`Spectrogram failed: ${e.message}`); }
    finally { this.specBusy = false; }
  }

  viewChannels() { return this.fx?.displayChannels() || this.channels; }

  updateInfo() {
    const sec = (s) => fmtTime(s / this.sr, true);
    const nch = this.channels.length;
    const outRate = Number(this.$('[data-f=rate]')?.value) || this.sr;
    const bits = this.$('[data-f=bits]')?.value;
    const sig = [this.length, this.sr, nch, outRate, bits, this.version, this.savedVersion, this.savedOnce,
      this.sel?.join(), this.cursor, this.undo.length, this.redo.length, this.meterVersion].join('|');
    if (sig === this.infoSig) return;
    this.infoSig = sig;
    this.fx?.selectionChanged();
    this.$('[data-a=mono]').hidden = nch === 1;
    const khz = (r) => `${+(r / 1000).toFixed(2)} kHz`;
    const chips = [
      [sec(this.length)],
      outRate === this.sr ? [khz(this.sr)] : [`${khz(this.sr)} → ${khz(outRate)}`, 'changed'],
      [nch === 1 ? 'Mono' : nch === 2 ? 'Stereo' : `${nch} ch`],
      [bits === '32' ? '32-bit float' : `${bits}-bit`],
    ];
    if (this.meterVersion === this.version) {
      const m = this.meter, over = m.peak > 0.005;
      if (isFinite(m.lufs)) chips.push([`${m.lufs.toFixed(1)} LUFS`, '', 'Integrated loudness of the whole sound (ITU-R BS.1770)']);
      chips.push(over
        ? [`Clips at +${m.peak.toFixed(1)} dB`, 'clip', 'Louder than full scale. A 16 or 24-bit file will clip here: normalize, lower the gain, or use the Maximizer.']
        : [`Peak ${m.peak <= -99 ? 'silent' : m.peak.toFixed(1) + ' dB'}`, '', `True peak ${m.truePeak.toFixed(1)} dB: the highest point of the waveform between samples`]);
    } else if (!this.meterTimer) {
      this.meterTimer = setTimeout(async () => {
        this.meterTimer = null;
        if (!this.channels) return;
        const v = this.version;
        try { const m = await run('measure', { chs: this.channels, sr: this.sr }, 1); if (v === this.version) { this.meter = m; this.meterVersion = v; } }
        catch { return; }
        this.infoSig = null; this.updateInfo();
      }, 300);
    }
    const info = this.$('[data-f=info]');
    info.replaceChildren(...chips.map(([t, c, tip]) => {
      const n = Object.assign(document.createElement('span'), { className: 'ed-chip' + (c ? ' ' + c : ''), textContent: t });
      if (tip) n.dataset.tip = tip;
      return n;
    }));
    const changed = this.version !== this.savedVersion;
    if (changed !== this.wasDirty) { this.wasDirty = changed; this.onDirtyChange?.(changed); }
    this.$('#saveBtn').classList.toggle('strong', changed);
    // The name reads like a path: the project (or Library) the file is in, then the file.
    const place = this.$('[data-f=place]'), where = this.path ? this.place?.() || '' : '';
    place.textContent = where; place.hidden = !where;
    place.dataset.tip = where === 'Library' ? 'This file is in a Library folder' : `This file is in the project ${where}`;
    // Last among the chips, and hard to miss: what is on screen is not on disk.
    if (changed || !this.path) {
      const chip = Object.assign(document.createElement('span'), { className: 'ed-chip edited', textContent: this.path ? 'Edited, not saved' : 'Not saved yet' });
      chip.dataset.tip = this.path ? 'What you hear is only in the editor. The file on disk is unchanged until you save a new one' : 'This sound is only in the editor until you save it';
      info.append(chip);
    }
    const pos = this.$('[data-f=pos]'), b = (t) => Object.assign(document.createElement('b'), { textContent: t });
    if (this.sel && this.sel[1] > this.sel[0]) pos.replaceChildren('Selection ', b(`${sec(this.sel[0])} – ${sec(this.sel[1])}`), ` · ${sec(this.sel[1] - this.sel[0])}`);
    else pos.replaceChildren();
    this.$('[data-a=undo]').disabled = !this.undo.length;
    this.$('[data-a=redo]').disabled = !this.redo.length;
  }

  // ---- playback
  // fromStart plays from the start of the selection, or of the sound, whatever the cursor says.
  play(resume = false, fromStart = false) {
    const pos = resume && this.playing ? this.playPos() : null;
    this.stopAudition();
    this.stopPlay();
    readyAudio();
    const hasSel = this.sel && this.sel[1] > this.sel[0];
    const [a, b] = hasSel ? this.sel : [this.loop || fromStart ? 0 : this.cursor, this.length];
    if (b - a < 1) return;
    const start = pos != null && pos >= a && pos < b ? Math.floor(pos) : (!hasSel && this.loop && !fromStart && this.cursor < b ? this.cursor : a);
    const pv = this.fx?.previewFor();
    let buffer, base = 0, end = b;
    if (pv && start >= pv.a && start < pv.b) {
      buffer = pv.buffer ||= toBuffer(pv.channels, this.sr); base = pv.a; end = Math.min(b, pv.b);
    } else {
      // Built once per version of the sound, not on every press of play.
      if (this.bufVersion !== this.version) { this.buf = toBuffer(this.channels, this.sr); this.bufVersion = this.version; }
      buffer = this.buf;
    }
    const src = ctx.createBufferSource();
    src.buffer = buffer;
    (this.fx ? this.fx.route(ctx, src) : src).connect(out);
    const loop = this.loop;
    if (loop) { src.loop = true; src.loopStart = (a - base) / this.sr; src.loopEnd = (end - base) / this.sr; src.start(0, (start - base) / this.sr); }
    else src.start(0, (start - base) / this.sr, (end - start) / this.sr);
    src.onended = () => { if (this.src === src) { this.playing = false; this.src = null; this.draw(); this.syncPlayBtn(); } };
    this.src = src; this.playLoop = loop; this.playA = a; this.playStart = start; this.playB = end; this.playT = ctx.currentTime; this.playing = true;
    this.syncPlayBtn();
    const tick = () => { if (!this.playing || this.src !== src) return; soundPlaying(); this.draw(); requestAnimationFrame(tick); };
    requestAnimationFrame(tick);
  }
  playPos() {
    let p = this.playStart + (ctx.currentTime - this.playT) * this.sr;
    if (this.playLoop && p >= this.playB) p = this.playA + (p - this.playA) % (this.playB - this.playA);
    return Math.min(this.playB, p);
  }
  stopPlay() {
    if (this.src) { const s = this.src; this.src = null; try { s.stop(); } catch { } }
    this.playing = false; this.syncPlayBtn();
  }
  toggle() {
    if (this.auditionSrc) { this.stopAudition(); return; }
    if (!this.playing && this.fx?.showsOtherLength()) return void this.audition(this.fx.toolName);
    this.playing ? this.stopPlay() : this.play(); this.draw();
  }
  syncPlayBtn() { const b = this.$('[data-a=play] .i'); if (b) b.textContent = this.playing ? 'stop' : 'play_arrow'; }

  // ---- editing
  // audio: true copies the whole sound, [a, b] copies only that span (for edits that change
  // samples in place and keep the length), false copies none.
  snapshot(audio) {
    const range = Array.isArray(audio) ? [...audio] : null;
    const channels = range ? this.channels.map(c => c.slice(range[0], range[1])) : audio ? this.channels.map(c => c.slice()) : null;
    return { channels, range, sel: this.sel && [...this.sel], cursor: this.cursor, regions: this.regions.map(r => ({ ...r })) };
  }
  restore(s) {
    if (s.range) { this.stopPlay(); this.channels.forEach((d, c) => d.set(s.channels[c], s.range[0])); this.version++; }
    else if (s.channels) { this.stopPlay(); this.channels = s.channels; this.version++; }
    this.sel = s.sel; this.cursor = s.cursor; this.regions = s.regions; this.renderRegions();
    if (!s.channels) this.persistRegions();
    this.refit();
  }
  refit() { if (this.spp * this.width() > this.viewLen()) this.fit(); else { this.clampView(); this.draw(); } }
  // Undo keeps full audio copies, so history is capped by memory rather than by step count.
  // Region-only steps store no audio: the audio is unchanged between them and their neighbours.
  trimHistory() {
    const bytes = (s) => s.channels ? s.channels.reduce((n, c) => n + c.byteLength, 0) : 0;
    let total = [...this.undo, ...this.redo].reduce((n, s) => n + bytes(s), 0);
    while (total > UNDO_BYTES && this.undo.length) total -= bytes(this.undo.shift());
  }
  pushUndo(audio) { this.undo.push(this.snapshot(audio)); this.redo = []; this.trimHistory(); }
  // label names the edit for readers of the code; the waveform itself shows that it happened.
  apply(fn, label, range = null) {
    if (!this.channels) return;
    this.stopPlay();
    this.pushUndo(range || true);
    fn();
    this.version++;
    this.cursor = Math.min(this.cursor, this.length);
    if (this.sel) this.sel = [Math.min(this.sel[0], this.length), Math.min(this.sel[1], this.length)];
    this.regions = this.regions.map(r => ({ ...r, a: Math.min(r.a, this.length), b: Math.min(r.b, this.length) })).filter(r => r.b > r.a);
    this.renderRegions();
    this.refit();
  }
  editRegions(fn) {
    this.pushUndo(false);
    const hadSel = !!this.sel;
    fn();
    this.renderRegions(); this.draw();
    this.persistRegions();
    if (hadSel && !this.sel && this.playing) this.play(true);
  }
  doUndo() { if (this.undo.length) { const s = this.undo.pop(); this.redo.push(this.snapshot(s.range || !!s.channels)); this.restore(s); this.trimHistory(); } }
  doRedo() { if (this.redo.length) { const s = this.redo.pop(); this.undo.push(this.snapshot(s.range || !!s.channels)); this.restore(s); this.trimHistory(); } }

  mapRange(fn) { const [a, b] = this.range(); this.channels.forEach(d => fn(d, a, b)); }

  // Replaces [a, b) with insert (or removes it). Region edges after the edit shift with the audio;
  // edges inside the replaced span go through inner, which defaults to collapsing them onto a.
  // With smooth on (and the setting not switched off), the joins are made click-free: a removal
  // crossfades the audio leading up to the cut into the audio leading up to its far side, and
  // an insertion dips briefly to silence at each end, since it has no audio to overlap with.
  splice(a, b, insert, inner = () => a, smooth = true) {
    const n = insert ? insert[0].length : 0, d = n - (b - a), len = this.length;
    const k = smooth && this.settings?.smooth_edits !== false && a > 0 && b < len
      ? Math.round(this.sr * (this.settings?.edit_fade_ms ?? 3) / 1000) : 0;
    const curve = (i, m) => Math.sin((i + .5) / m * Math.PI / 2) ** 2;
    this.channels = this.channels.map((ch, c) => {
      const out = new Float32Array(ch.length + d);
      out.set(ch.subarray(0, a));
      if (insert) out.set(insert[Math.min(c, insert.length - 1)], a);
      out.set(ch.subarray(b), a + n);
      if (k && !insert) {
        const m = Math.min(k, a, b - a);
        for (let i = 0; i < m; i++) { const w = curve(i, m); out[a - m + i] = ch[a - m + i] * (1 - w) + ch[b - m + i] * w; }
      } else if (k) {
        const m = Math.min(Math.max(1, k >> 1), a, n >> 1, len - b);
        for (let i = 0; i < m; i++) { const w = curve(i, m); out[a - 1 - i] *= w; out[a + i] *= w; out[a + n - 1 - i] *= w; out[a + n + i] *= w; }
      }
      return out;
    });
    const start = (p) => p < a ? p : p >= b ? p + d : inner(p);
    const end = (p) => p <= a ? p : p >= b ? p + d : inner(p);
    this.regions = this.regions.map(r => ({ ...r, a: start(r.a), b: end(r.b) })).filter(r => r.b > r.a);
  }
  crop(a, b) { this.splice(b, this.length); this.splice(0, a); }

  async action(name) {
    const sel = this.sel && this.sel[1] > this.sel[0];
    const needSel = () => { if (!sel) { this.notice('Select a region first'); return false; } return true; };
    const whole = () => sel && this.sel[0] === 0 && this.sel[1] === this.length;
    switch (name) {
      case 'play': return this.toggle();
      case 'loop': this.loop = !this.loop; this.$('[data-a=loop]').classList.toggle('on', this.loop); if (this.playing) this.play(true); else if (this.auditionSrc) this.audition(this.fx.toolName); return;
      case 'ab': return this.fx.toggleAb();
      case 'zoomin': return this.zoom(0.5);
      case 'zoomout': return this.zoom(2);
      case 'fit': return this.fit();
      case 'zoomsel': return this.zoomSel();
      case 'all': this.sel = [0, this.length]; return this.draw();
      case 'deselect': this.sel = null; return this.draw();
      case 'home': this.cursor = 0; this.start = 0; if (this.playing) this.play(); return this.draw();
      case 'end': this.cursor = this.length; this.start = this.length; this.clampView(); return this.draw();
      case 'bars': {
        if (this.bars) this.bars = false;
        else {
          const v = Number(await this.ask?.('Tempo in beats per minute', this.$('[data-f=bpm]').value));
          if (!(v >= 20 && v <= 400)) return;
          this.$('[data-f=bpm]').value = v; this.bars = true;
        }
        this.$('[data-a=bars]').classList.toggle('on', this.bars); this.layerKey = null;
        return this.draw();
      }
      case 'view': this.mode = this.mode === 'wave' ? 'spec' : 'wave'; this.$('[data-a=view]').classList.toggle('on', this.mode === 'spec'); return this.draw();
      case 'undo': return this.doUndo();
      case 'redo': return this.doRedo();
      case 'trim': if (!needSel()) return; return this.apply(() => {
        this.crop(...this.sel); this.sel = null; this.cursor = 0;
      }, 'Trimmed to selection');
      case 'delete': if (!needSel()) return;
        if (whole()) return this.notice('Cannot delete the whole sound');
        return this.apply(() => {
          const [a, b] = this.sel; this.splice(a, b); this.cursor = a; this.sel = null;
        }, 'Deleted selection');
      case 'copy': if (!needSel()) return; { const [a, b] = this.sel; this.clip = { chans: this.channels.map(d => d.slice(a, b)), sr: this.sr }; this.toast('Copied'); } return;
      case 'cut': if (!needSel()) return;
        if (whole()) return this.notice('Cannot cut the whole sound');
        { const [a, b] = this.sel; this.clip = { chans: this.channels.map(d => d.slice(a, b)), sr: this.sr }; }
        return this.apply(() => { const [a, b] = this.sel; this.splice(a, b); this.cursor = a; this.sel = null; }, 'Cut');
      case 'paste': {
        if (!this.clip) return this.notice('Nothing copied');
        let ins = this.clip.sr === this.sr ? this.clip.chans : resample(this.clip.chans, this.clip.sr, this.sr);
        if (ins.length > this.channels.length) ins = [mixMono(ins)];
        return this.apply(() => {
          const [a, b] = sel ? this.sel : [this.cursor, this.cursor];
          this.splice(a, b, ins); this.sel = [a, a + ins[0].length];
        }, 'Pasted');
      }
      case 'silence': if (!needSel()) return; return this.apply(() => this.mapRange((d, a, b) => d.fill(0, a, b)), 'Silenced', this.range());
      case 'fade': {
        const spans = this.fadeSpans().filter(m => m.b > m.a);
        if (!spans.length) return this.notice('Set a fade in or a fade out length first');
        return this.apply(() => spans.forEach(m => this.fade(m.a, m.b, m.fadeIn)), spans.length > 1 ? 'Fade in and fade out' : spans[0].fadeIn ? 'Fade in' : 'Fade out', this.range());
      }
      case 'normalize': {
        const target = Math.pow(10, Number(this.$('[data-f=norm]').value || -1) / 20);
        let peak = 0; this.mapRange((d, a, b) => { for (let i = a; i < b; i++) peak = Math.max(peak, Math.abs(d[i])); });
        if (!peak) return this.notice('Selection is silent');
        const k = target / peak;
        return this.apply(() => this.mapRange((d, a, b) => { for (let i = a; i < b; i++) d[i] *= k; }), `Normalized to ${this.$('[data-f=norm]').value} dBFS`, this.range());
      }
      case 'gain': {
        const db = Number(this.$('[data-f=gain]').value || 0), k = Math.pow(10, db / 20);
        return this.apply(() => this.mapRange((d, a, b) => { for (let i = a; i < b; i++) d[i] *= k; }), `Gain ${db > 0 ? '+' : ''}${db} dB`, this.range());
      }
      case 'reverse': return this.apply(() => this.mapRange((d, a, b) => d.subarray(a, b).reverse()), 'Reversed', this.range());
      case 'mono': if (this.channels.length < 2) return;
        return this.apply(() => { this.channels = [mixMono(this.channels)]; }, 'Mixed to mono');
      case 'speed': {
        const pct = Number(this.$('[data-f=speed]').value || 100);
        if (pct <= 0 || pct === 100) return;
        return this.respeed(pct / 100, `Speed ${pct}%`);
      }
      case 'tune': {
        const semis = Number(this.$('[data-f=tunesemis]').value || 0);
        if (!semis) return;
        return this.respeed(Math.pow(2, semis / 12), `Tuned by ${semis > 0 ? '+' : ''}${semis.toFixed(2)} semitones`);
      }
      case 'loudnorm': {
        const target = Number(this.$('[data-f=lufs]').value || -16), [a, b] = this.range(), v = this.version;
        return run('measure', { chs: this.channels.map(d => d.slice(a, b)), sr: this.sr }).then((m) => {
          if (this.version !== v) return this.notice('The sound changed, apply again');
          if (!isFinite(m.lufs)) return this.notice('Selection is silent');
          const gain = target - m.lufs, k = Math.pow(10, gain / 20);
          this.apply(() => this.channels.forEach((d) => { for (let i = a; i < b; i++) d[i] *= k; }), null, [a, b]);
          if (m.truePeak + gain > 0) this.notice(`At ${target} LUFS this sound peaks ${(m.truePeak + gain).toFixed(1)} dB over full scale. Use the Maximizer, or a lower target.`, 8000);
        }).catch(e => this.notice(e.message));
      }
      case 'stretch': {
        const pct = Number(this.$('[data-f=stretchlen]').value || 100), semis = Number(this.$('[data-f=semis]').value || 0);
        if (!(pct > 0) || (pct === 100 && !semis)) return;
        const [a, b] = this.range(), v = this.version;
        if (b - a < this.sr * 0.1) return this.notice('Select at least a tenth of a second');
        return run('stretchPitch', { chs: this.channels.map(d => d.slice(a, b)), sr: this.sr, length: pct / 100, semitones: semis }).then(({ channels: seg }) => {
          if (this.version !== v) return this.notice('The sound changed, apply again');
          const n = seg[0].length;
          this.apply(() => {
            this.splice(a, b, seg, p => a + Math.round((p - a) * n / (b - a)), false);
            if (sel) this.sel = [a, a + n];
          }, null);
        }).catch(e => this.notice(e.message));
      }
      case 'loopxfade': {
        const [a, b] = this.range(), n = Math.round(Number(this.$('[data-f=xfade]').value || 50) / 1000 * this.sr);
        if (b - a < 4 || n < 1) return;
        if (n > (b - a) / 2) return this.notice('The crossfade can be at most half of the loop');
        const seg = loopCrossfade(this.channels.map(d => d.subarray(a, b)), n), end = a + seg[0].length;
        this.apply(() => { this.splice(a, b, seg, p => Math.min(end, p), false); this.sel = [a, end]; this.cursor = a; }, null);
        if (!this.loop) this.action('loop');
        return;
      }
      case 'declick': {
        const [a, b] = this.range(), v = this.version;
        return run('declick', { chs: this.channels.map(d => d.slice(a, b)), sr: this.sr, sensitivity: Number(this.$('[data-f=clicksens]').value || 5) }).then((r) => {
          if (this.version !== v) return this.notice('The sound changed, apply again');
          this.apply(() => this.channels.forEach((d, i) => d.set(r.channels[i], a)), null, [a, b]);
          this.notice(r.info[0].toUpperCase() + r.info.slice(1));
        }).catch(e => this.notice(e.message));
      }
      case 'transients': {
        const [a, b] = this.range(), v = this.version;
        return run('onsets', { data: mixMono(this.channels.map(d => d.slice(a, b))), sr: this.sr, sensitivity: Number(this.$('[data-f=onsetsens]').value || 5) }).then(({ positions }) => {
          if (this.version !== v) return this.notice('The sound changed, slice again');
          const pts = [...new Set(positions.map(p => this.zeroCross(a + p)))].sort((x, y) => x - y);
          // A hit that is already sounding at the very start has no rise to detect; it gets the first slice.
          const head = Math.min(b, a + Math.round(this.sr * 0.01)), th = this.threshold();
          if ((pts[0] ?? b) - a > this.sr * 0.03 && this.channels.some(d => d.subarray(a, head).some(v => Math.abs(v) > th))) pts.unshift(a);
          const found = pts.map((p, i) => ({ a: p, b: pts[i + 1] ?? b })).filter(r => r.b - r.a >= this.sr * 0.01);
          if (!found.length) return this.notice('No hits found. Raise the sensitivity.');
          this.editRegions(() => { this.regions = found; });
        }).catch(e => this.notice(e.message));
      }
      case 'gridslice': {
        const bpm = Number(this.$('[data-f=bpm]').value), per = Number(this.$('[data-f=perbeat]').value) || 1, [a, b] = this.range();
        if (!(bpm > 0)) return this.notice('Enter the tempo first');
        const step = this.sr * 60 / bpm / per, found = [];
        if ((b - a) / step > 512) return this.notice('That would make more than 512 slices');
        for (let x = a; x + step * 0.5 <= b; x += step) found.push({ a: Math.round(x), b: Math.min(b, Math.round(x + step)) });
        if (!found.length) return this.notice('The sound is shorter than one slice');
        return this.editRegions(() => { this.regions = found; });
      }
      case 'snap': {
        if (!this.channels) return;
        if (this.sel && this.sel[1] > this.sel[0]) {
          const s = [this.zeroCross(this.sel[0]), this.zeroCross(this.sel[1])].sort((x, y) => x - y);
          if (s[1] > s[0]) this.sel = s;
          this.cursor = this.sel[0];
        } else this.cursor = this.zeroCross(this.cursor);
        if (this.playing) this.play();
        return this.draw();
      }
      case 'trimsilence': {
        const r = this.soundBounds(this.trimThreshold());
        if (!r) return this.notice('Nothing above the threshold');
        if (r[0] === 0 && r[1] === this.length) return this.notice('Nothing is cut at this level. Raise "Silence below" until the ends are marked.');
        return this.apply(() => { this.crop(...r); this.sel = null; this.cursor = 0; }, 'Trimmed silence at both ends');
      }
      case 'prep': return this.apply(() => {
        this.removeDc();
        const r = this.soundBounds(); if (r) this.crop(...r);
        this.normalizeAll();
        const fi = (this.settings?.prep_fade_in_ms ?? 2) / 1000, fo = (this.settings?.prep_fade_out_ms ?? 10) / 1000;
        this.fade(0, Math.min(this.length, Math.round(this.sr * fi)), true);
        this.fade(Math.max(0, this.length - Math.round(this.sr * fo)), this.length, false);
        this.sel = null; this.cursor = 0;
      }, 'Sampler prep: DC removed, silence trimmed, normalized, edge fades');
      case 'addregion': if (!needSel()) return;
        return this.editRegions(() => { this.regions.push({ a: this.sel[0], b: this.sel[1] }); this.regions.sort((x, y) => x.a - y.a); });
      case 'autoslice': {
        const found = this.detectRegions();
        if (!found.length) return this.notice('No sounds above the threshold');
        this.editRegions(() => { this.regions = found; });
        return;
      }
      case 'clearregions': if (!this.regions.length) return;
        return this.editRegions(() => { this.regions.forEach(r => this.dropSelOf(r)); this.regions = []; });
      case 'exportregions': return this.exportRegions();
      case 'sp404':
        return fetch('/api/sp404-card').then(r => r.json()).catch(() => ({}))
          .then(card => this.pickFolder('Choose the SD card (or any folder)', card.import || '')).then(async (dir) => {
          if (!dir) return;
          const blob = await this.render(this.channels, { mono: this.settings?.sp404_mono || this.settings?.export_mono, rate: 48000, bits: 16 });
          const res = await this.onSave(blob, dir, this.fileName(), { sp404: true });
          if (res) this.toast(`Saved for SP-404MKII: ${res}`, 5000);
        });
      case 'save': case 'saveto': return this.save();
      case 'download':
        return this.render(this.channels).then(b => {
          const a = document.createElement('a'), url = URL.createObjectURL(b);
          a.href = url; a.download = this.fileName('wav'); a.click();
          setTimeout(() => URL.revokeObjectURL(url), 60000);
        });
    }
  }

  // Play the range `ratio` times faster: pitch and length change together, as on a sampler.
  // Playing faster is resampling to fewer samples; the sinc resampler filters out what would alias.
  respeed(ratio, label) {
    const [a, b] = this.range(), v = this.version, hadSel = !!(this.sel && this.sel[1] > this.sel[0]);
    if (b - a < 2) return;
    return runPerChannel('resample', { chs: this.channels.map(d => d.slice(a, b)), from: Math.round(ratio * 100000), to: 100000 }).then(({ channels: seg }) => {
      if (this.version !== v) return this.notice('The sound changed, apply again');
      const n = seg[0].length;
      if (!n) return;
      this.apply(() => {
        this.splice(a, b, seg, p => a + Math.round((p - a) * n / (b - a)), false);
        if (hadSel) this.sel = [a, a + n];
      }, label);
    }).catch(e => this.notice(e.message));
  }

  // Where the fades go: the fade in over the first part of the selection (or of the whole sound),
  // the fade out over its last part, each as long as its ruler says.
  rangeMs() { const [a, b] = this.range(); return Math.round((b - a) / this.sr * 1000); }
  fadeSpans() {
    const [a, b] = this.range(), len = (f) => Math.max(0, Math.min(b - a, Math.round((Number(this.$(`[data-f=${f}]`).value) || 0) / 1000 * this.sr)));
    return [{ a, b: a + len('fadeinlen'), fadeIn: true }, { a: b - len('fadeoutlen'), b, fadeIn: false }];
  }
  fade(a, b, fadeIn) {
    const n = b - a; if (n <= 0) return;
    for (const d of this.channels) for (let i = 0; i < n; i++) {
      const t = i / n;
      d[a + i] *= fadeIn ? Math.sin(t * Math.PI / 2) ** 2 : Math.cos(t * Math.PI / 2) ** 2;
    }
  }
  removeDc() {
    for (const d of this.channels) { let m = 0; for (let i = 0; i < d.length; i++) m += d[i]; m /= d.length || 1; for (let i = 0; i < d.length; i++) d[i] -= m; }
  }
  normalizeAll() {
    const target = Math.pow(10, Number(this.settings?.prep_target_db ?? this.$('[data-f=norm]').value ?? -1) / 20);
    let peak = 0; for (const d of this.channels) for (let i = 0; i < d.length; i++) peak = Math.max(peak, Math.abs(d[i]));
    if (peak) for (const d of this.channels) for (let i = 0; i < d.length; i++) d[i] *= target / peak;
  }
  threshold() { return Math.pow(10, Number(this.$('[data-f=thresh]').value || -45) / 20); }
  // The level Trim silent ends cuts below. Its ruler starts at the bottom, which cuts nothing.
  trimThreshold() { const db = Number(this.$('[data-f=trimthresh]').value); return db <= -96 ? 0 : Math.pow(10, db / 20); }
  // What Trim silent ends would keep at its current level, worked out once per sound and level.
  trimBounds() {
    const key = `${this.version}|${this.$('[data-f=trimthresh]').value}`;
    if (this.trimKept?.key !== key) this.trimKept = { key, r: this.soundBounds(this.trimThreshold()) };
    return this.trimKept.r;
  }
  // The span from the first to the last sample above the threshold, padded by 5 ms; null when all is below.
  soundBounds(th = this.threshold()) {
    const m = mixMono(this.channels);
    let a = 0, b = m.length;
    while (a < b && Math.abs(m[a]) < th) a++;
    while (b > a && Math.abs(m[b - 1]) < th) b--;
    if (a >= b) return null;
    const pad = Math.round(this.sr * 0.005);
    return [Math.max(0, a - pad), Math.min(m.length, b + pad)];
  }
  // The nearest point within 10 ms where the sum of all channels crosses zero.
  zeroCross(s) {
    const chs = this.channels, len = this.length, w = Math.round(this.sr * 0.01);
    const at = (i) => { let v = 0; for (const d of chs) v += d[i]; return v; };
    for (let k = 0; k < w; k++) {
      for (const i of [s + k, s - k]) {
        if (i > 0 && i < len && (at(i - 1) <= 0) !== (at(i) <= 0)) return i;
      }
    }
    return s;
  }
  detectRegions() {
    // Frames louder than the threshold are "sound"; gaps shorter than the
    // minimum gap are bridged so one event with a quiet tail stays whole.
    const m = mixMono(this.channels), hop = Math.round(this.sr * 0.01), th = this.threshold();
    const minGap = Number(this.$('[data-f=gap]').value || 250) / 1000 * this.sr;
    const minLen = this.sr * 0.05, pre = this.sr * 0.01, post = this.sr * 0.06;
    const out = [];
    let cur = null;
    for (let i = 0; i < m.length; i += hop) {
      let rms = 0; const e = Math.min(m.length, i + hop);
      for (let j = i; j < e; j++) rms += m[j] * m[j];
      rms = Math.sqrt(rms / (e - i || 1));
      if (rms >= th) {
        if (cur && i - cur.b <= minGap) cur.b = e;
        else { if (cur) out.push(cur); cur = { a: i, b: e }; }
      }
    }
    if (cur) out.push(cur);
    const res = out.filter(r => r.b - r.a >= minLen).map(r => ({
      a: this.zeroCross(Math.max(0, Math.round(r.a - pre))),
      b: this.zeroCross(Math.min(m.length, Math.round(r.b + post))),
    })).sort((x, y) => x.a - y.a);
    for (let i = 0; i + 1 < res.length; i++) res[i].b = Math.min(res[i].b, res[i + 1].a);
    return res.filter(r => r.b > r.a);
  }
  // Removing a region also drops the selection it set, so playback and loop return to the whole sound.
  dropSelOf(r) {
    if (!this.sel || this.sel[0] !== r.a || this.sel[1] !== r.b) return false;
    this.sel = null; return true;
  }
  renderRegions() {
    const box = this.$('[data-f=regions]'); if (!box) return;
    box.innerHTML = '';
    this.regions.forEach((r, i) => {
      const row = document.createElement('div');
      row.className = 'region';
      row.style.background = `var(${REGION_COLORS[i % REGION_COLORS.length]})`;
      row.dataset.tip = `Region ${i + 1}: starts ${fmtTime(r.a / this.sr, true)}, ${fmtTime((r.b - r.a) / this.sr, true)} long. Click to select.`;
      row.innerHTML = `<button class="rb" data-r="play" aria-label="Play region"><span class="i">play_arrow</span></button>
        <button class="rb" data-r="select"></button>
        <button class="rb" data-r="name" aria-label="Rename region" data-tip="Rename this region"><span class="i">edit</span></button>
        <button class="rb" data-r="del" aria-label="Remove region"><span class="i">close</span></button>`;
      row.querySelector('[data-r=select]').textContent = r.name || String(i + 1).padStart(2, '0');
      row.dataset.tip += ' Double-click the name to change it.';
      const rename = async () => {
        if (!this.ask) return;
        const name = await this.ask(`Name of region ${i + 1}`, r.name || '');
        if (name != null) this.editRegions(() => { r.name = name.trim(); });
      };
      row.addEventListener('dblclick', (e) => { if (e.target.closest('[data-r=select]')) rename(); });
      row.addEventListener('click', (e) => {
        const act = e.target.closest('[data-r]')?.dataset.r; if (!act) return;
        if (act === 'name') return rename();
        if (act === 'del') return this.editRegions(() => { const [gone] = this.regions.splice(i, 1); this.dropSelOf(gone); });
        this.sel = [r.a, r.b]; this.cursor = r.a;
        if (act === 'play') this.play();
        this.draw();
      });
      box.appendChild(row);
    });
  }
  // Regions are kept with the file, in seconds, whenever what is on screen is what is on disk.
  persistRegions(path = this.path) {
    if (!path || !this.regionsApi || (path === this.path && this.dirty)) return;
    this.regionsApi.set(path, this.regions.map(r => ({ start: r.a / this.sr, end: r.b / this.sr, name: r.name || '' })));
  }
  async exportRegions() {
    if (!this.regions.length) return this.notice('Add or auto-detect regions first');
    const target = await this.target('Save each region as a new file'); if (!target) return;
    const dir = target.dir;
    const suffix = this.settings?.edit_suffix ?? '_edit';
    let base = this.$('[data-f=name]').value || 'sample';
    if (suffix && base.endsWith(suffix) && base.length > suffix.length) base = base.slice(0, -suffix.length);
    // The sound and its regions as they are now: edits made while the files are written do not leak in.
    const channels = this.channels.map(d => d.slice()), regions = this.regions.map(r => ({ ...r })), opts = { format: this.format(), source: this.path, project: target.project };
    let n = 0;
    for (const [i, r] of regions.entries()) {
      const blob = await this.render(channels.map(d => d.subarray(r.a, r.b)));
      const label = (r.name || '').replace(/[^\w\- ]+/g, '_').trim() || String(i + 1).padStart(2, '0');
      if (await this.onSave(blob, dir, `${base}_${label}.wav`, opts)) n++;
    }
    this.notice(`Saved ${n} new file${n === 1 ? '' : 's'} into ${target.where}`);
  }
  async render(channels, { mono = this.settings?.export_mono, rate, bits } = {}) {
    rate ||= Number(this.$('[data-f=rate]').value) || this.sr;
    bits ||= Number(this.$('[data-f=bits]').value);
    const { wav } = await run('render', { chs: channels, sr: this.sr, rate, bits, mono: !!mono });
    return new Blob([wav], { type: 'audio/wav' });
  }

  format() { return this.$('[data-f=format]')?.value || 'wav'; }
  fileName(ext = this.format()) { return (this.$('[data-f=name]').value || 'untitled') + '.' + ext; }
  // Where a new file goes: asked every time, since a save never replaces a file.
  async target(title) {
    if (this.whereToSave) return this.whereToSave(title);
    const dir = this.dir || await this.pickFolder();
    return dir ? { dir, where: dir } : null;
  }
  async save() {
    const target = await this.target(); if (!target) return;
    // An edit made, or another file opened, while the save runs must not be marked as saved.
    const file = this.fileId, version = this.version, from = this.path && this.name;
    const res = await this.onSave(await this.render(this.channels), target.dir, this.fileName(), { format: this.format(), source: this.path, project: target.project });
    if (!res) return;
    const name = res.split('/').pop();
    this.notice(`Saved ${name} into ${target.where}.${from ? ` ${from} is unchanged.` : ''}`, 7000);
    if (this.fileId === file) {
      // The editor now shows the saved file: later region changes and saves belong to it.
      this.dir = res.slice(0, res.lastIndexOf('/')); this.path = res; this.savedVersion = version; this.savedOnce = true;
      this.name = name; this.$('[data-f=title]').textContent = this.name;
      this.infoSig = null;
      this.persistRegions(); this.updateInfo();
    }
  }

  // The whole sound as it would be after a tool that keeps its length, for drawing and for Play
  // while the tool is open. Null for tools that change the length.
  async toolFull(name) {
    const [a, b] = this.range(), sr = this.sr, val = f => Number(this.$(`[data-f=${f}]`).value);
    const out = this.channels.map(d => d.slice());
    const scale = (k) => { for (const d of out) for (let i = a; i < b; i++) d[i] *= k; return out; };
    switch (name) {
      case 'gain': return scale(Math.pow(10, val('gain') / 20));
      case 'normalize': {
        let peak = 0; this.mapRange((d, x, y) => { for (let i = x; i < y; i++) peak = Math.max(peak, Math.abs(d[i])); });
        return peak ? scale(Math.pow(10, val('norm') / 20) / peak) : null;
      }
      case 'loudnorm': {
        const { lufs } = await run('measure', { chs: this.channels.map(d => d.slice(a, b)), sr }, 1);
        return isFinite(lufs) ? scale(Math.pow(10, (val('lufs') - lufs) / 20)) : null;
      }
      case 'fade': {
        for (const m of this.fadeSpans()) {
          const n = m.b - m.a;
          for (const d of out) for (let i = 0; i < n; i++) d[m.a + i] *= m.fadeIn ? Math.sin(i / n * Math.PI / 2) ** 2 : Math.cos(i / n * Math.PI / 2) ** 2;
        }
        return out;
      }
      case 'declick': {
        const r = await run('declick', { chs: this.channels.map(d => d.slice(a, b)), sr, sensitivity: val('clicksens') || 5 });
        out.forEach((d, i) => d.set(r.channels[i], a));
        return out;
      }
      // These change the length: the result takes the place of the range, and what follows moves.
      case 'speed': case 'tune': case 'stretch': case 'loopxfade': {
        const part = () => this.channels.map(d => d.slice(a, b));
        let p = null;
        if (name === 'speed') { const pct = val('speed'); if (pct > 0) p = (await runPerChannel('resample', { chs: part(), from: Math.round(pct * 100), to: 10000 })).channels; }
        else if (name === 'tune') p = (await runPerChannel('resample', { chs: part(), from: Math.round(Math.pow(2, (val('tunesemis') || 0) / 12) * 100000), to: 100000 })).channels;
        else if (name === 'stretch') { const pct = val('stretchlen'); if (pct > 0 && b - a >= sr * 0.1) p = (await run('stretchPitch', { chs: part(), sr, length: pct / 100, semitones: val('semis') || 0 })).channels; }
        else { const n = Math.round(val('xfade') / 1000 * sr); if (b - a >= 4 && n >= 1 && n <= (b - a) / 2) p = loopCrossfade(this.channels.map(d => d.subarray(a, b)), n); }
        if (!p) return null;
        return this.channels.map((d, i) => {
          const o = new Float32Array(a + p[i].length + d.length - b);
          o.set(d.subarray(0, a)); o.set(p[i], a); o.set(d.subarray(b), a + p[i].length);
          return o;
        });
      }
    }
    return null;
  }
  // The sound a tool would produce with its current values, for listening before applying.
  // Level tools measure the whole range but return only the first seconds of it.
  async toolSegment(name) {
    const [a, b] = this.range(), sr = this.sr, val = f => Number(this.$(`[data-f=${f}]`).value);
    const end = Math.min(b, a + Math.round(sr * AUDITION_S));
    const head = () => this.channels.map(d => d.slice(a, end));
    const scaled = (k) => head().map((d) => { for (let i = 0; i < d.length; i++) d[i] *= k; return d; });
    const key = `${this.version}:${a}:${b}`;
    this.segFrom = a;
    if (this.toolCache?.key !== key) this.toolCache = { key };
    const cache = this.toolCache;
    switch (name) {
      case 'gain': return scaled(Math.pow(10, val('gain') / 20));
      case 'normalize': {
        if (cache.peak == null) { let peak = 0; this.mapRange((d, x, y) => { for (let i = x; i < y; i++) peak = Math.max(peak, Math.abs(d[i])); }); cache.peak = peak; }
        return cache.peak ? scaled(Math.pow(10, val('norm') / 20) / cache.peak) : null;
      }
      case 'loudnorm': {
        cache.lufs ??= (await run('measure', { chs: this.channels.map(d => d.slice(a, b)), sr }, 1)).lufs;
        return isFinite(cache.lufs) ? scaled(Math.pow(10, (val('lufs') - cache.lufs) / 20)) : null;
      }
      case 'speed': {
        const pct = val('speed');
        if (!(pct > 0)) return null;
        return (await runPerChannel('resample', { chs: head(), from: Math.round(pct * 100), to: 10000 })).channels;
      }
      case 'stretch': {
        const pct = val('stretchlen');
        if (!(pct > 0) || end - a < sr * 0.1) return null;
        return (await run('stretchPitch', { chs: head(), sr, length: pct / 100, semitones: val('semis') || 0 })).channels;
      }
      case 'loopxfade': {
        const n = Math.round(val('xfade') / 1000 * sr);
        if (b - a < 4 || n < 1 || n > (b - a) / 2) return null;
        return loopCrossfade(this.channels.map(d => d.subarray(a, b)), n);
      }
      case 'fade': {
        const [from, to] = this.fadeExcerpt();
        if (to <= from) return null;
        this.segFrom = from;
        const spans = this.fadeSpans();
        return this.channels.map((d) => {
          const o = d.slice(from, to);
          for (const m of spans) {
            const n = m.b - m.a;
            for (let i = Math.max(m.a, from); i < Math.min(m.b, to); i++) {
              const t = (i - m.a) / n;
              o[i - from] *= m.fadeIn ? Math.sin(t * Math.PI / 2) ** 2 : Math.cos(t * Math.PI / 2) ** 2;
            }
          }
          return o;
        });
      }
      case 'tune': {
        const ratio = Math.pow(2, (val('tunesemis') || 0) / 12);
        return (await runPerChannel('resample', { chs: head(), from: Math.round(ratio * 100000), to: 100000 })).channels;
      }
      case 'declick': return (await run('declick', { chs: head(), sr, sensitivity: val('clicksens') || 5 })).channels;
    }
    return null;
  }
  async audition(name) {
    if (this.fx?.toolName !== name) return;
    const id = this.auditionId = (this.auditionId || 0) + 1;
    let seg = null;
    try {
      if (this.fx.bypass) {
        // The untouched sound over the same stretch the tool's own audition plays, so A and B line up.
        let [a, b] = this.range(); b = Math.min(b, a + Math.round(this.sr * AUDITION_S));
        if (name === 'fade') [a, b] = this.fadeExcerpt();
        this.segFrom = a; seg = this.channels.map(d => d.slice(a, b));
      }
      else seg = await this.toolSegment(name);
    } catch (e) { this.notice(e.message); }
    if (this.fx?.toolName !== name || id !== this.auditionId) return;
    this.onAudition?.();
    this.stopAudition(); this.stopPlay();
    if (!seg || !seg[0].length) return;
    readyAudio();
    const src = ctx.createBufferSource(), gain = ctx.createGain(), t = ctx.currentTime, dur = seg[0].length / this.sr;
    src.buffer = toBuffer(seg, this.sr);
    src.connect(gain).connect(out);
    // Starting or stopping in the middle of a wave clicks, so every audition fades in and out.
    // The fade in is short enough to leave a drum's attack alone.
    gain.gain.setValueAtTime(0, t); gain.gain.linearRampToValueAtTime(1, t + AUDITION_FADE_IN_S);
    this.aud = { t, from: this.segFrom ?? 0, len: seg[0].length, off: 0, loop: false };
    if (name === 'loopxfade') {
      // Start two seconds before the end, so the join comes up quickly, then keep looping.
      src.loop = true; src.start(t, Math.max(0, dur - 2));
      this.aud.loop = true; this.aud.off = Math.round(Math.max(0, dur - 2) * this.sr);
    } else if (this.loop) {
      src.loop = true; src.start(t); this.aud.loop = true;
    } else {
      if (dur > 4 * AUDITION_FADE_S) { gain.gain.setValueAtTime(1, t + dur - AUDITION_FADE_S); gain.gain.linearRampToValueAtTime(0, t + dur); }
      src.start(t);
    }
    src.gainNode = gain;
    this.auditionSrc = src;
    src.onended = () => { if (this.auditionSrc === src) { this.auditionSrc = null; this.draw(); } };
    const tick = () => { if (this.auditionSrc !== src) return; soundPlaying(); this.draw(); requestAnimationFrame(tick); };
    requestAnimationFrame(tick);
  }
  // Where the audition is, on the waveform as shown.
  auditionPos() {
    const a = this.aud, at = a.off + Math.max(0, ctx.currentTime - a.t) * this.sr;
    return a.from + (a.loop ? at % a.len : Math.min(a.len, at));
  }
  stopAudition() {
    this.auditionId = (this.auditionId || 0) + 1;
    const s = this.auditionSrc; if (!s) return;
    this.auditionSrc = null;
    requestAnimationFrame(() => this.draw());
    const t = ctx.currentTime, g = s.gainNode.gain;
    try {
      g.cancelScheduledValues(t); g.setValueAtTime(g.value, t); g.linearRampToValueAtTime(0, t + AUDITION_FADE_S);
      s.stop(t + AUDITION_FADE_S);
    } catch { }
  }
  // ---- toolbar menus
  bindMenus() {
    const menus = [...this.root.querySelectorAll('.tmenu')];
    const parts = m => [m.querySelector('.tmenu-btn'), m.querySelector('.tmenu-list')];
    const isOpen = m => !parts(m)[1].hidden;
    const close = (m) => { const [btn, list] = parts(m); list.hidden = true; btn.setAttribute('aria-expanded', 'false'); };
    const open = (m) => {
      menus.forEach(o => o !== m && close(o));
      const [btn, list] = parts(m);
      list.hidden = false; btn.setAttribute('aria-expanded', 'true');
      // Keep the list inside the window when its menu sits near the right edge.
      list.style.left = '0';
      const over = list.getBoundingClientRect().right - (window.innerWidth - 8);
      if (over > 0) list.style.left = -over + 'px';
    };
    const step = (m, dir) => {
      const items = [...parts(m)[1].querySelectorAll('button:not([hidden])')], i = items.indexOf(document.activeElement);
      items[(i + dir + items.length) % items.length]?.focus();
    };
    for (const m of menus) {
      const [btn, list] = parts(m);
      btn.addEventListener('click', () => { if (!this.channels) return; isOpen(m) ? close(m) : open(m); });
      btn.addEventListener('mouseenter', () => { if (menus.some(isOpen) && !isOpen(m)) open(m); });
      list.addEventListener('click', (e) => { if (e.target.closest('[role=menuitem]')) close(m); });
      m.addEventListener('keydown', (e) => {
        if (e.key === 'Escape' && isOpen(m)) { e.preventDefault(); e.stopPropagation(); close(m); btn.focus(); }
        else if (e.target.matches('input, select')) return;
        else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
          e.preventDefault(); e.stopPropagation();
          if (!isOpen(m)) { if (!this.channels) return; open(m); }
          step(m, e.key === 'ArrowDown' ? 1 : -1);
        } else if ((e.key === 'ArrowRight' || e.key === 'ArrowLeft') && isOpen(m)) {
          e.preventDefault(); e.stopPropagation();
          const next = menus[(menus.indexOf(m) + (e.key === 'ArrowRight' ? 1 : -1) + menus.length) % menus.length];
          open(next); parts(next)[0].focus();
        }
      });
    }
    document.addEventListener('mousedown', (e) => { if (e.target.closest?.('.dd-menu')) return; for (const m of menus) if (isOpen(m) && !m.contains(e.target)) close(m); });
  }

  // What is played to judge a fade: the fade last adjusted, with a second of the sound beside it.
  fadeExcerpt() {
    const [fin, fout] = this.fadeSpans(), most = Math.round(this.sr * AUDITION_S), [a, b] = this.range();
    const out = this.fx?.lastField === 1 || fin.b <= fin.a;
    return out ? [Math.max(a, fout.a - this.sr, b - most), b] : [a, Math.min(b, fin.b + this.sr, a + most)];
  }
  // The inner edge of each fade can be dragged on the waveform.
  fadeEdgeAt(x) {
    if (!this.channels) return null;
    return (this.fx?.toolMark() || []).find(m => Math.abs(this.sampleToX(m.fadeIn ? m.b : m.a) - x) <= 6) || null;
  }
  dragFade(s, fadeIn) {
    const [a, b] = this.range(), f = this.$(`[data-f=${fadeIn ? 'fadeinlen' : 'fadeoutlen'}]`);
    const ms = Math.round((fadeIn ? s - a : b - s) / this.sr * 1000);
    f.value = Math.max(0, Math.min(this.rangeMs(), ms));
    this.fx.lastField = fadeIn ? 0 : 1;
    this.fx.toolSync?.();
  }

  // ---- input
  bind() {
    this.root.addEventListener('change', (e) => { if (this.channels && e.target.closest('[data-f=rate], [data-f=bits]')) this.updateInfo(); });
    this.root.addEventListener('click', (e) => {
      const b = e.target.closest('[data-a]'); if (!b) return;
      if (TOOLS[b.dataset.a] && this.channels) this.fx.openTool(b.dataset.a); else this.action(b.dataset.a);
    });
    this.bindMenus();
    let dragFrom = null, moved = false, fadeDrag = null;
    this.canvas.addEventListener('mousedown', (e) => {
      if (!this.channels) return;
      const x = e.offsetX;
      const edge = this.fadeEdgeAt(x);
      if (edge) { fadeDrag = edge; return; }
      dragFrom = this.snap(this.xToSample(x)); moved = false;
      if (e.shiftKey && this.sel) { this.sel = [Math.min(this.sel[0], dragFrom), Math.max(this.sel[1], dragFrom)]; dragFrom = null; this.draw(); }
    });
    window.addEventListener('mousemove', (e) => {
      if (fadeDrag) return this.dragFade(this.xToSample(e.clientX - this.canvas.getBoundingClientRect().left), fadeDrag.fadeIn);
      if (dragFrom == null) return;
      const r = this.canvas.getBoundingClientRect();
      const s = this.snap(this.xToSample(e.clientX - r.left));
      if (Math.abs(s - dragFrom) / this.spp > 2) moved = true;
      if (moved) { this.sel = [Math.min(dragFrom, s), Math.max(dragFrom, s)]; this.draw(); }
    });
    window.addEventListener('mouseup', () => {
      if (fadeDrag) { fadeDrag = null; return; }
      if (dragFrom == null) return;
      if (!moved) { this.sel = null; this.cursor = dragFrom; if (this.playing) this.play(); }
      else this.cursor = this.sel[0];
      dragFrom = null; this.draw();
    });
    this.canvas.addEventListener('dblclick', () => { this.sel = [0, this.length]; this.draw(); });
    this.canvas.addEventListener('mousemove', (e) => { this.canvas.style.cursor = this.fadeEdgeAt(e.offsetX) ? 'ew-resize' : ''; if (this.mode !== 'spec' || !this.channels) return; this.hover = { x: e.offsetX, y: e.offsetY }; this.draw(); });
    this.canvas.addEventListener('mouseleave', () => { if (this.hover) { this.hover = null; this.draw(); } });
    this.canvas.addEventListener('wheel', (e) => {
      if (!this.channels) return;
      e.preventDefault();
      if (e.ctrlKey || e.metaKey) this.zoom(Math.exp(e.deltaY * 0.01), e.offsetX);
      else { this.start += (Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY) * this.spp; this.clampView(); this.draw(); }
    }, { passive: false });
  }

  handleKey(e) {
    if (this.root.hidden || !this.channels) return false;
    if (this.root.querySelector('.tmenu-list:not([hidden])')) return false;
    const mod = e.metaKey || e.ctrlKey;
    const map = {
      ' ': 'play', 'z': mod && (e.shiftKey ? 'redo' : 'undo'), 'a': mod && 'all', 'c': mod && 'copy',
      'x': mod && 'cut', 'v': mod && 'paste', 't': mod && 'trim', 'Backspace': 'delete', 'Delete': 'delete',
      '=': 'zoomin', '+': 'zoomin', '-': 'zoomout', 'f': !mod && 'fit', 'e': !mod && 'zoomsel', 'l': !mod && 'loop', 'b': !mod && 'ab',
      'r': !mod && 'addregion', 's': mod && (e.shiftKey ? 'saveto' : 'save'),
      'Home': 'home', 'End': 'end', 'Escape': this.sel && 'deselect',
    };
    const act = map[e.key.length === 1 ? e.key.toLowerCase() : e.key];
    if (!act) return false;
    e.preventDefault(); this.action(act); return true;
  }
}
