import { fmtTime } from './audio.js';
import { encodeWav } from './dsp.js';

// A take is held in memory until it is saved, so its length is capped: ten minutes of stereo is about 230 MB.
const BLOCK = 4096, MAX_SECONDS = 10 * 60;
// The drawing gets several bars per block of sound, so it moves smoothly instead of in steps.
const BARS_PER_BLOCK = 8;

/** Records from a microphone or audio interface. A finished take can be played, opened in the
    editor, or saved into a project or a library folder. */
export class Recorder {
  constructor(root, h) {
    this.root = root; this.h = h;
    this.takes = []; this.rec = null; this.n = 0;
    const { el, ib, icon } = h;
    this.btn = el('button', { class: 'primary rec-btn', onclick: () => this.toggle().catch(h.fail) }, icon('fiber_manual_record'), el('span', {}, 'Record'));
    this.device = el('select', { 'aria-label': 'Input' }, el('option', { value: '' }, 'Default input'));
    this.clock = el('span', { class: 'rec-clock' }, '0:00');
    this.canvas = el('canvas', { class: 'rec-canvas', 'aria-hidden': 'true' });
    this.meter = el('div', { class: 'rec-meter' }, el('i'));
    this.list = el('div', { class: 'list big rec-list' });
    root.querySelector('.cmdline').append(this.btn, el('label', { class: 'f' }, el('span', {}, 'Input'), this.device), el('span', { class: 'spacer' }), this.clock);
    root.querySelector('.stack').append(el('div', { class: 'card stage solo rec-stage' }, this.canvas, this.meter), this.list);
    this.paintList();
  }

  supported() { return !!navigator.mediaDevices?.getUserMedia; }

  async show() {
    if (!this.supported()) { this.btn.disabled = true; return this.h.notice('This window cannot reach the microphone.'); }
    this.listDevices().catch(() => { });
    this.drawWave();
  }

  // Device names are only given once the microphone has been allowed, so the list fills in after the first take.
  async listDevices() {
    const ins = (await navigator.mediaDevices.enumerateDevices()).filter(d => d.kind === 'audioinput');
    const keep = this.device.value;
    this.device.replaceChildren(this.h.el('option', { value: '' }, 'Default input'), ...ins.filter(d => d.deviceId && d.label).map(d => this.h.el('option', { value: d.deviceId }, d.label)));
    this.device.value = [...this.device.options].some(o => o.value === keep) ? keep : '';
  }

  toggle() { return this.rec ? this.stop() : this.start(); }
  setVolume(v) { if (this.playing) this.playing.a.volume = v; }

  async start() {
    // Asking for the microphone takes a moment, longer the first time: a second press meanwhile must not open it twice.
    if (this.starting || this.rec) return;
    this.starting = true;
    this.h.stopOthers();
    let stream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: { deviceId: this.device.value ? { exact: this.device.value } : undefined, echoCancellation: false, noiseSuppression: false, autoGainControl: false } });
    } catch (e) {
      this.starting = false;
      return this.h.notice(e?.name === 'NotAllowedError' ? 'Athar is not allowed to use the microphone. Allow it in System Settings, Privacy and Security, Microphone.' : `Could not open the input: ${e?.message || e}`, 9000);
    }
    const ac = new (window.AudioContext || window.webkitAudioContext)(), src = ac.createMediaStreamSource(stream);
    // The input's own channel count: the audio node reports two even for a mono microphone.
    const chs = Math.max(1, Math.min(2, stream.getAudioTracks()[0]?.getSettings?.().channelCount || 1));
    const node = ac.createScriptProcessor(BLOCK, chs, chs), silent = ac.createGain(); silent.gain.value = 0;
    const rec = this.rec = { ac, stream, node, chs, sr: ac.sampleRate, blocks: Array.from({ length: chs }, () => []), frames: 0, peaks: [], level: 0 };
    node.onaudioprocess = (e) => {
      if (this.rec !== rec) return;
      this.feed(rec, Array.from({ length: chs }, (_, c) => e.inputBuffer.getChannelData(c)));
      if (rec.frames / rec.sr >= MAX_SECONDS) { this.h.notice('Stopped at ten minutes, the longest a single take can be.', 8000); this.stop().catch(this.h.fail); }
    };
    src.connect(node); node.connect(silent); silent.connect(ac.destination);
    this.starting = false; this.h.onChange?.();
    this.btn.classList.add('on'); this.btn.querySelector('span:last-child').textContent = 'Stop'; this.btn.querySelector('.i').textContent = 'stop';
    this.listDevices().catch(() => { });
    const tick = () => { if (this.rec !== rec) return; this.clock.textContent = fmtTime(rec.frames / rec.sr); this.meter.firstChild.style.width = `${Math.min(100, Math.sqrt(rec.level) * 100)}%`; this.meter.classList.toggle('hot', rec.level >= 0.99); this.drawWave(rec.peaks); requestAnimationFrame(tick); };
    tick();
  }

  // One block of incoming sound: kept for the take, and measured for the drawing and the meter.
  feed(rec, data) {
    const n = data[0].length, step = Math.ceil(n / BARS_PER_BLOCK);
    let level = 0;
    for (let b = 0; b < n; b += step) {
      let peak = 0;
      for (const d of data) for (let i = b; i < Math.min(n, b + step); i++) { const v = Math.abs(d[i]); if (v > peak) peak = v; }
      rec.peaks.push(peak); if (peak > level) level = peak;
    }
    data.forEach((d, c) => rec.blocks[c].push(d.slice()));
    rec.frames += n; rec.level = level;
  }

  async stop() {
    const rec = this.rec; if (!rec) return;
    this.rec = null;
    rec.node.disconnect(); rec.stream.getTracks().forEach(t => t.stop()); rec.ac.close().catch(() => { });
    this.btn.classList.remove('on'); this.btn.querySelector('span:last-child').textContent = 'Record'; this.btn.querySelector('.i').textContent = 'fiber_manual_record';
    this.meter.firstChild.style.width = '0';
    if (!rec.frames) return;
    const channels = rec.blocks.map((parts) => { const out = new Float32Array(rec.frames); let at = 0; for (const p of parts) { out.set(p, at); at += p.length; } return out; });
    const stamp = new Date(), pad = (n) => String(n).padStart(2, '0');
    this.takes.unshift({ id: ++this.n, name: `Take ${stamp.getFullYear()}-${pad(stamp.getMonth() + 1)}-${pad(stamp.getDate())} ${pad(stamp.getHours())}.${pad(stamp.getMinutes())}.${pad(stamp.getSeconds())}`, channels, sr: rec.sr, peaks: rec.peaks, saved: null });
    this.paintList(); this.drawWave(rec.peaks); this.h.onChange?.();
  }

  drawWave(peaks = []) {
    const cv = this.canvas, dpr = window.devicePixelRatio || 1, W = cv.clientWidth, H = cv.clientHeight;
    if (!W || !H) return;
    if (cv.width !== W * dpr || cv.height !== H * dpr) { cv.width = W * dpr; cv.height = H * dpr; }
    const g = cv.getContext('2d'), css = getComputedStyle(document.documentElement);
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.fillStyle = css.getPropertyValue('--wave-bg'); g.fillRect(0, 0, W, H);
    g.fillStyle = css.getPropertyValue('--line'); g.fillRect(0, H / 2, W, 1);
    g.fillStyle = css.getPropertyValue('--wave');
    // The newest sound is at the right edge, and older sound scrolls off to the left.
    // Quiet sound is drawn taller than it measures, the way hearing works, so a soft voice still shows.
    const bars = Math.floor(W / 2), from = Math.max(0, peaks.length - bars);
    for (let i = from; i < peaks.length; i++) { const hgt = Math.max(1, Math.pow(Math.min(1, peaks[i]), 0.4) * (H - 10)); g.fillRect(W - (peaks.length - i) * 2, (H - hgt) / 2, 1.5, hgt); }
  }

  wav(t) { return new Blob([encodeWav(t.channels, t.sr, 24)], { type: 'audio/wav' }); }

  play(t) {
    this.h.stopOthers();
    const same = this.playing?.t === t;
    this.endPlay();
    if (same) return this.paintList();
    const url = URL.createObjectURL(this.wav(t)), a = new Audio(url); a.volume = this.h.volume();
    a.onended = () => { if (this.playing?.a === a) { this.endPlay(); this.paintList(); } };
    this.playing = { t, a, url }; a.play().catch(() => { }); this.paintList();
  }
  // The player holds a full copy of the take, which is released as soon as it stops.
  endPlay() { const p = this.playing; if (!p) return; this.playing = null; p.a.pause(); p.a.removeAttribute('src'); URL.revokeObjectURL(p.url); }
  stopPlay() { if (this.playing) { this.endPlay(); this.paintList(); } }

  async save(t) {
    const [folders, projects] = await Promise.all([this.h.api('/api/folders'), this.h.api('/api/projects')]);
    const options = [...projects.filter(p => p.folder).map(p => [`p:${p.id}`, `Project: ${p.name}`]), ...folders.filter(f => !f.offline).map(f => [`f:${f.path}`, `Library: ${f.path.split('/').pop()}`])];
    if (!options.length) return this.h.notice('Add a folder to the Library first, with Add folder on the Library page. Takes are saved there.');
    const v = await this.h.modal(`Save ${t.name}`, [{ label: 'Name', value: t.name }, { label: 'Into', options, value: this.lastDest || undefined }]);
    if (!v || !v[0].trim()) return;
    this.lastDest = v[1];
    const proj = v[1].startsWith('p:') ? projects.find(p => `p:${p.id}` === v[1]) : null, dir = proj ? proj.folder : v[1].slice(2);
    const fd = new FormData(), name = `${v[0].trim()}.wav`;
    fd.append('file', this.wav(t), name); fd.append('dest_dir', dir); fd.append('name', name);
    const path = (await this.h.api('/api/save', { method: 'POST', body: fd })).path;
    if (proj) await this.h.api(`/api/projects/${proj.id}/items`, { json: { path } }).catch(() => { });
    t.saved = path; t.name = v[0].trim();
    this.h.toast(`Saved ${name}`); this.h.onSave(); this.paintList(); this.h.onChange?.();
  }

  paintList() {
    const { el, ib } = this.h;
    this.list.replaceChildren(...(this.takes.length ? this.takes.map(t => el('div', { class: 'prow rec-row' },
      ib(this.playing?.t === t ? 'stop' : 'play_arrow', 'Play', () => this.play(t)),
      el('span', { class: 'qn' }, t.name),
      el('span', { class: 'qd' }, [fmtTime(t.channels[0].length / t.sr), t.channels.length > 1 ? 'stereo' : 'mono', t.saved ? 'saved' : 'not saved'].join(' · ')),
      ib('edit', 'Open in the editor', () => { this.stopPlay(); this.h.edit(t); }),
      ib(t.saved ? 'check' : 'save', t.saved ? `Saved as ${t.saved}` : 'Save into your library', () => this.save(t).catch(this.h.fail), t.saved ? 'on' : ''),
      ib('close', 'Discard this take', async () => { if (t.saved || await this.h.confirmModal(`Discard ${t.name}?`, 'It has not been saved, so the recording will be gone.', 'Discard')) { if (this.playing?.t === t) this.stopPlay(); this.takes = this.takes.filter(x => x !== t); this.paintList(); this.h.onChange?.(); } })))
      : [el('div', { class: 'empty' }, el('h3', {}, 'Nothing recorded yet'), el('p', { class: 'muted' }, 'Press Record, make a sound, press Stop. Takes stay here until you save or discard them.'))]));
  }

  hasUnsaved() { return !!this.rec || this.takes.some(t => !t.saved); }
  handleKey(e) { if (e.key === 'r' || e.key === 'R') { e.preventDefault(); this.toggle().catch(this.h.fail); return true; } return false; }
}
