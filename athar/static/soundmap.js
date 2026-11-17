import { lib } from './native.js';
import { invoke } from './engine.js';
import { fmtTime } from './time.js';

const DOT = 4.5, HIT = 12, DWELL_MS = 140;

/** The library as a map: sounds that sound alike sit near each other. Hover to hear, click to
    keep one, drag to move around, scroll to zoom. */
/** A sound on the map, at x and y between 0 and 1.
    @typedef {{ path: string, name: string, x: number, y: number, category: string | null, family: string | null, duration: number, sr: number }} Point */
/** A group name at the middle of its sounds; key is `g:` and a family or `c:` and a kind. @typedef {{ key: string, text: string, n: number, x: number, y: number }} Name */
/** Where a group name was drawn, to find it under the pointer. @typedef {{ x: number, y: number, w: number, h: number, name: string }} NameBox */
/** @typedef {typeof import('./ui.js')} UI */
/** What the page uses from main.js.
    @typedef {{ el: UI['el'], ib: UI['ib'], icon: UI['icon'], fail: UI['fail'], busy: UI['busy'],
      preview(p: Point): void, stop(): void, edit(p: Point): void, similar(p: Point): void }} SoundMapHelpers */

export class SoundMap {
  /** @param {HTMLElement} root @param {SoundMapHelpers} h */
  constructor(root, h) {
    this.root = root; this.h = h;
    /** @type {Point[]} */
    this.points = [];
    this.zoom = 1; this.ox = 0; this.oy = 0;
    /** @type {Point | null} */
    this.hover = null;
    /** @type {Point | null} */
    this.picked = null;
    this.stamp = '';
    /** @type {boolean | undefined} */ this.queued;
    /** @type {boolean | undefined} */ this.moving;
    /** @type {string | null | undefined} */ this.lockCat;
    /** @type {string | null | undefined} */ this.hoverCat;
    /** @type {Point | null | undefined} */ this.hoverPlaying;
    /** @type {[number, number] | null | undefined} */ this.pointer;
    /** @type {NameBox[] | undefined} */ this.labels;
    /** @type {{ key: string, canvas: HTMLCanvasElement } | undefined} */ this.land;
    /** The names for one zoom step, and the step asked for. @type {{ stamp: string, level: number, list: Name[] } | undefined} */ this.named;
    /** @type {string | undefined} */ this.namesWant;
    const { el, ib } = h;
    this.canvas = el('canvas', { class: 'map-canvas', tabindex: '0', 'aria-label': 'Map of your sounds' });
    this.info = el('div', { class: 'map-info' });
    this.note = el('span', { class: 'scope' });
    /** @type {HTMLElement} */ (root.querySelector('.cmdline')).append(
      ib('zoom_in', 'Zoom in', () => this.zoomBy(1.4)), ib('zoom_out', 'Zoom out', () => this.zoomBy(1 / 1.4)), ib('fit_screen', 'Show the whole map', () => { this.zoom = 1; this.ox = this.oy = 0; this.draw(); }),
      el('span', { class: 'spacer' }), this.note);
    /** @type {HTMLElement} */ (root.querySelector('.map-body')).append(this.canvas, this.info);
    this.bind();
    new ResizeObserver(() => this.draw()).observe(this.canvas);
  }

  async show() {
    if (!this.points.length) this.h.busy('Laying out your sounds');
    try {
      /** @type {Point[]} */
      const pts = await lib.soundMap();
      this.points = pts; this.stamp = String(Date.now());
      this.note.textContent = pts.length ? '' : 'Add a folder of sounds to see the map';
      if (this.picked && !pts.some(p => p.path === /** @type {Point} */ (this.picked).path)) this.picked = null;
      this.h.busy('');
    } catch (e) { this.h.fail(e); }
    this.paintInfo(); this.draw();
  }

  // Map position (0 to 1) to canvas pixels, with a margin so edge dots are not clipped.
  /** @param {{ x: number, y: number }} p @param {number} W @param {number} H @returns {[number, number]} */
  place(p, W, H) {
    const m = 28, s = Math.min(W, H) - m * 2;
    return [(W - s) / 2 + (p.x - 0.5) * s * this.zoom + s / 2 + this.ox, (H - s) / 2 + (0.5 - p.y) * s * this.zoom + s / 2 + this.oy];
  }

  // Many events can ask for a redraw between two frames; one drawing per frame serves them all.
  draw() {
    if (this.queued) return;
    this.queued = true;
    requestAnimationFrame(() => { this.queued = false; this.paint(); });
  }

  paint() {
    const cv = this.canvas, dpr = window.devicePixelRatio || 1, W = cv.clientWidth, H = cv.clientHeight;
    if (!W || !H) return;
    if (cv.width !== W * dpr || cv.height !== H * dpr) { cv.width = W * dpr; cv.height = H * dpr; }
    const g = /** @type {CanvasRenderingContext2D} */ (cv.getContext('2d')), css = getComputedStyle(document.documentElement), seen = /** @type {Record<string, string>} */ ({}), col = (/** @type {string} */ n) => seen[n] ??= css.getPropertyValue(n).trim();
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.fillStyle = col('--wave-bg'); g.fillRect(0, 0, W, H);
    const at = this.points.map(p => this.place(p, W, H));
    // Only a name tag lights up a whole kind. A single sound under the pointer lights up only itself.
    const focus = this.lockCat || this.hoverCat || null;
    // Ground first: every sound raises a soft hill, and the hills add up into a landscape. It is
    // drawn in flat bands, paper at sea level rising to the accent colour on the peaks.
    g.imageSmoothingEnabled = false;
    g.drawImage(this.terrain(at, W, H, col('--wave-bg'), col('--accent')), 0, 0, W, H);
    // Sounds show near the pointer, for the kind in focus, and everywhere once zoomed in.
    const near = this.pointer, everywhere = this.zoom >= 2.5;
    this.points.forEach((p, i) => {
      const [x, y] = at[i], close = near && (x - near[0]) ** 2 + (y - near[1]) ** 2 < 90 * 90, inFocus = focus && (focus === `c:${p.category}` || focus === `g:${p.family}`);
      if (!close && !inFocus && !everywhere) return;
      g.beginPath(); g.arc(x, y, inFocus ? DOT : DOT - 1, 0, Math.PI * 2);
      g.fillStyle = inFocus ? col('--ink') : col('--raised'); g.fill();
      g.strokeStyle = col('--ink'); g.lineWidth = 1.2; g.stroke();
    });
    // Names for the groups of sounds on screen. Far out they are broad families; closer in, the
    // kinds within them; closer still, the word their file names share, on its own. Bigger groups are named
    // first, and a name that would cover another is left out.
    g.font = '13px "Share Tech Mono", ui-monospace, monospace';
    /** @type {{ x: number, y: number, w: number, h: number }[]} */
    const placed = []; this.labels = [];
    for (const l of this.names()) {
      const [px, py] = this.place(l, W, H), w = g.measureText(l.text).width + 14, h = 20, x = px - w / 2, y = py - h - 12;
      if (x + w < 0 || x > W || y + h < 0 || y > H) continue;
      if (placed.some(b => x < b.x + b.w + 6 && x + w + 6 > b.x && y < b.y + b.h + 6 && y + h + 6 > b.y)) continue;
      placed.push({ x, y, w, h }); this.labels.push({ x, y, w, h, name: l.key });
      const on = focus === l.key;
      g.fillStyle = on ? col('--ink') : col('--raised'); g.fillRect(x, y, w, h);
      g.strokeStyle = col('--ink'); g.lineWidth = 1; g.strokeRect(x + 0.5, y + 0.5, w - 1, h - 1);
      g.fillStyle = on ? col('--raised') : col('--ink'); g.fillText(l.text, x + 7, y + 14);
    }
    for (const [p, r] of /** @type {[Point | null, number][]} */ ([[this.picked, DOT + 3], [this.hover, DOT + 2]])) {
      if (!p) continue;
      const [x, y] = this.place(p, W, H);
      g.fillStyle = col('--accent'); g.beginPath(); g.arc(x, y, r, 0, Math.PI * 2); g.fill();
    }
    if (this.hover) {
      const [x, y] = this.place(this.hover, W, H), text = [this.hover.name, this.hover.category, fmtTime(this.hover.duration)].filter(Boolean).join('   ');
      const tw = g.measureText(text).width + 16, bx = Math.min(W - tw - 4, Math.max(4, x + 12)), by = Math.max(4, y - 32);
      g.fillStyle = col('--ink'); g.fillRect(bx, by, tw, 24);
      g.fillStyle = col('--raised'); g.fillText(text, bx + 8, by + 16);
    }
  }

  // The landscape as a small picture, redrawn only when the view, the sounds or the colours change.
  /** @param {[number, number][]} at @param {number} W @param {number} H @param {string} paper @param {string} accent */
  terrain(at, W, H, paper, accent) {
    // Heights are worked out on a coarse grid, then every screen pixel reads its own height from between
    // the grid points, so the edges of the bands come out as smooth curves. While the map is being
    // dragged it is drawn at half the detail to keep up with the pointer.
    const scale = this.moving ? 1 : Math.min(2, window.devicePixelRatio || 1);
    const C = 3, gw = Math.ceil(W / C) + 2, gh = Math.ceil(H / C) + 2, key = [W, H, scale, this.zoom, this.ox, this.oy, this.points.length, this.stamp, paper, accent].join('|');
    if (this.land?.key === key) return this.land.canvas;
    // Each sound drops a unit of weight on the grid, shared between the four cells around it; the
    // grid is then blurred along rows and along columns. That gives the same hills as drawing one
    // per sound, at a cost that does not grow with the library.
    const r = (20 + 9 * Math.sqrt(this.zoom)) / C, reach = Math.ceil(r * 3.2), bins = new Float32Array(gw * gh);
    for (const [px, py] of at) {
      const cx = px / C, cy = py / C, x0 = Math.floor(cx), y0 = Math.floor(cy), fx = cx - x0, fy = cy - y0;
      if (x0 < 0 || y0 < 0 || x0 >= gw - 1 || y0 >= gh - 1) continue;
      const n = y0 * gw + x0;
      bins[n] += (1 - fx) * (1 - fy); bins[n + 1] += fx * (1 - fy); bins[n + gw] += (1 - fx) * fy; bins[n + gw + 1] += fx * fy;
    }
    const kernel = new Float32Array(reach * 2 + 1); for (let i = -reach; i <= reach; i++) kernel[i + reach] = Math.exp(-(i * i) / (2 * r * r));
    const mid = new Float32Array(gw * gh), f = new Float32Array(gw * gh);
    for (let y = 0; y < gh; y++) for (let x = 0; x < gw; x++) {
      const v = bins[y * gw + x]; if (!v) continue;
      for (let i = Math.max(-reach, -x); i <= Math.min(reach, gw - 1 - x); i++) mid[y * gw + x + i] += v * kernel[i + reach];
    }
    for (let y = 0; y < gh; y++) for (let x = 0; x < gw; x++) {
      const v = mid[y * gw + x]; if (v < 1e-6) continue;
      for (let i = Math.max(-reach, -y); i <= Math.min(reach, gh - 1 - y); i++) f[(y + i) * gw + x] += v * kernel[i + reach];
    }
    let top = 0; for (let i = 0; i < f.length; i++) if (f[i] > top) top = f[i];
    const rgb = (/** @type {string} */ hex) => { hex = hex.trim(); if (hex.length === 4) hex = '#' + [...hex.slice(1)].map(c => c + c).join(''); return [1, 3, 5].map(n => parseInt(hex.slice(n, n + 2), 16)); };
    const a = rgb(paper), b = rgb(accent), LEVELS = 7, shades = /** @type {number[][]} */ ([]);
    for (let l = 0; l <= LEVELS; l++) { const t = l / LEVELS, m = l ? t * t * 0.9 + 0.06 : 0; shades.push([a[0] + (b[0] - a[0]) * m, a[1] + (b[1] - a[1]) * m, a[2] + (b[2] - a[2]) * m]); }
    const pw = Math.round(W * scale), ph = Math.round(H * scale);
    const canvas = this.land?.canvas || document.createElement('canvas'); canvas.width = pw; canvas.height = ph;
    const ctx = /** @type {CanvasRenderingContext2D} */ (canvas.getContext('2d')), img = ctx.createImageData(pw, ph), d = new Uint32Array(img.data.buffer);
    const packed = shades.map(sh => (255 << 24) | (Math.round(sh[2]) << 16) | (Math.round(sh[1]) << 8) | Math.round(sh[0]));
    // Height to band, looked up from a table so the per-pixel work stays small.
    const steps = 1024, band = new Uint8Array(steps + 1);
    for (let n = 0; n <= steps; n++) band[n] = Math.min(LEVELS, Math.floor(Math.pow(n / steps, 0.6) * LEVELS + 0.35));
    const inv = top ? steps / top : 0, toGrid = 1 / (C * scale);
    for (let y = 0; y < ph; y++) {
      const gy = y * toGrid, y0 = gy | 0, fy = gy - y0, row = y0 * gw, out = y * pw;
      for (let x = 0; x < pw; x++) {
        const gx = x * toGrid, x0 = gx | 0, fx = gx - x0, n = row + x0;
        const v = (f[n] * (1 - fx) + f[n + 1] * fx) * (1 - fy) + (f[n + gw] * (1 - fx) + f[n + gw + 1] * fx) * fy;
        d[out + x] = packed[band[(v * inv) | 0]];
      }
    }
    ctx.putImageData(img, 0, 0);
    this.land = { key, canvas };
    return canvas;
  }

  // The names to show at the current zoom, biggest group first, in map units. They are worked
  // out natively (app/src/mapnames.rs) once per zoom step and set of sounds; until they arrive the
  // last ones stay, and the map redraws when they come.
  names() {
    const level = Math.min(5, Math.floor(Math.log2(this.zoom) + 0.35)), want = `${this.stamp}|${level}`;
    if (this.named?.stamp === this.stamp && this.named.level === level) return this.named.list;
    if (this.namesWant !== want) {
      this.namesWant = want;
      const stamp = this.stamp;
      invoke('map_names', { level }).then((/** @type {Name[]} */ list) => {
        if (this.namesWant !== want) return;
        this.named = { stamp, level, list }; this.draw();
      }).catch(() => { });
    }
    return this.named?.list || [];
  }

  /** @param {number} x @param {number} y */
  labelAt(x, y) { return (this.labels || []).find(b => x >= b.x && x <= b.x + b.w && y >= b.y && y <= b.y + b.h)?.name || null; }

  /** @param {number} x @param {number} y */
  nearest(x, y) {
    const W = this.canvas.clientWidth, H = this.canvas.clientHeight;
    /** @type {Point | null} */
    let best = null;
    let d = HIT * HIT;
    for (const p of this.points) { const [px, py] = this.place(p, W, H), q = (px - x) ** 2 + (py - y) ** 2; if (q < d) { d = q; best = p; } }
    return best;
  }

  /** @param {number} k */
  zoomBy(k, x = this.canvas.clientWidth / 2, y = this.canvas.clientHeight / 2) {
    const z = Math.min(40, Math.max(1, this.zoom * k)), r = z / this.zoom, cx = this.canvas.clientWidth / 2, cy = this.canvas.clientHeight / 2;
    this.ox = (x - cx) - ((x - cx) - this.ox) * r; this.oy = (y - cy) - ((y - cy) - this.oy) * r;
    this.zoom = z; if (z === 1) this.ox = this.oy = 0;
    this.draw();
  }

  bind() {
    const cv = this.canvas;
    /** @type {{ x: number, y: number, ox: number, oy: number, moved: boolean } | null} */
    let drag = null;
    let dwell = 0;
    // Only the main button drags, and a drag also ends if the pointer is taken away, as a context menu does.
    const endDrag = () => { drag = null; if (this.moving) { this.moving = false; this.draw(); } };
    cv.addEventListener('pointercancel', endDrag); cv.addEventListener('lostpointercapture', endDrag);
    cv.addEventListener('pointerdown', (e) => { if (e.button !== 0) return; drag = { x: e.clientX, y: e.clientY, ox: this.ox, oy: this.oy, moved: false }; cv.setPointerCapture(e.pointerId); });
    cv.addEventListener('pointermove', (e) => {
      if (drag) {
        const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
        if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
        if (drag.moved) { this.moving = true; this.ox = drag.ox + dx; this.oy = drag.oy + dy; this.draw(); }
        return;
      }
      this.pointer = [e.offsetX, e.offsetY];
      const over = this.labelAt(e.offsetX, e.offsetY), p = over ? null : this.nearest(e.offsetX, e.offsetY);
      cv.style.cursor = over || p ? 'pointer' : '';
      if (p === this.hover && over === this.hoverCat) return void this.draw();
      this.hover = p; this.hoverCat = over; this.draw();
      clearTimeout(dwell);
      // A sound started by resting on its dot stops as soon as the pointer leaves that dot.
      if (this.hoverPlaying && this.hoverPlaying !== p) { this.hoverPlaying = null; this.h.stop(); }
      // A short rest on a dot plays it, so sweeping across the map is not a wall of noise.
      if (p) dwell = setTimeout(() => { if (this.hover === p) { this.hoverPlaying = p; this.h.preview(p); } }, DWELL_MS);
    });
    cv.addEventListener('pointerup', (e) => {
      const d = drag; drag = null;
      if (this.moving) { this.moving = false; this.draw(); }
      if (!d || d.moved) return;
      // A name keeps its kind lit until it is clicked again; a dot is kept at the bottom.
      const over = this.labelAt(e.offsetX, e.offsetY);
      if (over) { this.lockCat = this.lockCat === over ? null : over; return this.draw(); }
      const p = this.nearest(e.offsetX, e.offsetY);
      // A clicked sound plays to its end, wherever the pointer goes next.
      if (p) { clearTimeout(dwell); this.picked = p; this.hoverPlaying = null; this.h.preview(p); this.paintInfo(); }
      else this.lockCat = null;
      this.draw();
    });
    cv.addEventListener('pointerleave', () => { clearTimeout(dwell); if (this.hoverPlaying) { this.hoverPlaying = null; this.h.stop(); } this.pointer = null; this.hover = null; this.hoverCat = null; this.draw(); });
    cv.addEventListener('wheel', (e) => { e.preventDefault(); this.zoomBy(e.deltaY < 0 ? 1.15 : 1 / 1.15, e.offsetX, e.offsetY); }, { passive: false });
  }

  paintInfo() {
    const { el, ib, icon } = this.h, p = this.picked;
    this.info.hidden = !p;
    if (!p) return this.info.replaceChildren();
    this.info.replaceChildren(
      ib('play_arrow', 'Play', () => this.h.preview(p)),
      el('span', { class: 'qn' }, p.name), el('span', { class: 'muted' }, [p.category, fmtTime(p.duration)].filter(Boolean).join(', ')),
      el('span', { class: 'spacer' }),
      el('button', { class: 'ghost', onclick: () => this.h.similar(p) }, icon('queue_music'), 'Find in Library'),
      el('button', { class: 'primary', onclick: () => this.h.edit(p) }, icon('edit'), 'Edit'));
  }

  /** @param {KeyboardEvent} e */
  handleKey(e) {
    if (e.key === ' ' && (this.picked || this.hover)) { e.preventDefault(); this.h.preview(/** @type {Point} */ (this.picked || this.hover)); return true; }
    if (e.key === 'Escape') { this.h.stop(); return true; }
    if (e.key === '=' || e.key === '+') { this.zoomBy(1.4); return true; }
    if (e.key === '-') { this.zoomBy(1 / 1.4); return true; }
    return false;
  }
}
