// The panel under the editor's waveform: the EQ, the effects, and the tools that take values.
// It holds their settings, shows and plays their result before it is applied, and applies it.

import { TOOLS, toolSpec } from './tools.js';
import { listen } from './engine.js';
import { doc, effects } from './native.js';
import { buildEq, drawEq, bandChanged, renderBandControls } from './fx-eq.js';
import { buildComp, buildMax, buildNr, buildSat, buildShape, buildLofi, drawSide } from './fx-effects.js';
import { buildTool } from './fx-tools.js';

/** @typedef {import('./editor.js').Editor} Editor */
/** @typedef {import('./tools.js').ToolSpec} ToolSpec */
/** @typedef {{ type: string, f: number, g: number, q: number, on: boolean }} Band */
/** @typedef {import('./fx-controls.js').Params} Params */
/** @typedef {{ src: string, length: number }} View */

/** @typedef {Error} Failure What a native command rejects with. */




const PREVIEW_SECS = 30;
/** @type {Record<string, string>} */
const TITLES = { eq: 'Parametric EQ', comp: 'Compressor', max: 'Maximizer', sat: 'Saturation', shape: 'Shape', lofi: 'Lo-fi', nr: 'Noise reduction' };
// The same names as they read in a question ("Apply EQ?").
/** @type {Record<string, string>} */
const LABELS = { ...TITLES, eq: 'EQ' };
// How long a control rests before its result is played.
const LISTEN_WAIT_MS = 220;
// Above this length the waveform no longer shows an effect's result; a preview of the first
// PREVIEW_SECS is processed for listening instead.
const LONG_S = 60;
// Tools whose result is as long as the original: the waveform and Play show their result while open.
/** @type {Set<string | null>} */
const SAME_LENGTH = new Set(['fade', 'gain', 'normalize', 'loudnorm', 'declick']);
// Tools whose result is longer or shorter: the waveform shows the result, and Play auditions it.
/** @type {Set<string | null>} */
const RESIZING = new Set(['speed', 'tune', 'stretch', 'loopxfade']);
// Tools that take long enough to work out that their preview waits for the control to be let go.
/** @type {Set<string | null>} */
const SLOW = new Set(['declick', 'stretch']);
const DEFAULTS = {
  comp: { threshold: -24, ratio: 4, knee: 6, attack: 10, release: 150, makeup: 0, autoMakeup: true },
  max: { gain: 6, ceiling: -1, release: 60 },
  nr: { reduction: 12, sensitivity: 1.5 },
  sat: { drive: 6, ceiling: -1, mix: 100 },
  shape: { attack: 30, tail: 0 },
  lofi: { bits: 12, rate: 26 },
};

export class FxPanel {
  /**
   * @param {HTMLElement} root @param {Editor} editor
   * @param {(msg: string) => void} toast @param {(msg: string) => void} notice
   */
  constructor(root, editor, toast, notice) {
    this.root = root; this.ed = editor; this.toast = toast; this.notice = notice;
    /** Asks which of two ways to go: resolves 'ok', 'alt' or something else for neither. @type {((title: string, text: string, ok: string, alt: string) => Promise<string | null>) | null} */
    this.choose = null;
    /** Told which section is open, for the cat's lessons. @type {((name: string | null) => void) | null} */
    this.onSection = null;

    // ---- the open section
    /** An effect's name, 'tool', or null while the panel is closed. @type {string | null} */
    this.tab = null;
    /** The open tool, while the section is 'tool'. @type {string | null} */
    this.toolName = null;
    /** On Original: the untouched sound is shown and heard. */
    this.bypass = false;
    /** A control was moved, and its change is neither applied nor discarded yet. */
    this.touched = false;
    /** The question about a pending change is on screen. */
    this.asking = false;
    /** Apply is running. */
    this.busy = false;
    /** A control of the panel is held down. */
    this.dragging = false;
    /** Ends everything the open section's controls set going; replaced on every build. */
    this.buildAbort = new AbortController();

    // ---- effect settings
    /** @type {Band[]} */
    this.bands = [];
    this.selBand = -1;
    this.comp = { ...DEFAULTS.comp }; this.max = { ...DEFAULTS.max }; this.nr = { ...DEFAULTS.nr };
    this.sat = { ...DEFAULTS.sat }; this.shape = { ...DEFAULTS.shape }; this.lofi = { ...DEFAULTS.lofi };
    /** The captured noise print, and a count that changes with every capture. @type {{ len: number } | null} */
    this.noise = null; this.noiseId = 0;

    // ---- tool state
    /** The open tool's values as it opened, for Reset. @type {string[]} */
    this.startValues = [];
    /** Which of the open tool's rulers moved last; the Fade audition plays that end. */
    this.lastField = 0;
    /** The sound and range the open tool's controls were built for (see Editor.rangeKey). @type {string | null} */
    this.toolRangeKey = null;
    /** Rebuilds Tune once the selection rests. @type {ReturnType<typeof setTimeout> | undefined} */
    this.tuneTimer = undefined;
    /** Tune is setting its own first value, which is not a change made by hand. */
    this.settingUp = false;
    /** The snapping Tune was last set to. @type {{ scale: string, key: number } | null} */
    this.tuneScale = null;
    /** Rewrites the open tool's line of text. @type {(() => void) | null} */
    this.toolHint = null;
    /** Brings the open tool's rulers in step with values the editor changed itself. @type {(() => void) | null} */
    this.toolSync = null;

    // ---- playing the result once a control rests
    /** A result is waiting to be played. */
    this.wantPlay = false;
    /** @type {ReturnType<typeof setTimeout> | undefined} */
    this.kickTimer = undefined;
    /** The timer above has yet to fire. */
    this.kickDue = false;

    // ---- the result shown on the waveform, worked out in the background
    /** The latest result: the view key it was made for, the sound, and the tool it belongs to. @type {{ key: string, out: View, name: string | null } | null} */
    this.view = null;
    /** The view key being worked out, or that failed. @type {string | null} */
    this.viewPending = null;
    /** @type {ReturnType<typeof setTimeout> | undefined} */
    this.viewTimer = undefined;
    /** A result is being worked out natively; the next one waits. */
    this.viewBusy = false;
    /** The view key just applied: the sound itself already shows it. @type {string | null} */
    this.quietKey = null;

    // ---- the preview of a long sound, for listening
    /** @type {{ a: number, b: number, v: number, key: string, src: string } | null} */
    this.pv = null;
    /** @type {ReturnType<typeof setTimeout> | undefined} */
    this.pvTimer = undefined;
    /** The file the "preview covers the first seconds" message was last shown for. @type {number | undefined} */
    this.toldShort = undefined;

    // ---- graphs
    /** @type {HTMLCanvasElement | null} */
    this.eqCanvas = null;
    /** The selected band's controls. @type {HTMLElement | null} */
    this.bandBox = null;
    /** The graph beside an effect's rulers. @type {HTMLCanvasElement | null} */
    this.sideCanvas = null;
    /** The EQ curves last received, and the key of those asked for. @type {{ key: string, data: number[][] } | null} */
    this.curves = null;
    /** @type {string | null} */
    this.curvesWant = null;
    /** The transfer curve last received, and the key of the one asked for. @type {{ key: string, out: number[] } | null} */
    this.sideCurve = null;
    /** @type {string | null} */
    this.sideWant = null;
    /** The noise profile drawn in Noise reduction; null inside until it arrives. @type {{ key: string, profile: number[] | null } | null} */
    this.autoProf = null;
    // The spectrum of what plays, in dB per bin, sent while the EQ is open. Each report moves
    // the picture a fifth of the way to the new values, so it does not flicker.
    this.spectrum = new Float32Array(2048).fill(-120);
    listen('spectrum', (/** @type {number[]} */ d) => {
      const out = this.spectrum;
      for (let i = 0; i < d.length && i < out.length; i++) out[i] = 0.8 * out[i] + 0.2 * d[i];
    });

    this.body = /** @type {HTMLElement} */ (root.querySelector('.fx-body'));
    this.applyBtn = /** @type {HTMLButtonElement} */ (root.querySelector('[data-x=apply]'));
    /** @type {Element} */ (root.querySelector('.fx-tabs')).addEventListener('click', (e) => {
      const b = /** @type {Element} */ (e.target).closest('[data-fx]'); if (b) this.open(/** @type {string} */ (b.dataset.fx));
    });
    this.toolTabs = /** @type {HTMLElement} */ (root.querySelector('[data-x=tooltabs]'));
    this.toolTabs.addEventListener('click', (e) => {
      const act = /** @type {HTMLElement | null} */ (/** @type {Element} */ (e.target).closest('[data-act]'));
      if (act) return void this.ed.action(/** @type {string} */ (act.dataset.act));
      const b = /** @type {Element} */ (e.target).closest('[data-tool]'); if (b && b.dataset.tool !== this.toolName) this.openTool(/** @type {string} */ (b.dataset.tool));
    });
    /** @type {Element} */ (root.querySelector('[data-x=ab]')).addEventListener('click', (e) => {
      const b = /** @type {Element} */ (e.target).closest('[data-ab]'); if (b) this.setBypass(b.dataset.ab === 'a');
    });
    this.applyBtn.onclick = () => this.apply();
    /** @type {Element} */ (root.querySelector('[data-x=close]')).onclick = () => this.close();
    /** @type {Element} */ (root.querySelector('[data-x=reset]')).onclick = () => this.reset();
    /** @type {Element} */ (root.querySelector('[data-x=discard]')).onclick = () => { this.touched = false; this.close(); };
    // Heavy recomputes wait while a control is held, so dragging a slider stays smooth.
    this.body.addEventListener('pointerdown', () => { this.dragging = true; });
    for (const t of ['pointerup', 'pointercancel', 'blur']) window.addEventListener(t, () => { this.dragging = false; });
    this.dressAb();
  }

  // An effect that is worked out before it is heard; the EQ runs live, and tools go their own way.
  get offline() { return this.tab && this.tab !== 'eq' && this.tab !== 'tool'; }
  /** The open tool's description; only asked while a tool is open. @returns {ToolSpec} */
  get spec() { return /** @type {ToolSpec} */ (toolSpec(this.toolName)); }
  /** The open tool's values as text, in the order of its fields. */
  toolValues() { return this.spec.fields.map(f => this.ed.field(f.f).value); }
  /** @param {string | null} tab @returns {Params} */
  params(tab) { return /** @type {Record<string, Params>} */ ({ comp: this.comp, max: this.max, nr: this.nr, sat: this.sat, shape: this.shape, lofi: this.lofi })[/** @type {string} */ (tab)]; }

  // Every section, effect or tool, has the same frame: its name, the Original / Edited switch,
  // and Apply and Discard under the controls. The menu item of whatever is open is marked.
  dress() {
    const tool = this.tab === 'tool', q = (/** @type {string} */ s) => /** @type {Element} */ (this.root.querySelector(s));
    q('.fx-tabs').hidden = tool;
    this.toolTabs.hidden = !tool;
    if (tool) this.dressToolTabs();
    q('[data-x=ab]').hidden = tool && !this.spec.audible;
    q('.fx-title').textContent = tool ? this.spec.title : (TITLES[/** @type {string} */ (this.tab)] || '');
    this.dressAb(); this.dressBar();
    this.onSection?.(this.section());
    this.root.querySelectorAll('[data-fx]').forEach(b => b.classList.toggle('on', b.dataset.fx === this.tab));
    document.querySelectorAll('[data-fx-open]').forEach(b => b.classList.toggle('on', b.dataset.fxOpen === this.tab));
    document.querySelectorAll('#editor [data-a]').forEach((b) => { if (toolSpec(b.dataset.a)) b.classList.toggle('on', tool && b.dataset.a === this.toolName); });
  }

  // The tools that share a menu (or the slicer row) with the open one, as icon tabs.
  dressToolTabs() {
    const item = document.querySelector(`#editor .tmenu-list [data-a=${this.toolName}], #editor .regions-row [data-a=${this.toolName}]`);
    // Reverse has no settings, so its tab runs it at once.
    const group = item ? [.../** @type {HTMLElement} */ (item.parentElement).querySelectorAll('[data-a]')].filter(b => toolSpec(b.dataset.a) || b.dataset.a === 'reverse') : [];
    this.toolTabs.hidden = group.length < 2;
    this.toolTabs.replaceChildren(...group.map((b) => {
      const name = /** @type {string} */ (b.dataset.a), t = document.createElement('button'), title = toolSpec(name)?.title || 'Reverse';
      t.type = 'button'; t.className = 'ib' + (name === this.toolName ? ' on' : '');
      if (toolSpec(name)) t.dataset.tool = name; else t.dataset.act = name;
      t.dataset.tip = title; t.setAttribute('aria-label', title);
      t.innerHTML = `<span class="i">${/** @type {Element} */ (b.querySelector('.i')).textContent}</span>`;
      return t;
    }));
  }

  label() { return this.tab === 'tool' ? this.spec.title : LABELS[/** @type {string} */ (this.tab)]; }
  // The name of the open section, for the cat's lessons.
  section() { return this.root.hidden ? null : this.tab === 'tool' ? this.toolName : this.tab; }
  long() { return this.ed.length > this.ed.sr * LONG_S; }

  dressAb() {
    this.root.querySelectorAll('[data-ab]').forEach((b) => {
      const on = (b.dataset.ab === 'a') === this.bypass;
      b.classList.toggle('on', on); b.setAttribute('aria-pressed', String(on));
    });
  }

  // Discard shows once there is a change to throw away.
  dressBar() {
    /** @type {Element} */ (this.root.querySelector('[data-x=discard]')).hidden = !this.touched;
  }

  // A control was moved by hand: the change is pending, the switch goes to Edited, and the
  // result is played once the control comes to rest.
  touch() {
    this.touched = true;
    if (this.bypass) { this.bypass = false; this.dressAb(); }
    this.dressBar();
    this.wantPlay = true;
    this.kickSoon();
  }
  kickSoon(ms = LISTEN_WAIT_MS) {
    clearTimeout(this.kickTimer);
    this.kickDue = true;
    this.kickTimer = setTimeout(() => { this.kickDue = false; this.kick(); }, ms);
  }
  // Plays the result of the current values. With the loop running it changes in place;
  // otherwise it plays once.
  kick() {
    const e = this.ed, tab = this.tab, name = this.toolName;
    if (!this.wantPlay || !tab || !e.loaded) return;
    const inPlace = e.playing && e.loop;
    const done = () => { this.wantPlay = false; };
    if (tab === 'tool') {
      if (!this.spec.audible) return done();
      // A tool is heard as a short excerpt around what it changes, unless the loop is on.
      if (!(e.loop && SAME_LENGTH.has(name) && !this.long())) { done(); return void e.audition(/** @type {string} */ (name)); }
    }
    // A loop started on the Original has no EQ in its path; it restarts in place to get one.
    if (tab === 'eq') { done(); if (e.playing ? e.playEq === this.bypass : (this.bands.length || this.bypass)) e.play(!!e.playing); return; }
    if (this.bypass || this.viewKey() === this.quietKey) { done(); return void (inPlace ? e.play(true) : e.play()); }
    if (tab !== 'tool' && this.long()) { done(); return this.schedulePreview(0, !inPlace); }
    // The result is still being worked out: it calls back here when it is ready.
    if (this.view?.key !== this.viewKey()) return void e.draw();
    done();
    inPlace ? e.play(true) : e.play();
  }

  // Leaving a section whose values were changed but not applied asks what to do with them.
  async canLeave() {
    if (!this.touched || !this.tab || !this.ed.loaded || !this.choose) { this.touched = false; return true; }
    if (this.asking) return false;
    this.asking = true;
    let pick;
    try { pick = await this.choose(`Apply ${this.label()}?`, 'Its settings are not in the sound yet. Apply keeps them, Discard leaves the sound as it was.', 'Apply', 'Discard'); }
    finally { this.asking = false; }
    if (pick === 'ok') { await this.apply(); return !this.touched; }
    if (pick === 'alt') { this.touched = false; return true; }
    return false;
  }

  // After a section opens or the switch is flicked: playback follows, and a sound at rest plays once.
  follow() {
    const e = this.ed;
    if (e.playing) e.play(true);
    else if (this.tab) { this.wantPlay = true; this.kickSoon(0); }
    e.draw();
  }

  // A section opens on the Original: nothing is worked out or heard until a control moves.
  opened() {
    const e = this.ed;
    if (e.playing) e.play(true);
    e.draw();
  }

  /** @param {string} name */
  async openTool(name) {
    if (this.tab === 'tool' && this.toolName === name && !this.root.hidden) return this.close();
    if (!await this.canLeave()) return;
    this.clearPreview(); this.clearView();
    this.ed.stopAudition();
    this.bypass = true;
    this.tab = 'tool'; this.toolName = name; this.root.hidden = false;
    this.watchSpectrum();
    // Trim silent ends opens on the level that cuts nothing, and Fade on no fade at all.
    if (name === 'trimsilence') this.ed.setNum('trimthresh', -96);
    if (name === 'fade') { this.clearFades(); this.lastField = 0; }
    this.startValues = this.toolValues();
    this.dress();
    this.build();
    this.opened();
  }

  // Puts every control of the open section back to where it started.
  reset() {
    const tab = this.tab; if (!tab) return;
    if (tab === 'eq') this.clearBands();
    else if (tab === 'tool') this.spec.fields.forEach((f, i) => { this.ed.field(f.f).value = this.startValues[i]; });
    else Object.assign(this.params(tab), /** @type {Record<string, Params>} */ (DEFAULTS)[tab]);
    this.build();
    this.touch();
    this.ed.draw();
  }

  clearFades() { this.ed.setNum('fadeinlen', 0); this.ed.setNum('fadeoutlen', 0); }

  // The parts of the sound the open Fade tool will change, for the editor to draw and to drag.
  toolMark() {
    return this.tab === 'tool' && this.toolName === 'fade' && (!this.bypass || !this.touched) ? this.ed.fadeSpans() : null;
  }

  // The editor calls this when the selection or cursor moves.
  selectionChanged() {
    if (this.tab !== 'tool') return;
    const moved = this.toolRangeKey !== this.ed.rangeKey();
    // Tune reads the pitch of the range: a new range, or an undo, needs a new reading.
    if (this.toolName === 'tune' && moved) {
      clearTimeout(this.tuneTimer);
      this.tuneTimer = setTimeout(() => { if (this.tab === 'tool' && this.toolName === 'tune') this.build(); }, 250);
      return;
    }
    // The fade rulers run to the length of the range: another range needs other rulers.
    if (this.toolName === 'fade' && moved) return void this.build();
    this.toolHint?.();
  }

  /** @param {string} tab */
  async open(tab) {
    if (this.tab === tab && !this.root.hidden) return this.close();
    if (!await this.canLeave()) return;
    this.clearPreview();
    this.bypass = true;
    this.ed.stopAudition();
    this.tab = tab; this.toolName = null; this.root.hidden = false;
    this.watchSpectrum();
    this.dress();
    this.build();
    this.opened();
  }

  build() {
    this.buildAbort.abort();
    this.buildAbort = new AbortController();
    this.body.innerHTML = '';
    this.eqCanvas = this.bandBox = this.sideCanvas = null;
    /** @type {Record<string, () => void>} */ ({ eq: () => buildEq(this), comp: () => buildComp(this), max: () => buildMax(this), nr: () => buildNr(this), sat: () => buildSat(this), shape: () => buildShape(this), lofi: () => buildLofi(this), tool: () => buildTool(this) })[/** @type {string} */ (this.tab)]();
  }

  /** @param {Element} el @param {ResizeObserverCallback} fn */
  observe(el, fn) {
    const ro = new ResizeObserver(fn);
    ro.observe(el);
    this.buildAbort.signal.addEventListener('abort', () => ro.disconnect());
  }

  async close() {
    if (!await this.canLeave()) return;
    this.clearPreview();
    this.clearView();
    this.buildAbort.abort();
    clearTimeout(this.kickTimer); this.wantPlay = false;
    this.ed.stopAudition();
    this.tab = null; this.toolName = null; this.root.hidden = true;
    this.watchSpectrum();
    this.bypass = false;
    this.dress();
    if (this.ed.playing) this.ed.play(true);
    this.ed.draw();
  }

  // Called by the editor after it loads another file.
  fileChanged() {
    this.clearPreview();
    this.clearView();
    this.autoProf = null;
    // The native side forgets its noise print when it opens a sound.
    this.noise = null;
    this.touched = false; this.wantPlay = false;
    this.bypass = true; this.dressAb();
    // Another sound starts every effect and tool from its initial settings. The tempo is the
    // sound's own, set by the editor as it loads.
    this.clearBands();
    for (const k in DEFAULTS) Object.assign(this.params(k), /** @type {Record<string, Params>} */ (DEFAULTS)[k]);
    for (const t of Object.values(TOOLS)) for (const f of t.fields) {
      const inp = this.ed.field(f.f);
      if (f.f !== 'bpm' && inp instanceof HTMLInputElement) inp.value = inp.defaultValue;
    }
    if (this.tab === 'tool') this.startValues = this.toolValues();
    if (this.tab) this.build();
    this.dressBar();
  }

  // The Original / Edited switch. The controls stay where they are: only what is heard, and the
  // waveform, change sides.
  /** @param {boolean} v */
  setBypass(v) {
    if (!this.tab) return;
    this.bypass = v;
    this.dressAb();
    this.follow();
  }
  toggleAb() { if (this.tab && !/** @type {Element} */ (this.root.querySelector('[data-x=ab]')).hidden) this.setBypass(!this.bypass); }

  // Whether what plays goes through the live EQ. Its bands are sent when they change, and by
  // the editor before playback starts.
  get eqLive() { return this.tab === 'eq' && !this.bypass; }
  syncEq() { doc.eq(this.bands).catch(() => { }); }
  // Applied, reset or another sound: the EQ starts again from no bands, live as well.
  clearBands() {
    this.bands = []; this.selBand = -1;
    if (this.eqLive) this.syncEq();
  }

  previewRange() {
    const e = this.ed, [a, b] = e.range();
    return [a, b, Math.min(b, a + e.sr * PREVIEW_SECS)];
  }

  // The preview to play ({ src, a, b }: where it sits in the sound), only when it was made
  // from the current sound and range. A stale preview is dropped and rebuilt for the new range.
  playSrc() {
    // The EQ runs live on the playing sound.
    if (!this.tab || this.bypass || this.tab === 'eq') return null;
    // Play uses the same sound the waveform shows while a section is open.
    const v = this.displayView();
    if (v && v.length === this.ed.length) return { src: v.src, a: 0, b: v.length };
    if (this.tab === 'tool' || !this.long()) return null;
    // A long sound is not redrawn; its first seconds are processed for listening.
    const [a, , end] = this.previewRange(), pv = this.pv;
    if (pv && pv.v === this.ed.version && pv.a === a && pv.b === end && pv.key === this.viewKey()) return pv;
    this.pv = null;
    this.schedulePreview(0);
    return null;
  }

  // Repaint the panel's own graph, for when the colours change under it.
  redraw() { if (!this.tab) return; try { this.tab === 'eq' ? drawEq(this) : drawSide(this); } catch { } }
  showsOtherLength() { return this.tab === 'tool' && RESIZING.has(this.toolName) && !this.bypass; }
  clearPreview() { this.pv = null; clearTimeout(this.pvTimer); }
  clearView() { clearTimeout(this.viewTimer); this.view = null; this.viewPending = null; }

  /** @param {number} ms */
  schedulePreview(ms, restart = false) {
    clearTimeout(this.pvTimer);
    const run = () => {
      if (this.dragging && this.tab === 'nr') { this.pvTimer = setTimeout(run, 150); return; }
      this.preview(restart);
    };
    this.pvTimer = setTimeout(run, ms);
  }

  // The noise print, or an automatic estimate, for drawing: fetched once per sound version,
  // range and print, then the panel redraws.
  /** @param {number} a @param {number} b */
  profileFor(a, b) {
    const e = this.ed, key = `${e.version}|${a}|${b}|${this.noise ? this.noiseId : 'auto'}`;
    if (this.autoProf?.key !== key) {
      this.autoProf = { key, profile: null };
      doc.analyse({ kind: 'profile', a, b }).then((p) => { if (this.autoProf?.key === key) { this.autoProf.profile = p; drawSide(this); } }).catch(() => { });
    }
    return this.autoProf.profile;
  }

  // The effect of the open panel over [a, b), as an edit for the native side.
  /** @param {number} a @param {number} b */
  fxEdit(a, b) {
    const tab = this.tab;
    return { kind: 'fx', tab, a, b, p: tab === 'eq' ? null : { ...this.params(tab) }, bands: tab === 'eq' ? this.bands : null, noise: this.noise ? 'print' : 'auto' };
  }

  // For a long sound: process the first seconds of the range, then play them (restart) or
  // swap them into the playback that is running.
  /** @param {boolean} restart */
  async preview(restart) {
    const e = this.ed, tab = this.tab;
    if (!this.offline || !e.loaded) return;
    const [a, b, end] = this.previewRange();
    if (b > end && restart && this.toldShort !== e.fileId) { this.toldShort = e.fileId; this.toast(`The preview covers the first ${PREVIEW_SECS} seconds. Apply treats all of it.`); }
    const v = e.version, key = this.viewKey();
    let r;
    try { r = await doc.view(`pv|${key}`, this.fxEdit(a, b), PREVIEW_SECS); } catch { return; }
    if (e.version !== v || this.tab !== tab || this.viewKey() !== key) return;
    this.pv = { a, b: a + r.length, v, key, src: `pv|${key}` };
    if (r.info) this.setInfo(r.info);
    if (restart) e.play(false, true); else if (e.playing) e.play(true);
  }

  paramsChanged() {
    this.touch();
    this.ed.draw();
    if (this.tab !== 'eq') drawSide(this);
  }

  async apply() {
    const e = this.ed, tab = this.tab;
    if (!e.loaded || !tab || this.busy) return;
    clearTimeout(this.kickTimer); this.wantPlay = false;
    if (this.bypass) { this.bypass = false; this.dressAb(); }
    if (tab === 'tool') {
      const name = /** @type {string} */ (this.toolName);
      e.stopAudition();
      this.busy = true; this.applyBtn.disabled = true;
      let applied;
      try { applied = await e.action(name); } finally { this.busy = false; this.applyBtn.disabled = false; }
      // A tool that refused, or whose edit failed, leaves the change pending.
      if (!applied) return;
      // An applied fade is in the sound now: its rulers go back to nothing and the shape leaves the waveform.
      if (name === 'fade' && this.tab === 'tool' && this.toolName === name) { this.clearFades(); this.build(); }
      this.touched = false;
      // Until a value changes, the waveform and Play show the applied sound itself, not a second pass.
      if (this.tab === 'tool' && this.toolName === name) { this.quietKey = this.viewKey(); e.draw(); }
      this.dressBar();
      return;
    }
    if (tab === 'eq' && !this.bands.some(b => b.on)) return this.notice('Add an EQ band first');
    const [a, b] = e.range(), v = e.version;
    this.busy = true; this.applyBtn.disabled = true;
    try {
      if (tab === 'nr' && b - a > e.sr * 20) { this.toast('Reducing noise, this can take a few seconds'); await new Promise(r => setTimeout(r, 30)); }
      if (e.version !== v || this.tab !== tab) return this.notice('The sound changed, apply again');
      if (!await e.edit(this.fxEdit(a, b))) return;
      this.clearPreview();
      if (tab === 'eq') { this.clearBands(); renderBandControls(this); drawEq(this); }
      this.quietKey = this.viewKey();
      this.touched = false; this.dressBar();
    } finally {
      this.busy = false; this.applyBtn.disabled = false;
    }
  }

  /** @param {string} t */
  setInfo(t) { const i = this.root.querySelector('.fx-info'); if (i) i.textContent = t; }

  // Names a result: the sound's version, the range, and every value that shapes it.
  viewKey() {
    const e = this.ed, tab = this.tab, name = this.toolName, [a, b] = e.range();
    const params = tab === 'tool' ? [name, e.sel ? 1 : 0, ...this.toolValues()]
      : tab === 'eq' ? this.bands.map(x => [x.type, x.f, x.g, x.q, x.on]) : tab === 'nr' ? [this.nr, this.noise ? this.noiseId : 'auto'] : this.params(tab);
    return `${tab}|${e.version}|${a}|${b}|${JSON.stringify(params)}`;
  }

  // What the editor should draw while an effect or tool is open: the sound with it applied to
  // the range ({ src, length }), or null for the sound itself. A result that is not there yet is
  // asked for; until it arrives the editor draws the last one for this sound and this tool.
  displayView() {
    const e = this.ed, tab = this.tab;
    if (!tab || this.bypass || !e.loaded || this.long()) return null;
    if (tab === 'tool' && !SAME_LENGTH.has(this.toolName) && !RESIZING.has(this.toolName)) return null;
    const key = this.viewKey();
    if (key === this.quietKey) return null;
    if (this.view?.key === key) return this.view.out;
    this.requestView(key);
    return this.view?.name === this.toolName && this.view.key.startsWith(`${tab}|${e.version}|`) ? this.view.out : null;
  }

  // Works out the result named `key` (the current viewKey) in the background, once the values
  // have rested, then redraws and plays it if a play was waiting. One key is asked for once.
  /** @param {string} key */
  requestView(key) {
    if (this.viewPending === key) return;
    this.viewPending = key;
    clearTimeout(this.viewTimer);
    const e = this.ed, tab = this.tab, name = this.toolName, v = e.version, [a, b] = e.range();
    const run = async () => {
      if (this.viewPending !== key || e.version !== v || this.tab !== tab) return;
      // The slow ones wait for the control to be let go; one result is worked out at a time.
      if (this.viewBusy || (this.dragging && (tab === 'nr' || SLOW.has(name)))) { this.viewTimer = setTimeout(run, this.viewBusy ? 40 : 150); return; }
      this.viewBusy = true;
      try {
        let out;
        if (tab === 'tool') out = await e.toolFull(/** @type {string} */ (name), `view|${key}`);
        else {
          // Only the range is processed, as Apply does; the rest of the sound is shown as it is.
          const r = await doc.view(`view|${key}`, this.fxEdit(a, b));
          out = { src: `view|${key}`, length: r.length };
          if (r.info && this.viewPending === key) this.setInfo(r.info);
        }
        this.viewBusy = false;
        if (this.viewPending !== key || e.version !== v || !out) return;
        this.view = { key, out, name };
        if (this.wantPlay) { e.draw(); if (!this.kickDue) this.kick(); }
        else if (e.playing && tab !== 'eq') e.play(true);
        else e.draw();
      } catch (err) {
        // These values have no result. They are not tried again, and no play waits for them.
        if (this.viewPending === key) { this.wantPlay = false; this.notice(/** @type {Failure} */ (err).message); }
      } finally { this.viewBusy = false; }
    };
    this.viewTimer = setTimeout(run, 60);
  }

  drawEq() { drawEq(this); }

  // The engine only sends the spectrum while the EQ is open.
  watchSpectrum() { effects.watchSpectrum(this.tab === 'eq').catch(() => { }); }

  bandChanged() { bandChanged(this); }
}
