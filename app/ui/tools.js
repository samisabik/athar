// Tools that take values before they run. Each opens in the panel under the waveform, with one
// ruler per field. A field's `f` names the number input in #edSettings that holds its value.

/** @typedef {{ f: string, label: string, unit: string, min: number, max: number, step: number }} ToolField */
/** @typedef {{ title: string, fields: ToolField[], audible?: boolean, presets?: [string, number][] }} ToolSpec */
/** Reads a field's current value by its name. @typedef {(field: string) => number} ReadField */
/** @typedef {{ a: number, b: number, fadeIn: boolean }} FadeSpan */

/** @type {Record<string, ToolField>} */
const F = {
  fadeinlen: { f: 'fadeinlen', label: 'Fade in', unit: ' ms', min: 0, max: 5000, step: 1 },
  fadeoutlen: { f: 'fadeoutlen', label: 'Fade out', unit: ' ms', min: 0, max: 5000, step: 1 },
  norm: { f: 'norm', label: 'Peak', unit: ' dB', min: -24, max: 0, step: 0.5 },
  gain: { f: 'gain', label: 'Gain', unit: ' dB', min: -24, max: 24, step: 0.5 },
  speed: { f: 'speed', label: 'Speed', unit: '%', min: 25, max: 400, step: 1 },
  thresh: { f: 'thresh', label: 'Silence below', unit: ' dB', min: -96, max: -10, step: 1 },
  trimthresh: { f: 'trimthresh', label: 'Silence below', unit: ' dB', min: -96, max: -10, step: 1 },
  gap: { f: 'gap', label: 'Slice gap', unit: ' ms', min: 20, max: 2000, step: 10 },
  lufs: { f: 'lufs', label: 'Loudness', unit: ' LUFS', min: -36, max: -6, step: 0.5 },
  stretchlen: { f: 'stretchlen', label: 'Length', unit: '%', min: 25, max: 400, step: 1 },
  semis: { f: 'semis', label: 'Pitch', unit: ' st', min: -24, max: 24, step: 1 },
  tunesemis: { f: 'tunesemis', label: 'Shift', unit: ' st', min: -12, max: 12, step: 0.01 },
  xfade: { f: 'xfade', label: 'Crossfade', unit: ' ms', min: 1, max: 1000, step: 1 },
  clicksens: { f: 'clicksens', label: 'Sensitivity', unit: '', min: 1, max: 10, step: 1 },
  onsetsens: { f: 'onsetsens', label: 'Sensitivity', unit: '', min: 1, max: 10, step: 1 },
  bpm: { f: 'bpm', label: 'Tempo', unit: ' BPM', min: 40, max: 300, step: 0.5 },
  perbeat: { f: 'perbeat', label: 'Slices per beat', unit: '', min: 0.25, max: 8, step: 0.25 },
};

/** @type {Record<string, ToolSpec>} */
export const TOOLS = {
  fade: { title: 'Fade', fields: [F.fadeinlen, F.fadeoutlen], audible: true },
  normalize: { title: 'Normalize peak', fields: [F.norm], audible: true, presets: [['-0.1 dB', -0.1], ['-1 dB', -1], ['-3 dB', -3], ['-6 dB', -6]] },
  loudnorm: { title: 'Normalize loudness', fields: [F.lufs], audible: true, presets: [['-14', -14], ['-16', -16], ['-18', -18], ['-23', -23]] },
  gain: { title: 'Gain', fields: [F.gain], audible: true, presets: [['-6', -6], ['-3', -3], ['+3', 3], ['+6', 6]] },
  speed: { title: 'Speed', fields: [F.speed], audible: true, presets: [['Octave down', 50], ['¾', 75], ['1.5×', 150], ['Octave up', 200]] },
  stretch: { title: 'Stretch and pitch', fields: [F.stretchlen, F.semis], audible: true, presets: [['Half', 50], ['¾', 75], ['1.5×', 150], ['Double', 200]] },
  tune: { title: 'Tune to note', fields: [F.tunesemis], audible: true },
  loopxfade: { title: 'Make it loop', fields: [F.xfade], audible: true, presets: [['10 ms', 10], ['50 ms', 50], ['200 ms', 200], ['500 ms', 500]] },
  declick: { title: 'Remove clicks', fields: [F.clicksens], audible: true },
  trimsilence: { title: 'Trim silent ends', fields: [F.trimthresh] },
  autoslice: { title: 'Slice at silences', fields: [F.thresh, F.gap] },
  transients: { title: 'Slice at hits', fields: [F.onsetsens] },
  gridslice: { title: 'Slice on a tempo grid', fields: [F.bpm, F.perbeat] },
};

/** @param {string | null | undefined} name @returns {ToolSpec | undefined} */
export const toolSpec = (name) => name ? TOOLS[name] : undefined;

// The shortest range Stretch works on, in seconds.
const STRETCH_MIN_S = 0.1;

/**
 * Where the fades go: the fade in over the first part of the range [a, b), the fade out over
 * its last part, each as long as its ruler says.
 * @param {ReadField} num @param {readonly [number, number]} range @param {number} sr @returns {[FadeSpan, FadeSpan]}
 */
export function fadeSpans(num, [a, b], sr) {
  const len = (/** @type {string} */ f) => Math.max(0, Math.min(b - a, Math.round((num(f) || 0) / 1000 * sr)));
  return [{ a, b: a + len('fadeinlen'), fadeIn: true }, { a: b - len('fadeoutlen'), b, fadeIn: false }];
}

/**
 * The edit a tool makes over [a, b) with its current values, as sent to the native side. The
 * preview, the audition and Apply all send this one. Null when the values cannot make an edit,
 * and for the tools that are not edits of the sound (the slicers, Trim silent ends).
 * @param {string} name @param {ReadField} num @param {readonly [number, number]} range @param {number} sr
 * @returns {Record<string, unknown> | null}
 */
export function toolEdit(name, num, [a, b], sr) {
  switch (name) {
    case 'gain': return { kind: 'gain', a, b, db: num('gain') || 0 };
    case 'normalize': return { kind: 'normalize', a, b, db: num('norm') };
    case 'loudnorm': return { kind: 'loudnorm', a, b, target: num('lufs') };
    case 'fade': return { kind: 'fade', spans: fadeSpans(num, [a, b], sr).filter(m => m.b > m.a) };
    case 'declick': return { kind: 'declick', a, b, sensitivity: num('clicksens') || 5 };
    // Speed and Tune play the range faster or slower: pitch and length change together, as on a sampler.
    case 'speed': { const pct = num('speed'); return pct > 0 ? { kind: 'respeed', a, b, ratio: pct / 100 } : null; }
    case 'tune': return { kind: 'respeed', a, b, ratio: Math.pow(2, (num('tunesemis') || 0) / 12) };
    case 'stretch': { const pct = num('stretchlen'); return pct > 0 && b - a >= sr * STRETCH_MIN_S ? { kind: 'stretch', a, b, length: pct / 100, semitones: num('semis') || 0 } : null; }
    case 'loopxfade': { const n = Math.round(num('xfade') / 1000 * sr); return b - a >= 4 && n >= 1 && n <= (b - a) / 2 ? { kind: 'loopxfade', a, b, n } : null; }
  }
  return null;
}

/**
 * Why Apply would not run a tool's edit: a sentence for the user, '' when the values leave the
 * sound as it is (nothing to do, nothing to say), or null when the edit can go ahead.
 * @param {string} name @param {ReadField} num @param {readonly [number, number]} range @param {number} sr
 * @returns {string | null}
 */
export function toolRefusal(name, num, [a, b], sr) {
  switch (name) {
    case 'fade': return fadeSpans(num, [a, b], sr).some(m => m.b > m.a) ? null : 'Set a fade in or a fade out length first';
    case 'speed': { const pct = num('speed'); return pct > 0 && pct !== 100 && b - a >= 2 ? null : ''; }
    case 'tune': return num('tunesemis') && b - a >= 2 ? null : '';
    case 'stretch': {
      const pct = num('stretchlen');
      if (!(pct > 0) || (pct === 100 && !num('semis'))) return '';
      return b - a < sr * STRETCH_MIN_S ? 'Select at least a tenth of a second' : null;
    }
    case 'loopxfade': {
      const n = Math.round(num('xfade') / 1000 * sr);
      if (b - a < 4 || n < 1) return '';
      return n > (b - a) / 2 ? 'The crossfade can be at most half of the loop' : null;
    }
  }
  return null;
}
