// The JS audio processing the Rust port (core/src/dsp.rs) is checked against, kept for its
// tests (dsp.test.mjs, ../dsp_parity.rs).
// Signal processing with no DOM or AudioContext, so it runs on the page and in worker.js alike.

/** Average of all channels. A single channel is returned as is, not copied. */
export function mixMono(chs) {
  if (chs.length === 1) return chs[0];
  const n = chs[0].length, out = new Float32Array(n), k = 1 / chs.length;
  for (const d of chs) for (let i = 0; i < n; i++) out[i] += d[i] * k;
  return out;
}

// Twiddle factors per transform size, computed once.
const TWIDDLES = new Map();
function twiddles(n) {
  let t = TWIDDLES.get(n);
  if (!t) {
    t = { c: new Float64Array(n / 2), s: new Float64Array(n / 2) };
    for (let k = 0; k < n / 2; k++) { t.c[k] = Math.cos(-2 * Math.PI * k / n); t.s[k] = Math.sin(-2 * Math.PI * k / n); }
    TWIDDLES.set(n, t);
  }
  return t;
}

/** In-place radix-2 FFT. The length must be a power of two. */
export function fft(re, im) {
  const n = re.length, { c, s } = twiddles(n);
  for (let i = 1, j = 0; i < n; i++) {
    let bit = n >> 1;
    for (; j & bit; bit >>= 1) j ^= bit;
    j ^= bit;
    if (i < j) { const tr = re[i]; re[i] = re[j]; re[j] = tr; const ti = im[i]; im[i] = im[j]; im[j] = ti; }
  }
  for (let len = 2; len <= n; len <<= 1) {
    const half = len >> 1, step = n / len;
    for (let i = 0; i < n; i += len) {
      for (let k = 0; k < half; k++) {
        const wr = c[k * step], wi = s[k * step], p = i + k, q = p + half;
        const tr = re[q] * wr - im[q] * wi, ti = re[q] * wi + im[q] * wr;
        re[q] = re[p] - tr; im[q] = im[p] - ti; re[p] += tr; im[p] += ti;
      }
    }
  }
}

// ---- resampling: Kaiser-windowed sinc, polyphase. The cutoff sits at 94% of the
// lower Nyquist, so downsampling also low-passes away what would alias.
const RS_Z = 16, RS_BETA = 9, RS_MAX_PHASES = 4096;
function besselI0(x) { let s = 1, t = 1; for (let k = 1; k < 40; k++) { t *= (x / (2 * k)) ** 2; s += t; } return s; }
function rsKernel(x) {
  const r = x / RS_Z; if (r >= 1) return 0;
  return (x ? Math.sin(Math.PI * x) / (Math.PI * x) : 1) * besselI0(RS_BETA * Math.sqrt(1 - r * r));
}
const gcd = (a, b) => b ? gcd(b, a % b) : a;

export function resample(channels, from, to) {
  if (from === to) return channels.map(d => d.slice());
  const len = channels[0].length, n = Math.round(len * to / from);
  const out = channels.map(() => new Float32Array(n));
  if (!len) return out;
  const g = gcd(from, to), up = to / g, down = from / g;
  const cut = Math.min(1, to / from) * 0.94, H = Math.ceil(RS_Z / cut), T = 2 * H;
  const phases = Math.min(up, RS_MAX_PHASES), W = new Float32Array(phases * T);
  for (let p = 0; p < phases; p++) {
    let sum = 0;
    for (let q = 0; q < T; q++) { const v = rsKernel(Math.abs(q - H + 1 - p / phases) * cut); W[p * T + q] = v; sum += v; }
    for (let q = 0; q < T; q++) W[p * T + q] /= sum;
  }
  for (let c = 0; c < channels.length; c++) {
    const d = channels[c], o = out[c];
    for (let i = 0; i < n; i++) {
      const num = i * down;
      let j = Math.floor(num / up), p = num - j * up;
      if (phases !== up) { p = Math.round(p / up * phases); if (p === phases) { p = 0; j++; } }
      const base = j - H + 1, w = p * T;
      let s = 0;
      if (base >= 0 && base + T <= len) for (let q = 0; q < T; q++) s += W[w + q] * d[base + q];
      else for (let q = 0; q < T; q++) { const k = base + q; if (k >= 0 && k < len) s += W[w + q] * d[k]; }
      o[i] = s;
    }
  }
  return out;
}

const CHANNEL_MASKS = { 1: 0x4, 2: 0x3, 4: 0x33, 6: 0x3f, 8: 0x63f };

// Float and more than two channels use WAVE_FORMAT_EXTENSIBLE with a fact chunk;
// plain 16/24-bit mono and stereo keep the classic 16-byte fmt chunk. 16-bit output gets
// TPDF dither, one step peak to peak, so the lost bits become noise instead of distortion.
// Returns the file as an ArrayBuffer.
export function encodeWav(channels, sr, bits = 16) {
  const n = channels[0].length, ch = channels.length, bps = bits / 8, float = bits === 32;
  const ext = float || ch > 2;
  const size = n * ch * bps, pad = size & 1;
  const head = ext ? 80 : 44;
  const buf = new ArrayBuffer(head + size + pad);
  const v = new DataView(buf);
  const w = (o, s) => { for (let i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i)); };
  w(0, 'RIFF'); v.setUint32(4, head - 8 + size + pad, true); w(8, 'WAVE'); w(12, 'fmt ');
  v.setUint32(16, ext ? 40 : 16, true);
  v.setUint16(20, ext ? 0xfffe : 1, true); v.setUint16(22, ch, true);
  v.setUint32(24, sr, true); v.setUint32(28, sr * ch * bps, true);
  v.setUint16(32, ch * bps, true); v.setUint16(34, bits, true);
  if (ext) {
    v.setUint16(36, 22, true); v.setUint16(38, bits, true); v.setUint32(40, CHANNEL_MASKS[ch] || 0, true);
    // Subformat GUID 0000000X-0000-0010-8000-00AA00389B71, X = 1 for PCM, 3 for float.
    v.setUint32(44, float ? 3 : 1, true); v.setUint16(48, 0, true); v.setUint16(50, 0x10, true);
    [0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71].forEach((b, i) => v.setUint8(52 + i, b));
    w(60, 'fact'); v.setUint32(64, 4, true); v.setUint32(68, n, true);
  }
  w(head - 8, 'data'); v.setUint32(head - 4, size, true);
  // Integer samples go in through typed arrays (little-endian on every Mac).
  let o = 0;
  if (float) {
    // Measured: for floats the DataView is the faster path, for integers the typed arrays are.
    for (let i = 0, at = head; i < n; i++) for (let c = 0; c < ch; c++, at += 4) v.setFloat32(at, channels[c][i], true);
  } else if (bits === 16) {
    const out = new Int16Array(buf, head, n * ch);
    let r = (Math.random() * 0xffffffff) >>> 0 || 1;
    const rand = () => { r ^= r << 13; r ^= r >>> 17; r ^= r << 5; return (r >>> 0) / 4294967296; };
    for (let i = 0; i < n; i++) for (let c = 0; c < ch; c++) {
      const x = Math.max(-1, Math.min(1, channels[c][i] + (rand() - rand()) / 0x8000));
      out[o++] = Math.round(x < 0 ? x * 0x8000 : x * 0x7fff);
    }
  } else {
    const out = new Uint8Array(buf, head, n * ch * 3);
    for (let i = 0; i < n; i++) for (let c = 0; c < ch; c++) {
      const x = Math.max(-1, Math.min(1, channels[c][i])), q = Math.round(x < 0 ? x * 0x800000 : x * 0x7fffff);
      out[o++] = q & 255; out[o++] = (q >> 8) & 255; out[o++] = (q >> 16) & 255;
    }
  }
  return buf;
}


export const db = (x) => 20 * Math.log10(Math.max(x, 1e-12));
export const lin = (d) => Math.pow(10, d / 20);

export function stats(chs) {
  let peak = 0, sum = 0;
  for (const d of chs) for (let i = 0; i < d.length; i++) { const v = Math.abs(d[i]); if (v > peak) peak = v; sum += d[i] * d[i]; }
  return { peak: db(peak), rms: db(Math.sqrt(sum / (chs.length * chs[0].length || 1))) };
}

export function compCurve(x, { threshold, ratio, knee }) {
  const over = x - threshold;
  if (2 * over < -knee) return x;
  if (knee > 0 && 2 * Math.abs(over) <= knee) return x + (1 / ratio - 1) * (over + knee / 2) ** 2 / (2 * knee);
  return threshold + over / ratio;
}

export function autoMakeup(p) { return -(p.threshold - compCurve(0, p)) * 0.5; }

/** Feed-forward compressor, stereo-linked peak detector, smoothing in dB. */
export function compress(chs, sr, p) {
  const n = chs[0].length, out = chs.map(d => new Float32Array(d.length));
  const aA = Math.exp(-1 / (Math.max(0.05, p.attack) / 1000 * sr));
  const aR = Math.exp(-1 / (Math.max(1, p.release) / 1000 * sr));
  const makeup = p.autoMakeup ? autoMakeup(p) : p.makeup;
  let env = 0, maxGr = 0;
  for (let i = 0; i < n; i++) {
    let x = 0; for (const d of chs) x = Math.max(x, Math.abs(d[i]));
    const xd = db(x), target = xd - compCurve(xd, p);
    env = target > env ? aA * env + (1 - aA) * target : aR * env + (1 - aR) * target;
    if (env > maxGr) maxGr = env;
    const g = lin(makeup - env);
    for (let c = 0; c < chs.length; c++) out[c][i] = chs[c][i] * g;
  }
  return { channels: out, info: `max reduction ${maxGr.toFixed(1)} dB, makeup ${makeup >= 0 ? '+' : ''}${makeup.toFixed(1)} dB` };
}

/** Gain followed by a look-ahead true-peak limiter at the ceiling. */
export function maximize(chs, sr, p) {
  const n = chs[0].length, G = lin(p.gain), C = lin(p.ceiling);
  const L = Math.max(1, Math.round(sr * 0.005)), W = 2 * L;
  // The gain each sample needs is worked out from the true-peak envelope, so peaks that fall
  // between samples are limited too, not only the sample values.
  const req = truePeakEnvelope(chs);
  for (let i = 0; i < n; i++) { const x = req[i] * G; req[i] = x > C ? C / x : 1; }
  // Minimum required gain over the next W samples, via a monotonic deque.
  const m = new Float32Array(n), dq = new Int32Array(n);
  let h = 0, t = 0, j = 0;
  for (let i = 0; i < n; i++) {
    for (; j < Math.min(n, i + W); j++) { while (t > h && req[dq[t - 1]] >= req[j]) t--; dq[t++] = j; }
    while (dq[h] < i) h++;
    m[i] = req[dq[h]];
  }
  const aR = Math.exp(-1 / (Math.max(1, p.release) / 1000 * sr));
  let g = 1;
  for (let i = 0; i < n; i++) { g = m[i] < g ? m[i] : g + (m[i] - g) * (1 - aR); m[i] = g; }
  // Box-smooth the attack so gain ramps down over L samples instead of jumping. m already holds
  // the minimum over the next W = 2L samples, so the average of its last L values never exceeds
  // what the current sample needs.
  const out = chs.map(d => new Float32Array(d.length));
  const ring = new Float32Array(L).fill(m[0] ?? 1);
  let acc = L * (m[0] ?? 1), maxGr = 0;
  for (let i = 0; i < n; i++) {
    acc += m[i] - ring[i % L]; ring[i % L] = m[i];
    const gs = acc / L;
    maxGr = Math.max(maxGr, -db(gs));
    for (let c = 0; c < chs.length; c++) out[c][i] = Math.max(-C, Math.min(C, chs[c][i] * G * gs));
  }
  const s = stats(out);
  return { channels: out, info: `peak ${s.peak.toFixed(1)} dB, RMS ${s.rms.toFixed(1)} dB, max limiting ${maxGr.toFixed(1)} dB` };
}

export const NR_N = 2048, NR_H = 512;
export const NR_WIN = Float32Array.from({ length: NR_N }, (_, i) => 0.5 - 0.5 * Math.cos(2 * Math.PI * i / NR_N));

export function frameMags(d, start, re, im) {
  for (let i = 0; i < NR_N; i++) { const s = start + i; re[i] = s >= 0 && s < d.length ? d[s] * NR_WIN[i] : 0; im[i] = 0; }
  fft(re, im);
  const mags = new Float32Array(NR_N / 2 + 1);
  for (let k = 0; k <= NR_N / 2; k++) mags[k] = Math.sqrt(re[k] * re[k] + im[k] * im[k]);
  return mags;
}

/** Average magnitude spectrum of a region: the "noise print". */
export function noiseProfile(chs, a, b) {
  const d = mixMono(chs.map(c => c.subarray(a, b)));
  const re = new Float32Array(NR_N), im = new Float32Array(NR_N);
  const prof = new Float32Array(NR_N / 2 + 1);
  let frames = 0;
  for (let s = 0; s === 0 || s + NR_N <= d.length; s += NR_H) {
    const m = frameMags(d, s, re, im);
    for (let k = 0; k < m.length; k++) prof[k] += m[k];
    frames++;
  }
  return prof.map(v => v / frames);
}

/** Without a noise print: per-bin 10th percentile over the quietest-looking frames. */
export function estimateProfile(chs) {
  const d = mixMono(chs);
  const re = new Float32Array(NR_N), im = new Float32Array(NR_N);
  const total = Math.max(1, Math.floor((d.length - NR_N) / NR_H) + 1);
  const F = Math.min(400, total), bins = NR_N / 2 + 1;
  const all = new Float32Array(F * bins);
  for (let f = 0; f < F; f++) all.set(frameMags(d, Math.floor(f * total / F) * NR_H, re, im), f * bins);
  const prof = new Float32Array(bins), col = new Float32Array(F);
  for (let k = 0; k < bins; k++) {
    for (let f = 0; f < F; f++) col[f] = all[f * bins + k];
    col.sort(); prof[k] = col[Math.floor(F * 0.1)];
  }
  return prof;
}

/** Spectral noise reduction: per-bin Wiener-style gain against the noise print,
    limited by the reduction floor, smoothed across time and frequency. */
export function denoise(chs, sr, profile, p) {
  const floor = lin(-p.reduction), sens = p.sensitivity, bins = NR_N / 2 + 1;
  const release = Math.exp(-NR_H / (sr * 0.08));
  const re = new Float32Array(NR_N), im = new Float32Array(NR_N);
  const G = new Float32Array(bins), Gs = new Float32Array(bins);
  const thr2 = Float64Array.from(profile, v => (v * sens + 1e-12) ** 2);
  const out = chs.map((d) => {
    const n = d.length, buf = new Float32Array(n + 2 * NR_N), prev = new Float32Array(bins).fill(1);
    for (let s = -NR_N + NR_H; s < n; s += NR_H) {
      for (let i = 0; i < NR_N; i++) { const x = s + i; re[i] = x >= 0 && x < n ? d[x] * NR_WIN[i] : 0; im[i] = 0; }
      fft(re, im);
      for (let k = 0; k < bins; k++) {
        const snr = (re[k] * re[k] + im[k] * im[k]) / thr2[k], w = snr / (snr + 1);
        let g = Math.max(floor, w * w);
        if (g < prev[k]) g = Math.max(g, prev[k] * release);
        prev[k] = g; G[k] = g;
      }
      for (let k = 0; k < bins; k++) Gs[k] = (G[Math.max(0, k - 1)] + G[k] * 2 + G[Math.min(bins - 1, k + 1)]) / 4;
      for (let k = 0; k < NR_N; k++) { const g = Gs[k <= NR_N / 2 ? k : NR_N - k]; re[k] *= g; im[k] = -im[k] * g; }
      fft(re, im);
      for (let i = 0; i < NR_N; i++) buf[s + NR_N + i] += (re[i] / NR_N) * NR_WIN[i];
    }
    // Hann analysis x Hann synthesis at 75% overlap sums to 1.5.
    const o = new Float32Array(n);
    for (let i = 0; i < n; i++) o[i] = buf[i + NR_N] / 1.5;
    return o;
  });
  return { channels: out, info: `reduced noise by up to ${p.reduction} dB` };
}

/** The loudest spectral level in a sound, in dB, from up to 256 frames spread across it. */
export function spectrogramPeak(d, sr) {
  const N = sr >= 88200 ? 8192 : 4096, half = N / 2, frames = Math.min(256, Math.max(1, Math.ceil(d.length / half)));
  const win = new Float32Array(N).map((_, i) => 0.5 - 0.5 * Math.cos(2 * Math.PI * i / (N - 1)));
  const re = new Float32Array(N), im = new Float32Array(N);
  let max = 0;
  for (let f = 0; f < frames; f++) {
    const c = Math.floor((f + .5) / frames * d.length);
    for (let i = 0; i < N; i++) { const s = c - half + i; re[i] = s >= 0 && s < d.length ? d[s] * win[i] : 0; im[i] = 0; }
    fft(re, im);
    for (let k = 0; k < half; k++) { const m = re[k] * re[k] + im[k] * im[k]; if (m > max) max = m; }
  }
  return 20 * Math.log10(Math.sqrt(max) * 4 / N + 1e-9);
}

const SPEC_STOPS = [[18, 16, 30], [52, 24, 96], [122, 61, 255], [214, 86, 160], [255, 168, 92], [255, 246, 214]];
function specColor(t) {
  t = Math.max(0, Math.min(1, t)) * (SPEC_STOPS.length - 1);
  const i = Math.min(SPEC_STOPS.length - 2, Math.floor(t)), f = t - i;
  return SPEC_STOPS[i].map((c, k) => c + (SPEC_STOPS[i + 1][k] - c) * f);
}

/** Spectrogram image of mono samples d: PW x rows RGBA pixels, one FFT per column starting at
    sample `start` and stepping `spp` samples, 20 Hz to Nyquist on a log axis. Low rows
    interpolate between bins so the bass stays smooth; high rows take the loudest bin they
    cover. peak is the level in dB drawn brightest, 80 dB below it is black; pass the same
    value for every image of a sound (see spectrogramPeak) so neighbouring tiles match. */
export function spectrogram(d, sr, start, spp, PW, rows, peak) {
  const N = sr >= 88200 ? 8192 : 4096, half = N / 2;
  const win = new Float32Array(N).map((_, i) => 0.5 - 0.5 * Math.cos(2 * Math.PI * i / (N - 1)));
  const re = new Float32Array(N), im = new Float32Array(N), mags = new Float32Array(half);
  const dbs = new Float32Array(PW * rows).fill(-200);
  const fmin = 20, fmax = sr / 2;
  const lo = new Float32Array(rows + 1);
  for (let y = 0; y <= rows; y++) lo[y] = Math.min(half - 1, fmin * Math.pow(fmax / fmin, 1 - y / rows) / sr * N);
  for (let x = 0; x < PW; x++) {
    const c = Math.floor(start + x * spp);
    if (c >= d.length) break;
    for (let i = 0; i < N; i++) { const s = c - half + i; re[i] = s >= 0 && s < d.length ? d[s] * win[i] : 0; im[i] = 0; }
    fft(re, im);
    for (let k = 0; k < half; k++) mags[k] = Math.sqrt(re[k] * re[k] + im[k] * im[k]) * 4 / N;
    for (let y = 0; y < rows; y++) {
      const top = lo[y], bot = lo[y + 1];
      let m;
      if (top - bot < 1) { const k = Math.floor((top + bot) / 2), t = (top + bot) / 2 - k; m = mags[k] * (1 - t) + mags[Math.min(half - 1, k + 1)] * t; }
      else { m = 0; for (let k = Math.floor(bot); k <= Math.ceil(top); k++) if (mags[k] > m) m = mags[k]; }
      const v = 20 * Math.log10(m + 1e-9);
      dbs[y * PW + x] = v;
    }
  }
  const px = new Uint8ClampedArray(PW * rows * 4), floor = peak - 80;
  for (let i = 0; i < dbs.length; i++) {
    const [r, g, b] = specColor((dbs[i] - floor) / 80), o = i * 4;
    px[o] = r; px[o + 1] = g; px[o + 2] = b; px[o + 3] = 255;
  }
  return px;
}

// ---------------------------------------------------------------- loudness and true peak

function biquad(d, b0, b1, b2, a1, a2) {
  const out = new Float32Array(d.length);
  let x1 = 0, x2 = 0, y1 = 0, y2 = 0;
  for (let i = 0; i < d.length; i++) {
    const x = d[i], y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
    x2 = x1; x1 = x; y2 = y1; y1 = y; out[i] = y;
  }
  return out;
}

// The K-weighting curve of ITU-R BS.1770: a high shelf for the head, then a high-pass.
function kWeight(d, sr) {
  let K = Math.tan(Math.PI * 1681.974450955533 / sr), Q = 0.7071752369554196;
  const Vh = Math.pow(10, 3.999843853973347 / 20), Vb = Math.pow(Vh, 0.4996667741545416);
  let a0 = 1 + K / Q + K * K;
  const shelved = biquad(d, (Vh + Vb * K / Q + K * K) / a0, 2 * (K * K - Vh) / a0, (Vh - Vb * K / Q + K * K) / a0, 2 * (K * K - 1) / a0, (1 - K / Q + K * K) / a0);
  K = Math.tan(Math.PI * 38.13547087602444 / sr); Q = 0.5003270373238773; a0 = 1 + K / Q + K * K;
  return biquad(shelved, 1, -2, 1, 2 * (K * K - 1) / a0, (1 - K / Q + K * K) / a0);
}

/** Integrated loudness in LUFS (ITU-R BS.1770-4: 400 ms blocks, 75% overlap, gated at -70 LUFS
    and at 10 LU under the ungated level). A sound shorter than one block is measured whole,
    ungated, which the standard does not define but is what a one-shot needs. Silence gives -Infinity. */
export function loudness(chs, sr) {
  const n = chs[0].length, block = Math.min(n, Math.round(sr * 0.4)), step = Math.max(1, Math.round(sr * 0.1));
  if (!n) return -Infinity;
  const weighted = chs.map(d => kWeight(d, sr)), blocks = [];
  for (let s = 0; s + block <= n; s += step) {
    let z = 0;
    for (const d of weighted) { let e = 0; for (let i = s; i < s + block; i++) e += d[i] * d[i]; z += e / block; }
    blocks.push(z);
  }
  const lufs = (z) => -0.691 + 10 * Math.log10(z);
  const mean = (a) => a.reduce((x, v) => x + v, 0) / a.length;
  if (block < Math.round(sr * 0.4)) return lufs(blocks[0]);
  const loud = blocks.filter(z => lufs(z) > -70);
  if (!loud.length) return -Infinity;
  const gate = lufs(mean(loud)) - 10, kept = loud.filter(z => lufs(z) > gate);
  return kept.length ? lufs(mean(kept)) : -Infinity;
}

// Four-times oversampling, 12 taps per phase, Hann-windowed sinc: finds peaks that fall between samples.
const TP_HALF = 6;
const TP_TAPS = [1, 2, 3].map(p => Float32Array.from({ length: 2 * TP_HALF }, (_, j) => {
  const x = j - TP_HALF + 1 - p / 4;
  return Math.sin(Math.PI * x) / (Math.PI * x) * (0.5 + 0.5 * Math.cos(Math.PI * x / TP_HALF));
}));

/** For every sample, the highest level the waveform reaches at or just after it, across all channels. */
export function truePeakEnvelope(chs) {
  const n = chs[0].length, env = new Float32Array(n);
  for (const d of chs) {
    for (let i = 0; i < n; i++) {
      let m = d[i] < 0 ? -d[i] : d[i];
      if (i >= TP_HALF - 1 && i + TP_HALF < n) {
        for (const taps of TP_TAPS) {
          let v = 0;
          for (let j = 0, k = i - TP_HALF + 1; j < 2 * TP_HALF; j++, k++) v += d[k] * taps[j];
          if (v < 0) v = -v;
          if (v > m) m = v;
        }
      }
      if (m > env[i]) env[i] = m;
    }
  }
  return env;
}

/** Loudness and peak levels of a sound: { lufs, peak, truePeak }, the peaks in dB full scale. */
export function measure(chs, sr) {
  const env = truePeakEnvelope(chs);
  let tp = 0; for (let i = 0; i < env.length; i++) if (env[i] > tp) tp = env[i];
  return { lufs: loudness(chs, sr), peak: stats(chs).peak, truePeak: db(tp) };
}

// ---------------------------------------------------------------- time and pitch

/** Change length without changing pitch (WSOLA). factor is new length over old length. Frames of
    40 ms are overlapped by half; each is taken from where it best continues the frame before it,
    within 12 ms of its ideal position, so periodic sounds keep their waveform. */
export function stretch(chs, sr, factor) {
  const len = chs[0].length;
  if (!len || Math.abs(factor - 1) < 1e-4) return chs.map(d => d.slice());
  const N = Math.round(sr * 0.04 / 2) * 2, Hs = N / 2, D = 4, tol = Math.round(sr * 0.012 / D), Nd = N / D | 0;
  const outLen = Math.max(1, Math.round(len * factor));
  const win = Float32Array.from({ length: N }, (_, i) => 0.5 - 0.5 * Math.cos(2 * Math.PI * i / N));
  // The search runs on a mono mix at a quarter of the sample rate.
  const mono = mixMono(chs), md = new Float32Array(Math.ceil(len / D) + Nd + tol + 2);
  for (let i = 0, k = 0; i + D <= len; i += D, k++) md[k] = (mono[i] + mono[i + 1] + mono[i + 2] + mono[i + 3]) / 4;
  const out = chs.map(() => new Float32Array(outLen + N)), wsum = new Float32Array(outLen + N);
  let prev = 0;
  for (let m = 0; m * Hs < outLen; m++) {
    let pos = Math.round(m * Hs / factor);
    if (m) {
      const nat = Math.min(md.length - Nd - 1, Math.round((prev + Hs) / D)), ideal = Math.round(pos / D);
      let best = 0, bestScore = -Infinity;
      for (let lag = -tol; lag <= tol; lag++) {
        const c = ideal + lag;
        if (c < 0 || c + Nd >= md.length) continue;
        let dot = 0, energy = 1e-9;
        for (let i = 0; i < Nd; i++) { const v = md[c + i]; dot += v * md[nat + i]; energy += v * v; }
        const score = dot / Math.sqrt(energy);
        if (score > bestScore) { bestScore = score; best = lag; }
      }
      pos = pos + best * D;
    }
    // A frame cut short by the end of the sound would stop at full weight and click, so the last
    // frames are taken whole from the end of the sound.
    pos = Math.max(0, Math.min(len - N, pos));
    const at = m * Hs, take = Math.min(N, len - pos);
    // The first frame starts at full weight: there is no earlier frame to fade in against.
    const w = i => (m === 0 && i < Hs ? 1 : win[i]);
    for (let c = 0; c < chs.length; c++) { const d = chs[c], o = out[c]; for (let i = 0; i < take; i++) o[at + i] += d[pos + i] * w(i); }
    for (let i = 0; i < take; i++) wsum[at + i] += w(i);
    prev = pos;
  }
  return out.map(o => { const r = o.subarray(0, outLen); for (let i = 0; i < outLen; i++) if (wsum[i] > 1e-6) r[i] /= wsum[i]; return r.slice(); });
}

/** Stretch to length (new over old) and shift pitch by semitones, each independent of the other. */
export function stretchPitch(chs, sr, length, semitones) {
  const ratio = Math.pow(2, semitones / 12);
  const stretched = stretch(chs, sr, length * ratio);
  return Math.abs(ratio - 1) < 1e-6 ? stretched : resample(stretched, Math.round(ratio * 100000), 100000);
}

// ---------------------------------------------------------------- colour: saturation, shape, lo-fi

/** What the soft clipper does to a level in dB: gain by `drive`, then bend towards `ceiling`. */
export function satCurve(inDb, { drive, ceiling }) {
  const c = lin(ceiling);
  return db(c * Math.tanh(lin(inDb + drive) / c));
}

/** Drive into a soft clipper. Output never passes the ceiling; `mix` blends the dry sound back in.
    Runs at twice the sample rate so the added harmonics do not fold back as aliasing. */
export function saturate(chs, sr, p) {
  const g = lin(p.drive), c = lin(p.ceiling), mix = Math.max(0, Math.min(1, (p.mix ?? 100) / 100));
  const up = resample(chs, 1, 2);
  for (const d of up) for (let i = 0; i < d.length; i++) d[i] = c * Math.tanh(d[i] * g / c);
  const wet = resample(up, 2, 1);
  const channels = chs.map((d, ch) => {
    const o = new Float32Array(d.length), w = wet[ch];
    for (let i = 0; i < d.length; i++) o[i] = d[i] * (1 - mix) + (w[i] || 0) * mix;
    return o;
  });
  return { channels, info: `peak ${stats(channels).peak.toFixed(1)} dB` };
}

/** Transient shaper. `attack` (-100 to 100) lowers or raises the hit; `tail` (-100 to 100) shortens
    or lengthens what rings after it. Both work from how fast the level is moving, not from the
    level itself, so quiet and loud hits are treated alike. All channels share one gain. */
export function shape(chs, sr, p) {
  const n = chs[0].length, k = (ms) => Math.exp(-1 / (sr * ms / 1000));
  const fastUp = k(0.5), slowUp = k(18), down = k(60), slowDown = k(450), smooth = k(1.5);
  const aAmt = (p.attack || 0) / 100, tAmt = (p.tail || 0) / 100 * 1.6, floor = 1e-5;
  const out = chs.map(() => new Float32Array(n));
  let ef = 0, es = 0, rf = 0, rs = 0, gs = 0;
  for (let i = 0; i < n; i++) {
    let x = 0;
    for (const d of chs) { const v = Math.abs(d[i]); if (v > x) x = v; }
    ef = x > ef ? x + (ef - x) * fastUp : x + (ef - x) * down;
    es = x > es ? x + (es - x) * slowUp : x + (es - x) * down;
    rf = x > rf ? x : x + (rf - x) * down;
    rs = x > rs ? x : x + (rs - x) * slowDown;
    const hit = Math.max(0, db(ef + floor) - db(es + floor)), ring = Math.max(0, db(rs + floor) - db(rf + floor));
    const want = Math.max(-36, Math.min(18, aAmt * hit + tAmt * ring));
    gs = want + (gs - want) * smooth;
    const gain = lin(gs);
    for (let c = 0; c < chs.length; c++) out[c][i] = chs[c][i] * gain;
  }
  return { channels: out, info: `peak ${stats(out).peak.toFixed(1)} dB` };
}

/** Old-sampler grit: hold each sample until the next tick of a lower rate, and round to fewer bits. */
export function lofi(chs, sr, p) {
  const step = Math.max(1, sr / Math.max(500, Math.min(sr, p.rate * 1000))), q = Math.pow(2, Math.max(2, Math.min(24, p.bits)) - 1);
  const crush = p.bits < 24;
  const channels = chs.map((d) => {
    const o = new Float32Array(d.length);
    let next = 0, held = 0;
    for (let i = 0; i < d.length; i++) {
      if (i >= next) { held = crush ? Math.round(d[i] * q) / q : d[i]; next += step; }
      o[i] = held;
    }
    return o;
  });
  return { channels, info: `${p.bits}-bit at ${(sr / step / 1000).toFixed(1)} kHz` };
}

// ---------------------------------------------------------------- pitch

const NOTES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
/** The nearest note to a frequency: its name, MIDI number, frequency, and how many cents off it is. */
export function noteAt(hz) {
  const m = 69 + 12 * Math.log2(hz / 440), midi = Math.round(m);
  return { midi, name: NOTES[((midi % 12) + 12) % 12] + (Math.floor(midi / 12) - 1), hz: 440 * Math.pow(2, (midi - 69) / 12), cents: Math.round((m - midi) * 100) };
}

/** The pitch a sound holds, in Hz, or null when it has none (noise, chords, very short sounds).
    YIN on overlapping frames; the answer is the middle value of the frames that are clearly pitched,
    so the pitch drop at the start of an 808 does not pull it off. Covers about 25 Hz to 2 kHz. */
export function detectPitch(d, sr) {
  // Half the rate is plenty for this range and makes the search eight times cheaper.
  const D = sr >= 32000 ? 2 : 1, r = sr / D, m = new Float32Array(Math.min(Math.floor(d.length / D), Math.round(r * 12)));
  for (let i = 0; i < m.length; i++) m[i] = D === 2 ? (d[2 * i] + d[2 * i + 1]) / 2 : d[i];
  const maxLag = Math.floor(r / 25), minLag = Math.max(2, Math.floor(r / 2000)), W = maxLag * 2;
  if (m.length < W + maxLag) return null;
  const diff = new Float32Array(maxLag + 1), found = [];
  let top = 0;
  const hop = Math.max(256, Math.floor((m.length - W - maxLag) / 60)), frames = [];
  for (let s = 0; s + W + maxLag <= m.length; s += hop) {
    let e = 0; for (let i = 0; i < W; i++) e += m[s + i] * m[s + i];
    frames.push([s, e]); if (e > top) top = e;
  }
  for (const [s, e] of frames) {
    if (e < top * 0.01) continue;
    let run = 0, lag = -1;
    diff[0] = 1;
    for (let t = 1; t <= maxLag; t++) {
      let sum = 0;
      for (let i = 0; i < W; i++) { const v = m[s + i] - m[s + i + t]; sum += v * v; }
      run += sum;
      diff[t] = run ? sum * t / run : 1;
    }
    for (let t = minLag; t < maxLag; t++) {
      if (diff[t] < 0.12) { while (t + 1 < maxLag && diff[t + 1] < diff[t]) t++; lag = t; break; }
    }
    if (lag < 0) continue;
    const a = diff[lag - 1], b = diff[lag], c = diff[lag + 1], den = a - 2 * b + c;
    found.push(r / (lag + (den ? (a - c) / (2 * den) : 0)));
  }
  if (found.length < Math.max(1, frames.length * 0.2)) return null;
  found.sort((x, y) => x - y);
  return found[found.length >> 1];
}

// ---------------------------------------------------------------- slicing, repair, loops, prep

/** Sample positions where a new sound starts, from the rise in spectral energy between frames.
    sensitivity runs from 1 (only the strongest hits) to 10 (every small attack). */
export function onsets(d, sr, sensitivity = 5) {
  const N = 1024, H = 256, half = N / 2, frames = Math.max(0, Math.floor((d.length - N) / H) + 1);
  const win = Float32Array.from({ length: N }, (_, i) => 0.5 - 0.5 * Math.cos(2 * Math.PI * i / N));
  const re = new Float32Array(N), im = new Float32Array(N), prev = new Float32Array(half), flux = new Float32Array(frames);
  for (let f = 0; f < frames; f++) {
    for (let i = 0; i < N; i++) { re[i] = d[f * H + i] * win[i]; im[i] = 0; }
    fft(re, im);
    let sum = 0;
    for (let k = 1; k < half; k++) { const m = Math.log1p(100 * Math.sqrt(re[k] * re[k] + im[k] * im[k])); if (m > prev[k]) sum += m - prev[k]; prev[k] = m; }
    flux[f] = f ? sum : 0;
  }
  let total = 0; for (const v of flux) total += v;
  const floor = total / (frames || 1), k = 4.2 - sensitivity * 0.32, gap = Math.ceil(sr * 0.05 / H), out = [];
  for (let f = 1, last = -gap; f < frames - 1; f++) {
    let local = 0, count = 0;
    for (let j = Math.max(0, f - 12); j < Math.min(frames, f + 12); j++) { local += flux[j]; count++; }
    if (flux[f] > k * local / count + 0.2 * floor && flux[f] >= flux[f - 1] && flux[f] > flux[f + 1] && f - last >= gap) {
      // Within the frames that showed the rise, the attack starts where the level first reaches
      // a fifth of its peak; the slice point sits 2 ms ahead of that.
      const lo = f * H, hi = Math.min(d.length, lo + N + H);
      let peak = 0, pos = lo;
      for (let i = lo; i < hi; i++) { const v = Math.abs(d[i]); if (v > peak) peak = v; }
      for (let i = lo; i < hi; i++) if (Math.abs(d[i]) > 0.2 * peak) { pos = Math.max(lo, i - Math.round(sr * 0.002)); break; }
      out.push(pos); last = f;
    }
  }
  return out;
}

/** Remove clicks: samples that jump far outside the local texture are replaced by a straight
    line across the gap. sensitivity runs from 1 (only gross clicks) to 10. Returns the count repaired. */
export function declick(chs, sr, sensitivity = 5) {
  const maxRun = Math.round(sr * 0.003), W = 1024, thr = 26 - sensitivity * 2.2;
  let repaired = 0;
  const channels = chs.map((d) => {
    const n = d.length, out = d.slice(), e = new Float32Array(n);
    for (let i = 2; i < n; i++) e[i] = Math.abs(d[i] - 2 * d[i - 1] + d[i - 2]);
    let sum = 0;
    for (let i = 0; i < Math.min(n, W); i++) sum += e[i];
    for (let i = 2; i < n - 2; i++) {
      const a = Math.max(0, i - W / 2), b = Math.min(n, i + W / 2);
      if (i > W / 2 && b < n) sum += e[b] - e[a - 1];
      if (e[i] <= thr * (sum / W) + 1e-5) continue;
      let end = i;
      while (end + 1 < n - 2 && end - i < maxRun && e[end + 1] > 0.5 * thr * (sum / W)) end++;
      const from = Math.max(1, i - 3), to = Math.min(n - 2, end + 3), y0 = out[from - 1], y1 = d[to + 1];
      for (let k = from; k <= to; k++) out[k] = y0 + (y1 - y0) * (k - from + 1) / (to - from + 2);
      repaired++; i = to;
    }
    return out;
  });
  return { channels, info: `${repaired} click${repaired === 1 ? '' : 's'} repaired` };
}

/** Make a sound loop without a bump: its last `n` samples are folded into its first `n` with an
    equal-power crossfade and then dropped, so the end runs into the start. */
export function loopCrossfade(chs, n) {
  const len = chs[0].length;
  n = Math.max(0, Math.min(n, len >> 1));
  return chs.map((d) => {
    const out = d.slice(0, len - n);
    for (let i = 0; i < n; i++) { const t = (i + .5) / n; out[i] = d[i] * Math.sin(t * Math.PI / 2) + d[len - n + i] * Math.cos(t * Math.PI / 2); }
    return out;
  });
}

/** The first and last sample above a threshold (linear), padded by 5 ms; null when all is below. */
export function soundBounds(chs, sr, threshold) {
  const m = mixMono(chs);
  let a = 0, b = m.length;
  while (a < b && Math.abs(m[a]) < threshold) a++;
  while (b > a && Math.abs(m[b - 1]) < threshold) b--;
  if (a >= b) return null;
  const pad = Math.round(sr * 0.005);
  return [Math.max(0, a - pad), Math.min(m.length, b + pad)];
}

export function fade(chs, a, b, fadeIn) {
  const n = b - a; if (n <= 0) return;
  for (const d of chs) for (let i = 0; i < n; i++) {
    const t = i / n;
    d[a + i] *= fadeIn ? Math.sin(t * Math.PI / 2) ** 2 : Math.cos(t * Math.PI / 2) ** 2;
  }
}

/** Sampler prep: remove DC, trim silent ends, normalize the peak, add short edge fades. */
export function prep(chs, sr, { silenceDb = -45, targetDb = -1, fadeInMs = 2, fadeOutMs = 10 } = {}) {
  let out = chs.map((d) => { let m = 0; for (let i = 0; i < d.length; i++) m += d[i]; m /= d.length || 1; return d.map(v => v - m); });
  const r = soundBounds(out, sr, lin(silenceDb));
  if (r) out = out.map(d => d.slice(r[0], r[1]));
  let peak = 0; for (const d of out) for (let i = 0; i < d.length; i++) peak = Math.max(peak, Math.abs(d[i]));
  if (peak) { const k = lin(targetDb) / peak; for (const d of out) for (let i = 0; i < d.length; i++) d[i] *= k; }
  const len = out[0].length;
  fade(out, 0, Math.min(len, Math.round(sr * fadeInMs / 1000)), true);
  fade(out, Math.max(0, len - Math.round(sr * fadeOutMs / 1000)), len, false);
  return out;
}

