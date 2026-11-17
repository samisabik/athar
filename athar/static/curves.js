// The arithmetic the effect panels draw with: transfer curves and note names. The sound itself
// is processed natively.

export const db = (/** @type {number} */ x) => 20 * Math.log10(Math.max(x, 1e-12));
export const lin = (/** @type {number} */ d) => Math.pow(10, d / 20);
// The noise reduction's analysis frame, which sets the frequency of each profile bin.
export const NR_N = 2048;

/** @typedef {{ threshold: number, ratio: number, knee: number }} Comp */
/** The compressor's output level for an input level, in dB. @param {number} x @param {Comp} c */
export function compCurve(x, { threshold, ratio, knee }) {
  const over = x - threshold;
  if (2 * over < -knee) return x;
  if (knee > 0 && 2 * Math.abs(over) <= knee) return x + (1 / ratio - 1) * (over + knee / 2) ** 2 / (2 * knee);
  return threshold + over / ratio;
}

/** @param {Comp} p */
export function autoMakeup(p) { return -(p.threshold - compCurve(0, p)) * 0.5; }

/** @param {number} inDb @param {{ drive: number, ceiling: number }} p */
export function satCurve(inDb, { drive, ceiling }) {
  const c = lin(ceiling);
  return db(c * Math.tanh(lin(inDb + drive) / c));
}

const NOTES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
/** The nearest note to a frequency: its name, MIDI number, frequency, and how many cents off it is. */
/** @param {number} hz */
export function noteAt(hz) {
  const m = 69 + 12 * Math.log2(hz / 440), midi = Math.round(m);
  return { midi, name: NOTES[((midi % 12) + 12) % 12] + (Math.floor(midi / 12) - 1), hz: 440 * Math.pow(2, (midi - 69) / 12), cents: Math.round((m - midi) * 100) };
}
