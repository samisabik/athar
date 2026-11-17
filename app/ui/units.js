// Display arithmetic for the effect panels: dB scales and note names. Effect curves and the sound
// are computed natively.

export const db = (/** @type {number} */ x) => 20 * Math.log10(Math.max(x, 1e-12));
// The noise reduction's analysis frame, which sets the frequency of each profile bin.
export const NR_N = 2048;

const NOTES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
/** The nearest note to a frequency: its name, MIDI number, frequency, and how many cents off it is. */
/** @param {number} hz */
export function noteAt(hz) {
  const m = 69 + 12 * Math.log2(hz / 440), midi = Math.round(m);
  return { midi, name: NOTES[((midi % 12) + 12) % 12] + (Math.floor(midi / 12) - 1), hz: 440 * Math.pow(2, (midi - 69) / 12), cents: Math.round((m - midi) * 100) };
}
