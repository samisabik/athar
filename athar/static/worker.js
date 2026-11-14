import * as dsp from './dsp.js';
import { renderMix } from './arrange.js';

// The mono sound the spectrogram is drawn from, sent once per version so each view only sends numbers.
let specData = null, specPeak = 0;

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
  mix: (args) => renderMix(args),
  specData: ({ data, sr }) => { specData = data; specPeak = dsp.spectrogramPeak(data, sr); return {}; },
  spec: ({ sr, start, spp, PW, rows }) => ({ pixels: dsp.spectrogram(specData, sr, start, spp, PW, rows, specPeak) }),
};

self.onmessage = (e) => {
  const { id, op, args } = e.data;
  try {
    const r = ops[op](args);
    const moved = [...(r.channels || []).map(c => c.buffer), r.wav, r.pixels?.buffer].filter(Boolean);
    self.postMessage({ id, r }, moved);
  } catch (err) {
    self.postMessage({ id, error: String(err?.message || err) });
  }
};
