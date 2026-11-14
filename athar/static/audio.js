export { mixMono, fft, resample } from './dsp.js';

const AudioCtx = window.AudioContext || window.webkitAudioContext;
// Importers see the current engine: these are replaced when it is rebuilt.
export let ctx = new AudioCtx();
// Everything the editor plays goes through this, so the volume dial covers it too.
export let out = ctx.createGain();
out.connect(ctx.destination);

// An engine left running for a long time can go silent while it still reports that it is
// playing, for example after the output device changes. Nothing in the page can detect that, so
// the engine is rebuilt whenever the editor starts a sound after a pause.
const STALE_MS = 15000, GESTURE_MS = 1500;
let lastSound = performance.now(), lastGesture = 0, deviceChanged = false;
export const onAudioRebuilt = [];
function rebuild() {
  const old = ctx, gain = out.gain.value;
  ctx = new AudioCtx();
  out = ctx.createGain(); out.gain.value = gain; out.connect(ctx.destination);
  old.close?.().catch(() => { });
  deviceChanged = false;
  for (const fn of onAudioRebuilt) fn();
}
// A new engine may only start sounding when it is made in answer to a click or a key press, so
// the rebuild waits for one. A sound started by a timer keeps the engine it has.
for (const type of ['pointerdown', 'keydown']) document.addEventListener(type, () => { lastGesture = performance.now(); }, true);
/** Call right before starting a sound in the editor. */
export function readyAudio() {
  const now = performance.now(), stale = deviceChanged || now - lastSound > STALE_MS;
  if (ctx.state === 'closed' || (stale && now - lastGesture < GESTURE_MS)) rebuild();
  lastSound = now;
  ctx.resume();
}
/** Call while a sound is playing, so a long playback does not count as a pause. */
export function soundPlaying() { lastSound = performance.now(); }
// A change of output device is remembered until the next rebuild, even if sound is playing now.
navigator.mediaDevices?.addEventListener?.('devicechange', () => { deviceChanged = true; });

async function fetchBytes(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${r.status} ${await r.text()}`);
  return r.arrayBuffer();
}

// Decoding through an OfflineAudioContext at the file's own rate keeps the
// original sample rate; a realtime context would resample to the device rate.
async function decodeAt(bytes, sr) {
  const c = sr ? new OfflineAudioContext(1, 1, sr) : ctx;
  return c.decodeAudioData(bytes);
}

export async function decodeUrl(url, fallbackUrl, sr) {
  try {
    return await decodeAt(await fetchBytes(url), sr);
  } catch (e) {
    if (!fallbackUrl) throw e;
    return await decodeAt(await fetchBytes(fallbackUrl), sr);
  }
}

/** The decoded buffer's own sample arrays, not copies: the editor edits them in place, and the
    buffer they came from is not used again. */
export function channelsOf(buffer) {
  return Array.from({ length: buffer.numberOfChannels }, (_, i) => buffer.getChannelData(i));
}

export function toBuffer(channels, sr) {
  const b = ctx.createBuffer(channels.length, Math.max(1, channels[0].length), sr);
  channels.forEach((d, i) => b.copyToChannel(d, i));
  return b;
}

export function fmtTime(sec, precise = false) {
  if (sec == null || !isFinite(sec)) return '';
  if (sec < 60) return precise ? sec.toFixed(3) + 's' : sec < 10 ? sec.toFixed(2) + 's' : sec.toFixed(1) + 's';
  const m = Math.floor(sec / 60), s = sec - m * 60;
  return `${m}:${(precise ? s.toFixed(3) : s.toFixed(0)).padStart(precise ? 6 : 2, '0')}`;
}
