// Runs athar/static/dsp.js and arrange.js on fixed test signals and writes the inputs and outputs
// to <out>/cases.json and <out>/data.bin for tests/dsp_parity.rs.
// Usage: node dsp_ref.mjs <out dir>
import { writeFileSync, mkdirSync } from 'node:fs';
import * as dsp from './js/dsp.js';
import { renderMix, mixdown, fitPeak } from './js/mix.js';

const outDir = process.argv[2];
mkdirSync(outDir, { recursive: true });

// Fixed dither: encodeWav seeds its xorshift with (Math.random() * 0xffffffff) >>> 0 || 1.
const R = 0.3141592653;
Math.random = () => R;
const SEED = ((R * 0xffffffff) >>> 0) || 1;

const parts = [];
let offset = 0;
function blob(arr, t = 'f32') {
  const a = t === 'f32' ? Float32Array.from(arr) : new Uint8Array(arr);
  const b = Buffer.from(a.buffer, a.byteOffset, a.byteLength);
  parts.push(b);
  const ref = { off: offset, len: a.length, t };
  offset += b.length;
  return ref;
}
const refs = (chs) => chs.map(d => blob(d));

function mulberry(seed) {
  return () => {
    seed |= 0; seed = seed + 0x6d2b79f5 | 0;
    let t = Math.imul(seed ^ seed >>> 15, 1 | seed);
    t = t + Math.imul(t ^ t >>> 7, 61 | t) ^ t;
    return ((t ^ t >>> 14) >>> 0) / 4294967296;
  };
}
const f32 = (n, f) => Float32Array.from({ length: n }, (_, i) => f(i));
const sine = (sr, hz, secs, amp = 0.5) => f32(Math.round(sr * secs), i => amp * Math.sin(2 * Math.PI * hz * i / sr));
const noise = (n, amp, seed) => { const r = mulberry(seed); return f32(n, () => amp * (2 * r() - 1)); };
function drums(sr, secs, seed = 7) {
  const r = mulberry(seed), n = Math.round(sr * secs), d = new Float32Array(n);
  for (let at = Math.round(sr * 0.05); at < n; at += Math.round(sr * 0.25)) {
    const snare = (at / Math.round(sr * 0.25)) % 2 === 1;
    for (let i = 0; i < sr * 0.2 && at + i < n; i++) {
      const t = i / sr, env = Math.exp(-t * (snare ? 30 : 18));
      d[at + i] += snare ? 0.6 * env * (2 * r() - 1) : 0.9 * env * Math.sin(2 * Math.PI * (50 * t + 400 * (1 - Math.exp(-t * 40)) / 40));
    }
  }
  return d;
}
function eightOhEight(sr, secs) {
  let ph = 0;
  return f32(Math.round(sr * secs), (i) => {
    const t = i / sr, hz = 55 + 65 * Math.exp(-t * 60);
    ph += 2 * Math.PI * hz / sr;
    return 0.8 * Math.exp(-t * 1.5) * Math.sin(ph);
  });
}
function clicks(sr, secs) {
  const d = sine(sr, 220, secs, 0.3);
  for (const at of [0.1, 0.33, 0.5, 0.71]) { const k = Math.round(at * sr); d[k] += 0.8; d[k + 1] -= 0.5; }
  d[d.length - 3] += 0.9;
  return d;
}

const add2 = (a, b) => Float32Array.from(a, (v, i) => v + b[i]);
const S = {
  sine44: [sine(44100, 440, 1)],
  stereo48: [add2(sine(48000, 440, 0.75), noise(36000, 0.05, 3)), sine(48000, 660, 0.75, 0.4)],
  drums44: [drums(44100, 1.5)],
  drumsSt: [drums(44100, 1), drums(44100, 1, 11)],
  silence: [new Float32Array(22050)],
  noise44: [noise(44100, 0.3, 5)],
  hot: [add2(sine(44100, 997, 0.5, 1.4), noise(22050, 0.1, 9))],
  clicks: [clicks(44100, 1)],
  e808: [eightOhEight(44100, 1.5)],
  sine48: [sine(48000, 440, 1)],
  sine22: [sine(22050, 100, 1)],
  sine96: [sine(96000, 1000, 0.5)],
  short1: [new Float32Array([0.5])],
  short3: [new Float32Array([0.1, -0.7, 0.3])],
  short100: [noise(100, 0.5, 13)],
  short500: [noise(500, 0.5, 17)],
  short1000: [noise(1000, 0.5, 19), noise(1000, 0.3, 23)],
  empty: [new Float32Array(0)],
  tri: [sine(44100, 220, 0.2), sine(44100, 330, 0.2), noise(8820, 0.2, 29)],
  padded: [Float32Array.from({ length: 30000 }, (_, i) => (i > 5000 && i < 25000 ? 0.3 * Math.sin(i / 20) : 0.0001) + 0.05)],
};

const cases = [];
const add = (name, fn, sr, chs, args, out) => cases.push({ name, fn, sr, chs: refs(chs), args, out });
const chOut = (chs) => refs(chs);
const num = (x) => (Number.isFinite(x) ? x : null);

// worker.js's op table, with results split into channels / info / bytes.
const ops = {
  compress: ({ chs, sr, p }) => dsp.compress(chs, sr, p),
  maximize: ({ chs, sr, p }) => dsp.maximize(chs, sr, p),
  denoise: ({ chs, sr, p, profile }) => dsp.denoise(chs, sr, profile, p),
  resample: ({ chs, from, to }) => ({ channels: dsp.resample(chs, from, to) }),
  measure: ({ chs, sr }) => dsp.measure(chs, sr),
  stretchPitch: ({ chs, sr, length, semitones }) => ({ channels: dsp.stretchPitch(chs, sr, length, semitones) }),
  declick: ({ chs, sr, sensitivity }) => dsp.declick(chs, sr, sensitivity),
  saturate: ({ chs, sr, p }) => dsp.saturate(chs, sr, p),
  shape: ({ chs, sr, p }) => dsp.shape(chs, sr, p),
  lofi: ({ chs, sr, p }) => dsp.lofi(chs, sr, p),
  pitch: ({ data, sr }) => ({ hz: dsp.detectPitch(data, sr) }),
  onsets: ({ data, sr, sensitivity }) => ({ positions: dsp.onsets(data, sr, sensitivity) }),
  render: ({ chs, sr, rate, bits, mono, prep }) => {
    let c = mono && chs.length > 1 ? [dsp.mixMono(chs)] : chs;
    if (prep) c = dsp.prep(c, sr, prep);
    if (rate !== sr) c = dsp.resample(c, sr, rate);
    return { wav: dsp.encodeWav(c, rate, bits) };
  },
  noiseProfile: ({ chs, a, b }) => ({ profile: Array.from(dsp.noiseProfile(chs, a, b)) }),
  estimateProfile: ({ chs }) => ({ profile: Array.from(dsp.estimateProfile(chs)) }),
  loopCrossfade: ({ chs, n }) => ({ channels: dsp.loopCrossfade(chs, n) }),
};
const MONO = new Set(['pitch', 'onsets']);

function op(name, op, sig, sr, p = {}) {
  const chs = S[sig].map(d => d.slice());
  const args = MONO.has(op) ? { data: chs[0], sr, ...p } : { chs, sr, ...p };
  const r = ops[op](args);
  const out = {};
  if (r.channels) out.channels = chOut(r.channels);
  if (r.wav) out.bytes = blob(new Uint8Array(r.wav), 'u8');
  const info = {};
  for (const [k, v] of Object.entries(r)) {
    if (k === 'channels' || k === 'wav') continue;
    info[k] = typeof v === 'number' ? num(v) : v;
  }
  out.info = info;
  add(`${op}/${name}`, 'run', sr, S[sig], { op, p: op === 'render' ? { ...p, ditherSeed: SEED } : p }, out);
}

const COMP = [
  { threshold: -24, ratio: 4, knee: 6, attack: 10, release: 150, makeup: 0, autoMakeup: true },
  { threshold: -30, ratio: 8, knee: 0, attack: 1, release: 50, makeup: 3, autoMakeup: false },
  { threshold: -12, ratio: 2, knee: 12, attack: 0.01, release: 0.5, makeup: -2, autoMakeup: false },
];
for (const [i, p] of COMP.entries()) for (const [sig, sr] of [['drums44', 44100], ['stereo48', 48000], ['silence', 44100], ['short100', 44100]]) op(`${sig}/${i}`, 'compress', sig, sr, { p });
const MAX = [{ gain: 6, ceiling: -1, release: 60 }, { gain: 12, ceiling: -0.3, release: 5 }, { gain: 0, ceiling: 0, release: 200 }];
for (const [i, p] of MAX.entries()) for (const [sig, sr] of [['drums44', 44100], ['stereo48', 48000], ['hot', 44100], ['short1', 44100], ['short100', 44100]]) op(`${sig}/${i}`, 'maximize', sig, sr, { p });

const prof1 = Array.from(dsp.noiseProfile(S.stereo48, 0, 8000)), prof2 = Array.from(dsp.estimateProfile(S.noise44));
for (const [i, p] of [{ reduction: 12, sensitivity: 1.5 }, { reduction: 24, sensitivity: 3 }].entries()) {
  op(`stereo48/${i}`, 'denoise', 'stereo48', 48000, { p, profile: prof1 });
  op(`drums44/${i}`, 'denoise', 'drums44', 44100, { p, profile: prof2 });
  op(`short100/${i}`, 'denoise', 'short100', 44100, { p, profile: prof2 });
}
for (const [from, to] of [[44100, 48000], [48000, 22050], [1, 2], [2, 1], [133484, 100000], [100000, 100000], [8909, 10000]]) {
  op(`stereo48/${from}-${to}`, 'resample', 'stereo48', 48000, { from, to });
  op(`short3/${from}-${to}`, 'resample', 'short3', 44100, { from, to });
}
op('empty', 'resample', 'empty', 44100, { from: 44100, to: 48000 });
for (const [sig, sr] of [['drums44', 44100], ['stereo48', 48000], ['silence', 44100], ['short1000', 44100], ['hot', 44100], ['tri', 44100], ['short1', 44100], ['empty', 44100]]) op(sig, 'measure', sig, sr);
for (const [length, semitones] of [[1.5, 0], [0.7, 0], [1, 3], [1.25, -5], [1, 0], [2.2, 7]]) {
  op(`drums44/${length}/${semitones}`, 'stretchPitch', 'drums44', 44100, { length, semitones });
  op(`stereo48/${length}/${semitones}`, 'stretchPitch', 'stereo48', 48000, { length, semitones });
  op(`short500/${length}/${semitones}`, 'stretchPitch', 'short500', 44100, { length, semitones });
}
for (const s of [5, 1, 10]) { op(`clicks/${s}`, 'declick', 'clicks', 44100, { sensitivity: s }); op(`drumsSt/${s}`, 'declick', 'drumsSt', 44100, { sensitivity: s }); }
op('short3', 'declick', 'short3', 44100, { sensitivity: 5 });
op('short100', 'declick', 'short100', 44100, { sensitivity: 9 });
for (const [i, p] of [{ drive: 6, ceiling: -1, mix: 100 }, { drive: 18, ceiling: -3, mix: 50 }, { drive: 3, ceiling: 0 }].entries()) {
  for (const [sig, sr] of [['stereo48', 48000], ['drums44', 44100], ['short3', 44100]]) op(`${sig}/${i}`, 'saturate', sig, sr, { p });
}
for (const [i, p] of [{ attack: 30, tail: 0 }, { attack: -60, tail: 80 }, { attack: 100, tail: -100 }].entries()) {
  for (const [sig, sr] of [['drumsSt', 44100], ['stereo48', 48000], ['short100', 44100]]) op(`${sig}/${i}`, 'shape', sig, sr, { p });
}
for (const [i, p] of [{ bits: 12, rate: 26 }, { bits: 4, rate: 8 }, { bits: 24, rate: 44.1 }, { bits: 8.5, rate: 0.2 }, { bits: 2, rate: 30 }].entries()) {
  for (const [sig, sr] of [['stereo48', 48000], ['drums44', 44100]]) op(`${sig}/${i}`, 'lofi', sig, sr, { p });
}
for (const [sig, sr] of [['e808', 44100], ['sine48', 48000], ['sine22', 22050], ['noise44', 44100], ['short1000', 44100], ['silence', 44100], ['sine44', 44100]]) op(sig, 'pitch', sig, sr);
for (const s of [5, 1, 10, undefined]) op(`drums44/${s}`, 'onsets', 'drums44', 44100, s === undefined ? {} : { sensitivity: s });
for (const [sig, sr] of [['drumsSt', 44100], ['silence', 44100], ['short500', 44100], ['short1000', 44100]]) op(sig, 'onsets', sig, sr, { sensitivity: 6 });
const PREP = { silenceDb: -45, targetDb: -1, fadeInMs: 2, fadeOutMs: 10 };
op('st16', 'render', 'stereo48', 48000, { rate: 48000, bits: 16, mono: false, prep: null });
op('st24mono44', 'render', 'stereo48', 48000, { rate: 44100, bits: 24, mono: true, prep: null });
op('st32', 'render', 'stereo48', 48000, { rate: 48000, bits: 32, mono: false });
op('prep24', 'render', 'padded', 44100, { rate: 44100, bits: 24, mono: false, prep: PREP });
op('prep16-48', 'render', 'drumsSt', 44100, { rate: 48000, bits: 16, mono: false, prep: { silenceDb: -20, targetDb: -3, fadeInMs: 5, fadeOutMs: 50 } });
op('prepSilence', 'render', 'silence', 44100, { rate: 44100, bits: 16, mono: false, prep: PREP });
op('tri16', 'render', 'tri', 44100, { rate: 44100, bits: 16, mono: false });
op('tri24', 'render', 'tri', 44100, { rate: 44100, bits: 24, mono: false });
op('odd', 'render', 'short3', 44100, { rate: 44100, bits: 24, mono: false });
op('defaultBits', 'render', 'short100', 44100, { rate: 44100, mono: false });
op('stereo48', 'noiseProfile', 'stereo48', 48000, { a: 1000, b: 9000 });
op('short', 'noiseProfile', 'short100', 44100, { a: 0, b: 100 });
op('noise44', 'estimateProfile', 'noise44', 44100);
op('short', 'estimateProfile', 'short500', 44100);
for (const n of [0, 100, 4410, 100000]) op(`drumsSt/${n}`, 'loopCrossfade', 'drumsSt', 44100, { n });

// Functions outside the op table.
{
  const r = mulberry(31), re = f32(1024, () => 2 * r() - 1), im = f32(1024, () => 2 * r() - 1);
  const ore = re.slice(), oim = im.slice();
  dsp.fft(ore, oim);
  add('fft/1024', 'fft', 0, [re, im], {}, { channels: chOut([ore, oim]) });
  const re2 = f32(8, i => i), im2 = new Float32Array(8), o2 = re2.slice(), oi2 = im2.slice();
  dsp.fft(o2, oi2);
  add('fft/8', 'fft', 0, [re2, im2], {}, { channels: chOut([o2, oi2]) });
}
for (const sig of ['stereo48', 'tri', 'sine44']) add(`mixMono/${sig}`, 'mixMono', 0, S[sig], {}, { channels: chOut([dsp.mixMono(S[sig])]) });
for (const sig of ['stereo48', 'silence', 'hot']) { const s = dsp.stats(S[sig]); add(`stats/${sig}`, 'stats', 0, S[sig], {}, { info: { peak: s.peak, rms: s.rms } }); }
for (const [i, p] of COMP.entries()) for (const x of [-60, -30, -24, -21, -18, -6, 0, 6]) add(`compCurve/${i}/${x}`, 'compCurve', 0, [], { x, p }, { info: { y: dsp.compCurve(x, p), makeup: dsp.autoMakeup(p) } });
for (const p of [{ drive: 6, ceiling: -1 }, { drive: 20, ceiling: -6 }]) for (const x of [-40, -12, 0, 3]) add(`satCurve/${p.drive}/${x}`, 'satCurve', 0, [], { x, p }, { info: { y: dsp.satCurve(x, p) } });
for (const [sig, sr] of [['stereo48', 48000], ['drums44', 44100], ['sine96', 96000], ['silence', 44100], ['short1', 44100]]) {
  const d = dsp.mixMono(S[sig]), peak = dsp.spectrogramPeak(d, sr);
  add(`spectrogramPeak/${sig}`, 'spectrogramPeak', sr, [d], {}, { info: { peak } });
  for (const [start, spp, PW, rows] of [[0, 300, 48, 96], [1234.5, 77.25, 20, 40], [-500, 1000, 8, 300]]) {
    add(`spectrogram/${sig}/${start}`, 'spectrogram', sr, [d], { start, spp, PW, rows, peak }, { bytes: blob(dsp.spectrogram(d, sr, start, spp, PW, rows, peak), 'u8') });
  }
}
for (const [sig, sr] of [['stereo48', 48000], ['drums44', 44100], ['short100', 44100], ['silence', 44100]]) {
  add(`loudness/${sig}`, 'loudness', sr, S[sig], {}, { info: { lufs: num(dsp.loudness(S[sig], sr)) } });
  add(`truePeakEnvelope/${sig}`, 'truePeakEnvelope', sr, S[sig], {}, { channels: chOut([dsp.truePeakEnvelope(S[sig])]) });
}
for (const f of [0.5, 1.00005, 1.8]) for (const [sig, sr] of [['drumsSt', 44100], ['short1000', 44100], ['short1', 44100]]) add(`stretch/${sig}/${f}`, 'stretch', sr, S[sig], { factor: f }, { channels: chOut(dsp.stretch(S[sig], sr, f)) });
for (const hz of [440, 27.5, 4186, 61.7, 1000, 8.1]) { const n = dsp.noteAt(hz); add(`noteAt/${hz}`, 'noteAt', 0, [], { hz }, { info: n }); }
for (const [sig, sr, th] of [['padded', 44100, 0.1], ['silence', 44100, 0.01], ['drumsSt', 44100, 0.5]]) { const r = dsp.soundBounds(S[sig], sr, th); add(`soundBounds/${sig}`, 'soundBounds', sr, S[sig], { threshold: th }, { info: { bounds: r } }); }
for (const [a, b, fin] of [[0, 1000, true], [500, 5000, false], [10, 10, true]]) { const c = S.drumsSt.map(d => d.slice()); dsp.fade(c, a, b, fin); add(`fade/${a}/${b}`, 'fade', 44100, S.drumsSt, { a, b, fadeIn: fin }, { channels: chOut(c) }); }
for (const [sig, p] of [['padded', {}], ['drumsSt', PREP], ['silence', PREP], ['short3', { targetDb: 0, fadeInMs: 0, fadeOutMs: 0 }]]) add(`prep/${sig}`, 'prep', 44100, S[sig], { p }, { channels: chOut(dsp.prep(S[sig], 44100, p)) });
for (const [sig, sr, bits] of [['stereo48', 48000, 16], ['stereo48', 48000, 24], ['stereo48', 48000, 32], ['tri', 44100, 16], ['short3', 44100, 24], ['short3', 44100, 16], ['tri', 44100, 32], ['hot', 44100, 16]]) {
  add(`encodeWav/${sig}/${bits}`, 'encodeWav', sr, S[sig], { bits, seed: SEED }, { bytes: blob(new Uint8Array(dsp.encodeWav(S[sig], sr, bits)), 'u8') });
}

// Mixes: lanes carry their own channels.
const MIXES = [
  { sr: 48000, seconds: 2, rate: 44100, bits: 24, mono: false, fadeEnd: true, lanes: [
    { sig: 'drums44', sr: 44100, gain: dsp.lin(-3), hits: [0, 0.5], fades: [{ in: 0.01, out: 0.05 }, null] },
    { sig: 'stereo48', sr: 48000, gain: 1, hits: [0.25], fades: [{ in: 0, out: 0 }] },
    { sig: 'hot', sr: 44100, gain: 1.2, hits: [1.0, 2.5] },
  ] },
  { sr: 44100, seconds: 1.2, rate: 44100, bits: 16, mono: true, fadeEnd: false, lanes: [
    { sig: 'drumsSt', sr: 44100, gain: 0.8, hits: [0.1], fades: [{ in: 0.2, out: 0.3 }] },
    { sig: 'sine22', sr: 22050, gain: 0.5, hits: [0, 0.6] },
  ] },
  { sr: 44100, seconds: 0.5, rate: 48000, bits: 32, mono: false, fadeEnd: true, lanes: [{ sig: 'sine44', sr: 44100, gain: 0.25, hits: [0.1] }] },
  { sr: 44100, seconds: 0.2, rate: 44100, bits: 16, mono: false, fadeEnd: false, lanes: [{ sig: 'hot', sr: 44100, gain: 2, hits: [0] }] },
];
for (const [i, m] of MIXES.entries()) {
  const lanes = m.lanes.map(l => ({ ...l, chs: S[l.sig] }));
  const { wav, over } = renderMix({ ...m, lanes });
  const md = mixdown(lanes, m.sr, m.seconds), fit = md.map(d => d.slice()), fo = fitPeak(fit);
  const args = { ...m, ditherSeed: SEED, lanes: m.lanes.map(({ sig, ...l }) => ({ ...l, chs: refs(S[sig]) })) };
  add(`mix/${i}`, 'mix', m.sr, [], args, { bytes: blob(new Uint8Array(wav), 'u8'), info: { over } });
  add(`mixdown/${i}`, 'mixdown', m.sr, [], args, { channels: chOut(md) });
  add(`fitPeak/${i}`, 'fitPeak', m.sr, md, {}, { channels: chOut(fit), info: { over: fo } });
}

writeFileSync(`${outDir}/data.bin`, Buffer.concat(parts));
writeFileSync(`${outDir}/cases.json`, JSON.stringify(cases));
console.log(`${cases.length} cases`);
