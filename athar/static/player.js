import { invoke, now, play, stop, onEnded } from './engine.js';

/** The library's preview player, played by the native engine. It answers to what main.js uses
    of an <audio> element: src, play(), pause(), currentTime, duration, loop, onended. */
export class Player {
  constructor() {
    this._src = ''; this._loop = false;
    /** @type {{ key: string, duration: number } | null} */ this.sound = null;
    /** @type {number | null} */ this.voice = null;
    /** @type {Promise<void> | null} */ this.load = null;
    /** @type {string | null} */ this.token = null;
    this.from = 0; this.at = 0; this.pausedAt = 0; this.paused = true;
    /** @type {(() => void) | null} */ this.onended = null;
    /** @type {(() => void) | null} */ this.onerror = null;
    /** @type {(() => void) | null} */ this.oncapped = null;
    /** @type {(() => void)[]} */ this.waiting = [];
  }
  get src() { return this._src; }
  // A file's path; the player was an <audio> element once, and keeps its names.
  set src(path) {
    this.halt(); this.paused = true; this.pausedAt = 0; this.sound = null; this._src = path;
    const load = this.load = invoke('audio_preview', { path }).then((r) => {
      if (load !== this.load) return;
      this.sound = { key: r.key, duration: r.length };
      if (r.capped) this.oncapped?.();
      this.waiting.splice(0).forEach(f => f());
    }, () => { if (load === this.load) this.onerror?.(); });
  }
  get duration() { return this.sound?.duration || NaN; }
  get loop() { return this._loop; }
  set loop(on) {
    if (on === this._loop) return;
    this._loop = on;
    if (!this.paused) { const t = this.currentTime; this.halt(); this.begin(t); }
  }
  get currentTime() {
    if (this.paused || !this.sound) return this.pausedAt;
    const d = this.sound.duration || Infinity, t = this.from + Math.max(0, now() - this.at);
    return this._loop ? t % d : Math.min(t, d);
  }
  set currentTime(t) {
    if (this.paused) { this.pausedAt = t; return; }
    this.halt(); this.begin(t);
  }
  /** @param {string} type @param {() => void} f */
  addEventListener(type, f) {
    if (type === 'loadedmetadata') { if (this.sound?.duration) f(); else this.waiting.push(f); }
  }
  async play() {
    if (!this.paused) return;
    this.paused = false;
    await this.load;
    if (!this.paused && this.voice == null) this.begin(this.pausedAt);
  }
  pause() { if (!this.paused) this.pausedAt = this.currentTime; this.paused = true; this.halt(); }
  halt() { const v = this.voice; this.voice = null; this.token = null; if (v != null) stop(v); }
  /** @param {number} t */
  async begin(t) {
    if (!this.sound) return;
    const token = this.token = String(Math.random()), d = this.sound.duration;
    this.from = t; this.at = now() + 0.03;
    const id = await play(this.sound.key, { offset: t, loop: this._loop ? [0, d] : null });
    if (this.token !== token) return void stop(id);
    this.voice = id; this.at = now();
    onEnded(id, () => {
      if (this.voice !== id) return;
      this.voice = null; this.paused = true; this.pausedAt = 0; this.onended?.();
    });
  }
}
