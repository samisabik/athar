// The editor page: the waveform or spectrogram of one sound, its selection, regions and playback,
// and every action of its toolbar. The sound itself is held and changed natively (app/src/doc.rs);
// effects and tools that take values open in the panel below (fx.js).

import { lib, doc } from './native.js';
import { fmtTime } from './time.js';
import { now, onEnded, forget } from './engine.js';
import { toolEdit, toolRefusal, fadeSpans } from './tools.js';
import { draw, REGION_COLORS } from './editor-draw.js';
import { bind } from './editor-input.js';

// A tool is auditioned over at most this many seconds, with a short fade at each end.
const AUDITION_S = 8, AUDITION_FADE_S = 0.02, AUDITION_FADE_IN_S = 0.002;

/** @typedef {[number, number]} Span */
/** @typedef {{ a: number, b: number, name: string }} Region */
/** @typedef {import('./tools.js').FadeSpan} FadeSpan */
/** @typedef {Error} Failure What a native command rejects with. */
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
 * What the editor asks of the app around it. `toast` is for passing confirmations and `notice`
 * for what the user must read; `place` names the project (or 'Library') the open file is in.
 * @typedef {{
 *   onSave: (p: RenderOpts, dir: string, name: string, opts?: SaveOpts) => Promise<string | null>,
 *   toast: Notify, notice: Notify,
 *   pickFolder: (title?: string, start?: string) => Promise<string | null>,
 *   ask: (title: string, value: string) => Promise<string | undefined>,
 *   whereToSave: (title?: string) => Promise<SaveTarget | null>,
 *   place: () => string,
 * }} EditorHooks
 */

export class Editor {
  /** @param {HTMLElement} root @param {EditorHooks} hooks */
  constructor(root, { onSave, toast, notice, pickFolder, ask, whereToSave, place }) {
    this.root = root;
    this.onSave = onSave; this.whereToSave = whereToSave; this.place = place;
    this.ask = ask;
    /** The effects panel. It needs the editor to exist, so editing.js sets it right after this constructor. @type {import('./fx.js').FxPanel} */
    this.fx = /** @type {import('./fx.js').FxPanel} */ (/** @type {unknown} */ (null));
    /** The app's settings, as last loaded. @type {Record<string, any> | null} */
    this.settings = null;
    /** Told when an audition is about to start, so that other players stop. @type {(() => void) | null} */
    this.onAudition = null;
    /** @type {((dirty: boolean) => void) | null} */
    this.onDirtyChange = null;
    /** The painted background and waveform, copied in on every frame. @type {HTMLCanvasElement | null} */
    this.layer = null;
    this.toast = toast; this.notice = notice;
    this.pickFolder = pickFolder;
    this.canvas = /** @type {HTMLCanvasElement} */ (root.querySelector('canvas'));
    this.g = /** @type {CanvasRenderingContext2D} */ (this.canvas.getContext('2d'));
    this.mode = 'wave';
    this.version = 0;
    this.undoN = 0; this.redoN = 0;
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
    /** @type {boolean | undefined} */ this.playing;
    /** @type {object | null | undefined} */ this.playToken;
    /** @type {number | null | undefined} */ this.voice;
    /** Whether what plays went out through the live EQ. @type {boolean} */ this.playEq = false;
    /** @type {boolean} */ this.playLoop;
    /** @type {number} */ this.playA;
    /** @type {number} */ this.playStart;
    /** @type {number} */ this.playB;
    /** @type {number} */ this.playT;
    /** @type {boolean | undefined} */ this.changing;
    /** Something was copied or cut; the native side holds it. @type {boolean | undefined} */ this.hasClip;
    /** @type {{ key: string, r: Span | null } | undefined} */ this.trimKept;
    /** @type {number | undefined} */ this.auditionId;
    /** @type {object | null | undefined} */ this.auditionToken;
    /** @type {boolean | undefined} */ this.auditioning;
    /** @type {Audition} */ this.aud;
    /** @type {number | null | undefined} */ this.auditionVoice;
    bind(this);
    new ResizeObserver(() => this.draw()).observe(this.canvas);
  }

  /** @param {string} sel @returns {any} */
  $(sel) { return this.root.querySelector(sel); }
  /** The control holding the value named by data-f: a save option, or a tool's field (see tools.js). @param {string} name @returns {HTMLInputElement | HTMLSelectElement} */
  field(name) { return this.$(`[data-f=${name}]`); }
  /** @param {string} name */
  num(name) { return Number(this.field(name).value); }
  /** @param {string} name @param {number} v */
  setNum(name, v) { this.field(name).value = String(v); }

  // The sound, its history and its regions are held by the native side; the page knows their
  // shape and draws them. `source` is { path } for a library file or { key } for a recorded take.
  /** @param {{ source: { path: string } | { key: string }, name: string, dir?: string | null, tempo?: number | string | null }} o */
  async load({ source, name, dir, tempo = null }) {
    this.stopPlay(); this.stopAudition(); this.shownLen = 0;
    const info = await doc.open(source);
    this.loaded = true; this.name = name; this.dir = dir;
    this.adopt(info);
    this.fileId = (this.fileId || 0) + 1;
    this.field('bpm').value = String(tempo || 120);
    this.hover = null;
    this.root.classList.remove('blank');
    this.fx.fileChanged();
    this.field('name').value = name.replace(/\.[^.]+$/, '') + (this.settings?.edit_suffix ?? '_edit');
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
  // The length of a beat in samples, or 0 while the grid is off.
  beatLen() { const bpm = this.num('bpm'); return this.bars && bpm > 0 ? this.sr * 60 / bpm : 0; }
  // How many beats apart the grid's lines or labels go to stay at least minPx apart: every
  // beat, every second one, every bar, then every 4, 8, 16 bars.
  /** @param {number} beat @param {number} minPx */
  beatStride(beat, minPx) {
    let every = 1;
    while (beat * every / this.spp < minPx) every *= every === 4 ? 4 : 2;
    return every;
  }
  // With the grid on, a position within a few pixels of a quarter beat lands on it.
  /** @param {number} s */
  snap(s) {
    const beat = this.beatLen(); if (!beat) return s;
    const step = beat / 4, near = Math.round(s / step) * step;
    return Math.abs(near - s) / this.spp <= 6 ? Math.round(Math.min(this.length, near)) : s;
  }
  clampView() { this.start = Math.max(0, Math.min(this.start, this.viewLen() - this.width() * this.spp)); }

  // What an edit or a tool works on: the selection, or the whole sound without one.
  /** @returns {Span} */
  range() {
    if (this.sel && this.sel[1] - this.sel[0] > 0) return this.sel;
    return [0, this.length];
  }
  // Names the sound and the range: it changes with every edit, undo and move of the selection.
  rangeKey() { return `${this.version}|${this.range().join('|')}`; }

  draw() { draw(this); }

  // What the waveform shows: the sound, or the result of the open effect or tool ({ src, length }).
  viewSrc() { return this.fx.displayView() || { src: 'doc', length: this.length }; }

  /** Builds the line of chips again, whatever it last showed. */
  refreshInfo() { this.infoSig = null; this.updateInfo(); }
  updateInfo() {
    const sec = (/** @type {number} */ s) => fmtTime(s / this.sr, true);
    const nch = this.nch;
    const outRate = this.num('rate') || this.sr;
    const bits = this.field('bits').value;
    // Everything the line of chips shows; nothing is rebuilt while it stays the same.
    const sig = [this.length, this.sr, nch, outRate, bits, this.version,
      this.sel?.join(), this.cursor, this.undoN, this.redoN, this.isDirty, this.meterVersion].join('|');
    if (sig === this.infoSig) return;
    this.infoSig = sig;
    this.fx.selectionChanged();
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
        try { const m = await doc.analyse({ kind: 'measure' }); if (v === this.version) { this.meter = m; this.meterVersion = v; } }
        catch { return; }
        this.refreshInfo();
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
    const place = this.$('[data-f=place]'), where = this.path ? this.place() : '';
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
    const pv = this.fx.playSrc();
    let src = 'doc', base = 0, end = b;
    if (pv && start >= pv.a && start < pv.b) { src = pv.src; base = pv.a; end = Math.min(b, pv.b); }
    const loop = this.loop, token = this.playToken = {};
    this.playLoop = loop; this.playA = a; this.playStart = start; this.playB = end; this.playT = now() + 0.03; this.playing = true;
    this.syncPlayBtn();
    // The live EQ plays the bands the native side holds, so it gets the current ones first.
    const eq = this.playEq = this.fx.eqLive;
    if (eq) this.fx.syncEq();
    let r;
    try { r = await doc.play({ src, a: Math.max(0, a - base), b: end - base, start: start - base, loop, eq }); }
    catch { if (this.playToken === token) { this.playing = false; this.syncPlayBtn(); this.draw(); } return; }
    if (this.playToken !== token) return void doc.stop(r.id);
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
    if (this.voice != null) { forget(this.voice); if (send) doc.stop(this.voice); this.voice = null; }
    this.playing = false; this.syncPlayBtn();
  }
  toggle() {
    if (this.auditioning) { this.stopAudition(); return; }
    if (!this.playing && this.fx.showsOtherLength()) return void this.audition(this.fx.toolName);
    this.playing ? this.stopPlay() : this.play(); this.draw();
  }
  syncPlayBtn() { const b = this.$('[data-a=play] .i'); if (b) b.textContent = this.playing ? 'stop' : 'play_arrow'; }

  // ---- editing
  // Every change is made natively, history included; the page sends its selection and cursor
  // along and shows what comes back.
  refit() { if (this.spp * this.width() > this.viewLen()) this.fit(); else { this.clampView(); this.draw(); } }
  // One change at a time: `send` makes it natively, given the selection and cursor, and resolves
  // with the new state. Resolves null when nothing changed, the reason shown as a notice.
  /** @param {(ui: import('./native.js').DocUi) => Promise<EdState & Record<string, any>>} send */
  async change(send) {
    if (!this.loaded || this.changing) return null;
    this.changing = true;
    try {
      const res = await send(this.ui());
      this.adopt(res);
      this.refit();
      return res;
    } catch (e) { this.notice(/** @type {Failure} */ (e).message); return null; }
    finally { this.changing = false; }
  }
  // An edit of the sound. Its joins are smoothed over a few milliseconds unless the settings say not to.
  /** @param {Record<string, unknown>} p */
  edit(p) {
    this.stopPlay();
    const smooth_ms = this.settings?.smooth_edits === false ? 0 : (this.settings?.edit_fade_ms ?? 3);
    return this.change((ui) => doc.edit({ smooth_ms, ...p }, ui));
  }
  /** @param {Record<string, unknown>} p */
  async editRegions(p) {
    const hadSel = !!this.sel;
    const r = await this.change((ui) => doc.regions(p, ui));
    if (r && hadSel && !this.sel && this.playing) this.play(true);
    return r;
  }
  doUndo() { this.stopPlay(); return this.change((ui) => doc.undo(false, ui)); }
  doRedo() { this.stopPlay(); return this.change((ui) => doc.undo(true, ui)); }

  // Everything a button, a menu item or a key can ask for, by name. For a tool this is its
  // Apply: it resolves with something truthy once the tool has run, and with nothing when it
  // refused or failed.
  /** @param {string} name @returns {Promise<unknown>} */
  async action(name) {
    const sel = this.sel && this.sel[1] > this.sel[0];
    const needSel = () => { if (!sel) { this.notice('Select a region first'); return false; } return true; };
    const whole = () => sel && /** @type {Span} */ (this.sel)[0] === 0 && /** @type {Span} */ (this.sel)[1] === this.length;
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
          const v = Number(await this.ask('Tempo in beats per minute', this.field('bpm').value));
          if (!(v >= 20 && v <= 400)) return;
          this.setNum('bpm', v); this.bars = true;
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
        try { await doc.edit({ kind: 'copy', a, b }); } catch (e) { return this.notice(/** @type {Failure} */ (e).message); }
        this.hasClip = true; return this.toast('Copied');
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
      case 'fade': case 'normalize': case 'gain': case 'speed': case 'tune': case 'loudnorm': case 'stretch': case 'loopxfade': case 'declick':
        return this.applyTool(name);
      case 'reverse': return this.edit({ kind: 'reverse', a, b });
      case 'mono': if (this.nch < 2) return; return this.edit({ kind: 'mono' });
      case 'transients': return this.editRegions({ op: 'hits', a, b, sensitivity: this.num('onsetsens') || 5, th: this.threshold() });
      case 'gridslice': {
        const bpm = this.num('bpm'), per = this.num('perbeat') || 1;
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
        db: Number(this.settings?.prep_target_db ?? this.num('norm')),
        fade_in_ms: this.settings?.prep_fade_in_ms ?? 2, fade_out_ms: this.settings?.prep_fade_out_ms ?? 10,
      });
      case 'addregion': if (!needSel()) return;
        return this.editRegions({ op: 'add', a: /** @type {Span} */ (this.sel)[0], b: /** @type {Span} */ (this.sel)[1] });
      case 'autoslice': return this.editRegions({ op: 'auto', th: this.threshold(), gap_ms: this.num('gap') || 250 });
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
      case 'save': return this.save();
      case 'download':
        return doc.download(this.renderOpts(), this.fileName('wav'))
          .then((path) => { if (path) this.toast(`Saved ${path.split('/').pop()}`); }, (/** @type {Failure} */ e) => this.notice(e.message));
    }
  }

  // Apply, for the tools whose edit toolEdit builds. Resolves with something truthy once the
  // sound is as the tool's values ask, which includes values that ask for no change; with null
  // when the tool refused or its edit failed.
  /** @param {string} name */
  async applyTool(name) {
    const num = (/** @type {string} */ f) => this.num(f);
    const refusal = toolRefusal(name, num, this.range(), this.sr);
    if (refusal) this.notice(refusal);
    if (refusal != null) return refusal === '' ? true : null;
    const p = this.toolEdit(name);
    if (!p) return null;
    const r = await this.edit(p);
    if (!r) return null;
    if (name === 'loudnorm' && r.over > 0) this.notice(`At ${p.target} LUFS this sound peaks ${r.over.toFixed(1)} dB over full scale. Use the Maximizer, or a lower target.`, 8000);
    if (name === 'loopxfade' && !this.loop) this.action('loop');
    if (name === 'declick' && r.info) this.notice(r.info[0].toUpperCase() + r.info.slice(1));
    return r;
  }

  rangeMs() { const [a, b] = this.range(); return Math.round((b - a) / this.sr * 1000); }
  // The fade in and the fade out of the Fade tool, over the range (see fadeSpans in tools.js).
  fadeSpans() { return fadeSpans((f) => this.num(f), this.range(), this.sr); }
  // The level, as an amplitude, below which the slicers and Prepare take the sound for silence.
  threshold() { return Math.pow(10, Number(this.field('thresh').value || -45) / 20); }
  // The level Trim silent ends cuts below. Its ruler starts at the bottom, which cuts nothing.
  trimThreshold() { const db = this.num('trimthresh'); return db <= -96 ? 0 : Math.pow(10, db / 20); }
  // What Trim silent ends would keep at its current level, worked out once per sound and level.
  // Null until it is known; the waveform redraws when it arrives.
  /** @returns {Span | null} */
  trimBounds() {
    const key = `${this.version}|${this.field('trimthresh').value}`;
    if (this.trimKept?.key !== key) {
      this.trimKept = { key, r: null };
      this.soundBounds(this.trimThreshold()).then((r) => { const kept = /** @type {{ key: string, r: Span | null }} */ (this.trimKept); if (kept.key === key) { kept.r = r; this.draw(); } }).catch(() => { });
    }
    return this.trimKept.r;
  }
  // The span from the first to the last sample above the threshold, padded by 5 ms; null when all is below.
  /** @returns {Promise<Span | null>} */
  soundBounds(th = this.threshold()) { return doc.analyse({ kind: 'bounds', th }); }
  // For each position, the nearest point within 10 ms where the sum of all channels crosses zero.
  /** @param {number[]} at @returns {Promise<number[]>} */
  zeroCross(at) { return doc.analyse({ kind: 'zerocross', at }); }
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
    const target = await this.whereToSave('Save each region as a new file'); if (!target) return;
    const dir = target.dir;
    const suffix = this.settings?.edit_suffix ?? '_edit';
    let base = this.field('name').value || 'sample';
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
    return { a, b, rate: rate || this.num('rate') || this.sr, bits: bits || this.num('bits'), mono: !!mono };
  }

  format() { return this.field('format').value || 'wav'; }
  fileName(ext = this.format()) { return (this.field('name').value || 'untitled') + '.' + ext; }
  // Where a new file goes is asked every time, since a save never replaces a file.
  async save() {
    const target = await this.whereToSave(); if (!target) return;
    // An edit made, or another file opened, while the save runs must not be marked as saved.
    const file = this.fileId, version = this.version, from = this.path && this.name;
    const res = await this.onSave(this.renderOpts(), target.dir, this.fileName(), { format: this.format(), source: this.path, project: target.project });
    if (!res) return;
    const name = /** @type {string} */ (res.split('/').pop());
    this.notice(`Saved ${name} into ${target.where}.${from ? ` ${from} is unchanged.` : ''}`, 7000);
    if (this.fileId === file) {
      // The editor now shows the saved file: later region changes and saves belong to it.
      this.dir = res.slice(0, res.lastIndexOf('/'));
      const st = await doc.saved(res, version); this.isDirty = st.dirty; this.path = st.path;
      this.name = name; this.$('[data-f=title]').textContent = this.name;
      this.refreshInfo();
    }
  }

  // The edit the tool `name` makes over the range with its current values; null when they make none.
  /** @param {string} name */
  toolEdit(name) { return toolEdit(name, (f) => this.num(f), this.range(), this.sr); }
  // The whole sound as it would be after the tool, for drawing and for Play while it is open:
  // { src, length }, or null.
  /** @param {string} name @param {string} key */
  async toolFull(name, key) {
    const p = this.toolEdit(name);
    if (!p) return null;
    const r = await doc.view(key, p);
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
      await doc.view(key, p);
      return { src: key, a: from, b: to, from };
    }
    if (name === 'loopxfade') {
      const r = await doc.view(key, p);
      return { src: key, a, b: a + (r.n ?? 0), from: a };
    }
    const r = await doc.view(key, p, AUDITION_S);
    return { src: key, a: 0, b: r.length, from: a };
  }
  /** Plays what the open tool `name` would make; nothing when that tool is not the one open. @param {string | null} name */
  async audition(name) {
    if (!name || this.fx.toolName !== name) return;
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
    } catch (e) { this.notice(/** @type {Failure} */ (e).message); }
    if (this.fx.toolName !== name || id !== this.auditionId) return;
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
    try { r = await doc.play({ src: seg.src, a: seg.a, b: seg.b, start: seg.a + off, loop, fade_in: AUDITION_FADE_IN_S, fade_out: AUDITION_FADE_S }); }
    catch { if (this.auditionToken === token) { this.auditioning = false; this.draw(); } return; }
    if (this.auditionToken !== token) return void doc.stop(r.id);
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
    if (v != null) { forget(v); if (send) doc.stop(v, AUDITION_FADE_S); }
  }
  // What is played to judge a fade: the fade last adjusted, with a second of the sound beside it.
  fadeExcerpt() {
    const [fin, fout] = this.fadeSpans(), most = Math.round(this.sr * AUDITION_S), [a, b] = this.range();
    const out = this.fx.lastField === 1 || fin.b <= fin.a;
    return out ? [Math.max(a, fout.a - this.sr, b - most), b] : [a, Math.min(b, fin.b + this.sr, a + most)];
  }
  // The inner edge of each fade can be dragged on the waveform.
  /** @param {number} x */
  fadeEdgeAt(x) {
    if (!this.loaded) return null;
    return (this.fx.toolMark() || []).find((m) => Math.abs(this.sampleToX(m.fadeIn ? m.b : m.a) - x) <= 6) || null;
  }
  /** @param {number} s @param {boolean} fadeIn */
  dragFade(s, fadeIn) {
    const [a, b] = this.range();
    const ms = Math.round((fadeIn ? s - a : b - s) / this.sr * 1000);
    this.setNum(fadeIn ? 'fadeinlen' : 'fadeoutlen', Math.max(0, Math.min(this.rangeMs(), ms)));
    this.fx.lastField = fadeIn ? 0 : 1;
    this.fx.toolSync?.();
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
      'r': !mod && 'addregion', 's': mod && 'save',
      'Home': 'home', 'End': 'end', 'Escape': this.sel && 'deselect',
    };
    const act = map[e.key.length === 1 ? e.key.toLowerCase() : e.key];
    if (!act) return false;
    e.preventDefault(); this.action(act); return true;
  }
}
