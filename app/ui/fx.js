import { db, NR_N, noteAt } from './units.js';
import { TOOLS } from './tools.js';
import { invoke, rate, listen } from './engine.js';

/** @typedef {import('./editor.js').Editor} Editor */
/** @typedef {{ f: string, label: string, unit: string, min: number, max: number, step: number }} ToolField */
/** @typedef {{ title: string, fields: ToolField[], audible?: boolean, presets?: [string, number][] }} ToolSpec */
/** @typedef {{ type: string, f: number, g: number, q: number, on: boolean }} Band */
/** @typedef {Record<string, number | boolean>} Params */
/** @typedef {{ src: string, length: number }} View */
/** @typedef {HTMLLabelElement & { sync: () => void }} Slider */

/** @param {string | null | undefined} name @returns {ToolSpec} */
const toolSpec = (name) => /** @type {Record<string, ToolSpec>} */ (/** @type {unknown} */ (TOOLS))[/** @type {string} */ (name)];

/** @type {Record<string, { node: string, label: string, gain: boolean }>} */
const BAND_TYPES = {
  bell: { node: 'peaking', label: 'Bell', gain: true },
  lowshelf: { node: 'lowshelf', label: 'Low shelf', gain: true },
  highshelf: { node: 'highshelf', label: 'High shelf', gain: true },
  lowcut: { node: 'highpass', label: 'Low cut', gain: false },
  highcut: { node: 'lowpass', label: 'High cut', gain: false },
  notch: { node: 'notch', label: 'Notch', gain: false },
};

// ---------------------------------------------------------------- panel

const FMIN = 20, FMAX = 20000, DBR = 24;
const css = (/** @type {string} */ n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
const BAND_COLORS = ['--c1', '--c3', '--c2', '--c5', '--c4'];

/**
 * @param {string} label @param {string} key @param {number} min @param {number} max @param {number} step
 * @param {string} unit @param {Params} obj @param {() => void} onInput @returns {Slider}
 */
function slider(label, key, min, max, step, unit, obj, onInput) {
  const wrap = /** @type {Slider} */ (document.createElement('label'));
  wrap.className = 'sl';
  wrap.innerHTML = `<span>${label}</span><input type="range" min="${min}" max="${max}" step="${step}"><output></output>`;
  const inp = /** @type {HTMLInputElement} */ (wrap.querySelector('input')), out = /** @type {HTMLOutputElement} */ (wrap.querySelector('output'));
  const show = () => { out.textContent = `${Number(obj[key]).toFixed(step < 1 ? (step < 0.1 ? 2 : 1) : 0)}${unit}`; };
  inp.value = String(obj[key]); show();
  inp.oninput = () => { obj[key] = Number(inp.value); show(); onInput(); };
  wrap.sync = () => { inp.value = String(obj[key]); show(); };
  // Double-click the readout to type an exact value.
  out.dataset.tip = 'Double-click to type a value';
  out.ondblclick = (e) => {
    e.preventDefault();
    const box = document.createElement('input');
    box.type = 'number'; box.min = String(min); box.max = String(max); box.step = 'any'; box.value = String(obj[key]); box.className = 'sl-type';
    box.setAttribute('aria-label', `${label}, between ${min} and ${max}`);
    let done = false;
    const finish = (/** @type {boolean} */ keep) => {
      if (done) return; done = true;
      const v = Number(box.value);
      box.replaceWith(out);
      if (keep && box.value !== '' && isFinite(v)) { obj[key] = Math.min(max, Math.max(min, v)); wrap.sync(); onInput(); }
    };
    box.onkeydown = (ev) => { ev.stopPropagation(); if (ev.key === 'Enter') finish(true); else if (ev.key === 'Escape') finish(false); };
    box.onblur = () => finish(true);
    out.replaceWith(box); box.focus(); box.select();
  };
  return wrap;
}

const PREVIEW_SECS = 30;
/** @type {Record<string, string>} */
const TITLES = { eq: 'Parametric EQ', comp: 'Compressor', max: 'Maximizer', sat: 'Saturation', shape: 'Shape', lofi: 'Lo-fi', nr: 'Noise reduction' };
/** @type {Record<string, string>} */
const LABELS = { eq: 'EQ', comp: 'Compressor', max: 'Maximizer', sat: 'Saturation', shape: 'Shape', lofi: 'Lo-fi', nr: 'Noise reduction' };
const LISTEN_WAIT_MS = 220;
// Tools whose result is as long as the original: the waveform and Play show their result while open.
/** @type {Set<string | null>} */
const SAME_LENGTH = new Set(['fade', 'gain', 'normalize', 'loudnorm', 'declick']);
// Tools whose result is longer or shorter: the waveform shows the result, and Play auditions it.
/** @type {Set<string | null>} */
const RESIZING = new Set(['speed', 'tune', 'stretch', 'loopxfade']);
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
   * @param {(msg: string) => void} toast @param {(msg: string) => void} [notice]
   */
  constructor(root, editor, toast, notice) {
    this.root = root; this.ed = editor; this.toast = toast; this.notice = notice || toast;
    /** @type {string | null} */
    this.tab = null;
    this.bypass = false;
    /** @type {Band[]} */
    this.bands = [];
    this.selBand = -1;
    this.comp = { ...DEFAULTS.comp }; this.max = { ...DEFAULTS.max }; this.nr = { ...DEFAULTS.nr };
    this.sat = { ...DEFAULTS.sat }; this.shape = { ...DEFAULTS.shape }; this.lofi = { ...DEFAULTS.lofi };
    /** Asks which of two ways to go: resolves 'ok', 'alt' or something else for neither. @type {((title: string, text: string, ok: string, alt: string) => Promise<string | null>) | null} */
    this.choose = null;
    /** The transfer curve last drawn, and the one asked for. @type {{ key: string, out: number[] } | null} */
    this.sideCurve = null;
    /** @type {string | null} */
    this.sideWant = null;
    /** Told which section is open, for the cat's lessons. @type {((name: string | null) => void) | null} */
    this.onSection = null;
    /** @type {{ scale: string, key: number } | null} */
    this.tuneScale = null;
    this.touched = false; this.wantPlay = false;
    /** @type {string | null} */
    this.toolName = null;
    /** @type {{ len: number } | null} */
    this.noise = null; this.noiseId = 0;
    // The spectrum of what plays, sent while the EQ is open and smoothed the way an AnalyserNode is.
    this.spectrum = new Float32Array(2048).fill(-120);
    listen('spectrum', (d) => {
      const out = this.spectrum;
      for (let i = 0; i < d.length && i < out.length; i++) out[i] = 0.8 * out[i] + 0.2 * d[i];
      if (this.tab === 'eq' && this.ed.playing) this.drawEq();
    });
    /** @type {{ key: string, data: number[][] } | null} */
    this.curves = null;
    this.dragging = false;
    /** @type {AbortController} */
    this.buildAbort;
    /** @type {string[]} */
    this.startValues;
    /** @type {{ key: string, out: View, name: string | null } | null | undefined} */
    this.view;
    /** @type {string | null | undefined} */
    this.viewPending;
    /** @type {{ a: number, b: number, v: number, key: string, src: string } | null | undefined} */
    this.pv;
    /** @type {{ key: string, profile: number[] | null } | null | undefined} */
    this.autoProf;
    /** @type {string | undefined} */
    this.curvesWant;
    /** @type {number | undefined} */
    this.toldShort;
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

  get offline() { return this.tab && this.tab !== 'eq' && this.tab !== 'tool'; }
  /** @param {string | null} tab @returns {Params} */
  params(tab) { return /** @type {Record<string, Params>} */ ({ comp: this.comp, max: this.max, nr: this.nr, sat: this.sat, shape: this.shape, lofi: this.lofi })[/** @type {string} */ (tab)]; }

  // Every section, effect or tool, has the same frame: its name, the Original / Edited switch,
  // and Apply and Discard under the controls. The menu item of whatever is
  // open is marked.
  dress() {
    const tool = this.tab === 'tool', q = (/** @type {string} */ s) => /** @type {Element} */ (this.root.querySelector(s));
    q('.fx-tabs').hidden = tool;
    this.toolTabs.hidden = !tool;
    if (tool) this.dressToolTabs();
    q('[data-x=ab]').hidden = tool && !toolSpec(this.toolName)?.audible;
    q('.fx-title').textContent = tool ? toolSpec(this.toolName).title : (TITLES[/** @type {string} */ (this.tab)] || '');
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

  label() { return this.tab === 'tool' ? toolSpec(this.toolName).title : LABELS[/** @type {string} */ (this.tab)]; }
  // The name of the open section, for the cat's lessons.
  section() { return this.root.hidden ? null : this.tab === 'tool' ? this.toolName : this.tab; }
  long() { return this.ed.length > this.ed.sr * 60; }

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
      if (!toolSpec(name).audible) return done();
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
    if (name === 'trimsilence') this.ed.$('[data-f=trimthresh]').value = -96;
    if (name === 'fade') { this.ed.$('[data-f=fadeinlen]').value = 0; this.ed.$('[data-f=fadeoutlen]').value = 0; this.lastField = 0; }
    this.startValues = toolSpec(name).fields.map(f => this.ed.$(`[data-f=${f.f}]`).value);
    this.dress();
    this.build();
    this.opened();
  }

  // Puts every control of the open section back to where it started.
  reset() {
    const tab = this.tab; if (!tab) return;
    if (tab === 'eq') { this.bands = []; this.selBand = -1; }
    else if (tab === 'tool') toolSpec(this.toolName).fields.forEach((f, i) => { this.ed.$(`[data-f=${f.f}]`).value = this.startValues[i]; });
    else Object.assign(this.params(tab), /** @type {Record<string, Params>} */ (DEFAULTS)[tab]);
    this.build();
    this.touch();
    this.ed.draw();
  }

  // The parts of the sound the open Fade tool will change, for the editor to draw and to drag.
  toolMark() {
    return this.tab === 'tool' && this.toolName === 'fade' && (!this.bypass || !this.touched) ? this.ed.fadeSpans() : null;
  }

  // The editor calls this when the selection or cursor moves.
  selectionChanged() {
    if (this.tab !== 'tool') return;
    const e = this.ed;
    // Tune reads the pitch of the range: a new range, or an undo, needs a new reading.
    if (this.toolName === 'tune' && this.tuneKey && this.tuneKey !== `${e.version}|${e.range().join('|')}`) { clearTimeout(this.tuneTimer); this.tuneTimer = setTimeout(() => { if (this.tab === 'tool' && this.toolName === 'tune') this.build(); }, 250); return; }
    // The fade rulers run to the length of the range: another range needs other rulers.
    if (this.toolName === 'fade' && this.fadeKey !== `${e.version}|${e.range().join('|')}`) return void this.build();
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
    this.buildAbort?.abort();
    this.buildAbort = new AbortController();
    this.body.innerHTML = '';
    this.eqCanvas = this.bandBox = this.sideCanvas = null;
    /** @type {Record<string, () => void>} */ ({ eq: () => this.buildEq(), comp: () => this.buildComp(), max: () => this.buildMax(), nr: () => this.buildNr(), sat: () => this.buildSat(), shape: () => this.buildShape(), lofi: () => this.buildLofi(), tool: () => this.buildTool() })[/** @type {string} */ (this.tab)]();
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
    this.buildAbort?.abort();
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
    this.noise = null; invoke('ed_noise', { range: null }).catch(() => { });
    this.touched = false; this.wantPlay = false;
    this.bypass = true; this.dressAb();
    // Another sound starts every effect and tool from its initial settings. The tempo is the
    // sound's own, set by the editor as it loads.
    this.bands = []; this.selBand = -1;
    for (const k in DEFAULTS) Object.assign(this.params(k), /** @type {Record<string, Params>} */ (DEFAULTS)[k]);
    for (const t of Object.values(TOOLS)) for (const f of t.fields) if (f.f !== 'bpm') { const inp = this.ed.$(`[data-f=${f.f}]`); inp.value = inp.defaultValue; }
    if (this.tab === 'tool') this.startValues = toolSpec(this.toolName).fields.map(f => this.ed.$(`[data-f=${f.f}]`).value);
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

  // Whether what plays goes through the live EQ; the bands are sent before it starts.
  liveEq() {
    const on = this.tab === 'eq' && !this.bypass;
    if (on) this.syncEq();
    return on;
  }
  syncEq() { invoke('ed_eq', { bands: this.bands }).catch(() => { }); }

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
  redraw() { if (!this.tab) return; try { this.tab === 'eq' ? this.drawEq() : this.drawSide(); } catch { } }
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
      invoke('ed_analyse', { p: { kind: 'profile', a, b } }).then((p) => { if (this.autoProf?.key === key) { this.autoProf.profile = p; this.drawSide(); } }).catch(() => { });
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
    try { r = await invoke('ed_view', { key: `pv|${key}`, p: this.fxEdit(a, b), headS: PREVIEW_SECS }); } catch { return; }
    if (e.version !== v || this.tab !== tab || this.viewKey() !== key) return;
    this.pv = { a, b: a + r.length, v, key, src: `pv|${key}` };
    if (r.info) this.setInfo(r.info);
    if (restart) e.play(false, true); else if (e.playing) e.play(true);
  }

  paramsChanged() {
    this.touch();
    this.ed.draw();
    if (this.tab !== 'eq') this.drawSide();
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
      const before = e.version;
      try { await e.action(name); } finally { this.busy = false; this.applyBtn.disabled = false; }
      // An applied fade is in the sound now: its rulers go back to nothing and the shape leaves the waveform.
      if (name === 'fade' && e.version !== before && this.tab === 'tool' && this.toolName === name) {
        e.$('[data-f=fadeinlen]').value = 0; e.$('[data-f=fadeoutlen]').value = 0;
        this.build();
      }
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
      if (tab === 'eq') { this.bands = []; this.selBand = -1; this.renderBandControls(); this.drawEq(); }
      this.quietKey = this.viewKey();
      this.touched = false; this.dressBar();
    } finally {
      this.busy = false; this.applyBtn.disabled = false;
    }
  }

  /** @param {string} t */
  setInfo(t) { const i = this.root.querySelector('.fx-info'); if (i) i.textContent = t; }

  // What the editor should draw while an effect or tool is open: the sound with it applied to the range.
  // Computed in the background after the settings stop changing; until then the editor draws the
  // last result for this sound, or the original.
  // Names the preview: the sound's version, the range, and every value that shapes the result.
  viewKey() {
    const e = this.ed, tab = this.tab, name = this.toolName, [a, b] = e.range();
    const params = tab === 'tool' ? [name, e.sel ? 1 : 0, ...toolSpec(name).fields.map(f => e.$(`[data-f=${f.f}]`).value)]
      : tab === 'eq' ? this.bands.map(x => [x.type, x.f, x.g, x.q, x.on]) : tab === 'nr' ? [this.nr, this.noise ? this.noiseId : 'auto'] : this.params(tab);
    return `${tab}|${e.version}|${a}|${b}|${JSON.stringify(params)}`;
  }

  displayView() {
    const e = this.ed, tab = this.tab;
    if (!tab || this.bypass || !e.loaded || e.length > e.sr * 60) return null;
    if (tab === 'tool' && !SAME_LENGTH.has(this.toolName) && !RESIZING.has(this.toolName)) return null;
    const [a, b] = e.range(), v = e.version;
    const name = this.toolName, key = this.viewKey();
    if (key === this.quietKey) return null;
    if (this.view?.key === key) return this.view.out;
    if (this.viewPending !== key) {
      this.viewPending = key;
      clearTimeout(this.viewTimer);
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
            const r = await invoke('ed_view', { key: `view|${key}`, p: this.fxEdit(a, b), headS: null });
            out = { src: `view|${key}`, length: r.length };
            if (r.info && this.viewPending === key) this.setInfo(r.info);
          }
          this.viewBusy = false;
          if (this.viewPending !== key || e.version !== v || !out) return;
          this.view = { key, out, name };
          if (this.wantPlay) { e.draw(); if (!this.kickDue) this.kick(); }
          else if (e.playing && tab !== 'eq') e.play(true);
          else e.draw();
        } catch { } finally { this.viewBusy = false; }
      };
      this.viewTimer = setTimeout(run, 60);
    }
    // While the new result is on its way, keep showing the last one for this sound and this tool.
    return this.view?.name === name && this.view.key.startsWith(`${tab}|${v}|`) ? this.view.out : null;
  }

  // ---------------- EQ
  buildEq() {
    this.body.innerHTML = `<canvas class="eq-graph"></canvas><div class="eq-band"></div>`;
    this.eqCanvas = /** @type {HTMLCanvasElement} */ (this.body.querySelector('canvas'));
    this.bandBox = /** @type {HTMLElement} */ (this.body.querySelector('.eq-band'));
    this.bindEq();
    this.renderBandControls();
    this.drawEq();
    this.observe(this.eqCanvas, () => this.drawEq());
    const { signal } = this.buildAbort;
    const loop = () => { if (signal.aborted) return; if (this.ed.playing) this.drawEq(); requestAnimationFrame(loop); };
    requestAnimationFrame(loop);
  }

  /** @param {number} f @param {number} W */
  fx(f, W) { return Math.log(f / FMIN) / Math.log(FMAX / FMIN) * W; }
  /** @param {number} x @param {number} W */
  xf(x, W) { return FMIN * Math.pow(FMAX / FMIN, x / W); }
  /** @param {number} g @param {number} H */
  gy(g, H) { return H / 2 - g / DBR * (H / 2 - 14); }
  /** @param {number} y @param {number} H */
  yg(y, H) { return (H / 2 - y) / (H / 2 - 14) * DBR; }

  /** @param {Band} b @param {number} W @param {number} H */
  bandXY(b, W, H) {
    const gain = BAND_TYPES[b.type].gain ? b.g : 0;
    return [this.fx(b.f, W), this.gy(gain, H)];
  }

  drawEq() {
    if (!this.ed.playing) this.ed.draw();
    const cv = this.eqCanvas; if (!cv) return;
    const dpr = window.devicePixelRatio || 1, W = cv.clientWidth, H = cv.clientHeight;
    if (cv.width !== W * dpr || cv.height !== H * dpr) { cv.width = W * dpr; cv.height = H * dpr; }
    const g = /** @type {CanvasRenderingContext2D} */ (cv.getContext('2d')); g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.fillStyle = css('--wave-bg'); g.fillRect(0, 0, W, H);
    g.font = '10px "Share Tech Mono", "Martian Mono", monospace';
    g.strokeStyle = css('--line'); g.fillStyle = css('--muted'); g.lineWidth = 1;
    for (const f of [50, 100, 200, 500, 1000, 2000, 5000, 10000]) {
      const x = Math.round(this.fx(f, W)) + .5;
      g.beginPath(); g.moveTo(x, 0); g.lineTo(x, H); g.stroke();
      g.fillText(f >= 1000 ? f / 1000 + 'k' : String(f), x + 3, H - 4);
    }
    for (const d of [-18, -12, -6, 0, 6, 12, 18]) {
      const y = Math.round(this.gy(d, H)) + .5;
      g.globalAlpha = d === 0 ? 1 : .6;
      g.beginPath(); g.moveTo(0, y); g.lineTo(W, y); g.stroke();
      g.globalAlpha = 1; g.fillText((d > 0 ? '+' : '') + d, 4, y - 3);
    }

    if (this.ed.playing && this.liveEq()) {
      const data = this.readSpectrum(), sr = rate;
      g.beginPath(); g.moveTo(0, H);
      for (let x = 0; x <= W; x += 2) {
        const k = Math.min(data.length - 1, Math.round(this.xf(x, W) / (sr / 2) * data.length));
        const v = Math.max(0, (data[k] + 100) / 90);
        g.lineTo(x, H - v * H);
      }
      g.lineTo(W, H); g.closePath();
      g.fillStyle = css('--c4'); g.globalAlpha = .55; g.fill(); g.globalAlpha = 1;
    }

    const N = Math.ceil(W / 2) + 1;
    const total = new Float32Array(N).fill(0);
    const resp = this.response(N, W);
    this.bands.forEach((b, i) => {
      if (!b.on || !resp?.[i]) return;
      const mag = resp[i];
      for (let k = 0; k < N; k++) total[k] += mag[k];
      if (i === this.selBand) {
        g.beginPath();
        for (let k = 0; k < N; k++) { const y = this.gy(Math.max(-DBR, Math.min(DBR, mag[k])), H); k ? g.lineTo(k * 2, y) : g.moveTo(0, y); }
        g.lineTo(W, this.gy(0, H)); g.lineTo(0, this.gy(0, H)); g.closePath();
        g.fillStyle = css(BAND_COLORS[i % BAND_COLORS.length]); g.globalAlpha = .45; g.fill(); g.globalAlpha = 1;
      }
    });
    g.beginPath();
    for (let k = 0; k < N; k++) { const y = this.gy(Math.max(-DBR, Math.min(DBR, total[k])), H); k ? g.lineTo(k * 2, y) : g.moveTo(0, y); }
    g.strokeStyle = this.bypass ? css('--muted') : css('--accent'); g.lineWidth = 2; g.stroke(); g.lineWidth = 1;

    this.bands.forEach((b, i) => {
      const [x, y] = this.bandXY(b, W, H);
      g.beginPath(); g.arc(x, y, i === this.selBand ? 9 : 7, 0, Math.PI * 2);
      g.fillStyle = b.on ? css(BAND_COLORS[i % BAND_COLORS.length]) : css('--line'); g.fill();
      g.strokeStyle = css('--ink'); g.lineWidth = i === this.selBand ? 2 : 1; g.stroke(); g.lineWidth = 1;
      g.fillStyle = css('--ink'); g.fillText(String(i + 1), x - 3, y + 3.5);
    });
    if (!this.bands.length) {
      g.fillStyle = css('--muted'); g.font = '13px "Share Tech Mono", "Martian Mono", monospace';
      g.fillText('Double-click to add a band. Drag to move, scroll to change width.', 40, 26);
    }
  }

  // The EQ curve of each band, in dB at N points across the width, computed natively from the
  // same filters that play. Null until it arrives; the panel redraws then.
  /** @param {number} N @param {number} W */
  response(N, W) {
    const key = JSON.stringify([N, W, rate, this.bands]);
    if (this.curves?.key === key) return this.curves.data;
    if (this.curvesWant !== key) {
      this.curvesWant = key;
      const freqs = Array.from({ length: N }, (_, i) => this.xf(i * 2, W));
      invoke('eq_response', { bands: this.bands, freqs }).then((data) => {
        if (this.curvesWant !== key) return;
        this.curves = { key, data }; this.drawEq();
      }).catch(() => { });
    }
    return this.curves?.data?.length === this.bands.length ? this.curves.data : null;
  }

  readSpectrum() { return this.spectrum; }
  watchSpectrum() { invoke('audio_watch_spectrum', { on: this.tab === 'eq' }).catch(() => { }); }

  /** @param {number} x @param {number} y */
  hitBand(x, y) {
    const cv = /** @type {HTMLCanvasElement} */ (this.eqCanvas), W = cv.clientWidth, H = cv.clientHeight;
    let best = -1, bd = 14;
    this.bands.forEach((b, i) => { const [bx, by] = this.bandXY(b, W, H); const d = Math.hypot(bx - x, by - y); if (d < bd) { bd = d; best = i; } });
    return best;
  }

  bindEq() {
    const cv = /** @type {HTMLCanvasElement} */ (this.eqCanvas), { signal } = this.buildAbort;
    let drag = -1;
    const pos = (/** @type {MouseEvent} */ e) => { const r = cv.getBoundingClientRect(); return [e.clientX - r.left, e.clientY - r.top]; };
    cv.addEventListener('mousedown', (e) => {
      const [x, y] = pos(e); drag = this.hitBand(x, y);
      if (drag >= 0) { this.selBand = drag; this.renderBandControls(); this.drawEq(); }
    });
    window.addEventListener('mousemove', (e) => {
      if (drag < 0) return;
      const [x, y] = pos(e), W = cv.clientWidth, H = cv.clientHeight, b = this.bands[drag];
      b.f = Math.max(FMIN, Math.min(FMAX, this.xf(x, W)));
      if (BAND_TYPES[b.type].gain) b.g = Math.max(-DBR, Math.min(DBR, this.yg(y, H)));
      this.bandChanged(false);
    }, { signal });
    window.addEventListener('mouseup', () => { drag = -1; }, { signal });
    cv.addEventListener('dblclick', (e) => {
      const [x, y] = pos(e), W = cv.clientWidth, H = cv.clientHeight;
      const hit = this.hitBand(x, y);
      if (hit >= 0) { this.bands.splice(hit, 1); this.selBand = -1; return this.bandChanged(true); }
      const f = this.xf(x, W);
      const type = f < 40 ? 'lowcut' : f > 14000 ? 'highcut' : 'bell';
      this.bands.push({ type, f, g: type === 'bell' ? Math.max(-DBR, Math.min(DBR, Math.round(this.yg(y, H) * 2) / 2)) : 0, q: type === 'bell' ? 1 : 0.71, on: true });
      this.selBand = this.bands.length - 1;
      this.bandChanged(true);
    });
    cv.addEventListener('wheel', (e) => {
      const [x, y] = pos(e), hit = this.hitBand(x, y), i = hit >= 0 ? hit : this.selBand;
      if (i < 0) return;
      e.preventDefault();
      const b = this.bands[i];
      b.q = Math.max(0.1, Math.min(18, b.q * Math.exp(-e.deltaY * 0.004)));
      this.selBand = i; this.bandChanged(false);
    }, { passive: false });
  }

  /** @param {boolean} structural */
  bandChanged(structural) {
    this.touch();
    if (this.tab === 'eq' && !this.bypass) this.syncEq();
    this.renderBandControls();
    this.drawEq();
  }

  renderBandControls() {
    const box = this.bandBox; if (!box) return;
    const b = this.bands[this.selBand];
    if (!b) {
      box.innerHTML = `<span class="muted">${this.bands.length} band${this.bands.length === 1 ? '' : 's'}</span>`;
      return;
    }
    const opts = Object.entries(BAND_TYPES).map(([k, t]) => `<option value="${k}" ${k === b.type ? 'selected' : ''}>${t.label}</option>`).join('');
    box.innerHTML = `<span class="band-dot" style="background:var(${BAND_COLORS[this.selBand % BAND_COLORS.length]})">${this.selBand + 1}</span>
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
      this.bandChanged(k === 'type');
    });
    /** @type {Element} */ (box.querySelector('[data-k=on]')).onclick = () => { b.on = !b.on; this.bandChanged(true); };
    /** @type {Element} */ (box.querySelector('[data-k=del]')).onclick = () => { this.bands.splice(this.selBand, 1); this.selBand = -1; this.bandChanged(true); };
  }

  // ---------------- compressor / maximizer / noise
  /** @param {(Element | null)[]} controls */
  sideLayout(controls, graph = true) {
    this.body.innerHTML = `<div class="fx-side${graph ? '' : ' plain'}"><div class="fx-controls"></div>${graph ? '<canvas class="fx-graph"></canvas>' : ''}</div><div class="fx-info muted"></div>`;
    const c = /** @type {Element} */ (this.body.querySelector('.fx-controls'));
    controls.forEach(x => x && c.append(x));
    this.sideCanvas = /** @type {HTMLCanvasElement | null} */ (this.body.querySelector('.fx-graph'));
    if (!this.sideCanvas) return;
    this.observe(this.sideCanvas, () => this.drawSide());
    this.drawSide();
  }

  // A row of one-click values under the rulers.
  /** @param {[string, () => void, string?][]} items */
  presetRow(items) {
    const row = document.createElement('div'); row.className = 'fx-presets';
    for (const [label, fn, tip] of items) {
      const b = document.createElement('button'); b.type = 'button'; b.textContent = label; b.onclick = fn;
      if (tip) b.dataset.tip = tip;
      row.append(b);
    }
    return row;
  }

  buildSat() {
    const on = () => this.paramsChanged(), p = this.sat;
    this.sideLayout([
      slider('Drive', 'drive', 0, 24, 0.5, ' dB', p, on),
      slider('Ceiling', 'ceiling', -6, 0, 0.1, ' dB', p, on),
      slider('Mix', 'mix', 0, 100, 1, '%', p, on),
    ]);
    this.setInfo('Drive pushes the sound into a soft clipper. At full mix it never passes the ceiling.');
  }

  buildShape() {
    const on = () => this.paramsChanged(), p = this.shape;
    this.sideLayout([
      slider('Attack', 'attack', -100, 100, 1, '%', p, on),
      slider('Tail', 'tail', -100, 100, 1, '%', p, on),
    ], false);
    this.setInfo('Attack: more or less punch on each hit. Tail: shorter or longer ring after it.');
  }

  buildLofi() {
    const on = () => this.paramsChanged(), p = this.lofi;
    const bits = slider('Bit depth', 'bits', 4, 16, 1, '-bit', p, on), rate = slider('Sample rate', 'rate', 2, 48, 0.5, ' kHz', p, on);
    const set = (/** @type {number} */ b, /** @type {number} */ r) => () => { p.bits = b; p.rate = r; bits.sync(); rate.sync(); on(); };
    this.sideLayout([bits, rate, this.presetRow([['12-bit 26 kHz', set(12, 26)], ['12-bit 13 kHz', set(12, 13)], ['8-bit 11 kHz', set(8, 11)], ['8-bit 4 kHz', set(8, 4)]])], false);
  }

  // ---------------- tools: the same rulers, bound to the values the editor's actions read
  buildTool() {
    const name = this.toolName, spec = toolSpec(name), e = this.ed, { signal } = this.buildAbort;
    const inputs = spec.fields.map(f => e.$(`[data-f=${f.f}]`));
    if (name === 'fade') for (const inp of inputs) inp.value = Math.min(Number(inp.value) || 0, e.rangeMs());
    /** @type {(() => void) | null} */
    let hint = null;
    const changed = () => {
      hint?.();
      // The value Tune sets by itself when it opens is not a change made by hand.
      if (!this.settingUp) this.touch();
      e.draw();
    };
    const rows = spec.fields.map((f, i) => {
      /** @type {Params} */
      const value = {};
      Object.defineProperty(value, 'v', { get: () => Number(inputs[i].value) || 0, set: (/** @type {number} */ v) => { inputs[i].value = v; } });
      // A fade can be as long as the sound or the selection, so its ruler runs to exactly that.
      const max = name === 'fade' ? Math.max(1, e.rangeMs()) : f.max;
      return slider(f.label, 'v', f.min, max, f.step, f.unit, value, () => { this.lastField = i; changed(); });
    });
    const setFirst = (/** @type {number} */ v) => () => { inputs[0].value = v; rows[0].sync(); changed(); };
    const presets = spec.presets ? this.presetRow(spec.presets.map(([label, v]) => [label, setFirst(v)])) : null;
    this.sideLayout([...rows, presets], false);
    this.toolHint = () => hint?.();
    // Called when the editor changes a tool's value itself, as when a fade is dragged on the waveform.
    this.toolSync = () => { rows.forEach(r => r.sync()); changed(); };
    if (name === 'fade') {
      this.fadeKey = `${e.version}|${e.range().join('|')}`;
      hint = () => {
        const [fin, fout] = e.fadeSpans(), ms = (/** @type {{ a: number, b: number }} */ m) => `${Math.round((m.b - m.a) / e.sr * 1000)} ms`, on = e.sel && e.sel[1] > e.sel[0] ? 'the selection' : 'the sound';
        const parts = [fin.b > fin.a && `fades in over the first ${ms(fin)}`, fout.b > fout.a && `fades out over the last ${ms(fout)}`].filter(Boolean);
        this.setInfo(parts.length ? `${on[0].toUpperCase() + on.slice(1)} ${parts.join(' and ')}. Drag the end of a curve on the waveform to change it.`
          : `Set a length for either end of ${on}, or for both.`);
      };
      hint();
    } else if (name === 'trimsilence') {
      hint = () => {
        const r = e.trimBounds(), s = (/** @type {number} */ n) => `${(n / e.sr).toFixed(2)} s`;
        this.setInfo(!r ? 'Everything is below this level, so nothing would be left.'
          : r[0] === 0 && r[1] === e.length ? 'Nothing is cut yet. Raise the level until the quiet ends are covered on the waveform.'
            : `Cuts ${s(r[0])} from the start and ${s(e.length - r[1])} from the end.`);
      };
      hint();
    } else if (name === 'speed') {
      hint = () => { const p = Number(inputs[0].value) || 100, st = 12 * Math.log2(p / 100); this.setInfo(`${st >= 0 ? '+' : '−'}${Math.abs(st).toFixed(1)} semitones, ${p > 100 ? 'shorter' : p < 100 ? 'longer' : 'same length'}`); };
      hint();
    } else if (name === 'tune') this.buildTune(inputs[0], rows[0], changed, signal, (fn) => { hint = fn; });
  }

  // Reads the pitch of the range, offers the nearest notes, and says where the shift lands.
  /**
   * @param {HTMLInputElement} input @param {Slider} row @param {() => void} changed @param {AbortSignal} signal
   * @param {(fn: () => void) => void} setHint
   */
  async buildTune(input, row, changed, signal, setHint) {
    const e = this.ed, [a, b] = e.range();
    this.tuneKey = `${e.version}|${a}|${b}`;
    input.value = '0'; row.sync();
    this.setInfo('Listening for the pitch…');
    let hz = null;
    try { hz = (await invoke('ed_analyse', { p: { kind: 'pitch', a, b } })).hz; } catch { }
    if (signal.aborted) return;
    if (!hz) return this.setInfo('No clear pitch found here. Shift by ear; the sound gets shorter as it goes up.');
    const near = noteAt(hz), shiftTo = (/** @type {number} */ midi) => 12 * Math.log2(440 * Math.pow(2, (midi - 69) / 12) / hz);
    const describe = () => {
      const s = Number(input.value) || 0, land = noteAt(hz * Math.pow(2, s / 12));
      this.setInfo(`Now ${near.name} ${near.cents >= 0 ? '+' : '−'}${Math.abs(near.cents)} cents (${hz.toFixed(1)} Hz). With this shift: ${land.name} ${land.cents >= 0 ? '+' : '−'}${Math.abs(land.cents)} cents. Length changes with pitch.`);
    };
    setHint(describe);
    // The ruler steps through notes, so every stop is in tune. A key and scale narrow the stops
    // further; "Free" brings back the plain shift in semitones.
    const NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
    /** @type {Record<string, [string, number[] | null]>} */
    const SCALES = { all: ['All notes', null], major: ['Major', [0, 2, 4, 5, 7, 9, 11]], minor: ['Minor', [0, 2, 3, 5, 7, 8, 10]],
      majpent: ['Major pentatonic', [0, 2, 4, 7, 9]], minpent: ['Minor pentatonic', [0, 3, 5, 7, 10]], free: ['Free, no snapping', null] };
    const st = this.tuneScale ||= { scale: 'all', key: 0 };
    const nameOf = (/** @type {number} */ midi) => NAMES[((midi % 12) + 12) % 12] + (Math.floor(midi / 12) - 1);
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
      noteOut.textContent = `${nameOf(midi)}  ${s >= 0 ? '+' : '−'}${Math.abs(s).toFixed(2)} st`;
      if (announce) changed(); else describe();
    };
    const layout = (/** @type {boolean} */ announce) => {
      const free = st.scale === 'free', steps = SCALES[st.scale][1];
      row.hidden = !free; noteRow.hidden = free; keySel.disabled = free || !steps;
      if (free) return announce ? changed() : describe();
      stops = [];
      for (let m = near.midi - 12; m <= near.midi + 12; m++) if (!steps || steps.includes((((m - st.key) % 12) + 12) % 12)) stops.push(m);
      // Stay on the note the current shift is closest to.
      const want = 69 + 12 * Math.log2(hz * Math.pow(2, (Number(input.value) || 0) / 12) / 440);
      let at = 0; stops.forEach((m, k) => { if (Math.abs(m - want) < Math.abs(stops[at] - want)) at = k; });
      noteIn.max = String(stops.length - 1); noteIn.value = String(at);
      setNote(stops[at], announce);
    };
    noteIn.oninput = () => setNote(stops[+noteIn.value], true);
    const keySel = select(NAMES.map((n, k) => [k, n]), st.key, (v) => { st.key = +v; layout(true); }, 'Key');
    const scaleSel = select(Object.entries(SCALES).map(([k, v]) => [k, v[0]]), st.scale, (v) => { st.scale = v; layout(true); }, 'Scale');
    const scaleRow = document.createElement('div'); scaleRow.className = 'fx-presets fx-scale';
    const lab = (/** @type {string} */ text, /** @type {Node} */ node) => { const l = document.createElement('label'); l.className = 'f'; const sp = document.createElement('span'); sp.textContent = text; l.append(sp, node); return l; };
    scaleRow.append(lab('Snap to', scaleSel), lab('Key', keySel));
    row.after(noteRow, scaleRow);
    input.value = shiftTo(near.midi).toFixed(2); row.sync();
    // The shift is only known now that the pitch has been read: show and play the tuned sound, not the untouched one.
    this.settingUp = true;
    try { layout(true); } finally { this.settingUp = false; }
    e.draw();
    describe();
  }

  buildComp() {
    const on = () => this.paramsChanged(), p = this.comp;
    const makeup = slider('Makeup', 'makeup', 0, 24, 0.5, ' dB', p, () => { p.autoMakeup = false; auto.classList.remove('on'); on(); });
    const auto = document.createElement('button');
    auto.className = 'ib toggle' + (p.autoMakeup ? ' on' : ''); auto.dataset.tip = 'Automatic makeup gain';
    auto.innerHTML = '<span class="i">auto_mode</span>';
    auto.onclick = () => { p.autoMakeup = !p.autoMakeup; auto.classList.toggle('on', p.autoMakeup); on(); };
    makeup.append(auto);
    this.sideLayout([
      slider('Threshold', 'threshold', -60, 0, 0.5, ' dB', p, on),
      slider('Ratio', 'ratio', 1, 20, 0.1, ':1', p, on),
      slider('Knee', 'knee', 0, 24, 0.5, ' dB', p, on),
      slider('Attack', 'attack', 0.1, 200, 0.1, ' ms', p, on),
      slider('Release', 'release', 5, 2000, 1, ' ms', p, on),
      makeup,
    ]);
  }

  buildMax() {
    const on = () => this.paramsChanged(), p = this.max;
    const gain = slider('Amplify', 'gain', 0, 30, 0.5, ' dB', p, on);
    const auto = document.createElement('button');
    auto.className = 'ib'; auto.dataset.tip = 'Auto: amplify until the sound reaches about -12 dB RMS, limited at the ceiling';
    auto.innerHTML = '<span class="i">auto_mode</span>';
    auto.onclick = async () => {
      const [a, b] = this.ed.range();
      const s = await invoke('ed_analyse', { p: { kind: 'stats', a, b } });
      p.gain = Math.max(0, Math.min(30, Math.round((-12 - s.rms) * 2) / 2));
      gain.sync(); on();
      this.setInfo(`input peak ${s.peak.toFixed(1)} dB, RMS ${s.rms.toFixed(1)} dB, amplify ${p.gain} dB`);
    };
    gain.append(auto);
    this.sideLayout([
      gain,
      slider('Ceiling', 'ceiling', -6, 0, 0.1, ' dB', p, on),
      slider('Release', 'release', 5, 500, 1, ' ms', p, on),
    ]);
  }

  buildNr() {
    const on = () => this.paramsChanged(), p = this.nr;
    const cap = document.createElement('div'); cap.className = 'row';
    cap.innerHTML = `<button class="ib" data-tip="Capture noise print from the selection (select a part with only noise)"><span class="i">noise_aware</span></button>
      <button class="ib" data-tip="Forget the noise print and estimate noise automatically"><span class="i">restart_alt</span></button>
      <span class="muted nr-state"></span>`;
    const [capBtn, clrBtn] = cap.querySelectorAll('button');
    const state = /** @type {Element} */ (cap.querySelector('.nr-state'));
    const showState = () => {
      state.textContent = this.noise ? `Noise print: ${(this.noise.len / this.ed.sr).toFixed(2)} s` : 'Noise print: automatic';
    };
    capBtn.onclick = () => {
      const e = this.ed;
      if (!e.sel || e.sel[1] - e.sel[0] < e.sr * 0.05) return this.notice('Select at least 50 ms of noise only, then capture');
      invoke('ed_noise', { range: [e.sel[0], e.sel[1]] }).catch(() => { });
      this.noise = { len: e.sel[1] - e.sel[0] };
      this.noiseId++;
      showState(); this.toast('Noise print captured. Select what to clean (or nothing for the whole file).'); on();
    };
    clrBtn.onclick = () => { this.noise = null; invoke('ed_noise', { range: null }).catch(() => { }); showState(); on(); };
    showState();
    this.sideLayout([
      cap,
      slider('Reduction', 'reduction', 0, 40, 1, ' dB', p, on),
      slider('Sensitivity', 'sensitivity', 0.5, 4, 0.05, '', p, on),
    ]);
  }

  drawSide() {
    const cv = this.sideCanvas; if (!cv || !cv.isConnected) return;
    const dpr = window.devicePixelRatio || 1, W = cv.clientWidth, H = cv.clientHeight;
    if (!W || !H) return;
    if (cv.width !== W * dpr || cv.height !== H * dpr) { cv.width = W * dpr; cv.height = H * dpr; }
    const g = /** @type {CanvasRenderingContext2D} */ (cv.getContext('2d')); g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.fillStyle = css('--wave-bg'); g.fillRect(0, 0, W, H);
    g.strokeStyle = css('--line'); g.lineWidth = 1;
    g.font = '10px "Share Tech Mono", "Martian Mono", monospace'; g.fillStyle = css('--muted');

    if (this.tab === 'comp' || this.tab === 'max' || this.tab === 'sat') {
      const R = 60, X = (/** @type {number} */ d) => (d + R) / R * W, Y = (/** @type {number} */ d) => H - (d + R) / R * H;
      for (let d = -48; d < 0; d += 12) {
        g.beginPath(); g.moveTo(X(d), 0); g.lineTo(X(d), H); g.moveTo(0, Y(d)); g.lineTo(W, Y(d)); g.stroke();
        g.fillText(String(d), X(d) + 2, H - 3);
      }
      g.setLineDash([3, 3]); g.beginPath(); g.moveTo(0, H); g.lineTo(W, 0); g.stroke(); g.setLineDash([]);
      // The curve comes from the effect's own formulas, natively; the panel redraws when it arrives.
      const n = Math.ceil(W) + 1, tab = this.tab, key = JSON.stringify([tab, this.params(tab), n]);
      if (this.sideCurve?.key === key) {
        const out = this.sideCurve.out;
        g.beginPath();
        out.forEach((o, x) => { x ? g.lineTo(x, Y(o)) : g.moveTo(x, Y(o)); });
        g.strokeStyle = css('--accent'); g.lineWidth = 2; g.stroke();
      } else if (this.sideWant !== key) {
        this.sideWant = key;
        invoke('fx_curve', { tab, p: this.params(tab), from: -R, to: 0, n }).then((/** @type {number[]} */ out) => {
          if (this.sideWant !== key) return;
          this.sideCurve = { key, out }; this.drawSide();
        }).catch(() => { });
      }
      g.fillStyle = css('--muted'); g.fillText('in →', W - 34, H - 14); g.fillText('out', 4, 12);
    } else if (this.tab === 'nr') {
      const e = this.ed; if (!e.loaded) return;
      const prof = this.profileFor(.../** @type {[number, number]} */ (e.range()));
      if (!prof) return;
      const bins = prof.length, xOf = (/** @type {number} */ k) => Math.log(Math.max(1, k * e.sr / NR_N) / FMIN) / Math.log(FMAX / FMIN) * W;
      const yOf = (/** @type {number} */ m) => H - Math.max(0, (db(m / (NR_N / 4)) + 110) / 100) * H;
      for (const f of [100, 1000, 10000]) { const x = Math.log(f / FMIN) / Math.log(FMAX / FMIN) * W; g.beginPath(); g.moveTo(x, 0); g.lineTo(x, H); g.stroke(); g.fillText(f >= 1000 ? f / 1000 + 'k' : String(f), x + 2, H - 3); }
      const path = (/** @type {number} */ scale) => { g.beginPath(); for (let k = 1; k < bins; k++) { const x = xOf(k), y = yOf(prof[k] * scale); k > 1 ? g.lineTo(x, y) : g.moveTo(x, y); } };
      path(this.nr.sensitivity); g.strokeStyle = css('--accent'); g.lineWidth = 2; g.stroke();
      path(1); g.lineTo(W, H); g.lineTo(0, H); g.closePath(); g.fillStyle = css('--c1'); g.globalAlpha = .6; g.fill(); g.globalAlpha = 1;
      g.fillStyle = css('--muted'); g.fillText('noise print (fill) and gate threshold (line)', 6, 12);
    }
  }
}
