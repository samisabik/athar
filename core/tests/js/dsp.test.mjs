// Run: node --test tests/
import test from 'node:test';
import assert from 'node:assert/strict';
import { encodeWav, resample, spectrogram, spectrogramPeak } from './dsp.js';

const sine = (n, hz, sr, amp = 1) => Float32Array.from({ length: n }, (_, i) => amp * Math.sin(2 * Math.PI * hz * i / sr));
const rms = (d) => Math.sqrt(d.reduce((a, v) => a + v * v, 0) / d.length);
const pcm16 = (buf) => new Int16Array(buf, 44);

test('16-bit export dithers: a tone below one step survives as signal plus noise', () => {
  const n = 48000, quiet = sine(n, 440, 48000, 0.4 / 32768);
  const out = pcm16(encodeWav([quiet], 48000, 16));
  let dot = 0; for (let i = 0; i < n; i++) dot += out[i] * quiet[i] * 32768;
  const recovered = dot / (n * 0.5 * 0.4 * 0.4);
  assert.ok(Math.abs(recovered - 1) < 0.15, `tone recovered at ${recovered.toFixed(2)} of its level`);
  const noise = rms(Float32Array.from(out));
  assert.ok(noise > 0.4 && noise < 1.0, `noise ${noise.toFixed(2)} steps RMS`);
});

test('24-bit export is exact, with no dither', () => {
  const v = new DataView(encodeWav([new Float32Array([0, 0.5, -0.5])], 48000, 24));
  assert.equal(v.getUint8(44) | v.getUint8(45) << 8 | v.getUint8(46) << 16, 0);
  assert.equal(v.getUint8(47) | v.getUint8(48) << 8 | v.getUint8(49) << 16, Math.round(0.5 * 0x7fffff));
});

test('speeding up 2x removes what would alias and keeps what fits', () => {
  const sr = 48000, n = sr;
  const high = resample([sine(n, 15000, sr)], 20000, 10000)[0];   // would land at 30 kHz, above Nyquist
  const low = resample([sine(n, 2000, sr)], 20000, 10000)[0];
  assert.equal(high.length, n / 2);
  assert.ok(rms(high.subarray(500, -500)) < 0.01, `aliased energy ${rms(high).toFixed(4)}`);
  assert.ok(Math.abs(rms(low.subarray(500, -500)) - Math.SQRT1_2) < 0.01);
});

test('spectrogram tiles of one sound share a colour scale and join without a seam', () => {
  const sr = 48000, d = sine(sr * 2, 1000, sr, 0.5), peak = spectrogramPeak(d, sr), rows = 64, spp = 64;
  const whole = spectrogram(d, sr, 0, spp, 512, rows, peak);
  const right = spectrogram(d, sr, 256 * spp, spp, 256, rows, peak);
  for (let y = 0; y < rows; y++) for (let c = 0; c < 4; c++)
    assert.equal(right[(y * 256) * 4 + c], whole[(y * 512 + 256) * 4 + c]);
  assert.ok(whole.reduce((a, v) => Math.max(a, v), 0) > 200, 'the tone reaches the bright end of the scale');
});

import { fft, denoise, estimateProfile } from './dsp.js';

test('fft matches a direct transform', () => {
  const n = 64, x = Float32Array.from({ length: n }, (_, i) => Math.sin(i * .37) + .5 * Math.cos(i * 1.3));
  const re = Float32Array.from(x), im = new Float32Array(n);
  fft(re, im);
  for (let k = 0; k < n; k++) {
    let a = 0, b = 0;
    for (let i = 0; i < n; i++) { a += x[i] * Math.cos(-2 * Math.PI * k * i / n); b += x[i] * Math.sin(-2 * Math.PI * k * i / n); }
    assert.ok(Math.abs(re[k] - a) < 1e-3 && Math.abs(im[k] - b) < 1e-3, `bin ${k}`);
  }
});

test('32-bit float export is bit exact', () => {
  const l = new Float32Array([0, .25, -1.5]), r = new Float32Array([1, -.125, 3]);
  assert.deepEqual([...new Float32Array(encodeWav([l, r], 48000, 32), 80)], [0, 1, .25, -.125, -1.5, 3]);
});

test('noise reduction lowers the noise and keeps the tone', () => {
  const sr = 48000, n = sr * 2;
  let s = 7; const noise = Float32Array.from({ length: n }, () => { s = (s * 1664525 + 1013904223) >>> 0; return (s / 4294967296 - .5) * .02; });
  const tone = sine(n, 1000, sr, .3), mix = tone.map((v, i) => v + noise[i]);
  const out = denoise([mix], sr, estimateProfile([noise]), { reduction: 24, sensitivity: 2 }).channels[0];
  const residual = out.map((v, i) => v - tone[i]).subarray(4096, -4096);
  assert.ok(rms(residual) < rms(noise) * .75, `noise ${rms(noise).toFixed(4)} -> ${rms(residual).toFixed(4)}`);
  assert.ok(Math.abs(rms(out.subarray(4096, -4096)) - rms(tone)) < .01);
});

import { loudness, measure, maximize, stretch, stretchPitch, onsets, declick, loopCrossfade, prep } from './dsp.js';

const noise = (n, amp, seed = 3) => { let s = seed; return Float32Array.from({ length: n }, () => { s = (s * 1664525 + 1013904223) >>> 0; return (s / 4294967296 - .5) * 2 * amp; }); };
// The strongest frequency in a signal, by counting zero crossings.
const pitchOf = (d, sr) => { let z = 0; for (let i = 1; i < d.length; i++) if ((d[i - 1] < 0) !== (d[i] < 0)) z++; return z / 2 / (d.length / sr); };

test('loudness follows BS.1770: a 997 Hz sine at -20 dBFS in both channels reads about -20 LUFS', () => {
  const sr = 48000, x = sine(sr * 5, 997, sr, 0.1);
  assert.ok(Math.abs(loudness([x, x], sr) - (-20.0)) < 0.2, `got ${loudness([x, x], sr).toFixed(2)}`);
  assert.ok(Math.abs(loudness([x], sr) - (-23.0)) < 0.2, 'one channel is 3 LU lower');
  assert.equal(loudness([new Float32Array(sr)], sr), -Infinity);
  const half = x.map(v => v * 0.5);
  assert.ok(Math.abs(loudness([x, x], sr) - loudness([half, half], sr) - 6.02) < 0.05, 'halving the level lowers it 6 LU');
});

test('true peak finds what falls between samples', () => {
  // A sine at a quarter of the sample rate, phased so every sample lands at 0.707 of its real peak.
  const sr = 48000, x = Float32Array.from({ length: 4800 }, (_, i) => Math.sin(Math.PI / 2 * i + Math.PI / 4));
  const m = measure([x], sr);
  assert.ok(Math.abs(m.peak - (-3.01)) < 0.05, `sample peak ${m.peak.toFixed(2)}`);
  assert.ok(Math.abs(m.truePeak) < 0.3, `true peak ${m.truePeak.toFixed(2)}`);
});

test('the maximizer holds the true peak at the ceiling', () => {
  const sr = 48000, x = Float32Array.from({ length: sr }, (_, i) => 0.5 * Math.sin(Math.PI / 2 * i + Math.PI / 4));
  const out = maximize([x], sr, { gain: 12, ceiling: -1, release: 60 }).channels;
  assert.ok(measure(out, sr).truePeak < -0.7, `true peak ${measure(out, sr).truePeak.toFixed(2)} dB`);
  assert.ok(measure(out, sr).truePeak > -2, 'and it is still loud');
});

test('stretch changes length and keeps pitch; pitch shift changes pitch and keeps length', () => {
  const sr = 48000, x = sine(sr * 2, 440, sr, 0.5);
  const longer = stretch([x], sr, 1.5)[0];
  assert.equal(longer.length, sr * 3);
  assert.ok(Math.abs(pitchOf(longer.subarray(4800, -4800), sr) - 440) < 3, `pitch ${pitchOf(longer, sr).toFixed(1)}`);
  assert.ok(Math.abs(rms(longer.subarray(4800, -4800)) - rms(x)) < 0.03, 'level is kept');
  const up = stretchPitch([x], sr, 1, 12)[0];
  assert.ok(Math.abs(up.length - x.length) < 10, `length ${up.length}`);
  assert.ok(Math.abs(pitchOf(up.subarray(4800, -4800), sr) - 880) < 6, `pitch ${pitchOf(up, sr).toFixed(1)}`);
});

test('onsets finds each hit of a drum pattern', () => {
  const sr = 48000, d = new Float32Array(sr * 2), hits = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5];
  for (const t of hits) { const a = Math.round(t * sr), burst = noise(4000, 0.6, a); for (let i = 0; i < 4000; i++) d[a + i] += burst[i] * Math.exp(-i / 500); }
  const found = onsets(d, sr, 5).map(p => p / sr);
  assert.equal(found.length, hits.length, `found ${found.map(v => v.toFixed(3))}`);
  found.forEach((t, i) => assert.ok(Math.abs(t - hits[i]) < 0.015, `hit ${i} at ${t.toFixed(3)}`));
});

test('declick repairs clicks and leaves the music alone', () => {
  const sr = 48000, clean = sine(sr, 220, sr, 0.3), dirty = clean.slice();
  for (const at of [5000, 17000, 30000, 41000]) dirty[at] += 0.8;
  const { channels, info } = declick([dirty], sr, 5);
  assert.match(info, /^4 clicks/);
  const err = channels[0].map((v, i) => v - clean[i]);
  assert.ok(Math.max(...err.map(Math.abs)) < 0.02, `worst error ${Math.max(...err.map(Math.abs)).toFixed(4)}`);
  assert.match(declick([clean], sr, 5).info, /^0 clicks/);
});

test('a crossfaded loop has no step where its end meets its start', () => {
  const sr = 48000, x = noise(sr, 0.3).map((v, i) => v * 0.3 + 0.5 * Math.sin(2 * Math.PI * 100.37 * i / sr));
  const raw = Math.abs(x[0] - x.at(-1)), out = loopCrossfade([x], 2400)[0];
  assert.equal(out.length, sr - 2400);
  // What follows the last sample in the source is what the loop now starts with.
  assert.ok(Math.abs(out[0] - x[sr - 2400]) < 0.02, 'the start continues from the end');
  assert.ok(raw > 0.05, 'the untreated loop did have a step');
});

test('prep trims silence, removes DC, normalizes and fades the edges', () => {
  const sr = 48000, d = new Float32Array(sr).fill(0.05);
  for (let i = 12000; i < 36000; i++) d[i] += 0.25 * Math.sin(2 * Math.PI * 300 * i / sr);
  const out = prep([d], sr, { silenceDb: -30, targetDb: -1, fadeInMs: 2, fadeOutMs: 10 })[0];
  assert.ok(Math.abs(out.length - 24000) < 600, `length ${out.length}`);
  assert.ok(Math.abs(Math.max(...out.map(Math.abs)) - Math.pow(10, -1 / 20)) < 0.01);
  assert.equal(Math.abs(out[0]), 0);
  assert.ok(Math.abs(out.at(-1)) < 0.01);
});

test('pitch detection names the held note of a sine and of an 808 with a pitch drop', async () => {
  const { detectPitch, noteAt } = await import('./dsp.js');
  const sr = 48000, sine = Float32Array.from({ length: sr }, (_, i) => 0.5 * Math.sin(2 * Math.PI * 55 * i / sr));
  assert.equal(noteAt(detectPitch(sine, sr)).name, 'A1');
  const k = new Float32Array(sr); let ph = 0;
  for (let i = 0; i < sr; i++) { const t = i / sr; ph += 2 * Math.PI * 43.65 * (1 + 2.2 * Math.exp(-t * 35)) / sr; k[i] = 0.9 * Math.sin(ph) * Math.exp(-t * 3.2); }
  const n = noteAt(detectPitch(k, sr));
  assert.equal(n.name, 'F1'); assert.ok(Math.abs(n.cents) <= 5);
  assert.equal(detectPitch(Float32Array.from({ length: sr }, () => Math.random() * 2 - 1), sr), null);
});

test('saturation at full mix stays under its ceiling and leaves quiet sound alone at no drive', async () => {
  const { saturate } = await import('./dsp.js');
  const sr = 48000, tone = (amp) => Float32Array.from({ length: sr / 2 }, (_, i) => amp * Math.sin(2 * Math.PI * 60 * i / sr));
  const peak = (d) => d.subarray(2000, d.length - 2000).reduce((m, v) => Math.max(m, Math.abs(v)), 0);
  const ceiling = Math.pow(10, -1 / 20);
  assert.ok(peak(saturate([tone(1.3)], sr, { drive: 12, ceiling: -1, mix: 100 }).channels[0]) <= ceiling + 1e-3);
  assert.ok(Math.abs(peak(saturate([tone(0.05)], sr, { drive: 0, ceiling: -1, mix: 100 }).channels[0]) - 0.05) < 0.001);
});

test('shape changes the hit and the tail on their own', async () => {
  const { shape } = await import('./dsp.js');
  const sr = 48000, hit = new Float32Array(sr);
  for (let i = 4800; i < sr; i++) { const t = i / sr; hit[i] = 0.8 * Math.sin(2 * Math.PI * 180 * t) * Math.exp(-(t - 0.1) * 6); }
  const rms = (d, a, b) => { let s = 0; for (let i = a; i < b; i++) s += d[i] * d[i]; return Math.sqrt(s / (b - a)); };
  const head = (d) => rms(d, 4800, 5280), tail = (d) => rms(d, 24000, 33600);
  const punch = shape([hit], sr, { attack: 100, tail: 0 }).channels[0], short = shape([hit], sr, { attack: 0, tail: -100 }).channels[0];
  assert.ok(head(punch) > head(hit) * 2); assert.ok(Math.abs(tail(punch) / tail(hit) - 1) < 0.05);
  assert.ok(tail(short) < tail(hit) * 0.2); assert.ok(Math.abs(head(short) / head(hit) - 1) < 0.05);
});

test('lo-fi at full rate and 24 bits returns the sound unchanged, and holds samples at a lower rate', async () => {
  const { lofi } = await import('./dsp.js');
  const sr = 48000, d = Float32Array.from({ length: 4800 }, (_, i) => 0.5 * Math.sin(2 * Math.PI * 440 * i / sr));
  assert.deepEqual(lofi([d], sr, { bits: 24, rate: 48 }).channels[0], d);
  const o = lofi([d], sr, { bits: 24, rate: 8 }).channels[0];
  let held = 0; for (let i = 1; i < o.length; i++) if (o[i] === o[i - 1]) held++;
  assert.ok(held > o.length * 0.8);
});
