import { el, ib, icon, notice, toast, fail, confirmModal } from './ui.js';
import { lib } from './native.js';
import { fmtTime } from './time.js';
import { invoke, listen, play as playSound, stop as stopSound, onEnded } from './engine.js';
import { fitCanvas, palette } from './canvas.js';
import { dirOf } from './names.js';
import { askSavePlace } from './places.js';
import { registerPlayer, stopOthers } from './players.js';
import { openInEditor } from './editing.js';
import { refreshSide } from './sidebar.js';

// A take is held in memory until it is saved, so its length is capped: ten minutes of stereo is about 230 MB.
const MAX_SECONDS = 10 * 60;

/** A finished take, held by the engine under `key` until it is saved or discarded.
    @typedef {{ id: number, name: string, key: string, frames: number, channels: number, sr: number, peaks: number[], saved: string | null }} Take */
/** The take being recorded. @typedef {{ sr: number, frames: number, peaks: number[], level: number }} Recording */
/** Records from a microphone or audio interface. A finished take can be played, opened in the
    editor, or saved into a project or a library folder. */
export class Recorder {
  /** @param {HTMLElement} root @param {() => void} onChange Told when a take starts, ends, is saved or is discarded. */
  constructor(root, onChange) {
    registerPlayer('recorder', () => this.stopPlay());
    // While recording, the native side sends how far the take has got.
    listen('recording', (/** @type {{ error?: string, frames: number, level: number, peaks: number[] }} */ p) => {
      const rec = this.rec; if (!rec) return;
      if (p.error) { notice(`The input stopped: ${p.error}`, 9000); return void this.stop().catch(fail); }
      rec.frames = p.frames; rec.level = p.level; rec.peaks.push(...p.peaks);
      if (rec.frames / rec.sr >= MAX_SECONDS) { notice('Stopped at ten minutes, the longest a single take can be.', 8000); this.stop().catch(fail); }
    });
    this.root = root; this.onChange = onChange;
    /** @type {Take[]} */
    this.takes = [];
    /** @type {Recording | null} */
    this.rec = null;
    this.n = 0;
    /** @type {{ t: Take, id?: number } | null | undefined} */ this.playing;
    /** @type {boolean | undefined} */ this.starting;
    /** Where the last take was saved, offered again for the next. @type {string | undefined} */ this.lastPlace;
    this.btn = el('button', { class: 'primary rec-btn', onclick: () => this.toggle().catch(fail) }, icon('fiber_manual_record'), el('span', {}, 'Record'));
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
    this.device.replaceChildren(el('option', { value: '' }, 'Default input'), ...names.map((/** @type {string} */ n) => el('option', { value: n }, n)));
    this.device.value = names.includes(keep) ? keep : '';
  }

  toggle() { return this.rec ? this.stop() : this.start(); }

  // The take is recorded and kept natively; the page follows its length and level.
  async start() {
    // Opening the input takes a moment, longer the first time: a second press meanwhile must not open it twice.
    if (this.starting || this.rec) return;
    this.starting = true;
    stopOthers('recorder');
    let info;
    try { info = await invoke('rec_start', { device: this.device.value }); }
    catch (e) {
      this.starting = false;
      return notice(`Could not open the input: ${/** @type {Error} */ (e)?.message || e}. If Athar is not allowed to use the microphone, allow it in System Settings, Privacy and Security, Microphone.`, 9000);
    }
    const rec = this.rec = { sr: info.sr, frames: 0, peaks: /** @type {number[]} */ ([]), level: 0 };
    this.starting = false; this.onChange();
    this.showRecording(true);
    this.listDevices().catch(() => { });
    const tick = () => { if (this.rec !== rec) return; this.clock.textContent = fmtTime(rec.frames / rec.sr); /** @type {HTMLElement} */ (this.meter.firstChild).style.width = `${Math.min(100, Math.sqrt(rec.level) * 100)}%`; this.meter.classList.toggle('hot', rec.level >= 0.99); this.drawWave(rec.peaks); requestAnimationFrame(tick); };
    tick();
  }

  /** @param {boolean} on */
  showRecording(on) {
    this.btn.classList.toggle('on', on);
    /** @type {HTMLElement} */ (this.btn.querySelector('span:last-child')).textContent = on ? 'Stop' : 'Record';
    /** @type {HTMLElement} */ (this.btn.querySelector('.i')).textContent = on ? 'stop' : 'fiber_manual_record';
  }

  async stop() {
    const rec = this.rec; if (!rec) return;
    this.rec = null;
    let r;
    // The button goes back to Record even when the take could not be closed, since nothing records any more.
    try { r = await invoke('rec_stop'); }
    finally {
      this.showRecording(false);
      /** @type {HTMLElement} */ (this.meter.firstChild).style.width = '0';
      this.onChange();
    }
    if (!r.key) return;
    const stamp = new Date(), pad = (/** @type {number} */ n) => String(n).padStart(2, '0');
    this.takes.unshift({ id: ++this.n, name: `Take ${stamp.getFullYear()}-${pad(stamp.getMonth() + 1)}-${pad(stamp.getDate())} ${pad(stamp.getHours())}.${pad(stamp.getMinutes())}.${pad(stamp.getSeconds())}`, key: r.key, frames: r.frames, channels: r.channels, sr: r.sr, peaks: rec.peaks, saved: null });
    this.paintList(); this.drawWave(rec.peaks); this.onChange();
  }

  /** @param {number[]} [peaks] */
  drawWave(peaks = []) {
    const fit = fitCanvas(this.canvas);
    if (!fit) return;
    const { g, W, H } = fit, col = palette();
    g.fillStyle = col('--wave-bg'); g.fillRect(0, 0, W, H);
    g.fillStyle = col('--line'); g.fillRect(0, H / 2, W, 1);
    g.fillStyle = col('--wave');
    // The newest sound is at the right edge, and older sound scrolls off to the left.
    // Quiet sound is drawn taller than it measures, the way hearing works, so a soft voice still shows.
    const bars = Math.floor(W / 2), from = Math.max(0, peaks.length - bars);
    for (let i = from; i < peaks.length; i++) { const hgt = Math.max(1, Math.pow(Math.min(1, peaks[i]), 0.4) * (H - 10)); g.fillRect(W - (peaks.length - i) * 2, (H - hgt) / 2, 1.5, hgt); }
  }

  /** @param {Take} t */
  play(t) {
    stopOthers('recorder');
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
    const to = await askSavePlace(`Save ${t.name}`, t.name, this.lastPlace, 'Add a folder to the Library first, with Add folder on the Library page. Takes are saved there.');
    if (!to) return;
    this.lastPlace = to.place;
    const file = `${to.name}.wav`;
    const { path } = await lib.saveTake(t.key, { dest_dir: to.dir, name: file });
    if (to.project) await lib.projectItem(to.project.id, { path }).catch(() => { });
    t.saved = path; t.name = to.name;
    toast(`Saved ${file}`); refreshSide(); this.paintList(); this.onChange();
  }

  // A take that was saved opens next to its file; one that was not, in the first Library folder that is connected.
  /** @param {Take} t */
  edit(t) {
    this.stopPlay();
    const dir = t.saved ? dirOf(t.saved) : () => lib.folders().then((/** @type {import('./search.js').Folder[]} */ fs) => fs.find(f => !f.offline)?.path || '');
    return openInEditor({ source: { key: t.key }, name: `${t.name}.wav`, dir });
  }

  paintList() {
    this.list.replaceChildren(...(this.takes.length ? this.takes.map(t => el('div', { class: 'prow rec-row' },
      ib(this.playing?.t === t ? 'stop' : 'play_arrow', 'Play', () => this.play(t)),
      el('span', { class: 'qn' }, t.name),
      el('span', { class: 'qd' }, [fmtTime(t.frames / t.sr), t.channels > 1 ? 'stereo' : 'mono', t.saved ? 'saved' : 'not saved'].join(' · ')),
      ib('edit', 'Open in the editor', () => this.edit(t)),
      ib(t.saved ? 'check' : 'save', t.saved ? `Saved as ${t.saved}` : 'Save into your library', () => this.save(t).catch(fail), t.saved ? 'on' : ''),
      ib('close', 'Discard this take', async () => { if (t.saved || await confirmModal(`Discard ${t.name}?`, 'It has not been saved, so the recording will be gone.', 'Discard')) { if (this.playing?.t === t) this.stopPlay(); this.takes = this.takes.filter(x => x !== t); invoke('rec_discard', { key: t.key }).catch(() => { }); this.paintList(); this.onChange(); } })))
      : [el('div', { class: 'empty' }, el('h3', {}, 'Nothing recorded yet'), el('p', { class: 'muted' }, 'Press Record, make a sound, press Stop. Takes stay here until you save or discard them.'))]));
  }

  hasUnsaved() { return !!this.rec || this.takes.some(t => !t.saved); }
  /** @param {KeyboardEvent} e */
  handleKey(e) {
    if (e.metaKey || e.ctrlKey || e.altKey || e.key.toLowerCase() !== 'r') return;
    e.preventDefault(); this.toggle().catch(fail);
  }
}
