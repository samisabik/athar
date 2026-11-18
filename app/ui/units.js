// Display arithmetic for the editor and its effect panels: dB scales, note names and frequency
// labels. Effect curves and the sound are computed natively.

export const db = (/** @type {number} */ x) => 20 * Math.log10(Math.max(x, 1e-12));
// The noise reduction's analysis frame, which sets the frequency of each profile bin.
export const NR_N = 2048;

export const NOTE_NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
/** A MIDI note as it is written, octave included: 69 is "A4". @param {number} midi */
export const midiName = (midi) => NOTE_NAMES[((midi % 12) + 12) % 12] + (Math.floor(midi / 12) - 1);
/** @param {number} midi */
export const midiHz = (midi) => 440 * Math.pow(2, (midi - 69) / 12);
/** @param {number} hz @returns {number} The MIDI note number, fractions included. */
export const hzMidi = (hz) => 69 + 12 * Math.log2(hz / 440);
/** The nearest note to a frequency: its name, MIDI number, frequency, and how many cents off it is. @param {number} hz */
export function noteAt(hz) {
  const m = hzMidi(hz), midi = Math.round(m);
  return { midi, name: midiName(midi), hz: midiHz(midi), cents: Math.round((m - midi) * 100) };
}
/** A frequency as an axis writes it: 500, 2k. @param {number} hz */
export const hzLabel = (hz) => hz >= 1000 ? hz / 1000 + 'k' : String(hz);
