import { resample, mixMono, encodeWav, lin, db } from './dsp.js';

// The arranger's arithmetic, kept apart from the page so it can run in the worker and in tests.
export const MAX_LANES = 16, MAX_HITS = 64, MAX_SECONDS = 10 * 60;
export const BEATS_PER_BAR = 4, STEPS_PER_BEAT = 4;
export const MIN_GAIN_DB = -24, MAX_GAIN_DB = 6;
// A mix that goes over full scale is turned down to this peak when it is saved.
const CEILING = lin(-0.1);

export const barSeconds = (bpm) => 60 / bpm * BEATS_PER_BAR;
export const stepSeconds = (bpm) => 60 / bpm / STEPS_PER_BEAT;

/** The nearest sixteenth note to t. */
export function snapTime(t, bpm) {
  const step = stepSeconds(bpm);
  return Math.max(0, Math.round(t / step) * step);
}

/** Where the last sound ends, in seconds. */
export function contentEnd(lanes) {
  let end = 0;
  for (const l of lanes) for (const at of l.hits) end = Math.max(end, at + l.duration);
  return end;
}

/** The length of one pass. Off the grid it is the end of the last sound. On the grid it is the
    chosen number of bars, or with none chosen, enough whole bars to hold every sound. */
export function passLength(a) {
  const end = contentEnd(a.lanes);
  if (!a.grid) return end;
  const bar = barSeconds(a.bpm);
  return (a.bars || Math.max(1, Math.ceil(end / bar - 1e-6))) * bar;
}

/** A lane sounds unless it is muted, or another lane is soloed and it is not. */
export function audible(lane, lanes) {
  return !lane.mute && (lane.solo || !lanes.some(l => l.solo));
}

/** The hits of a lane that start inside one pass. */
export function hitsInPass(lane, length) {
  return lane.hits.filter(at => at < length - 1e-6);
}

/** Sums lanes into one sound. A lane is { chs, sr, gain, hits }: its channels, their sample rate,
    a linear gain and its start times in seconds, and optionally fades, one { in, out } in
    seconds per hit. A mono lane goes to both sides of a stereo mix. */
export function mixdown(lanes, sr, seconds) {
  const n = Math.max(1, Math.round(seconds * sr));
  const channels = Array.from({ length: lanes.some(l => l.chs.length > 1) ? 2 : 1 }, () => new Float32Array(n));
  for (const l of lanes) {
    const chs = l.sr === sr ? l.chs : resample(l.chs, l.sr, sr);
    l.hits.forEach((at, k) => {
      const len = chs[0].length, start = Math.round(at * sr), m = Math.min(len, n - start);
      const f = l.fades?.[k], fin = Math.round((f?.in || 0) * sr), fout = Math.round((f?.out || 0) * sr);
      channels.forEach((o, c) => {
        const d = chs[Math.min(c, chs.length - 1)];
        if (!fin && !fout) { for (let i = 0; i < m; i++) o[start + i] += d[i] * l.gain; return; }
        // Equal-power curves, so the level stays even across the overlap.
        for (let i = 0; i < m; i++) {
          let g = l.gain;
          if (i < fin) g *= Math.sin(i / fin * Math.PI / 2);
          if (i >= len - fout) g *= Math.cos((i - (len - fout)) / fout * Math.PI / 2);
          o[start + i] += d[i] * g;
        }
      });
    });
  }
  return channels;
}

/** Turns the sound down, in place, when it goes over full scale. Returns how far over it was, in dB. */
export function fitPeak(channels) {
  let peak = 0;
  for (const d of channels) for (let i = 0; i < d.length; i++) { const v = Math.abs(d[i]); if (v > peak) peak = v; }
  if (peak <= 1) return 0;
  const k = CEILING / peak;
  for (const d of channels) for (let i = 0; i < d.length; i++) d[i] *= k;
  return db(peak);
}

/** The whole save: mix, fold to mono if asked, fade the cut end, convert the rate, fit the peak, encode. */
export function renderMix({ lanes, sr, seconds, rate, bits, mono, fadeEnd }) {
  let c = mixdown(lanes, sr, seconds);
  if (mono && c.length > 1) c = [mixMono(c)];
  if (fadeEnd) {
    const n = c[0].length, f = Math.min(n, Math.round(sr * 0.005));
    for (const d of c) for (let i = 0; i < f; i++) d[n - f + i] *= 1 - (i + 1) / f;
  }
  if (rate !== sr) c = resample(c, sr, rate);
  const over = fitPeak(c);
  return { wav: encodeWav(c, rate, bits), over };
}
