// Tools that take values before they run. Each opens in the panel under the waveform, with one
// ruler per field. A field's `f` names the number input in #edSettings that holds its value.
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
