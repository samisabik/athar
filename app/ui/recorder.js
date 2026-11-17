import { lib } from './native.js';
import { fmtTime } from './time.js';
import { invoke, listen, play as playSound, stop as stopSound, onEnded } from './engine.js';

// A take is held in memory until it is saved, so its length is capped: ten minutes of stereo is about 230 MB.
const MAX_SECONDS = 10 * 60;

/** @typedef {typeof import('./ui.js')} UI */
/** A finished take, held by the engine under `key` until it is saved or discarded.
    @typedef {{ id: number, name: string, key: string, frames: number, channels: number, sr: number, peaks: number[], saved: string | null }} Take */
/** The take being recorded. @typedef {{ sr: number, frames: number, peaks: number[], level: number }} Recording */
/** What the page uses from main.js.
    @typedef {{ el: UI['el'], ib: UI['ib'], icon: UI['icon'], notice: UI['notice'], toast: UI['toast'], fail: UI['fail'], modal: UI['modal'], confirmModal: UI['confirmModal'],
      volume(): number, stopOthers(): void, onSave(): void, onChange?(): void, edit(t: Take): Promise<void> | void }} RecorderHelpers */

/** Records from a microphone or audio interface. A finished take can be played, opened in the
    editor, or saved into a project or a library folder. */
export class Recorder {
  /** @param {HTMLElement} root @param {RecorderHelpers} h */
  constructor(root, h) {
    // While recording, the native side sends how far the take has got.
    listen('recording', (/** @type {{ error?: string, frames: number, level: number, peaks: number[] }} */ p) => {
      const rec = this.rec; if (!rec) return;
      if (p.error) { this.h.notice(`The input stopped: ${p.error}`, 9000); return void this.stop().catch(this.h.fail); }
      rec.frames = p.frames; rec.level = p.level; rec.peaks.push(...p.peaks);
      if (rec.frames / rec.sr >= MAX_SECONDS) { this.h.notice('Stopped at ten minutes, the longest a single take can be.', 8000); this.stop().catch(this.h.fail); }
    });
    this.root = root; this.h = h;
    /** @type {Take[]} */
    this.takes = [];
    /** @type {Recording | null} */
    this.rec = null;
    this.n = 0;
    /** @type {{ t: Take, id?: number } | null | undefined} */ this.playing;
    /** @type {boolean | undefined} */ this.starting;
    /** @type {string | undefined} */ this.lastDest;
    const { el, ib, icon } = h;
    this.btn = el('button', { class: 'primary rec-btn', onclick: () => this.toggle().catch(h.fail) }, icon('fiber_manual_record'), el('span', {}, 'Record'));
    this.device = el('select', { 'aria-label': 'Input' }, el('option', { value: '' }, 'Default input'));
    this.clock = el('span', { class: 'rec-clock' }, '0:00');
    this.canvas = el('canvas', { class: 'rec-canvas', 'aria-hidden': 'true' });
    this.meter = el('div', { class: 'rec-meter' }, el('i'));
    this.list = el('div', { class: 'list big rec-list' });
    /** @type {HTMLElement} */ (root.querySelector('.cmdline')).append(this.btn, el('label', { class: 'f' }, el('span', {}, 'Input'), this.device), el('span', { class: 'spacer' }), this.clock);
    /** @type {HTMLElement} */ (root.querySelector('.stack')).append(el('div', { class: 'card stage solo rec-stage' }, this.canvas, this.meter), this.list);
    this.paintList();
  }

  async show() {
    this.listDevices().catch(() => { });
    this.drawWave();
  }

  async listDevices() {
    const names = await invoke('rec_devices'), keep = this.device.value;
    this.device.replaceChildren(this.h.el('option', { value: '' }, 'Default input'), ...names.map((/** @type {string} */ n) => this.h.el('option', { value: n }, n)));
    this.device.value = names.includes(keep) ? keep : '';
  }

  toggle() { return this.rec ? this.stop() : this.start(); }

  // The take is recorded and kept natively; the page follows its length and level.
  async start() {
    // Opening the input takes a moment, longer the first time: a second press meanwhile must not open it twice.
    if (this.starting || this.rec) return;
    this.starting = true;
    this.h.stopOthers();
    let info;
    try { info = await invoke('rec_start', { device: this.device.value }); }
    catch (e) {
      this.starting = false;
      return this.h.notice(`Could not open the input: ${/** @type {Error} */ (e)?.message || e}. If Athar is not allowed to use the microphone, allow it in System Settings, Privacy and Security, Microphone.`, 9000);
    }
    const rec = this.rec = { sr: info.sr, frames: 0, peaks: /** @type {number[]} */ ([]), level: 0 };
    this.starting = false; this.h.onChange?.();
    this.btn.classList.add('on'); /** @type {HTMLElement} */ (this.btn.querySelector('span:last-child')).textContent = 'Stop'; /** @type {HTMLElement} */ (this.btn.querySelector('.i')).textContent = 'stop';
    this.listDevices().catch(() => { });
    const tick = () => { if (this.rec !== rec) return; this.clock.textContent = fmtTime(rec.frames / rec.sr); /** @type {HTMLElement} */ (this.meter.firstChild).style.width = `${Math.min(100, Math.sqrt(rec.level) * 100)}%`; this.meter.classList.toggle('hot', rec.level >= 0.99); this.drawWave(rec.peaks); requestAnimationFrame(tick); };
    tick();
  }

  async stop() {
    const rec = this.rec; if (!rec) return;
    this.rec = null;
    const r = await invoke('rec_stop');
    this.btn.classList.remove('on'); /** @type {HTMLElement} */ (this.btn.querySelector('span:last-child')).textContent = 'Record'; /** @type {HTMLElement} */ (this.btn.querySelector('.i')).textContent = 'fiber_manual_record';
    /** @type {HTMLElement} */ (this.meter.firstChild).style.width = '0';
    if (!r.key) return;
    const stamp = new Date(), pad = (/** @type {number} */ n) => String(n).padStart(2, '0');
    this.takes.unshift({ id: ++this.n, name: `Take ${stamp.getFullYear()}-${pad(stamp.getMonth() + 1)}-${pad(stamp.getDate())} ${pad(stamp.getHours())}.${pad(stamp.getMinutes())}.${pad(stamp.getSeconds())}`, key: r.key, frames: r.frames, channels: r.channels, sr: r.sr, peaks: rec.peaks, saved: null });
    this.paintList(); this.drawWave(rec.peaks); this.h.onChange?.();
  }

  /** @param {number[]} [peaks] */
  drawWave(peaks = []) {
    const cv = this.canvas, dpr = window.devicePixelRatio || 1, W = cv.clientWidth, H = cv.clientHeight;
    if (!W || !H) return;
    if (cv.width !== W * dpr || cv.height !== H * dpr) { cv.width = W * dpr; cv.height = H * dpr; }
    const g = /** @type {CanvasRenderingContext2D} */ (cv.getContext('2d')), css = getComputedStyle(document.documentElement);
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.fillStyle = css.getPropertyValue('--wave-bg'); g.fillRect(0, 0, W, H);
    g.fillStyle = css.getPropertyValue('--line'); g.fillRect(0, H / 2, W, 1);
    g.fillStyle = css.getPropertyValue('--wave');
    // The newest sound is at the right edge, and older sound scrolls off to the left.
    // Quiet sound is drawn taller than it measures, the way hearing works, so a soft voice still shows.
    const bars = Math.floor(W / 2), from = Math.max(0, peaks.length - bars);
    for (let i = from; i < peaks.length; i++) { const hgt = Math.max(1, Math.pow(Math.min(1, peaks[i]), 0.4) * (H - 10)); g.fillRect(W - (peaks.length - i) * 2, (H - hgt) / 2, 1.5, hgt); }
  }

  /** @param {Take} t */
  play(t) {
    this.h.stopOthers();
    const same = this.playing?.t === t;
    this.endPlay();
    if (same) return this.paintList();
    /** @type {{ t: Take, id?: number }} */
    const p = this.playing = { t };
    this.paintList();
    playSound(t.key).then((id) => {
      if (this.playing !== p) return stopSound(id);
      p.id = id;
      onEnded(id, () => { if (this.playing === p) { this.playing = null; this.paintList(); } });
    }).catch(() => { if (this.playing === p) { this.playing = null; this.paintList(); } });
  }
  endPlay() { const p = this.playing; if (!p) return; this.playing = null; if (p.id != null) stopSound(p.id); }
  stopPlay() { if (this.playing) { this.endPlay(); this.paintList(); } }

  /** @param {Take} t */
  async save(t) {
    /** @type {[{ path: string, offline?: boolean }[], { id: number, name: string, folder?: string | null }[]]} */
    const [folders, projects] = await Promise.all([lib.folders(), lib.projects()]);
    const options = [...projects.filter(p => p.folder).map(p => /** @type {[string, string]} */ ([`p:${p.id}`, `Project: ${p.name}`])), ...folders.filter(f => !f.offline).map(f => /** @type {[string, string]} */ ([`f:${f.path}`, `Library: ${f.path.split('/').pop()}`]))];
    if (!options.length) return this.h.notice('Add a folder to the Library first, with Add folder on the Library page. Takes are saved there.');
    const v = await this.h.modal(`Save ${t.name}`, [{ label: 'Name', value: t.name }, { label: 'Into', options, value: this.lastDest || undefined }]);
    if (!v || !v[0].trim()) return;
    this.lastDest = v[1];
    const proj = v[1].startsWith('p:') ? projects.find(p => `p:${p.id}` === v[1]) : null, dir = /** @type {string} */ (proj ? proj.folder : v[1].slice(2));
    const name = `${v[0].trim()}.wav`;
    const { path } = await lib.saveTake(t.key, { dest_dir: dir, name });
    if (proj) await lib.projectItem(proj.id, { path }).catch(() => { });
    t.saved = path; t.name = v[0].trim();
    this.h.toast(`Saved ${name}`); this.h.onSave(); this.paintList(); this.h.onChange?.();
  }

  paintList() {
    const { el, ib } = this.h;
    this.list.replaceChildren(...(this.takes.length ? this.takes.map(t => el('div', { class: 'prow rec-row' },
      ib(this.playing?.t === t ? 'stop' : 'play_arrow', 'Play', () => this.play(t)),
      el('span', { class: 'qn' }, t.name),
      el('span', { class: 'qd' }, [fmtTime(t.frames / t.sr), t.channels > 1 ? 'stereo' : 'mono', t.saved ? 'saved' : 'not saved'].join(' · ')),
      ib('edit', 'Open in the editor', () => { this.stopPlay(); this.h.edit(t); }),
      ib(t.saved ? 'check' : 'save', t.saved ? `Saved as ${t.saved}` : 'Save into your library', () => this.save(t).catch(this.h.fail), t.saved ? 'on' : ''),
      ib('close', 'Discard this take', async () => { if (t.saved || await this.h.confirmModal(`Discard ${t.name}?`, 'It has not been saved, so the recording will be gone.', 'Discard')) { if (this.playing?.t === t) this.stopPlay(); this.takes = this.takes.filter(x => x !== t); invoke('rec_discard', { key: t.key }).catch(() => { }); this.paintList(); this.h.onChange?.(); } })))
      : [el('div', { class: 'empty' }, el('h3', {}, 'Nothing recorded yet'), el('p', { class: 'muted' }, 'Press Record, make a sound, press Stop. Takes stay here until you save or discard them.'))]));
  }

  hasUnsaved() { return !!this.rec || this.takes.some(t => !t.saved); }
  /** @param {KeyboardEvent} e */
  handleKey(e) { if (e.key === 'r' || e.key === 'R') { e.preventDefault(); this.toggle().catch(this.h.fail); return true; } return false; }
}
