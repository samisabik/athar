import { lib } from './native.js';
import { fmtTime } from './time.js';
import { invoke, now, listen } from './engine.js';
import { MAX_LANES, MAX_HITS, MAX_SECONDS, MIN_GAIN_DB, MAX_GAIN_DB } from './arrange.js';
import { ArrBrowser } from './arrbrowser.js';

// Sizes the stylesheet shares: the lane heads' width and a lane's height (style.css, "arranger").
const HEAD_W = 168, LANE_H = 76;
const DOUBLE_CLICK_MS = 400, MAGNET_PX = 9, WAVE_H = 24, MAX_ZOOM = 32, NUDGE = 0.01;
const TICKS = [0.05, 0.1, 0.25, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300];
/** @typedef {{ path: string, name: string, duration: number, sr: number, at: number, in: number, out: number }} Clip */
/** @typedef {{ name: string, gain: number, mute: boolean, solo: boolean, clips: Clip[] }} Lane */
/** @typedef {{ loop: boolean, lanes: Lane[] }} Arrangement */
/** A clip's sound as loaded natively: its length, and its outline for drawing.
    @typedef {{ info: { duration: number, sr: number, channels: number, peaks: number[] } | null, svg: string, error: string | null, ready?: Promise<void> }} Sound */
/** When each recent pass began on the engine clock, and from where in the arrangement. @typedef {{ passes: [number, number][] }} Playing */
/** WebKit's pinch gesture. @typedef {Event & { scale: number, clientX: number }} GestureEvent */

/** @returns {Arrangement} */
const empty = () => ({ loop: true, lanes: [] });
const stem = (/** @type {string} */ name) => name.replace(/\.[^.]+$/, '');
const plural = (/** @type {number} */ n, /** @type {string} */ word) => `${n} ${word}${n === 1 ? '' : 's'}`;
// The same curve as an outline, for the part of a clip a fade takes away: the area above the
// level line, from silence at the clip's edge up to full level where the fade ends.
const curve = (/** @type {boolean} */ flip) => Array.from({ length: 17 }, (_, i) => { const x = 1 - i / 16; return `${((flip ? 1 - x : x) * 100).toFixed(1)}% ${((1 - Math.sin(x * Math.PI / 2)) * 100).toFixed(1)}%`; }).join(', ');
const WEDGE_IN = `polygon(0 0, 100% 0, ${curve(false)})`, WEDGE_OUT = `polygon(100% 0, 0 0, ${curve(true)})`;

// The sound's outline as one closed shape, stretched to whatever width its clip has.
// `peaks` is the loudest sample of each stretch of the sound, as the native side measures it.
/** @param {number[]} peaks */
function waveSvg(peaks) {
  const bins = peaks.length;
  // Quiet sound is drawn taller than it measures, so a soft tail still shows.
  const mid = WAVE_H / 2, h = Array.from(peaks, p => Math.max(0.4, Math.pow(Math.min(1, p), 0.6) * (mid - 1)));
  let d = `M0 ${mid}`;
  for (let k = 0; k < bins; k++) d += `L${k + 0.5} ${(mid - h[k]).toFixed(1)}`;
  d += `L${bins} ${mid}`;
  for (let k = bins - 1; k >= 0; k--) d += `L${k + 0.5} ${(mid + h[k]).toFixed(1)}`;
  return `<svg viewBox="0 0 ${bins} ${WAVE_H}" preserveAspectRatio="none" aria-hidden="true"><path d="${d}Z"/></svg>`;
}

/** Plays several sounds together. A lane is a named track (Drums, Vocal, Synth) with a level; it
    holds clips, each one a sound placed in time with a fade in and a fade out of its own. Sounds
    come from the Arranger's own browser. What is heard can be saved as one new sound. */
export class Arranger {
  /** @param {HTMLElement} root @param {Record<string, any>} h the page's helpers (see main.js) */
  constructor(root, h) {
    listen('arranger', (st) => this.passes(st));
    this.root = root; this.h = h;
    this.a = empty();
    /** @type {Map<string, Sound>} */ this.sounds = new Map();
    /** @type {{ lane: Lane, clip: Clip } | null} */ this.sel = null;
    /** What copy keeps for paste. @type {{ lane: Lane, clip: Clip } | null} */ this.clip = null;
    this.zoom = 1; this.pps = 100; this.trackW = 0; this.cursor = 0; this.dragging = false;
    /** @type {Playing | null} */ this.playing = null;
    /** @type {Map<Clip, HTMLElement> | null} */ this.nodes = null;
    /** @type {HTMLElement | null} */ this.head = null;
    /** @type {number | undefined} */ this.saveTimer = undefined;
    /** @type {Promise<void> | undefined} */ this.saving = undefined;
    /** @type {number | null} */ this.zoomWant = null;
    this.zoomX = 0; this.zoomFrame = 0;
    /** @type {{ clip: Clip, t: number } | null} */ this.lastClick = null;
    /** @type {string | undefined} */ this.lastDest = undefined;
    /** What the native side says is heard: each lane, the clips a mix would hold, the first lane heard.
        @type {{ heard: boolean[], clips: number, first: string | null }} */
    this.shown = { heard: [], clips: 0, first: null };
    const { el, ib, icon } = h;
    this.playBtn = ib('play_arrow', 'Play or stop (space)', () => this.toggle(), 'strong');
    this.loopBtn = ib('repeat', 'Loop (L)', () => this.set({ loop: !this.a.loop }), 'toggle');
    this.clock = el('span', { class: 'arr-clock' });
    this.saveBtn = el('button', { class: 'primary', 'data-tip': 'Save what you hear as one new sound', onclick: () => this.bounce().catch(h.fail) }, icon('save'), 'Save mix');
    this.clearBtn = ib('delete_sweep', 'Remove every lane from the Arranger', () => this.clear());
    this.browserBtn = ib('chevron_left', 'Hide the browser, for more room to arrange (B)', () => this.showBrowser(this.browser.root.hidden));
    /** @type {HTMLElement} */ (root.querySelector('.cmdline')).append(this.browserBtn, el('span', { class: 'sep' }), this.playBtn, this.loopBtn, this.clock, el('span', { class: 'spacer' }),
      ib('zoom_out', 'Zoom out (-)', () => this.setZoom(this.zoom / 2)), ib('zoom_in', 'Zoom in (+)', () => this.setZoom(this.zoom * 2)),
      el('span', { class: 'sep' }), this.clearBtn, this.saveBtn);
    /** @type {HTMLElement} */
    this.scroll = el('div', { class: 'arr-scroll' });

    // Sounds come in from the Arranger's own browser: dragged onto a lane, or below the lanes for a new one.
    this.browser = new ArrBrowser(h, (it) => { this.dropSound(it, 0, this.sel?.lane || null).catch(h.fail); });
    this.browser.onDragEnd = () => this.dropLine(null);
    // A pinch on the trackpad arrives as a wheel event with ctrl held; cmd and scroll does the same.
    // WebKit also reports a pinch as its own gesture, with the scale since the fingers went down.
    /** @type {{ zoom: number, x: number } | null} */
    let pinch = null;
    this.scroll.addEventListener('wheel', (e) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      if (!pinch) this.zoomAt(Math.exp(-e.deltaY * 0.01), e.clientX);
    }, { passive: false });
    this.scroll.addEventListener('gesturestart', (/** @type {any} */ ev) => { const e = /** @type {GestureEvent} */ (ev); e.preventDefault(); pinch = { zoom: this.zoom, x: e.clientX }; });
    this.scroll.addEventListener('gesturechange', (/** @type {any} */ ev) => { const e = /** @type {GestureEvent} */ (ev); if (!pinch) return; e.preventDefault(); this.zoomAt(1, e.clientX || pinch.x, pinch.zoom * e.scale); });
    this.scroll.addEventListener('gestureend', (e) => { e.preventDefault(); pinch = null; });
    this.scroll.addEventListener('dragover', (e) => {
      const it = this.browser.dragging; if (!it) return;
      e.preventDefault(); if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
      this.dropLine(this.magnet(this.timeAt(e), it.duration || 0).t, this.laneAt(e));
    });
    this.scroll.addEventListener('dragleave', (e) => { if (!this.scroll.contains(/** @type {Node | null} */ (e.relatedTarget))) this.dropLine(null); });
    this.scroll.addEventListener('drop', (e) => {
      const it = this.browser.dragging; if (!it) return;
      e.preventDefault(); this.browser.dragging = null; this.dropLine(null);
      this.dropSound(it, this.magnet(this.timeAt(e), it.duration || 0).t, this.laneAt(e)).catch(h.fail);
    });
    /** @type {HTMLElement} */ (root.querySelector('.stack')).append(el('div', { class: 'arr-body' }, this.browser.root, el('div', { class: 'arr-stage' }, this.scroll)));
    let open = true;
    try { open = localStorage.getItem('athar.arranger.browser') !== 'hidden'; } catch { }
    this.showBrowser(open, false);
    window.addEventListener('resize', () => { if (this.visible()) this.render(); });
    this.render();
  }

  visible() { return !!this.scroll.clientWidth; }

  // The browser can be folded away; the lanes take its room. The choice is kept for next time.
  /** @param {boolean} open */
  showBrowser(open, keep = true) {
    this.browser.root.hidden = !open;
    const tip = open ? 'Hide the browser, for more room to arrange (B)' : 'Show the browser (B)';
    this.browserBtn.querySelector('.i').textContent = open ? 'chevron_left' : 'chevron_right';
    this.browserBtn.dataset.tip = tip; this.browserBtn.setAttribute('aria-label', tip); this.browserBtn.setAttribute('aria-expanded', String(open));
    if (!keep) return;
    try { localStorage.setItem('athar.arranger.browser', open ? 'shown' : 'hidden'); } catch { }
    this.render();
  }

  // ---- what is kept between sessions
  plain() {
    return { loop: this.a.loop, lanes: this.a.lanes.map(({ name, gain, mute, solo, clips }) => ({ name, gain, mute, solo, clips: clips.map(c => ({ path: c.path, name: c.name, duration: c.duration, sr: c.sr, at: c.at, in: c.in, out: c.out })) })) };
  }
  // Every change goes to the native side at once, so what plays follows; it is stored a moment
  // after the changes stop.
  save() { this.push(false); clearTimeout(this.saveTimer); this.saveTimer = setTimeout(() => this.flush(), 300); }
  /** @param {boolean} store */
  async push(store) {
    this.shown = await invoke('arr_set', { a: this.plain(), store });
    this.scroll.querySelectorAll('.arr-row[data-li]').forEach((r) => r.classList.toggle('quiet', this.shown.heard[Number(/** @type {HTMLElement} */ (r).dataset.li)] === false));
  }
  flush() {
    if (!this.saveTimer) return this.saving;
    clearTimeout(this.saveTimer); this.saveTimer = undefined;
    return this.saving = this.push(true).catch(this.h.fail);
  }
  // The stored arrangement is read again each time, because a file renamed or moved elsewhere
  // in Athar has its new path there.
  async sync() {
    await this.flush();
    this.a = { ...empty(), ...await lib.arrangement() };
    this.push(false).catch(() => { });
    this.sel = null;
    const used = new Set(this.all().map(x => x.clip.path));
    for (const [path, s] of this.sounds) if (s.error || !used.has(path)) this.sounds.delete(path);
  }

  async show() {
    this.browser.show();
    await this.sync();
    this.render();
    this.all().forEach(x => this.load(x.clip));
  }
  leave() { this.stop(); this.flush(); }

  /** @param {Partial<Arrangement>} changes */
  set(changes) { Object.assign(this.a, changes); this.save(); this.render(); }

  /** Every clip with its lane. */
  all() { return this.a.lanes.flatMap(lane => lane.clips.map(clip => ({ lane, clip }))); }
  /** Where the last clip ends, in seconds: the length of one pass. */
  length() { return this.all().reduce((end, x) => Math.max(end, x.clip.at + x.clip.duration), 0); }

  /** @param {number} z */
  setZoom(z) { this.zoom = Math.min(MAX_ZOOM, Math.max(1, z)); if (!this.relayout()) this.render(); }
  // Zoom by a factor (or to a given zoom), keeping the moment under the pointer where it is on
  // screen. A pinch sends many small steps: they are gathered and applied once per frame.
  /** @param {number} factor @param {number} clientX @param {number | null} [to] */
  zoomAt(factor, clientX, to = null) {
    this.zoomWant = Math.min(MAX_ZOOM, Math.max(1, to ?? (this.zoomWant ?? this.zoom) * factor));
    this.zoomX = clientX;
    if (this.zoomFrame) return;
    this.zoomFrame = requestAnimationFrame(() => {
      this.zoomFrame = 0;
      const grid = this.scroll.querySelector('.arr-grid'), want = this.zoomWant, x = this.zoomX;
      this.zoomWant = null;
      if (!grid || want == null || want === this.zoom) return;
      const inView = x - this.scroll.getBoundingClientRect().left - HEAD_W;
      const t = (x - grid.getBoundingClientRect().left - HEAD_W) / this.pps;
      this.setZoom(want);
      this.scroll.scrollLeft = Math.max(0, t * this.pps - inView);
    });
  }
  // How wide a second is, and the lanes: a little more than the arrangement holds, so there is room to drag a clip later.
  measure() {
    const room = Math.max(240, (this.scroll.clientWidth || 900) - HEAD_W - 2), span = Math.max(this.length(), 1) * 1.2;
    this.pps = room / span * this.zoom;
    this.trackW = Math.max(room, span * this.pps);
  }
  // After a zoom nothing but positions change, so what is drawn is moved and resized in place
  // (the waveforms stretch with their clips). False when there is nothing drawn to move.
  relayout() {
    const grid = this.scroll.querySelector('.arr-grid');
    if (!grid || !this.nodes || this.dragging) return false;
    this.measure();
    grid.style.width = `${HEAD_W + this.trackW}px`;
    grid.querySelector('.arr-ruler')?.replaceWith(this.rulerRow());
    for (const [clip, node] of this.nodes) {
      node.style.left = `${clip.at * this.pps}px`; node.style.width = `${Math.max(4, clip.duration * this.pps)}px`;
      this.dressFades(node, clip);
    }
    /** @type {HTMLElement} */ (grid.querySelector('.arr-after')).style.left = `${HEAD_W + this.length() * this.pps}px`;
    this.placeHead(this.playing ? this.position(this.playing, now()) : this.cursor);
    return true;
  }

  /** @param {Clip} clip */
  load(clip) {
    let s = this.sounds.get(clip.path);
    if (s) return s.ready;
    /** @type {Sound} */
    const fresh = s = { info: null, svg: '', error: null };
    const path = clip.path;
    fresh.ready = invoke('arr_load', { path }).then((info) => {
      fresh.info = info; fresh.svg = waveSvg(info.peaks);
      for (const x of this.all()) if (x.clip.path === path) x.clip.duration = info.duration;
    }, (e) => { fresh.error = e?.message || String(e); }).then(() => { if (this.sounds.get(path) === fresh && !this.dragging) this.render(); });
    this.sounds.set(path, fresh);
    return fresh.ready;
  }
  /** @param {string} path */
  forget(path) {
    if (this.all().some(x => x.clip.path === path)) return;
    this.sounds.delete(path);
    invoke('arr_keep', { paths: [...this.sounds.keys()] }).catch(() => { });
  }

  // ---- lanes
  async addLane() {
    if (this.a.lanes.length >= MAX_LANES) return this.h.notice(`The Arranger holds ${MAX_LANES} lanes. Remove one to add another.`, 6000);
    const v = await this.h.modal('New lane', [{ label: 'Name, such as Drums, Vocal or Synth', value: `Lane ${this.a.lanes.length + 1}` }]);
    if (!v || !v[0]) return;
    this.a.lanes.push({ name: v[0].slice(0, 60), gain: 0, mute: false, solo: false, clips: [] });
    this.save(); this.render();
  }
  /** @param {Lane} lane */
  async renameLane(lane) {
    const v = await this.h.modal('Name of the lane', [{ label: 'Name', value: lane.name }]);
    if (!v || !v[0]) return;
    lane.name = v[0].slice(0, 60);
    this.save(); this.render();
  }
  /** @param {Lane} lane */
  async removeLane(lane) {
    if (lane.clips.length && !await this.h.confirmModal(`Remove ${lane.name}?`, `The lane and its ${plural(lane.clips.length, 'clip')} leave the Arranger. The files stay where they are.`, 'Remove')) return;
    this.a.lanes = this.a.lanes.filter(l => l !== lane);
    if (this.sel?.lane === lane) this.sel = null;
    lane.clips.forEach(c => this.forget(c.path));
    if (!this.a.lanes.length) { this.stop(); this.cursor = 0; }
    this.save(); this.render();
  }
  async clear() {
    if (!this.a.lanes.length) return;
    if (!await this.h.confirmModal('Empty the Arranger?', 'Every lane leaves the Arranger. The files stay where they are.', 'Empty')) return;
    this.stop(); this.sounds.clear(); invoke('arr_keep', { paths: [] }).catch(() => { }); this.sel = null; this.cursor = 0;
    this.set({ lanes: [] });
  }

  // ---- places in time
  /** @param {number} t */
  place(t) { return Math.min(MAX_SECONDS, Math.max(0, t)); }

  // Where in time the pointer is over the lanes. With nothing arranged yet, that is the start.
  /** @param {MouseEvent} e */
  timeAt(e) {
    const grid = this.scroll.querySelector('.arr-grid');
    return grid ? this.place((e.clientX - grid.getBoundingClientRect().left - HEAD_W) / this.pps) : 0;
  }
  // The lane under the pointer, or null below the last one.
  /** @param {MouseEvent} e */
  laneAt(e) {
    const rows = [...this.scroll.querySelectorAll('.arr-row[data-li]')];
    const row = rows.find((r) => { const b = r.getBoundingClientRect(); return e.clientY >= b.top && e.clientY < b.bottom; });
    return row ? this.a.lanes[Number(/** @type {HTMLElement} */ (row).dataset.li)] : null;
  }

  /** Pulls a clip that is being moved onto the nearest edge of another one. t is where it would
      start and dur its length; its start or its end catches on the start or end of any other
      clip, in any lane, and on the very beginning, once within a few pixels. Returns the start
      to use and, when it caught, the edge it caught on. */
  /** @param {number} t @param {number} dur @param {Clip | null} [skip] @returns {{ t: number, edge: number | null }} */
  magnet(t, dur, skip = null) {
    const edges = [0];
    for (const { clip } of this.all()) if (clip !== skip) edges.push(clip.at, clip.at + clip.duration);
    /** @type {{ t: number, edge: number } | null} */
    let best = null, reach = MAGNET_PX / this.pps;
    for (const edge of edges) for (const mine of dur > 0 ? [0, dur] : [0]) {
      const d = Math.abs(t + mine - edge);
      if (d < reach && edge - mine >= 0) { reach = d; best = { t: edge - mine, edge }; }
    }
    return best || { t: Math.max(0, t), edge: null };
  }

  // A line across the lanes. While a sound is dragged in from the browser it shows where the
  // sound would start, and the lane it would join is lit (lane is null below the last lane, for
  // a new one). While a clip is moved it is fainter: the edge the clip has caught on.
  /** @param {number | null} t @param {Lane | null} [lane] */
  dropLine(t, lane = undefined) {
    const fromBrowser = lane !== undefined;
    this.scroll.querySelectorAll('.arr-row.target').forEach(r => r.classList.remove('target'));
    const grid = this.scroll.querySelector('.arr-grid');
    this.scroll.classList.toggle('dropping', t != null && fromBrowser && !grid);
    if (!grid) return;
    let line = /** @type {HTMLElement | null} */ (grid.querySelector('.arr-dropline'));
    if (t == null) return void line?.remove();
    if (fromBrowser) (lane ? grid.querySelector(`.arr-row[data-li="${this.a.lanes.indexOf(lane)}"]`) : grid.querySelector('.arr-new'))?.classList.add('target');
    if (!line) { line = /** @type {HTMLElement} */ (this.h.el('div', { class: 'arr-dropline' })); grid.append(line); }
    line.classList.toggle('guide', !fromBrowser);
    line.style.height = `${this.a.lanes.length * LANE_H}px`;
    line.style.transform = `translateX(${HEAD_W + t * this.pps}px)`;
  }

  /** Put a sound from the browser at time t, in a lane or, with none given, in a new lane named after it. */
  /** @param {{ path: string, name: string, duration?: number, sr?: number }} it @param {number} t @param {Lane | null} [lane] */
  async dropSound(it, t, lane = null) {
    const { notice } = this.h;
    if ((it.duration || 0) > MAX_SECONDS) return notice(`${it.name} is longer than ten minutes. Save a shorter piece of it from the editor first.`, 7000);
    if (!lane) {
      if (this.a.lanes.length >= MAX_LANES) return notice(`The Arranger holds ${MAX_LANES} lanes. Drop the sound onto one of them, or remove a lane.`, 6000);
      lane = { name: stem(it.name).slice(0, 60), gain: 0, mute: false, solo: false, clips: [] };
      this.a.lanes.push(lane);
    }
    this.addClip(lane, { path: it.path, name: it.name, duration: it.duration || 0, sr: it.sr || 0, at: this.place(t), in: 0, out: 0 });
  }

  /** @param {Lane} lane @param {Clip} clip */
  addClip(lane, clip) {
    if (lane.clips.length >= MAX_HITS) return this.h.notice(`A lane holds ${MAX_HITS} clips at most.`);
    lane.clips.push(clip);
    this.sel = { lane, clip };
    this.save(); this.render(); this.load(clip);
  }
  /** @param {Lane} lane @param {Clip} clip */
  removeClip(lane, clip) {
    lane.clips = lane.clips.filter(c => c !== clip);
    this.sel = null; this.forget(clip.path);
    this.save(); this.render();
  }
  // The copy lands right after the clip, in the same lane.
  /** @param {Lane} lane @param {Clip} clip */
  duplicate(lane, clip) { this.addClip(lane, { ...clip, at: this.place(clip.at + clip.duration) }); }
  /** @param {Clip} clip @param {number} dir */
  nudge(clip, dir) { clip.at = this.place(clip.at + dir * NUDGE); this.save(); this.render(); }

  // Copy remembers the picked clip; paste places it again where playback starts (the line set
  // by clicking the time ruler), in the lane of the picked clip, or the lane it came from.
  /** @param {boolean} cut */
  copy(cut) {
    const sel = this.sel; if (!sel) return this.h.notice('Click a clip in a lane first, then copy it.');
    this.clip = { clip: { ...sel.clip }, lane: sel.lane };
    if (cut) this.removeClip(sel.lane, sel.clip);
    this.h.toast(cut ? `Cut ${stem(sel.clip.name)}` : `Copied ${stem(sel.clip.name)}`);
  }
  paste() {
    const c = this.clip; if (!c) return this.h.notice('Nothing copied yet.');
    const lane = this.sel?.lane || (this.a.lanes.includes(c.lane) ? c.lane : this.a.lanes[0]);
    if (!lane) return this.dropSound(c.clip, this.cursor).catch(this.h.fail);
    let t = this.cursor;
    while (lane.clips.some(x => x.path === c.clip.path && Math.abs(x.at - t) < 1e-6)) t += c.clip.duration || 0.1;
    this.addClip(lane, { ...c.clip, at: this.place(t) });
  }

  // A clip moves sideways in time and up or down between lanes.
  /** @param {PointerEvent} e @param {Lane} lane @param {Clip} clip @param {HTMLElement} node */
  dragClip(e, lane, clip, node) {
    if (e.button !== 0) return;
    e.preventDefault(); e.stopPropagation();
    // Holding alt leaves a copy behind.
    if (e.altKey && lane.clips.length < MAX_HITS) {
      lane.clips.push({ ...clip });
      const stay = /** @type {HTMLElement} */ (node.cloneNode(true)); stay.classList.remove('on'); node.before(stay);
    }
    this.sel = { lane, clip };
    this.scroll.querySelectorAll('.arr-hit.on').forEach(n => n.classList.remove('on'));
    node.classList.add('on');
    node.setPointerCapture(e.pointerId);
    const x0 = e.clientX, t0 = clip.at, from = this.a.lanes.indexOf(lane);
    const rows = [...this.scroll.querySelectorAll('.arr-row[data-li]')].map(r => r.getBoundingClientRect());
    let moved = e.altKey, to = from;
    this.dragging = true;
    const move = (/** @type {PointerEvent} */ ev) => {
      const dx = ev.clientX - x0;
      if (!moved && Math.abs(dx) < 3 && rows[from] && ev.clientY >= rows[from].top && ev.clientY < rows[from].bottom) return;
      moved = true;
      // The clip catches on the edges of the others. Holding cmd places it freely.
      const snap = ev.metaKey ? { t: Math.max(0, t0 + dx / this.pps), edge: null } : this.magnet(t0 + dx / this.pps, clip.duration, clip);
      clip.at = Math.min(MAX_SECONDS, snap.t);
      const over = rows.findIndex(r => ev.clientY >= r.top && ev.clientY < r.bottom);
      if (over >= 0) to = over;
      this.dropLine(snap.edge);
      node.style.left = `${clip.at * this.pps}px`;
      node.style.transform = `translateY(${(to - from) * LANE_H}px)`;
      this.clock.textContent = fmtTime(clip.at, true);
    };
    const up = () => {
      for (const type of ['pointerup', 'pointercancel', 'lostpointercapture']) node.removeEventListener(type, up);
      node.removeEventListener('pointermove', move);
      this.dragging = false; this.dropLine(null);
      const target = this.a.lanes[to];
      if (target && target !== lane && target.clips.length < MAX_HITS) {
        lane.clips = lane.clips.filter(c => c !== clip);
        target.clips.push(clip);
        this.sel = { lane: target, clip };
      }
      if (moved) this.save();
      this.render();
      // Two clicks on the same clip, without moving it, open its sound in the editor.
      if (moved) { this.lastClick = null; return; }
      const now = performance.now(), again = this.lastClick?.clip === clip && now - this.lastClick.t < DOUBLE_CLICK_MS;
      this.lastClick = again ? null : { clip, t: now };
      if (again && !this.sounds.get(clip.path)?.error) this.h.edit(clip);
    };
    node.addEventListener('pointermove', move);
    for (const type of ['pointerup', 'pointercancel', 'lostpointercapture']) node.addEventListener(type, up);
  }

  // A fade handle sets how long its clip takes to come in, or to go out. The two cannot cross.
  /** @param {PointerEvent} e @param {Clip} clip @param {'in' | 'out'} side @param {HTMLElement} node */
  dragFade(e, clip, side, node) {
    if (e.button !== 0) return;
    e.preventDefault(); e.stopPropagation();
    const handle = /** @type {HTMLElement} */ (e.currentTarget), x0 = e.clientX, f0 = clip[side], other = side === 'in' ? clip.out : clip.in;
    handle.setPointerCapture(e.pointerId);
    this.dragging = true;
    const move = (/** @type {PointerEvent} */ ev) => {
      const dx = (ev.clientX - x0) * (side === 'in' ? 1 : -1);
      clip[side] = Math.min(Math.max(0, clip.duration - other), Math.max(0, f0 + dx / this.pps));
      this.dressFades(node, clip);
      this.clock.textContent = `fade ${side} ${Math.round(clip[side] * 1000)} ms`;
    };
    const up = () => {
      for (const type of ['pointerup', 'pointercancel', 'lostpointercapture']) handle.removeEventListener(type, up);
      handle.removeEventListener('pointermove', move);
      this.dragging = false;
      this.save(); this.render();
      if (this.playing) this.start();
    };
    handle.addEventListener('pointermove', move);
    for (const type of ['pointerup', 'pointercancel', 'lostpointercapture']) handle.addEventListener(type, up);
  }
  // The faded parts of a clip are covered by a wedge, with a handle where each fade ends.
  /** @param {HTMLElement} node @param {Clip} clip */
  dressFades(node, clip) {
    const px = (/** @type {number} */ s) => `${s * this.pps}px`;
    const part = (/** @type {string} */ sel) => /** @type {HTMLElement} */ (node.querySelector(sel));
    part('.arr-fade.in').style.width = px(clip.in);
    part('.arr-fade.out').style.width = px(clip.out);
    part('.arr-fh.in').style.left = px(clip.in);
    part('.arr-fh.out').style.right = px(clip.out);
  }

  // ---- drawing
  render() {
    const { el, icon } = this.h, a = this.a, len = this.length();
    this.loopBtn.classList.toggle('on', a.loop); this.loopBtn.setAttribute('aria-pressed', String(a.loop));
    this.playBtn.disabled = this.saveBtn.disabled = !this.all().length;
    this.clearBtn.disabled = !a.lanes.length;
    this.syncPlay();
    if (!a.lanes.length) {
      this.nodes = null;
      this.scroll.replaceChildren(el('div', { class: 'empty' }, el('h3', {}, 'Nothing to arrange yet'),
        el('p', { class: 'muted' }, 'Drag a sound here from the browser on the left: it gets a lane of its own. Or make a lane first, name it (Drums, Vocal, Synth), and drop sounds into it.'),
        el('div', { class: 'actions' }, el('button', { class: 'ghost', onclick: () => this.addLane().catch(this.h.fail) }, icon('add'), 'Add a lane'))));
      this.head = null; this.showClock(0);
      return;
    }
    if (!this.dragging) this.measure();
    this.nodes = new Map();
    const keep = [this.scroll.scrollLeft, this.scroll.scrollTop];
    this.head = el('div', { class: 'arr-playhead' });
    const grid = el('div', { class: 'arr-grid', style: `width:${HEAD_W + this.trackW}px` },
      this.rulerRow(), a.lanes.map((l, li) => this.laneRow(l, li)), this.newRow(),
      el('div', { class: 'arr-after', style: `left:${HEAD_W + len * this.pps}px` }), this.head);
    this.scroll.replaceChildren(grid);
    [this.scroll.scrollLeft, this.scroll.scrollTop] = keep;
    this.placeHead();
  }

  rulerRow() {
    const { el } = this.h, end = this.trackW / this.pps;
    /** @type {[number, string][]} */
    const marks = [];
    const step = TICKS.find(s => s * this.pps >= 72) || TICKS[TICKS.length - 1];
    for (let t = 0; t < end; t += step) marks.push([t, t < 60 ? `${+t.toFixed(2)}s` : fmtTime(t)]);
    const track = el('div', { class: 'arr-track', 'data-tip': 'Click to choose where playback starts',
      onpointerdown: (/** @type {PointerEvent} */ e) => {
        if (e.button !== 0) return;
        this.cursor = this.place((e.clientX - track.getBoundingClientRect().left) / this.pps);
        if (this.playing) this.start(); else this.placeHead();
      } }, marks.map(([t, label]) => el('span', { class: 'arr-mark', style: `left:${t * this.pps}px` }, label)));
    return el('div', { class: 'arr-row arr-ruler' }, el('div', { class: 'arr-head' }), track);
  }

  /** @param {Lane} lane @param {number} li */
  laneRow(lane, li) {
    const { el, ib, focusable } = this.h;
    const flag = (/** @type {string} */ label, /** @type {'mute' | 'solo'} */ key, /** @type {string} */ tip) => el('button', { class: 'ib toggle' + (lane[key] ? ' on' : ''), 'data-tip': tip, 'aria-label': tip, 'aria-pressed': String(lane[key]),
      onclick: () => { lane[key] = !lane[key]; this.save(); this.render(); } }, this.h.icon(label));
    const gain = el('input', { type: 'range', min: MIN_GAIN_DB, max: MAX_GAIN_DB, step: 0.5, 'aria-label': `Level of ${lane.name}` });
    const tell = () => { gain.dataset.tip = `Level ${this.gainText(lane.gain)}. Double-click to set it back to 0 dB`; gain.setAttribute('aria-valuetext', this.gainText(lane.gain)); };
    gain.value = String(lane.gain); tell();
    gain.oninput = () => { lane.gain = Number(gain.value); tell(); this.clock.textContent = this.gainText(lane.gain); this.save(); };
    gain.onchange = () => this.placeHead();
    gain.ondblclick = () => { gain.value = '0'; gain.dispatchEvent(new Event('input')); };
    const rename = () => this.renameLane(lane).catch(this.h.fail);
    const head = el('div', { class: 'arr-head' },
      flag('volume_off', 'mute', 'Mute (M)'), flag('headphones', 'solo', 'Solo: hear only this lane and the other soloed ones (S)'),
      focusable(el('span', { class: 'qn arr-lname', 'data-tip': 'Double-click to rename the lane', ondblclick: rename }, lane.name), rename),
      gain,
      ib('close', 'Remove this lane. The files stay where they are', () => this.removeLane(lane).catch(this.h.fail)));
    const track = el('div', { class: 'arr-track',
      onpointerdown: (/** @type {PointerEvent} */ e) => { if (!/** @type {Element} */ (e.target).closest('.arr-hit') && this.sel) { this.sel = null; this.scroll.querySelectorAll('.arr-hit.on').forEach(n => n.classList.remove('on')); } } },
      lane.clips.map((clip) => {
        const s = this.sounds.get(clip.path);
        const node = el('div', { class: 'arr-hit' + (this.sel?.clip === clip ? ' on' : '') + (s?.error ? ' missing' : '') + (s && !s.info && !s.error ? ' loading' : ''),
          style: `left:${clip.at * this.pps}px;width:${Math.max(4, clip.duration * this.pps)}px`, 'data-tip': s?.error ? `Not found: ${clip.path}` : null });
        node.innerHTML = s?.svg || '';
        node.append(el('span', { class: 'arr-cname' }, stem(clip.name)),
          el('span', { class: 'arr-fade in' }), el('span', { class: 'arr-fade out' }),
          .../** @type {('in' | 'out')[]} */ (['in', 'out']).map(side => el('span', { class: `arr-fh ${side}`, 'data-tip': `Drag to fade ${side}`, onpointerdown: (/** @type {PointerEvent} */ e) => this.dragFade(e, clip, side, node) })));
        /** @type {HTMLElement} */ (node.querySelector('.arr-fade.in')).style.clipPath = WEDGE_IN; /** @type {HTMLElement} */ (node.querySelector('.arr-fade.out')).style.clipPath = WEDGE_OUT;
        this.dressFades(node, clip);
        this.nodes?.set(clip, node);
        node.onpointerdown = (/** @type {PointerEvent} */ e) => this.dragClip(e, lane, clip, node);
        return node;
      }));
    return el('div', { class: 'arr-row' + (this.shown.heard[li] === false ? ' quiet' : ''), 'data-li': li }, head, track);
  }

  // Under the last lane: the way to make an empty lane, and the place a dragged sound becomes a new one.
  newRow() {
    const { el, icon } = this.h;
    return el('div', { class: 'arr-row arr-new' },
      el('div', { class: 'arr-head' }, el('button', { class: 'ghost', onclick: () => this.addLane().catch(this.h.fail) }, icon('add'), 'Add a lane')),
      el('div', { class: 'arr-track' }));
  }

  /** @param {number} g */
  gainText(g) { return `${g > 0 ? '+' : ''}${g.toFixed(1)} dB`; }
  /** @param {number} pos */
  showClock(pos) { this.clock.textContent = this.all().length ? `${fmtTime(pos)} / ${fmtTime(this.length())}` : ''; }
  placeHead(pos = this.cursor) {
    if (this.head) this.head.style.transform = `translateX(${HEAD_W + pos * this.pps}px)`;
    this.showClock(pos);
  }

  // ---- playback
  // Played natively: save() sends the arrangement whenever it changes.
  syncPlay() { this.playBtn.querySelector('.i').textContent = this.playing ? 'stop' : 'play_arrow'; }

  toggle() { if (this.playing) this.stop(); else this.start(); }

  async start() {
    this.stop();
    const len = this.length();
    if (!len || !this.all().some(x => this.sounds.get(x.clip.path)?.info)) return;
    this.h.stopOthers();
    const from = this.cursor < len ? this.cursor : 0;
    /** @type {Playing} */
    const p = { passes: [[now() + 0.05, from]] };
    this.playing = p;
    this.syncPlay();
    try { await this.push(false); p.passes = [[await invoke('arr_play', { from }), from]]; } catch (e) { if (this.playing === p) this.stop(); return this.h.fail(e); }
    const tick = () => {
      if (this.playing !== p) return;
      if (!this.dragging) this.placeHead(this.position(p, now()));
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }

  // The native side says when a pass begins and when playing ends.
  /** @param {Playing & { playing: boolean }} st */
  passes(st) {
    const p = this.playing; if (!p) return;
    if (!st.playing) return this.stop(false);
    p.passes = st.passes;
  }

  /** @param {Playing} p @param {number} t */
  position(p, t) {
    for (let i = p.passes.length - 1; i >= 0; i--) if (t >= p.passes[i][0]) return t - p.passes[i][0] + p.passes[i][1];
    return p.passes[0]?.[1] ?? 0;
  }

  stop(send = true) {
    const p = this.playing; if (!p) return;
    this.playing = null;
    if (send) invoke('arr_stop').catch(() => { });
    this.syncPlay(); this.placeHead();
  }

  // ---- saving what is heard as one sound
  async bounce() {
    const { modal, notice, toast } = this.h, a = this.a;
    await this.push(false);
    const { clips, first } = this.shown;
    if (!clips) return notice('No lane is sounding, so there is nothing to save.');
    const [folders, projects] = await Promise.all([lib.folders(), lib.projects()]);
    /** @type {[string, string][]} */
    const options = [...projects.filter((/** @type {any} */ p) => p.folder).map((/** @type {any} */ p) => [`p:${p.id}`, `Project: ${p.name}`]), ...folders.filter((/** @type {any} */ f) => !f.offline).map((/** @type {any} */ f) => [`f:${f.path}`, `Library: ${f.path.split('/').pop()}`])];
    if (!options.length) return notice('Add a folder to the Library first, with Add folder on the Library page. The mix is saved there.');
    const v = await modal(`Save the mix of ${plural(clips, 'clip')}`, [{ label: 'Name', value: `${first || 'Arranger'} mix` }, { label: 'Into', options, value: options.some(o => o[0] === this.lastDest) ? this.lastDest : undefined }]);
    if (!v || !v[0]) return;
    this.lastDest = v[1];
    const proj = v[1].startsWith('p:') ? projects.find((/** @type {any} */ p) => `p:${p.id}` === v[1]) : null, dir = proj ? proj.folder : v[1].slice(2);
    const st = this.h.settings();
    this.saveBtn.disabled = true;
    try {
      // Each heard clip is mixed as a sound placed once, at its lane's level, with its own fades.
      await this.push(false);
      const { path, over } = await lib.saveMix({ rate: st.export_rate || 0, bits: st.export_bits || 24, mono: !!st.export_mono }, { dest_dir: dir, name: `${v[0]}.wav`, format: st.export_format || 'wav' });
      if (proj) await lib.projectItem(proj.id, { path });
      const saved = `Saved ${path.split('/').pop()}${proj ? ` into ${proj.name}` : ''}`;
      if (over) notice(`${saved}. The mix was ${over.toFixed(1)} dB over full scale, so it was turned down to fit.`, 8000); else toast(saved);
      this.h.onSave();
    } finally { this.saveBtn.disabled = !this.all().length; }
  }

  /** @param {KeyboardEvent} e */
  handleKey(e) {
    if ((e.metaKey || e.ctrlKey) && !e.altKey) {
      const key = e.key.toLowerCase();
      if (key === 'c' || key === 'x') { e.preventDefault(); this.copy(key === 'x'); }
      else if (key === 'v') { e.preventDefault(); this.paste(); }
      return;
    }
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const k = e.key.toLowerCase(), sel = this.sel;
    if (k === ' ') { e.preventDefault(); this.toggle(); }
    else if (k === 'l') this.set({ loop: !this.a.loop });
    else if (k === 'b') this.showBrowser(this.browser.root.hidden);
    else if (e.key === 'Escape') { if (this.playing) this.stop(); else if (sel) { this.sel = null; this.render(); } }
    else if (e.key === 'Home') { e.preventDefault(); this.cursor = 0; if (this.playing) this.start(); else this.placeHead(); }
    else if (k === '+' || k === '=') this.setZoom(this.zoom * 2);
    else if (k === '-') this.setZoom(this.zoom / 2);
    else if (!sel) return;
    else if (e.key === 'Backspace' || e.key === 'Delete') { e.preventDefault(); this.removeClip(sel.lane, sel.clip); }
    else if (k === 'd') this.duplicate(sel.lane, sel.clip);
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') { e.preventDefault(); this.nudge(sel.clip, e.key === 'ArrowLeft' ? -1 : 1); }
    else if (k === 'm' || k === 's') { const key = k === 'm' ? 'mute' : 'solo'; sel.lane[key] = !sel.lane[key]; this.save(); this.render(); }
  }
}
