import { lib } from './native.js';
import { fmtTime } from './time.js';
import { invoke, now, onEnded, forget } from './engine.js';
import { TOOLS } from './tools.js';

const REGION_COLORS = ['--c1', '--c2', '--c3', '--c4', '--c5'];

const UNDO_BYTES = 1.5 * 2 ** 30;
const SPEC_BG = '#12101e';
const SPEC_TILE = 256, SPEC_TILES_KEPT = 160;
const NOTE_NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
/** @param {number} f */
function noteOf(f) { const m = Math.round(69 + 12 * Math.log2(f / 440)); return NOTE_NAMES[((m % 12) + 12) % 12] + (Math.floor(m / 12) - 1); }

const AUDITION_S = 8, AUDITION_FADE_S = 0.02, AUDITION_FADE_IN_S = 0.002;

/** @typedef {[number, number]} Span */
/** @typedef {{ a: number, b: number, name: string }} Region */
/** @typedef {{ a: number, b: number, fadeIn: boolean }} FadeSpan */
/**
 * @typedef {{ length: number, channels: number, sr: number, version: number, dirty: boolean, path?: string | null,
 *   undo: number, redo: number, regions: Region[], sel: Span | null, cursor: number }} EdState
 */
/** @typedef {{ lufs: number, peak: number, truePeak: number }} Meter */
/** @typedef {{ canvas: HTMLCanvasElement, k: number, zoom: string, spd: number }} SpecTile */
/** @typedef {{ dataKey: string, src: string, zoom: string, spd: number, rows: number, missing: number[] }} SpecWant */
/** @typedef {{ t: number, from: number, len: number, off: number, loop: boolean }} Audition */
/** @typedef {{ a: number, b: number, rate: number, bits: number, mono: boolean }} RenderOpts */
/** @typedef {{ format?: string, source?: string | null, project?: number, sp404?: boolean }} SaveOpts */
/** @typedef {{ dir: string, where: string, project?: number }} SaveTarget */
/** @typedef {(msg: string, ms?: number) => void} Notify */
/**
 * @typedef {{
 *   onSave: (p: RenderOpts, dir: string, name: string, opts?: SaveOpts) => Promise<string | null>,
 *   toast: Notify, notice?: Notify,
 *   pickFolder: (title?: string, start?: string) => Promise<string | null>,
 *   ask?: (title: string, value: string) => Promise<string | undefined>,
 *   whereToSave?: (title?: string) => Promise<SaveTarget | null>,
 *   place?: () => string,
 * }} EditorHooks
 */

export class Editor {
  /** @param {HTMLElement} root @param {EditorHooks} hooks */
  constructor(root, { onSave, toast, notice, pickFolder, ask, whereToSave, place }) {
    this.root = root;
    this.onSave = onSave; this.whereToSave = whereToSave; this.place = place;
    this.ask = ask;
    /** The effects panel, set once it is made. @type {import('./fx.js').FxPanel} */
    this.fx = /** @type {import('./fx.js').FxPanel} */ (/** @type {unknown} */ (null));
    /** The app's settings, as last loaded. @type {Record<string, any> | null} */
    this.settings = null;
    /** Told when an audition starts, and when the sound becomes edited or saved. @type {(() => void) | null} */
    this.onAudition = null;
    /** @type {((dirty: boolean) => void) | null} */
    this.onDirtyChange = null;
    /** The painted background and waveform, copied in on every frame. @type {HTMLCanvasElement | null} */
    this.layer = null;
    this.toast = toast; this.notice = notice || toast;
    this.pickFolder = pickFolder;
    this.canvas = /** @type {HTMLCanvasElement} */ (root.querySelector('canvas'));
    this.g = /** @type {CanvasRenderingContext2D} */ (this.canvas.getContext('2d'));
    this.mode = 'wave';
    this.version = 0;
    this.undoN = 0; this.redoN = 0;
    this.clip = null;
    this.loop = false;
    /** @type {boolean | undefined} */ this.loaded;
    /** @type {string} */ this.name;
    /** @type {string | null | undefined} */ this.dir;
    /** @type {string | null} */ this.path;
    /** @type {number} */ this.len;
    /** @type {number} */ this.nch;
    /** @type {number} */ this.sr;
    /** @type {boolean} */ this.isDirty;
    /** @type {Region[]} */ this.regions;
    /** @type {Span | null} */ this.sel;
    /** @type {number} */ this.cursor;
    /** @type {number} */ this.start;
    /** @type {number} */ this.spp;
    /** @type {number} */ this.shownLen;
    /** @type {number | undefined} */ this.fileId;
    /** @type {{ x: number, y: number } | null} */ this.hover;
    /** @type {boolean | undefined} */ this.bars;
    /** @type {string | null | undefined} */ this.layerKey;
    /** @type {string | null | undefined} */ this.peaksKey;
    /** @type {{ dataKey: string, map: Map<string, SpecTile> } | undefined} */ this.tiles;
    /** @type {SpecWant | null | undefined} */ this.specWant;
    /** @type {string | undefined} */ this.specFailed;
    /** @type {boolean | undefined} */ this.specBusy;
    /** @type {string | null | undefined} */ this.infoSig;
    /** @type {Meter} */ this.meter;
    /** @type {number | undefined} */ this.meterVersion;
    /** @type {ReturnType<typeof setTimeout> | null | undefined} */ this.meterTimer;
    /** @type {boolean | undefined} */ this.wasDirty;
    /** @type {boolean | undefined} */ this.savedOnce;
    /** @type {boolean | undefined} */ this.playing;
    /** @type {object | null | undefined} */ this.playToken;
    /** @type {number | null | undefined} */ this.voice;
    /** @type {boolean} */ this.playLoop;
    /** @type {number} */ this.playA;
    /** @type {number} */ this.playStart;
    /** @type {number} */ this.playB;
    /** @type {number} */ this.playT;
    /** @type {boolean | undefined} */ this.changing;
    /** @type {boolean | undefined} */ this.hasClip;
    /** @type {{ key: string, r: Span | null } | undefined} */ this.trimKept;
    /** @type {number | undefined} */ this.auditionId;
    /** @type {object | null | undefined} */ this.auditionToken;
    /** @type {boolean | undefined} */ this.auditioning;
    /** @type {Audition} */ this.aud;
    /** @type {number | null | undefined} */ this.auditionVoice;
    this.bind();
    new ResizeObserver(() => this.draw()).observe(this.canvas);
  }

  /** @param {string} sel @returns {any} */
  $(sel) { return this.root.querySelector(sel); }

  // The sound, its history and its regions are held by the native side; the page knows their
  // shape and draws them. `source` is { path } for a library file or { key } for a recorded take.
  /** @param {{ source: { path: string } | { key: string }, name: string, dir?: string | null, path?: string | null, tempo?: number | string | null }} o */
  async load({ source, name, dir, path = null, tempo = null }) {
    this.stopPlay(); this.stopAudition(); this.shownLen = 0;
    const info = await invoke('ed_open', source);
    this.loaded = true; this.name = name; this.dir = dir;
    this.adopt(info);
    this.fileId = (this.fileId || 0) + 1;
    this.$('[data-f=bpm]').value = tempo || 120;
    this.hover = null;
    this.root.classList.remove('blank');
    this.fx?.fileChanged();
    this.$('[data-f=name]').value = name.replace(/\.[^.]+$/, '') + (this.settings?.edit_suffix ?? '_edit');
    this.$('[data-f=title]').textContent = name;
    this.fit();
  }

  // The state after any change on the native side: the sound's shape, the history, the regions,
  // and where the selection and cursor went.
  /** @param {EdState} info */
  adopt(info) {
    this.len = info.length; this.nch = info.channels; this.sr = info.sr;
    this.version = info.version; this.isDirty = info.dirty; this.path = info.path ?? null;
    this.undoN = info.undo; this.redoN = info.redo;
    this.regions = info.regions; this.sel = info.sel; this.cursor = info.cursor;
    this.renderRegions();
  }
  ui() { return { sel: this.sel && this.sel[1] > this.sel[0] ? [...this.sel] : null, cursor: Math.round(this.cursor) }; }

  get length() { return this.loaded ? this.len : 0; }
  get dirty() { return !!this.loaded && this.isDirty; }

  // ---- view
  // While a tool that changes the length is open, the waveform shows its result, which is longer
  // or shorter than the sound itself. Zoom and scroll follow what is shown.
  viewLen() { return this.shownLen || this.length; }
  fit() { this.start = 0; this.spp = Math.max(1 / 16, this.viewLen() / Math.max(1, this.width())); this.draw(); }
  width() { return this.canvas.clientWidth || 800; }
  /** @param {number} x */
  xToSample(x) { return Math.round(Math.max(0, Math.min(this.length, this.start + x * this.spp))); }
  /** @param {number} s */
  sampleToX(s) { return (s - this.start) / this.spp; }
  /** @param {number} f */
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
  /** @param {number} s */
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
    if (!this.loaded) return this.drawEmpty();
    const dpr = window.devicePixelRatio || 1;
    const W = this.canvas.clientWidth, H = this.canvas.clientHeight;
    if (!W || !H) return;
    if (this.canvas.width !== W * dpr || this.canvas.height !== H * dpr) {
      this.canvas.width = W * dpr; this.canvas.height = H * dpr;
    }
    const g = this.g;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    // A view that showed the whole sound keeps showing the whole of whatever replaces it.
    const shown = this.viewSrc().length;
    if (shown !== this.shownLen) {
      const whole = this.start === 0 && this.spp * W >= (this.shownLen || this.length) * 0.999;
      this.shownLen = shown;
      if (whole) { this.start = 0; this.spp = Math.max(1 / 16, shown / W); } else this.clampView();
    }
    const css = getComputedStyle(document.documentElement);
    const col = (/** @type {string} */ n) => css.getPropertyValue(n).trim();
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
      const level = (/** @type {number} */ s) => (fin.b > fin.a && s < fin.b ? Math.sin((s - fin.a) / (fin.b - fin.a) * Math.PI / 2) ** 2 : 1)
        * (fout.b > fout.a && s > fout.a ? Math.cos((s - fout.a) / (fout.b - fout.a) * Math.PI / 2) ** 2 : 1);
      const x0 = Math.max(0, xa), x1 = Math.min(W, xb), yOf = (/** @type {number} */ x) => bottom - level(Math.min(fout.b, Math.max(fin.a, this.start + x * this.spp))) * (bottom - top);
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
    const px = this.playing ? this.sampleToX(this.playPos()) : this.auditioning ? this.sampleToX(this.auditionPos()) : null;
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
  // The waveform's points come from the native side for exactly the pixels shown; until they
  // arrive the last layer stays on screen.
  /** @param {(n: string) => string} col @param {number} RULER @param {number} W @param {number} H @param {number} dpr @param {number} tick */
  drawWave(col, RULER, W, H, dpr, tick) {
    const view = this.viewSrc();
    const paint = [col('--wave-bg'), col('--line'), col('--wave'), col('--muted')];
    // Behind the result of an open effect or tool, the sound as it is now stays visible, fainter.
    const ghost = view.src !== 'doc' && view.length === this.length;
    const beat = this.beatLen();
    const key = [this.start, this.spp, W, H, dpr, this.version, view.src === 'doc' ? '' : view.src, tick, beat, ...paint].join('|');
    if (this.layerKey !== key && this.peaksKey !== key) {
      this.peaksKey = key;
      const want = ghost ? ['doc', view.src] : [view.src];
      Promise.all(want.map(src => invoke('ed_peaks', { src, start: this.start, spp: this.spp, width: Math.ceil(W) })))
        .then((bufs) => {
          if (this.peaksKey !== key) return;
          this.paintLayer(key, bufs.map(b => new Float32Array(b)), ghost, paint, RULER, W, H, dpr, tick, beat);
          this.draw();
        })
        .catch(() => { if (this.peaksKey === key) this.peaksKey = null; });
    }
    if (!this.layer) return;
    this.g.save(); this.g.setTransform(1, 0, 0, 1, 0, 0);
    this.g.drawImage(this.layer, 0, 0);
    this.g.restore();
  }

  /**
   * @param {string} key @param {Float32Array[]} sets @param {boolean} ghost @param {string[]} paint
   * @param {number} RULER @param {number} W @param {number} H @param {number} dpr @param {number} tick @param {number} beat
   */
  paintLayer(key, sets, ghost, paint, RULER, W, H, dpr, tick, beat) {
    const layer = this.layer ||= document.createElement('canvas');
    layer.width = this.canvas.width; layer.height = this.canvas.height;
    const g = /** @type {CanvasRenderingContext2D} */ (layer.getContext('2d'));
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
    const lanes = this.nch, laneH = (H - RULER) / lanes, width = Math.ceil(W);
    /** @type {[Float32Array, string, number, boolean][]} */
    const layers = ghost ? [[sets[0], paint[3], 0.45, false], [sets[1], paint[2], 1, true]] : [[sets[0], paint[2], 1, true]];
    for (const [data, ink, alpha, main] of layers) for (let c = 0; c < lanes; c++) {
      const y0 = RULER + c * laneH, mid = y0 + laneH / 2, amp = laneH / 2 - 3;
      g.globalAlpha = 1;
      if (main) { g.strokeStyle = paint[1]; g.beginPath(); g.moveTo(0, mid + .5); g.lineTo(W, mid + .5); g.stroke(); }
      g.globalAlpha = alpha;
      g.fillStyle = ink;
      if (this.spp <= 1) {
        // The samples themselves, from the first whole sample at or before the left edge.
        const per = data.length / lanes, d = data.subarray(c * per, (c + 1) * per), i0 = Math.floor(Math.max(0, this.start));
        g.strokeStyle = ink; g.beginPath();
        for (let x = 0; x <= W; x++) {
          const i = Math.floor(this.start + x * this.spp) - i0;
          if (i >= d.length) break;
          const y = mid - d[i] * amp;
          x ? g.lineTo(x, y) : g.moveTo(x, y);
        }
        g.stroke();
        if (this.spp < 0.12) {
          for (let i = Math.ceil(this.start); i - i0 < d.length && i < this.start + W * this.spp; i++)
            g.fillRect(this.sampleToX(i) - 1.5, mid - d[i - i0] * amp - 1.5, 3, 3);
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
    this.layerKey = key;
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
    const col = (/** @type {string} */ n) => css.getPropertyValue(n).trim();
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

  // The picture is made of tiles SPEC_TILE device pixels wide, computed natively and kept per
  // zoom level, so scrolling reuses them. While a zoom level is still filling in, tiles from other
  // zoom levels are drawn stretched underneath.
  /** @param {number} top @param {number} W @param {number} H */
  drawSpec(top, W, H) {
    const view = this.viewSrc(), dpr = window.devicePixelRatio || 1;
    const PW = Math.round(W * dpr), rows = Math.round(H * dpr), spd = this.spp / dpr;
    const dataKey = `${this.version}|${view.src}`;
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
    this.specWant = missing.length && this.specFailed !== dataKey ? { dataKey, src: view.src, zoom, spd, rows, missing } : null;
    if (this.specWant) this.specLoop();
    // Frequency axis, plus a readout of frequency and note under the pointer.
    const g = this.g, fmin = 20, fmax = this.sr / 2;
    const yOf = (/** @type {number} */ f) => top + H * (1 - Math.log(f / fmin) / Math.log(fmax / fmin));
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
        const k = /** @type {number} */ (w.missing.shift()), key = `${w.zoom}|${k}`, tiles = /** @type {{ dataKey: string, map: Map<string, SpecTile> }} */ (this.tiles);
        if (tiles.dataKey !== w.dataKey || tiles.map.has(key)) continue;
        const pixels = new Uint8ClampedArray(await invoke('ed_spec', { src: w.src, start: k * SPEC_TILE * w.spd, spp: w.spd, pw: SPEC_TILE, rows: w.rows }));
        const canvas = document.createElement('canvas');
        canvas.width = SPEC_TILE; canvas.height = w.rows;
        /** @type {CanvasRenderingContext2D} */ (canvas.getContext('2d')).putImageData(new ImageData(pixels, SPEC_TILE, w.rows), 0, 0);
        tiles.map.set(key, { canvas, k, zoom: w.zoom, spd: w.spd });
        // Oldest tiles go first; a Map keeps insertion order.
        while (tiles.map.size > SPEC_TILES_KEPT) tiles.map.delete(/** @type {string} */ (tiles.map.keys().next().value));
        this.draw();
      }
    } catch (e) { this.specFailed = this.specWant?.dataKey; this.specWant = null; this.notice(`Spectrogram failed: ${/** @type {Error} */ (e).message || e}`); }
    finally { this.specBusy = false; }
  }

  // What the waveform shows: the sound, or the result of the open effect or tool ({ src, length }).
  viewSrc() { return this.fx?.displayView() || { src: 'doc', length: this.length }; }

  updateInfo() {
    const sec = (/** @type {number} */ s) => fmtTime(s / this.sr, true);
    const nch = this.nch;
    const outRate = Number(this.$('[data-f=rate]')?.value) || this.sr;
    const bits = this.$('[data-f=bits]')?.value;
    const sig = [this.length, this.sr, nch, outRate, bits, this.version, this.savedOnce,
      this.sel?.join(), this.cursor, this.undoN, this.redoN, this.isDirty, this.meterVersion].join('|');
    if (sig === this.infoSig) return;
    this.infoSig = sig;
    this.fx?.selectionChanged();
    this.$('[data-a=mono]').hidden = nch === 1;
    const khz = (/** @type {number} */ r) => `${+(r / 1000).toFixed(2)} kHz`;
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
        if (!this.loaded) return;
        const v = this.version;
        try { const m = await invoke('ed_analyse', { p: { kind: 'measure' } }); if (v === this.version) { this.meter = m; this.meterVersion = v; } }
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
    const changed = this.isDirty;
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
    const pos = this.$('[data-f=pos]'), b = (/** @type {string} */ t) => Object.assign(document.createElement('b'), { textContent: t });
    if (this.sel && this.sel[1] > this.sel[0]) pos.replaceChildren('Selection ', b(`${sec(this.sel[0])} – ${sec(this.sel[1])}`), ` · ${sec(this.sel[1] - this.sel[0])}`);
    else pos.replaceChildren();
    this.$('[data-a=undo]').disabled = !this.undoN;
    this.$('[data-a=redo]').disabled = !this.redoN;
  }

  // ---- playback
  // fromStart plays from the start of the selection, or of the sound, whatever the cursor says.
  async play(resume = false, fromStart = false) {
    const pos = resume && this.playing ? this.playPos() : null;
    this.stopAudition(false);
    this.stopPlay(false);
    const hasSel = this.sel && this.sel[1] > this.sel[0];
    const [a, b] = hasSel ? /** @type {Span} */ (this.sel) : [this.loop || fromStart ? 0 : this.cursor, this.length];
    if (b - a < 1) return;
    const start = pos != null && pos >= a && pos < b ? Math.floor(pos) : (!hasSel && this.loop && !fromStart && this.cursor < b ? this.cursor : a);
    const pv = this.fx?.playSrc();
    let src = 'doc', base = 0, end = b;
    if (pv && start >= pv.a && start < pv.b) { src = pv.src; base = pv.a; end = Math.min(b, pv.b); }
    const loop = this.loop, token = this.playToken = {};
    this.playLoop = loop; this.playA = a; this.playStart = start; this.playB = end; this.playT = now() + 0.03; this.playing = true;
    this.syncPlayBtn();
    const eq = this.fx?.liveEq() || false;
    let r;
    try { r = await invoke('ed_play', { p: { src, a: Math.max(0, a - base), b: end - base, start: start - base, loop, eq } }); }
    catch (e) { if (this.playToken === token) { this.playing = false; this.syncPlayBtn(); this.draw(); } return; }
    if (this.playToken !== token) return void invoke('ed_stop', { id: r.id }).catch(() => { });
    this.voice = r.id; this.playT = r.at;
    onEnded(r.id, () => { if (this.voice === r.id) { this.voice = null; this.playing = false; this.draw(); this.syncPlayBtn(); } });
    const tick = () => { if (!this.playing || this.playToken !== token) return; this.draw(); requestAnimationFrame(tick); };
    requestAnimationFrame(tick);
  }
  /** @returns {number} */
  playPos() {
    let p = this.playStart + Math.max(0, now() - this.playT) * this.sr;
    if (this.playLoop && p >= this.playB) p = this.playA + (p - this.playA) % (this.playB - this.playA);
    return Math.min(this.playB, p);
  }
  // send: false when another sound is about to take over, which stops this one by itself.
  stopPlay(send = true) {
    this.playToken = null;
    if (this.voice != null) { forget(this.voice); if (send) invoke('ed_stop', { id: this.voice }).catch(() => { }); this.voice = null; }
    this.playing = false; this.syncPlayBtn();
  }
  toggle() {
    if (this.auditioning) { this.stopAudition(); return; }
    if (!this.playing && this.fx?.showsOtherLength()) return void this.audition(this.fx.toolName);
    this.playing ? this.stopPlay() : this.play(); this.draw();
  }
  syncPlayBtn() { const b = this.$('[data-a=play] .i'); if (b) b.textContent = this.playing ? 'stop' : 'play_arrow'; }

  // ---- editing
  // Every change is made natively, history included; the page sends its selection and cursor
  // along and shows what comes back.
  refit() { if (this.spp * this.width() > this.viewLen()) this.fit(); else { this.clampView(); this.draw(); } }
  /** @param {string} cmd @param {Record<string, unknown>} args */
  async change(cmd, args) {
    if (!this.loaded || this.changing) return null;
    this.changing = true;
    try {
      const res = await invoke(cmd, { ...args, ui: this.ui() });
      this.adopt(res);
      this.refit();
      return res;
    } catch (e) { this.notice(String(/** @type {Error} */ (e)?.message || e)); return null; }
    finally { this.changing = false; }
  }
  /** @param {Record<string, unknown>} p */
  edit(p) {
    this.stopPlay();
    return this.change('ed_edit', { p: { smooth_ms: this.settings?.smooth_edits === false ? 0 : (this.settings?.edit_fade_ms ?? 3), ...p } });
  }
  /** @param {Record<string, unknown>} p */
  async editRegions(p) {
    const hadSel = !!this.sel;
    const r = await this.change('ed_regions', { p });
    if (r && hadSel && !this.sel && this.playing) this.play(true);
    return r;
  }
  doUndo() { this.stopPlay(); return this.change('ed_undo', { redo: false }); }
  doRedo() { this.stopPlay(); return this.change('ed_undo', { redo: true }); }

  /** @param {string} name */
  async action(name) {
    const sel = this.sel && this.sel[1] > this.sel[0];
    const needSel = () => { if (!sel) { this.notice('Select a region first'); return false; } return true; };
    const whole = () => sel && /** @type {Span} */ (this.sel)[0] === 0 && /** @type {Span} */ (this.sel)[1] === this.length;
    const val = (/** @type {string} */ f) => Number(this.$(`[data-f=${f}]`).value);
    const [a, b] = this.range();
    switch (name) {
      case 'play': return this.toggle();
      case 'loop': this.loop = !this.loop; this.$('[data-a=loop]').classList.toggle('on', this.loop); if (this.playing) this.play(true); else if (this.auditioning) this.audition(this.fx.toolName); return;
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
      case 'trim': if (!needSel()) return;
        return this.edit({ kind: 'trim', a, b });
      case 'delete': if (!needSel()) return;
        if (whole()) return this.notice('Cannot delete the whole sound');
        return this.edit({ kind: 'delete', a, b });
      case 'copy': if (!needSel()) return;
        await invoke('ed_edit', { p: { kind: 'copy', a, b } }); this.hasClip = true; return this.toast('Copied');
      case 'cut': if (!needSel()) return;
        if (whole()) return this.notice('Cannot cut the whole sound');
        if (await this.edit({ kind: 'cut', a, b })) this.hasClip = true;
        return;
      case 'paste': {
        if (!this.hasClip) return this.notice('Nothing copied');
        const [x, y] = sel ? /** @type {Span} */ (this.sel) : [this.cursor, this.cursor];
        return this.edit({ kind: 'paste', a: x, b: y });
      }
      case 'silence': if (!needSel()) return; return this.edit({ kind: 'silence', a, b });
      case 'fade': {
        const spans = this.fadeSpans().filter(m => m.b > m.a);
        if (!spans.length) return this.notice('Set a fade in or a fade out length first');
        return this.edit({ kind: 'fade', spans });
      }
      case 'normalize': return this.edit({ kind: 'normalize', a, b, db: val('norm') || -1 });
      case 'gain': return this.edit({ kind: 'gain', a, b, db: val('gain') || 0 });
      case 'reverse': return this.edit({ kind: 'reverse', a, b });
      case 'mono': if (this.nch < 2) return; return this.edit({ kind: 'mono' });
      case 'speed': {
        const pct = val('speed') || 100;
        if (pct <= 0 || pct === 100) return;
        return this.respeed(pct / 100);
      }
      case 'tune': {
        const semis = val('tunesemis') || 0;
        if (!semis) return;
        return this.respeed(Math.pow(2, semis / 12));
      }
      case 'loudnorm': {
        const target = val('lufs') || -16;
        const r = await this.edit({ kind: 'loudnorm', a, b, target });
        if (r && r.over > 0) this.notice(`At ${target} LUFS this sound peaks ${r.over.toFixed(1)} dB over full scale. Use the Maximizer, or a lower target.`, 8000);
        return;
      }
      case 'stretch': {
        const pct = val('stretchlen') || 100, semis = val('semis') || 0;
        if (!(pct > 0) || (pct === 100 && !semis)) return;
        if (b - a < this.sr * 0.1) return this.notice('Select at least a tenth of a second');
        return this.edit({ kind: 'stretch', a, b, length: pct / 100, semitones: semis });
      }
      case 'loopxfade': {
        const n = Math.round((val('xfade') || 50) / 1000 * this.sr);
        if (b - a < 4 || n < 1) return;
        if (n > (b - a) / 2) return this.notice('The crossfade can be at most half of the loop');
        const r = await this.edit({ kind: 'loopxfade', a, b, n });
        if (r && !this.loop) this.action('loop');
        return;
      }
      case 'declick': {
        const r = await this.edit({ kind: 'declick', a, b, sensitivity: val('clicksens') || 5 });
        if (r?.info) this.notice(r.info[0].toUpperCase() + r.info.slice(1));
        return;
      }
      case 'transients': return this.editRegions({ op: 'hits', a, b, sensitivity: val('onsetsens') || 5, th: this.threshold() });
      case 'gridslice': {
        const bpm = val('bpm'), per = val('perbeat') || 1;
        return this.editRegions({ op: 'grid', a, b, step: bpm > 0 ? this.sr * 60 / bpm / per : 0 });
      }
      case 'snap': {
        if (!this.loaded) return;
        if (sel) {
          const s = (await this.zeroCross(/** @type {Span} */ (this.sel))).sort((x, y) => x - y);
          if (s[1] > s[0]) this.sel = /** @type {Span} */ (s);
          this.cursor = /** @type {Span} */ (this.sel)[0];
        } else [this.cursor] = await this.zeroCross([this.cursor]);
        if (this.playing) this.play();
        return this.draw();
      }
      case 'trimsilence': return this.edit({ kind: 'trimsilence', th: this.trimThreshold() });
      case 'prep': return this.edit({
        kind: 'prep', th: this.threshold(),
        db: Number(this.settings?.prep_target_db ?? (val('norm') || -1)),
        fade_in_ms: this.settings?.prep_fade_in_ms ?? 2, fade_out_ms: this.settings?.prep_fade_out_ms ?? 10,
      });
      case 'addregion': if (!needSel()) return;
        return this.editRegions({ op: 'add', a: /** @type {Span} */ (this.sel)[0], b: /** @type {Span} */ (this.sel)[1] });
      case 'autoslice': return this.editRegions({ op: 'auto', th: this.threshold(), gap_ms: val('gap') || 250 });
      case 'clearregions': if (!this.regions.length) return;
        return this.editRegions({ op: 'clear' });
      case 'exportregions': return this.exportRegions();
      case 'sp404':
        return lib.sp404Card().catch(() => ({}))
          .then(card => this.pickFolder('Choose the SD card (or any folder)', card.import || '')).then(async (dir) => {
          if (!dir) return;
          const res = await this.onSave(this.renderOpts({ mono: this.settings?.sp404_mono || this.settings?.export_mono, rate: 48000, bits: 16 }), dir, this.fileName(), { sp404: true });
          if (res) this.toast(`Saved for SP-404MKII: ${res}`, 5000);
        });
      case 'save': case 'saveto': return this.save();
      case 'download':
        return invoke('ed_download', { p: this.renderOpts(), name: this.fileName('wav') })
          .then((path) => { if (path) this.toast(`Saved ${path.split('/').pop()}`); }, (e) => this.notice(String(e?.message || e)));
    }
  }

  // Play the range `ratio` times faster: pitch and length change together, as on a sampler.
  /** @param {number} ratio */
  respeed(ratio) {
    const [a, b] = this.range();
    if (b - a < 2) return;
    return this.edit({ kind: 'respeed', a, b, ratio });
  }

  // Where the fades go: the fade in over the first part of the selection (or of the whole sound),
  // the fade out over its last part, each as long as its ruler says.
  rangeMs() { const [a, b] = this.range(); return Math.round((b - a) / this.sr * 1000); }
  fadeSpans() {
    const [a, b] = this.range(), len = (/** @type {string} */ f) => Math.max(0, Math.min(b - a, Math.round((Number(this.$(`[data-f=${f}]`).value) || 0) / 1000 * this.sr)));
    return [{ a, b: a + len('fadeinlen'), fadeIn: true }, { a: b - len('fadeoutlen'), b, fadeIn: false }];
  }
  threshold() { return Math.pow(10, Number(this.$('[data-f=thresh]').value || -45) / 20); }
  // The level Trim silent ends cuts below. Its ruler starts at the bottom, which cuts nothing.
  trimThreshold() { const db = Number(this.$('[data-f=trimthresh]').value); return db <= -96 ? 0 : Math.pow(10, db / 20); }
  // What Trim silent ends would keep at its current level, worked out once per sound and level.
  // Null until it is known; the waveform redraws when it arrives.
  /** @returns {Span | null} */
  trimBounds() {
    const key = `${this.version}|${this.$('[data-f=trimthresh]').value}`;
    if (this.trimKept?.key !== key) {
      this.trimKept = { key, r: null };
      this.soundBounds(this.trimThreshold()).then((r) => { const kept = /** @type {{ key: string, r: Span | null }} */ (this.trimKept); if (kept.key === key) { kept.r = r; this.draw(); } }).catch(() => { });
    }
    return this.trimKept.r;
  }
  // The span from the first to the last sample above the threshold, padded by 5 ms; null when all is below.
  /** @returns {Promise<Span | null>} */
  soundBounds(th = this.threshold()) { return invoke('ed_analyse', { p: { kind: 'bounds', th } }); }
  // For each position, the nearest point within 10 ms where the sum of all channels crosses zero.
  /** @param {number[]} at @returns {Promise<number[]>} */
  zeroCross(at) { return invoke('ed_analyse', { p: { kind: 'zerocross', at } }); }
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
      /** @type {Element} */ (row.querySelector('[data-r=select]')).textContent = r.name || String(i + 1).padStart(2, '0');
      row.dataset.tip += ' Double-click the name to change it.';
      const rename = async () => {
        if (!this.ask) return;
        const name = await this.ask(`Name of region ${i + 1}`, r.name || '');
        if (name != null) this.editRegions({ op: 'rename', i, name });
      };
      row.addEventListener('dblclick', (e) => { if (/** @type {Element} */ (e.target).closest('[data-r=select]')) rename(); });
      row.addEventListener('click', (e) => {
        const act = /** @type {Element} */ (e.target).closest('[data-r]')?.dataset.r; if (!act) return;
        if (act === 'name') return rename();
        if (act === 'del') return this.editRegions({ op: 'remove', i });
        this.sel = [r.a, r.b]; this.cursor = r.a;
        if (act === 'play') this.play();
        this.draw();
      });
      box.appendChild(row);
    });
  }
  async exportRegions() {
    if (!this.regions.length) return this.notice('Add or auto-detect regions first');
    const target = await this.target('Save each region as a new file'); if (!target) return;
    const dir = target.dir;
    const suffix = this.settings?.edit_suffix ?? '_edit';
    let base = this.$('[data-f=name]').value || 'sample';
    if (suffix && base.endsWith(suffix) && base.length > suffix.length) base = base.slice(0, -suffix.length);
    // No edit is taken while the files are written, so every file comes from the same sound.
    const regions = this.regions.map(r => ({ ...r })), opts = { format: this.format(), source: this.path, project: target.project };
    let n = 0;
    this.changing = true;
    try {
      for (const [i, r] of regions.entries()) {
        const label = (r.name || '').replace(/[^\w\- ]+/g, '_').trim() || String(i + 1).padStart(2, '0');
        if (await this.onSave(this.renderOpts({ a: r.a, b: r.b }), dir, `${base}_${label}.wav`, opts)) n++;
      }
    } finally { this.changing = false; }
    this.notice(`Saved ${n} new file${n === 1 ? '' : 's'} into ${target.where}`);
  }
  // How to render [a, b) of the sound for saving: rate and depth from the save options.
  /** @param {{ a?: number, b?: number, mono?: boolean, rate?: number, bits?: number }} [o] */
  renderOpts({ a = 0, b = this.length, mono = this.settings?.export_mono, rate = 0, bits = 0 } = {}) {
    return { a, b, rate: rate || Number(this.$('[data-f=rate]').value) || this.sr, bits: bits || Number(this.$('[data-f=bits]').value), mono: !!mono };
  }

  format() { return this.$('[data-f=format]')?.value || 'wav'; }
  fileName(ext = this.format()) { return (this.$('[data-f=name]').value || 'untitled') + '.' + ext; }
  // Where a new file goes: asked every time, since a save never replaces a file.
  /** @param {string} [title] @returns {Promise<SaveTarget | null>} */
  async target(title) {
    if (this.whereToSave) return this.whereToSave(title);
    const dir = this.dir || await this.pickFolder();
    return dir ? { dir, where: dir } : null;
  }
  async save() {
    const target = await this.target(); if (!target) return;
    // An edit made, or another file opened, while the save runs must not be marked as saved.
    const file = this.fileId, version = this.version, from = this.path && this.name;
    const res = await this.onSave(this.renderOpts(), target.dir, this.fileName(), { format: this.format(), source: this.path, project: target.project });
    if (!res) return;
    const name = /** @type {string} */ (res.split('/').pop());
    this.notice(`Saved ${name} into ${target.where}.${from ? ` ${from} is unchanged.` : ''}`, 7000);
    if (this.fileId === file) {
      // The editor now shows the saved file: later region changes and saves belong to it.
      this.dir = res.slice(0, res.lastIndexOf('/')); this.savedOnce = true;
      const st = await invoke('ed_saved', { path: res, version }); this.isDirty = st.dirty; this.path = st.path;
      this.name = name; this.$('[data-f=title]').textContent = this.name;
      this.infoSig = null;
      this.updateInfo();
    }
  }

  // The edit a tool makes with its current values, as sent to the native side; null when its
  // values make no change.
  /** @param {string} name */
  toolEdit(name) {
    const [a, b] = this.range(), sr = this.sr, val = (/** @type {string} */ f) => Number(this.$(`[data-f=${f}]`).value);
    switch (name) {
      case 'gain': return { kind: 'gain', a, b, db: val('gain') || 0 };
      case 'normalize': return { kind: 'normalize', a, b, db: val('norm') };
      case 'loudnorm': return { kind: 'loudnorm', a, b, target: val('lufs') };
      case 'fade': return { kind: 'fade', spans: this.fadeSpans().filter(m => m.b > m.a) };
      case 'declick': return { kind: 'declick', a, b, sensitivity: val('clicksens') || 5 };
      case 'speed': { const pct = val('speed'); return pct > 0 ? { kind: 'respeed', a, b, ratio: pct / 100 } : null; }
      case 'tune': return { kind: 'respeed', a, b, ratio: Math.pow(2, (val('tunesemis') || 0) / 12) };
      case 'stretch': { const pct = val('stretchlen'); return pct > 0 && b - a >= sr * 0.1 ? { kind: 'stretch', a, b, length: pct / 100, semitones: val('semis') || 0 } : null; }
      case 'loopxfade': { const n = Math.round(val('xfade') / 1000 * sr); return b - a >= 4 && n >= 1 && n <= (b - a) / 2 ? { kind: 'loopxfade', a, b, n } : null; }
    }
    return null;
  }
  // The whole sound as it would be after the tool, for drawing and for Play while it is open:
  // { src, length }, or null.
  /** @param {string} name @param {string} key */
  async toolFull(name, key) {
    const p = this.toolEdit(name);
    if (!p) return null;
    const r = await invoke('ed_view', { key, p, headS: null });
    return { src: key, length: r.length };
  }
  // The sound a tool would produce, for listening before applying: { src, a, b, from }, where
  // [a, b) is the part of the preview `src` to play and `from` where it sits in the sound.
  // Level tools measure the whole range but are heard over its first seconds.
  /** @param {string} name */
  async toolSegment(name) {
    const p = this.toolEdit(name);
    if (!p) return null;
    const [a] = this.range(), key = `seg|${this.version}|${JSON.stringify(p)}`;
    if (name === 'fade') {
      const [from, to] = this.fadeExcerpt();
      if (to <= from) return null;
      await invoke('ed_view', { key, p, headS: null });
      return { src: key, a: from, b: to, from };
    }
    if (name === 'loopxfade') {
      const r = await invoke('ed_view', { key, p, headS: null });
      return { src: key, a, b: a + (r.n ?? 0), from: a };
    }
    const r = await invoke('ed_view', { key, p, headS: AUDITION_S });
    return { src: key, a: 0, b: r.length, from: a };
  }
  /** Plays what the open tool `name` would make; nothing when that tool is not the one open. @param {string | null} name */
  async audition(name) {
    if (!name) return;
    if (this.fx?.toolName !== name) return;
    const id = this.auditionId = (this.auditionId || 0) + 1;
    let seg = null;
    try {
      if (this.fx.bypass) {
        // The untouched sound over the same stretch the tool's own audition plays, so A and B line up.
        let [a, b] = this.range(); b = Math.min(b, a + Math.round(this.sr * AUDITION_S));
        if (name === 'fade') [a, b] = this.fadeExcerpt();
        seg = { src: 'doc', a, b, from: a };
      }
      else seg = await this.toolSegment(name);
    } catch (e) { this.notice(String(/** @type {Error} */ (e)?.message || e)); }
    if (this.fx?.toolName !== name || id !== this.auditionId) return;
    this.onAudition?.();
    this.stopAudition(false); this.stopPlay(false);
    if (!seg || seg.b <= seg.a) return;
    const len = seg.b - seg.a, dur = len / this.sr;
    // Start two seconds before the end of a loop crossfade, so the join comes up quickly.
    const xfade = name === 'loopxfade', loop = xfade || this.loop;
    const off = xfade ? Math.round(Math.max(0, dur - 2) * this.sr) : 0;
    // Starting or stopping in the middle of a wave clicks, so every audition fades in and out.
    // The fade in is short enough to leave a drum's attack alone.
    const token = this.auditionToken = {};
    this.auditioning = true;
    this.aud = { t: now() + 0.03, from: seg.from, len, off, loop };
    let r;
    try { r = await invoke('ed_play', { p: { src: seg.src, a: seg.a, b: seg.b, start: seg.a + off, loop, fade_in: AUDITION_FADE_IN_S, fade_out: AUDITION_FADE_S } }); }
    catch { if (this.auditionToken === token) { this.auditioning = false; this.draw(); } return; }
    if (this.auditionToken !== token) return void invoke('ed_stop', { id: r.id }).catch(() => { });
    this.auditionVoice = r.id; this.aud.t = r.at;
    onEnded(r.id, () => { if (this.auditionVoice === r.id) { this.auditionVoice = null; this.auditioning = false; this.draw(); } });
    const tick = () => { if (this.auditionToken !== token || !this.auditioning) return; this.draw(); requestAnimationFrame(tick); };
    requestAnimationFrame(tick);
  }
  // Where the audition is, on the waveform as shown.
  auditionPos() {
    const a = this.aud, at = a.off + Math.max(0, now() - a.t) * this.sr;
    return a.from + (a.loop ? at % a.len : Math.min(a.len, at));
  }
  stopAudition(send = true) {
    this.auditionId = (this.auditionId || 0) + 1;
    this.auditionToken = null;
    if (!this.auditioning) return;
    this.auditioning = false;
    requestAnimationFrame(() => this.draw());
    const v = this.auditionVoice; this.auditionVoice = null;
    if (v != null) { forget(v); if (send) invoke('ed_stop', { id: v, fade: AUDITION_FADE_S }).catch(() => { }); }
  }
  // ---- toolbar menus
  bindMenus() {
    const menus = /** @type {HTMLElement[]} */ ([...this.root.querySelectorAll('.tmenu')]);
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
      btn.addEventListener('click', () => { if (!this.loaded) return; isOpen(m) ? close(m) : open(m); });
      btn.addEventListener('mouseenter', () => { if (menus.some(isOpen) && !isOpen(m)) open(m); });
      list.addEventListener('click', (e) => { if (/** @type {Element} */ (e.target).closest('[role=menuitem]')) close(m); });
      m.addEventListener('keydown', (e) => {
        if (e.key === 'Escape' && isOpen(m)) { e.preventDefault(); e.stopPropagation(); close(m); btn.focus(); }
        else if (/** @type {Element} */ (e.target).matches('input, select')) return;
        else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
          e.preventDefault(); e.stopPropagation();
          if (!isOpen(m)) { if (!this.loaded) return; open(m); }
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

  // What is played to judge a fade: the fade last adjusted, with a second of the sound beside it.
  fadeExcerpt() {
    const [fin, fout] = this.fadeSpans(), most = Math.round(this.sr * AUDITION_S), [a, b] = this.range();
    const out = this.fx?.lastField === 1 || fin.b <= fin.a;
    return out ? [Math.max(a, fout.a - this.sr, b - most), b] : [a, Math.min(b, fin.b + this.sr, a + most)];
  }
  // The inner edge of each fade can be dragged on the waveform.
  /** @param {number} x */
  fadeEdgeAt(x) {
    if (!this.loaded) return null;
    return (this.fx?.toolMark() || []).find((/** @type {FadeSpan} */ m) => Math.abs(this.sampleToX(m.fadeIn ? m.b : m.a) - x) <= 6) || null;
  }
  /** @param {number} s @param {boolean} fadeIn */
  dragFade(s, fadeIn) {
    const [a, b] = this.range(), f = this.$(`[data-f=${fadeIn ? 'fadeinlen' : 'fadeoutlen'}]`);
    const ms = Math.round((fadeIn ? s - a : b - s) / this.sr * 1000);
    f.value = Math.max(0, Math.min(this.rangeMs(), ms));
    this.fx.lastField = fadeIn ? 0 : 1;
    this.fx.toolSync?.();
  }

  // ---- input
  bind() {
    this.root.addEventListener('change', (e) => { if (this.loaded && /** @type {Element} */ (e.target).closest('[data-f=rate], [data-f=bits]')) this.updateInfo(); });
    this.root.addEventListener('click', (e) => {
      const b = /** @type {Element} */ (e.target).closest('[data-a]'); if (!b) return;
      const a = /** @type {string} */ (b.dataset.a);
      if (/** @type {Record<string, unknown>} */ (TOOLS)[a] && this.loaded) this.fx.openTool(a); else this.action(a);
    });
    this.bindMenus();
    /** @type {number | null} */ let dragFrom = null;
    let moved = false;
    /** @type {FadeSpan | null} */ let fadeDrag = null;
    this.canvas.addEventListener('mousedown', (e) => {
      if (!this.loaded) return;
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
      else this.cursor = /** @type {Span} */ (this.sel)[0];
      dragFrom = null; this.draw();
    });
    this.canvas.addEventListener('dblclick', () => { this.sel = [0, this.length]; this.draw(); });
    this.canvas.addEventListener('mousemove', (e) => { this.canvas.style.cursor = this.fadeEdgeAt(e.offsetX) ? 'ew-resize' : ''; if (this.mode !== 'spec' || !this.loaded) return; this.hover = { x: e.offsetX, y: e.offsetY }; this.draw(); });
    this.canvas.addEventListener('mouseleave', () => { if (this.hover) { this.hover = null; this.draw(); } });
    this.canvas.addEventListener('wheel', (e) => {
      if (!this.loaded) return;
      e.preventDefault();
      if (e.ctrlKey || e.metaKey) this.zoom(Math.exp(e.deltaY * 0.01), e.offsetX);
      else { this.start += (Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY) * this.spp; this.clampView(); this.draw(); }
    }, { passive: false });
  }

  /** @param {KeyboardEvent} e */
  handleKey(e) {
    if (this.root.hidden || !this.loaded) return false;
    if (this.root.querySelector('.tmenu-list:not([hidden])')) return false;
    const mod = e.metaKey || e.ctrlKey;
    /** @type {Record<string, string | false | null>} */
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
