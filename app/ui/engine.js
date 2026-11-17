// The native audio engine, seen from the page: its clock, which sounds have ended, and the few
// requests that are not the editor's own (see the ed_* commands for those).

const T = window.__TAURI_INTERNALS__;
/** @param {string} cmd @param {unknown} [args] @param {{ headers?: Record<string, string> }} [opts] @returns {Promise<any>} */
export const invoke = (cmd, args, opts) => T.invoke(cmd, args, opts);

/** Calls cb with the payload of every `event` the native side sends. */
/** @param {string} event @param {(payload: any) => void} cb */
export function listen(event, cb) {
  return invoke('plugin:event|listen', { event, target: { kind: 'Any' }, handler: T.transformCallback((e) => cb(e.payload)) });
}

let base = { t: 0, at: performance.now() }, last = 0;
export let rate = 48000;
/** @type {Map<number, () => void>} */
const waiting = new Map();

// The engine sends its clock a few times a second, and at once when sounds end.
listen('engine', (/** @type {{ now: number, rate: number, ended: number[] }} */ s) => {
  base = { t: s.now, at: performance.now() }; rate = s.rate || rate;
  for (const id of s.ended) { const f = waiting.get(id); waiting.delete(id); f?.(); }
});
// A page that starts fresh, after a launch or a reload, starts in silence.
invoke('audio_reset').catch(() => { });
invoke('audio_info').then((s) => { base = { t: s.now, at: performance.now() }; rate = s.rate || rate; }).catch(() => { });

/** The engine's clock in seconds, counted on from its last report; it never runs backwards. */
export function now() {
  last = Math.max(last, base.t + (performance.now() - base.at) / 1000);
  return last;
}

/** Calls f once the sound `id` has finished playing. */
/** @param {number} id @param {() => void} f */
export function onEnded(id, f) { waiting.set(id, f); }
/** @param {number} id */
export function forget(id) { waiting.delete(id); }

export const setVolume = (/** @type {number} */ v) => invoke('audio_volume', { v }).catch(() => { });

/** Plays a sound the engine holds (`key`) from `offset` seconds; resolves with its id. */
/** @param {string} key @param {{ offset?: number, duration?: number | null, loop?: [number, number] | null }} [o] @returns {Promise<number>} */
export async function play(key, { offset = 0, duration = null, loop = null } = {}) {
  const id = Math.floor(Math.random() * 2 ** 31) + 2 ** 32;
  await invoke('audio_start', { req: { id, sound: key, when: 0, offset, duration, looped: loop, chain: [] } });
  return id;
}
export const stop = (/** @type {number} */ id) => { forget(id); return invoke('audio_stop', { id, when: 0 }).catch(() => { }); };
