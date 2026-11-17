//! Port of `athar/static/dsp.js`. Arithmetic runs in f64 and buffers are f32, in the same order
//! as the JavaScript, so results match it to within float rounding.

#![allow(clippy::needless_range_loop, clippy::manual_clamp)]

use serde_json::{Value, json};
use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::{Arc, Mutex, OnceLock};

/// JavaScript `Math.round`: halves go towards positive infinity.
pub(crate) fn jround(x: f64) -> f64 {
    let r = x.floor();
    if x - r >= 0.5 { r + 1.0 } else { r }
}

/// JavaScript `Number.prototype.toFixed(1)`.
pub(crate) fn to_fixed1(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let a = x.abs();
    let q = a * 4.0;
    let tie = a < 1e15 && q.fract() == 0.0 && (q as i64) % 2 == 1;
    let s = if tie { format!("{:.1}", a + 0.05) } else { format!("{a:.1}") };
    if x < 0.0 { format!("-{s}") } else { s }
}

/// JavaScript `String(number)` for ordinary magnitudes.
pub(crate) fn js_num(x: f64) -> String {
    if x.is_nan() {
        "NaN".into()
    } else if x.is_infinite() {
        if x > 0.0 { "Infinity".into() } else { "-Infinity".into() }
    } else if x.fract() == 0.0 && x.abs() < 1e21 {
        format!("{}", x as i128)
    } else {
        format!("{x}")
    }
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

fn num(p: &Value, key: &str) -> Option<f64> {
    p.get(key).and_then(Value::as_f64)
}

fn finite_or_null(x: f64) -> Value {
    if x.is_finite() { json!(x) } else { Value::Null }
}

/// Average of all channels.
pub fn mix_mono<S: AsRef<[f32]>>(chs: &[S]) -> Vec<f32> {
    if chs.len() == 1 {
        return chs[0].as_ref().to_vec();
    }
    let n = chs[0].as_ref().len();
    let k = 1.0 / chs.len() as f64;
    let mut out = vec![0f32; n];
    for d in chs {
        let d = d.as_ref();
        for i in 0..n {
            out[i] = (out[i] as f64 + d[i] as f64 * k) as f32;
        }
    }
    out
}

type Twiddles = Arc<(Vec<f64>, Vec<f64>)>;

fn twiddles(n: usize) -> Twiddles {
    static CACHE: OnceLock<Mutex<HashMap<usize, Twiddles>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    cache
        .entry(n)
        .or_insert_with(|| {
            let (mut c, mut s) = (vec![0f64; n / 2], vec![0f64; n / 2]);
            for k in 0..n / 2 {
                let a = -2.0 * PI * k as f64 / n as f64;
                c[k] = a.cos();
                s[k] = a.sin();
            }
            Arc::new((c, s))
        })
        .clone()
}

/// In-place radix-2 FFT on f32 buffers. The length must be a power of two.
pub fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let tw = twiddles(n);
    let (c, s) = (&tw.0, &tw.1);
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let half = len >> 1;
        let step = n / len;
        let mut i = 0;
        while i < n {
            for k in 0..half {
                let (wr, wi) = (c[k * step], s[k * step]);
                let (p, q) = (i + k, i + k + half);
                let (rq, iq) = (re[q] as f64, im[q] as f64);
                let tr = rq * wr - iq * wi;
                let ti = rq * wi + iq * wr;
                re[q] = (re[p] as f64 - tr) as f32;
                im[q] = (im[p] as f64 - ti) as f32;
                re[p] = (re[p] as f64 + tr) as f32;
                im[p] = (im[p] as f64 + ti) as f32;
            }
            i += len;
        }
        len <<= 1;
    }
}

const RS_Z: f64 = 16.0;
const RS_BETA: f64 = 9.0;
const RS_MAX_PHASES: u64 = 4096;

fn bessel_i0(x: f64) -> f64 {
    let (mut s, mut t) = (1.0, 1.0);
    for k in 1..40 {
        let y = x / (2 * k) as f64;
        t *= y * y;
        s += t;
    }
    s
}

fn rs_kernel(x: f64) -> f64 {
    let r = x / RS_Z;
    if r >= 1.0 {
        return 0.0;
    }
    let sinc = if x != 0.0 { (PI * x).sin() / (PI * x) } else { 1.0 };
    sinc * bessel_i0(RS_BETA * (1.0 - r * r).sqrt())
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Kaiser-windowed sinc resampler (dsp.js `resample`), cutoff at 94% of the lower Nyquist.
pub fn resample_sinc(channels: &[Vec<f32>], from: u32, to: u32) -> Vec<Vec<f32>> {
    if from == to || channels.is_empty() {
        return channels.to_vec();
    }
    let len = channels[0].len();
    let n = jround(len as f64 * to as f64 / from as f64).max(0.0) as usize;
    let mut out = vec![vec![0f32; n]; channels.len()];
    if len == 0 {
        return out;
    }
    let g = gcd(from as u64, to as u64);
    let (up, down) = (to as u64 / g, from as u64 / g);
    let cut = (to as f64 / from as f64).min(1.0) * 0.94;
    let h = (RS_Z / cut).ceil() as i64;
    let t = (2 * h) as usize;
    let phases = up.min(RS_MAX_PHASES);
    let mut w = vec![0f32; phases as usize * t];
    for p in 0..phases as usize {
        let mut sum = 0.0;
        for q in 0..t {
            let v = rs_kernel(((q as i64 - h + 1) as f64 - p as f64 / phases as f64).abs() * cut);
            w[p * t + q] = v as f32;
            sum += v;
        }
        for q in 0..t {
            w[p * t + q] = (w[p * t + q] as f64 / sum) as f32;
        }
    }
    for (d, o) in channels.iter().zip(out.iter_mut()) {
        for (i, oi) in o.iter_mut().enumerate() {
            let num = i as u64 * down;
            let mut j = num / up;
            let mut p = num - j * up;
            if phases != up {
                p = jround(p as f64 / up as f64 * phases as f64) as u64;
                if p == phases {
                    p = 0;
                    j += 1;
                }
            }
            let base = j as i64 - h + 1;
            let wp = &w[p as usize * t..(p as usize + 1) * t];
            let mut s = 0.0;
            if base >= 0 && base as usize + t <= len {
                let b = base as usize;
                for q in 0..t {
                    s += wp[q] as f64 * d[b + q] as f64;
                }
            } else {
                for q in 0..t {
                    let k = base + q as i64;
                    if k >= 0 && (k as usize) < len {
                        s += wp[q] as f64 * d[k as usize] as f64;
                    }
                }
            }
            *oi = s as f32;
        }
    }
    out
}

fn channel_mask(ch: usize) -> u32 {
    match ch {
        1 => 0x4,
        2 => 0x3,
        4 => 0x33,
        6 => 0x3f,
        8 => 0x63f,
        _ => 0,
    }
}

/// A seed for the 16-bit dither when the caller does not need reproducible output.
pub fn random_seed() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    (h.finish() as u32).max(1)
}

/// WAV file bytes (16/24-bit PCM or 32-bit float), with a random dither seed.
pub fn encode_wav(channels: &[Vec<f32>], sr: u32, bits: u32) -> Result<Vec<u8>, String> {
    encode_wav_seeded(channels, sr, bits, random_seed())
}

/// `encode_wav` with the dither's xorshift state given. dsp.js seeds it with
/// `(Math.random() * 0xffffffff) >>> 0 || 1`; the same value here gives the same bytes.
pub fn encode_wav_seeded(channels: &[Vec<f32>], sr: u32, bits: u32, seed: u32) -> Result<Vec<u8>, String> {
    if !matches!(bits, 16 | 24 | 32) {
        return Err(format!("unsupported bit depth {bits}"));
    }
    if channels.is_empty() {
        return Err("no channels".into());
    }
    let (n, ch, bps, float) = (channels[0].len(), channels.len(), (bits / 8) as usize, bits == 32);
    let ext = float || ch > 2;
    let size = n * ch * bps;
    let pad = size & 1;
    let head = if ext { 80 } else { 44 };
    let mut v = Vec::with_capacity(head + size + pad);
    let u16le = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    let u32le = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    v.extend_from_slice(b"RIFF");
    u32le(&mut v, (head - 8 + size + pad) as u32);
    v.extend_from_slice(b"WAVEfmt ");
    u32le(&mut v, if ext { 40 } else { 16 });
    u16le(&mut v, if ext { 0xfffe } else { 1 });
    u16le(&mut v, ch as u16);
    u32le(&mut v, sr);
    u32le(&mut v, sr.wrapping_mul((ch * bps) as u32));
    u16le(&mut v, (ch * bps) as u16);
    u16le(&mut v, bits as u16);
    if ext {
        u16le(&mut v, 22);
        u16le(&mut v, bits as u16);
        u32le(&mut v, channel_mask(ch));
        u32le(&mut v, if float { 3 } else { 1 });
        u16le(&mut v, 0);
        u16le(&mut v, 0x10);
        v.extend_from_slice(&[0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71]);
        v.extend_from_slice(b"fact");
        u32le(&mut v, 4);
        u32le(&mut v, n as u32);
    }
    v.extend_from_slice(b"data");
    u32le(&mut v, size as u32);
    if float {
        for i in 0..n {
            for d in channels {
                v.extend_from_slice(&d[i].to_le_bytes());
            }
        }
    } else if bits == 16 {
        let mut r = seed.max(1);
        let mut rand = || {
            r ^= r << 13;
            r ^= r >> 17;
            r ^= r << 5;
            r as f64 / 4294967296.0
        };
        for i in 0..n {
            for d in channels {
                let a = rand();
                let b = rand();
                let x = (d[i] as f64 + (a - b) / 32768.0).clamp(-1.0, 1.0);
                let q = jround(if x < 0.0 { x * 32768.0 } else { x * 32767.0 }) as i16;
                v.extend_from_slice(&q.to_le_bytes());
            }
        }
    } else {
        for i in 0..n {
            for d in channels {
                let x = (d[i] as f64).clamp(-1.0, 1.0);
                let q = jround(if x < 0.0 { x * 8388608.0 } else { x * 8388607.0 }) as i32;
                v.extend_from_slice(&[(q & 255) as u8, ((q >> 8) & 255) as u8, ((q >> 16) & 255) as u8]);
            }
        }
    }
    v.resize(head + size + pad, 0);
    Ok(v)
}

/// Linear level to dB, floored at -240 dB.
pub fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

/// dB to linear level.
pub fn lin(d: f64) -> f64 {
    10f64.powf(d / 20.0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub peak: f64,
    pub rms: f64,
}

/// Peak and RMS level in dB over all channels.
pub fn stats(chs: &[Vec<f32>]) -> Stats {
    let (mut peak, mut sum) = (0f64, 0f64);
    for d in chs {
        for &x in d {
            let v = (x as f64).abs();
            if v > peak {
                peak = v;
            }
            sum += x as f64 * x as f64;
        }
    }
    let count = chs.len() * chs.first().map_or(0, Vec::len);
    Stats { peak: db(peak), rms: db((sum / count.max(1) as f64).sqrt()) }
}

/// A processor's output channels and its one-line summary.
#[derive(Debug, Clone)]
pub struct Processed {
    pub channels: Vec<Vec<f32>>,
    pub info: String,
}

#[derive(Debug, Clone, Copy)]
pub struct CompParams {
    pub threshold: f64,
    pub ratio: f64,
    pub knee: f64,
    pub attack: f64,
    pub release: f64,
    pub makeup: f64,
    pub auto_makeup: bool,
}

impl CompParams {
    /// Reads the fx.js keys; missing ones take fx.js defaults.
    pub fn from_json(p: &Value) -> Self {
        CompParams {
            threshold: num(p, "threshold").unwrap_or(-24.0),
            ratio: num(p, "ratio").unwrap_or(4.0),
            knee: num(p, "knee").unwrap_or(6.0),
            attack: num(p, "attack").unwrap_or(10.0),
            release: num(p, "release").unwrap_or(150.0),
            makeup: num(p, "makeup").unwrap_or(0.0),
            auto_makeup: if p.get("autoMakeup").is_some() { truthy(p.get("autoMakeup")) } else { true },
        }
    }
}

/// Static compressor curve: output level in dB for an input level in dB.
pub fn comp_curve(x: f64, p: &CompParams) -> f64 {
    let over = x - p.threshold;
    if 2.0 * over < -p.knee {
        return x;
    }
    if p.knee > 0.0 && 2.0 * over.abs() <= p.knee {
        let y = over + p.knee / 2.0;
        return x + (1.0 / p.ratio - 1.0) * (y * y) / (2.0 * p.knee);
    }
    p.threshold + over / p.ratio
}

pub fn auto_makeup(p: &CompParams) -> f64 {
    -(p.threshold - comp_curve(0.0, p)) * 0.5
}

/// Feed-forward compressor, stereo-linked peak detector, smoothing in dB.
pub fn compress(chs: &[Vec<f32>], sr: u32, p: &CompParams) -> Processed {
    let sr = sr as f64;
    let n = chs[0].len();
    let mut out: Vec<Vec<f32>> = chs.iter().map(|d| vec![0f32; d.len()]).collect();
    let a_a = (-1.0 / (p.attack.max(0.05) / 1000.0 * sr)).exp();
    let a_r = (-1.0 / (p.release.max(1.0) / 1000.0 * sr)).exp();
    let makeup = if p.auto_makeup { auto_makeup(p) } else { p.makeup };
    let (mut env, mut max_gr) = (0f64, 0f64);
    for i in 0..n {
        let mut x = 0f64;
        for d in chs {
            x = x.max((d[i] as f64).abs());
        }
        let xd = db(x);
        let target = xd - comp_curve(xd, p);
        env = if target > env { a_a * env + (1.0 - a_a) * target } else { a_r * env + (1.0 - a_r) * target };
        if env > max_gr {
            max_gr = env;
        }
        let g = lin(makeup - env);
        for (c, d) in chs.iter().enumerate() {
            out[c][i] = (d[i] as f64 * g) as f32;
        }
    }
    let sign = if makeup >= 0.0 { "+" } else { "" };
    Processed { channels: out, info: format!("max reduction {} dB, makeup {sign}{} dB", to_fixed1(max_gr), to_fixed1(makeup)) }
}

#[derive(Debug, Clone, Copy)]
pub struct MaxParams {
    pub gain: f64,
    pub ceiling: f64,
    pub release: f64,
}

impl MaxParams {
    pub fn from_json(p: &Value) -> Self {
        MaxParams {
            gain: num(p, "gain").unwrap_or(6.0),
            ceiling: num(p, "ceiling").unwrap_or(-1.0),
            release: num(p, "release").unwrap_or(60.0),
        }
    }
}

/// Gain followed by a look-ahead true-peak limiter at the ceiling.
pub fn maximize(chs: &[Vec<f32>], sr: u32, p: &MaxParams) -> Processed {
    let sr = sr as f64;
    let n = chs[0].len();
    let (g_in, c_lim) = (lin(p.gain), lin(p.ceiling));
    let l = (jround(sr * 0.005) as usize).max(1);
    let w = 2 * l;
    let mut req = true_peak_envelope(chs);
    for r in req.iter_mut() {
        let x = *r as f64 * g_in;
        *r = (if x > c_lim { c_lim / x } else { 1.0 }) as f32;
    }
    let mut m = vec![0f32; n];
    let mut dq = vec![0usize; n];
    let (mut h, mut t, mut j) = (0usize, 0usize, 0usize);
    for i in 0..n {
        while j < n.min(i + w) {
            while t > h && req[dq[t - 1]] >= req[j] {
                t -= 1;
            }
            dq[t] = j;
            t += 1;
            j += 1;
        }
        while dq[h] < i {
            h += 1;
        }
        m[i] = req[dq[h]];
    }
    let a_r = (-1.0 / (p.release.max(1.0) / 1000.0 * sr)).exp();
    let mut g = 1f64;
    for mi in m.iter_mut() {
        let v = *mi as f64;
        g = if v < g { v } else { g + (v - g) * (1.0 - a_r) };
        *mi = g as f32;
    }
    let mut out: Vec<Vec<f32>> = chs.iter().map(|d| vec![0f32; d.len()]).collect();
    let m0 = m.first().copied().unwrap_or(1.0);
    let mut ring = vec![m0; l];
    let mut acc = l as f64 * m0 as f64;
    let mut max_gr = 0f64;
    for i in 0..n {
        acc += m[i] as f64 - ring[i % l] as f64;
        ring[i % l] = m[i];
        let gs = acc / l as f64;
        max_gr = max_gr.max(-db(gs));
        for (c, d) in chs.iter().enumerate() {
            out[c][i] = (d[i] as f64 * g_in * gs).min(c_lim).max(-c_lim) as f32;
        }
    }
    let s = stats(&out);
    Processed {
        info: format!("peak {} dB, RMS {} dB, max limiting {} dB", to_fixed1(s.peak), to_fixed1(s.rms), to_fixed1(max_gr)),
        channels: out,
    }
}

pub const NR_N: usize = 2048;
pub const NR_H: usize = 512;

fn hann(n: usize, period: usize) -> Vec<f32> {
    (0..n).map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / period as f64).cos()) as f32).collect()
}

fn nr_win() -> &'static [f32] {
    static W: OnceLock<Vec<f32>> = OnceLock::new();
    W.get_or_init(|| hann(NR_N, NR_N))
}

/// Magnitudes of one Hann-windowed NR_N-point frame of `d` starting at `start`.
pub fn frame_mags(d: &[f32], start: i64, re: &mut [f32], im: &mut [f32]) -> Vec<f32> {
    let win = nr_win();
    for i in 0..NR_N {
        let s = start + i as i64;
        re[i] = if s >= 0 && (s as usize) < d.len() { (d[s as usize] as f64 * win[i] as f64) as f32 } else { 0.0 };
        im[i] = 0.0;
    }
    fft(re, im);
    (0..=NR_N / 2).map(|k| (re[k] as f64 * re[k] as f64 + im[k] as f64 * im[k] as f64).sqrt() as f32).collect()
}

/// Average magnitude spectrum of samples a..b: the "noise print".
pub fn noise_profile(chs: &[Vec<f32>], a: usize, b: usize) -> Vec<f32> {
    let parts: Vec<&[f32]> = chs
        .iter()
        .map(|c| {
            let e = b.min(c.len());
            &c[a.min(e)..e]
        })
        .collect();
    let d = mix_mono(&parts);
    let (mut re, mut im) = (vec![0f32; NR_N], vec![0f32; NR_N]);
    let mut prof = vec![0f32; NR_N / 2 + 1];
    let mut frames = 0usize;
    let mut s = 0usize;
    while s == 0 || s + NR_N <= d.len() {
        let m = frame_mags(&d, s as i64, &mut re, &mut im);
        for k in 0..m.len() {
            prof[k] = (prof[k] as f64 + m[k] as f64) as f32;
        }
        frames += 1;
        s += NR_H;
    }
    prof.iter().map(|&v| (v as f64 / frames as f64) as f32).collect()
}

/// Noise print without a selection: per-bin 10th percentile over up to 400 frames.
pub fn estimate_profile(chs: &[Vec<f32>]) -> Vec<f32> {
    let d = mix_mono(chs);
    let (mut re, mut im) = (vec![0f32; NR_N], vec![0f32; NR_N]);
    let total = ((d.len() as f64 - NR_N as f64) / NR_H as f64).floor().max(0.0) as usize + 1;
    let total = total.max(1);
    let f_count = total.min(400);
    let bins = NR_N / 2 + 1;
    let mut all = vec![0f32; f_count * bins];
    for f in 0..f_count {
        let m = frame_mags(&d, ((f * total / f_count) * NR_H) as i64, &mut re, &mut im);
        all[f * bins..(f + 1) * bins].copy_from_slice(&m);
    }
    let mut prof = vec![0f32; bins];
    let mut col = vec![0f32; f_count];
    for k in 0..bins {
        for f in 0..f_count {
            col[f] = all[f * bins + k];
        }
        col.sort_by(f32::total_cmp);
        prof[k] = col[(f_count as f64 * 0.1).floor() as usize];
    }
    prof
}

#[derive(Debug, Clone, Copy)]
pub struct NrParams {
    pub reduction: f64,
    pub sensitivity: f64,
}

impl NrParams {
    pub fn from_json(p: &Value) -> Self {
        NrParams { reduction: num(p, "reduction").unwrap_or(12.0), sensitivity: num(p, "sensitivity").unwrap_or(1.5) }
    }
}

/// Spectral noise reduction against a noise print (see `noise_profile`, `estimate_profile`).
pub fn denoise(chs: &[Vec<f32>], sr: u32, profile: &[f32], p: &NrParams) -> Processed {
    let win = nr_win();
    let floor = lin(-p.reduction);
    let sens = p.sensitivity;
    let bins = NR_N / 2 + 1;
    let release = (-(NR_H as f64) / (sr as f64 * 0.08)).exp();
    let (mut re, mut im) = (vec![0f32; NR_N], vec![0f32; NR_N]);
    let (mut gk, mut gsm) = (vec![0f32; bins], vec![0f32; bins]);
    let thr2: Vec<f64> = (0..bins)
        .map(|k| {
            let v = profile.get(k).map_or(f64::NAN, |&v| v as f64) * sens + 1e-12;
            v * v
        })
        .collect();
    let channels = chs
        .iter()
        .map(|d| {
            let n = d.len();
            let mut buf = vec![0f32; n + 2 * NR_N];
            let mut prev = vec![1f32; bins];
            let mut s = -(NR_N as i64) + NR_H as i64;
            while s < n as i64 {
                for i in 0..NR_N {
                    let x = s + i as i64;
                    re[i] = if x >= 0 && (x as usize) < n { (d[x as usize] as f64 * win[i] as f64) as f32 } else { 0.0 };
                    im[i] = 0.0;
                }
                fft(&mut re, &mut im);
                for k in 0..bins {
                    let snr = (re[k] as f64 * re[k] as f64 + im[k] as f64 * im[k] as f64) / thr2[k];
                    let w = snr / (snr + 1.0);
                    let mut g = floor.max(w * w);
                    if g < prev[k] as f64 {
                        g = g.max(prev[k] as f64 * release);
                    }
                    prev[k] = g as f32;
                    gk[k] = g as f32;
                }
                for k in 0..bins {
                    gsm[k] = ((gk[k.saturating_sub(1)] as f64 + gk[k] as f64 * 2.0 + gk[(k + 1).min(bins - 1)] as f64) / 4.0) as f32;
                }
                for k in 0..NR_N {
                    let g = gsm[if k <= NR_N / 2 { k } else { NR_N - k }] as f64;
                    re[k] = (re[k] as f64 * g) as f32;
                    im[k] = (-(im[k] as f64) * g) as f32;
                }
                fft(&mut re, &mut im);
                let at = (s + NR_N as i64) as usize;
                for i in 0..NR_N {
                    buf[at + i] = (buf[at + i] as f64 + (re[i] as f64 / NR_N as f64) * win[i] as f64) as f32;
                }
                s += NR_H as i64;
            }
            (0..n).map(|i| (buf[i + NR_N] as f64 / 1.5) as f32).collect()
        })
        .collect();
    Processed { channels, info: format!("reduced noise by up to {} dB", js_num(p.reduction)) }
}

fn spec_size(sr: u32) -> usize {
    if sr >= 88200 { 8192 } else { 4096 }
}

/// The loudest spectral level of mono samples in dB, from up to 256 frames spread across them.
pub fn spectrogram_peak(d: &[f32], sr: u32) -> f64 {
    let n = spec_size(sr);
    let half = n / 2;
    let frames = 256.min(((d.len() as f64 / half as f64).ceil() as usize).max(1));
    let win = hann(n, n - 1);
    let (mut re, mut im) = (vec![0f32; n], vec![0f32; n]);
    let mut max = 0f64;
    for f in 0..frames {
        let c = ((f as f64 + 0.5) / frames as f64 * d.len() as f64).floor() as i64;
        for i in 0..n {
            let s = c - half as i64 + i as i64;
            re[i] = if s >= 0 && (s as usize) < d.len() { (d[s as usize] as f64 * win[i] as f64) as f32 } else { 0.0 };
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        for k in 0..half {
            let m = re[k] as f64 * re[k] as f64 + im[k] as f64 * im[k] as f64;
            if m > max {
                max = m;
            }
        }
    }
    20.0 * (max.sqrt() * 4.0 / n as f64 + 1e-9).log10()
}

const SPEC_STOPS: [[f64; 3]; 6] = [[18., 16., 30.], [52., 24., 96.], [122., 61., 255.], [214., 86., 160.], [255., 168., 92.], [255., 246., 214.]];

fn spec_color(t: f64) -> [f64; 3] {
    let t = t.min(1.0).max(0.0) * (SPEC_STOPS.len() - 1) as f64;
    let i = (SPEC_STOPS.len() - 2).min(t.floor() as usize);
    let f = t - i as f64;
    let (a, b) = (SPEC_STOPS[i], SPEC_STOPS[i + 1]);
    [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f]
}

/// `Uint8ClampedArray` conversion: clamp, then round half to even.
fn clamp_u8(x: f64) -> u8 {
    if x.is_nan() || x <= 0.0 {
        return 0;
    }
    if x >= 255.0 {
        return 255;
    }
    let f = x.floor();
    if f + 0.5 < x {
        (f + 1.0) as u8
    } else if x < f + 0.5 || (f as u8).is_multiple_of(2) {
        f as u8
    } else {
        (f + 1.0) as u8
    }
}

/// Spectrogram image of mono samples: `pw` x `rows` RGBA pixels, one FFT per column starting at
/// sample `start` and stepping `spp` samples, 20 Hz to Nyquist on a log axis, `peak` dB brightest.
pub fn spectrogram(d: &[f32], sr: u32, start: f64, spp: f64, pw: usize, rows: usize, peak: f64) -> Vec<u8> {
    let n = spec_size(sr);
    let half = n / 2;
    let win = hann(n, n - 1);
    let (mut re, mut im, mut mags) = (vec![0f32; n], vec![0f32; n], vec![0f32; half]);
    let mut dbs = vec![-200f32; pw * rows];
    let (fmin, fmax) = (20f64, sr as f64 / 2.0);
    let lo: Vec<f32> = (0..=rows)
        .map(|y| ((half - 1) as f64).min(fmin * (fmax / fmin).powf(1.0 - y as f64 / rows as f64) / sr as f64 * n as f64) as f32)
        .collect();
    for x in 0..pw {
        let c = (start + x as f64 * spp).floor();
        if c >= d.len() as f64 {
            break;
        }
        let c = c as i64;
        for i in 0..n {
            let s = c - half as i64 + i as i64;
            re[i] = if s >= 0 && (s as usize) < d.len() { (d[s as usize] as f64 * win[i] as f64) as f32 } else { 0.0 };
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        for k in 0..half {
            mags[k] = ((re[k] as f64 * re[k] as f64 + im[k] as f64 * im[k] as f64).sqrt() * 4.0 / n as f64) as f32;
        }
        for y in 0..rows {
            let (top, bot) = (lo[y] as f64, lo[y + 1] as f64);
            let m = if top - bot < 1.0 {
                let mid = (top + bot) / 2.0;
                let k = mid.floor();
                let t = mid - k;
                let k = k as usize;
                mags[k] as f64 * (1.0 - t) + mags[(half - 1).min(k + 1)] as f64 * t
            } else {
                let mut m = 0f64;
                for k in bot.floor() as usize..=top.ceil() as usize {
                    if mags[k] as f64 > m {
                        m = mags[k] as f64;
                    }
                }
                m
            };
            dbs[y * pw + x] = (20.0 * (m + 1e-9).log10()) as f32;
        }
    }
    let floor = peak - 80.0;
    let mut px = vec![0u8; pw * rows * 4];
    for (i, &v) in dbs.iter().enumerate() {
        let [r, g, b] = spec_color((v as f64 - floor) / 80.0);
        px[i * 4..i * 4 + 4].copy_from_slice(&[clamp_u8(r), clamp_u8(g), clamp_u8(b), 255]);
    }
    px
}

fn biquad(d: &[f32], b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Vec<f32> {
    let (mut x1, mut x2, mut y1, mut y2) = (0f64, 0f64, 0f64, 0f64);
    d.iter()
        .map(|&x| {
            let x = x as f64;
            let y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            x2 = x1;
            x1 = x;
            y2 = y1;
            y1 = y;
            y as f32
        })
        .collect()
}

fn k_weight(d: &[f32], sr: f64) -> Vec<f32> {
    let mut k = (PI * 1681.974450955533 / sr).tan();
    let mut q = 0.7071752369554196;
    let vh = 10f64.powf(3.999843853973347 / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let mut a0 = 1.0 + k / q + k * k;
    let shelved = biquad(
        d,
        (vh + vb * k / q + k * k) / a0,
        2.0 * (k * k - vh) / a0,
        (vh - vb * k / q + k * k) / a0,
        2.0 * (k * k - 1.0) / a0,
        (1.0 - k / q + k * k) / a0,
    );
    k = (PI * 38.13547087602444 / sr).tan();
    q = 0.5003270373238773;
    a0 = 1.0 + k / q + k * k;
    biquad(&shelved, 1.0, -2.0, 1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0)
}

/// Integrated loudness in LUFS (ITU-R BS.1770-4). Silence gives negative infinity.
pub fn loudness(chs: &[Vec<f32>], sr: u32) -> f64 {
    let srf = sr as f64;
    let n = chs[0].len();
    let full = jround(srf * 0.4) as usize;
    let block = n.min(full);
    let step = (jround(srf * 0.1) as usize).max(1);
    if n == 0 {
        return f64::NEG_INFINITY;
    }
    let weighted: Vec<Vec<f32>> = chs.iter().map(|d| k_weight(d, srf)).collect();
    let mut blocks = Vec::new();
    let mut s = 0;
    while s + block <= n {
        let mut z = 0f64;
        for d in &weighted {
            let mut e = 0f64;
            for &x in &d[s..s + block] {
                e += x as f64 * x as f64;
            }
            z += e / block as f64;
        }
        blocks.push(z);
        s += step;
    }
    let lufs = |z: f64| -0.691 + 10.0 * z.log10();
    let mean = |a: &[f64]| a.iter().fold(0.0, |x, v| x + v) / a.len() as f64;
    if block < full {
        return lufs(blocks[0]);
    }
    let loud: Vec<f64> = blocks.into_iter().filter(|&z| lufs(z) > -70.0).collect();
    if loud.is_empty() {
        return f64::NEG_INFINITY;
    }
    let gate = lufs(mean(&loud)) - 10.0;
    let kept: Vec<f64> = loud.into_iter().filter(|&z| lufs(z) > gate).collect();
    if kept.is_empty() { f64::NEG_INFINITY } else { lufs(mean(&kept)) }
}

const TP_HALF: usize = 6;

fn tp_taps() -> &'static [[f32; 2 * TP_HALF]; 3] {
    static T: OnceLock<[[f32; 2 * TP_HALF]; 3]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [[0f32; 2 * TP_HALF]; 3];
        for (pi, row) in t.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                let x = j as f64 - TP_HALF as f64 + 1.0 - (pi + 1) as f64 / 4.0;
                *v = ((PI * x).sin() / (PI * x) * (0.5 + 0.5 * (PI * x / TP_HALF as f64).cos())) as f32;
            }
        }
        t
    })
}

/// For every sample, the highest level the 4x-oversampled waveform reaches at or just after it.
pub fn true_peak_envelope(chs: &[Vec<f32>]) -> Vec<f32> {
    let taps = tp_taps();
    let n = chs[0].len();
    let mut env = vec![0f32; n];
    for d in chs {
        for i in 0..n {
            let mut m = (d[i] as f64).abs();
            if i + 1 >= TP_HALF && i + TP_HALF < n {
                for row in taps {
                    let mut v = 0f64;
                    let base = i + 1 - TP_HALF;
                    for j in 0..2 * TP_HALF {
                        v += d[base + j] as f64 * row[j] as f64;
                    }
                    let v = v.abs();
                    if v > m {
                        m = v;
                    }
                }
            }
            if m > env[i] as f64 {
                env[i] = m as f32;
            }
        }
    }
    env
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measure {
    pub lufs: f64,
    pub peak: f64,
    pub true_peak: f64,
}

/// Integrated loudness, sample peak and true peak (dB full scale).
pub fn measure(chs: &[Vec<f32>], sr: u32) -> Measure {
    let env = true_peak_envelope(chs);
    let mut tp = 0f64;
    for &v in &env {
        if v as f64 > tp {
            tp = v as f64;
        }
    }
    Measure { lufs: loudness(chs, sr), peak: stats(chs).peak, true_peak: db(tp) }
}

/// WSOLA time stretch without pitch change. `factor` is new length over old length.
pub fn stretch(chs: &[Vec<f32>], sr: u32, factor: f64) -> Vec<Vec<f32>> {
    let len = chs[0].len();
    if len == 0 || (factor - 1.0).abs() < 1e-4 {
        return chs.to_vec();
    }
    let sr = sr as f64;
    let n = jround(sr * 0.04 / 2.0) as usize * 2;
    let hs = n / 2;
    const D: usize = 4;
    let tol = jround(sr * 0.012 / D as f64) as i64;
    let nd = n / D;
    let out_len = (jround(len as f64 * factor) as i64).max(1) as usize;
    let win = hann(n, n);
    let mono = mix_mono(chs);
    let md_len = len.div_ceil(D) + nd + tol as usize + 2;
    let mut md = vec![0f32; md_len];
    let mut i = 0;
    let mut k = 0;
    while i + D <= len {
        md[k] = ((mono[i] as f64 + mono[i + 1] as f64 + mono[i + 2] as f64 + mono[i + 3] as f64) / 4.0) as f32;
        i += D;
        k += 1;
    }
    let mut out = vec![vec![0f32; out_len + n]; chs.len()];
    let mut wsum = vec![0f32; out_len + n];
    let mut prev = 0i64;
    let mut m = 0usize;
    while m * hs < out_len {
        let mut pos = jround((m * hs) as f64 / factor) as i64;
        if m > 0 {
            let nat = (md_len as i64 - nd as i64 - 1).min(jround((prev + hs as i64) as f64 / D as f64) as i64) as usize;
            let ideal = jround(pos as f64 / D as f64) as i64;
            let (mut best, mut best_score) = (0i64, f64::NEG_INFINITY);
            for lag in -tol..=tol {
                let c = ideal + lag;
                if c < 0 || c as usize + nd >= md_len {
                    continue;
                }
                let c = c as usize;
                let (mut dot, mut energy) = (0f64, 1e-9f64);
                for i in 0..nd {
                    let v = md[c + i] as f64;
                    dot += v * md[nat + i] as f64;
                    energy += v * v;
                }
                let score = dot / energy.sqrt();
                if score > best_score {
                    best_score = score;
                    best = lag;
                }
            }
            pos += best * D as i64;
        }
        let pos = pos.min(len as i64 - n as i64).max(0) as usize;
        let at = m * hs;
        let take = n.min(len - pos);
        let w = |i: usize| if m == 0 && i < hs { 1.0 } else { win[i] as f64 };
        for (d, o) in chs.iter().zip(out.iter_mut()) {
            for i in 0..take {
                o[at + i] = (o[at + i] as f64 + d[pos + i] as f64 * w(i)) as f32;
            }
        }
        for i in 0..take {
            wsum[at + i] = (wsum[at + i] as f64 + w(i)) as f32;
        }
        prev = pos as i64;
        m += 1;
    }
    out.into_iter()
        .map(|mut o| {
            o.truncate(out_len);
            for (v, &ws) in o.iter_mut().zip(&wsum) {
                if ws as f64 > 1e-6 {
                    *v = (*v as f64 / ws as f64) as f32;
                }
            }
            o
        })
        .collect()
}

/// Stretch to `length` (new over old) and shift pitch by `semitones`, independently.
pub fn stretch_pitch(chs: &[Vec<f32>], sr: u32, length: f64, semitones: f64) -> Vec<Vec<f32>> {
    let ratio = 2f64.powf(semitones / 12.0);
    let stretched = stretch(chs, sr, length * ratio);
    if (ratio - 1.0).abs() < 1e-6 { stretched } else { resample_sinc(&stretched, jround(ratio * 100000.0) as u32, 100000) }
}

#[derive(Debug, Clone, Copy)]
pub struct SatParams {
    pub drive: f64,
    pub ceiling: f64,
    pub mix: f64,
}

impl SatParams {
    pub fn from_json(p: &Value) -> Self {
        SatParams {
            drive: num(p, "drive").unwrap_or(6.0),
            ceiling: num(p, "ceiling").unwrap_or(-1.0),
            mix: num(p, "mix").unwrap_or(100.0),
        }
    }
}

/// What the soft clipper does to a level in dB.
pub fn sat_curve(in_db: f64, drive: f64, ceiling: f64) -> f64 {
    let c = lin(ceiling);
    db(c * (lin(in_db + drive) / c).tanh())
}

/// Drive into a tanh soft clipper at twice the sample rate; `mix` (0-100) blends the dry sound back.
pub fn saturate(chs: &[Vec<f32>], _sr: u32, p: &SatParams) -> Processed {
    let (g, c) = (lin(p.drive), lin(p.ceiling));
    let mix = (p.mix / 100.0).min(1.0).max(0.0);
    let mut up = resample_sinc(chs, 1, 2);
    for d in up.iter_mut() {
        for v in d.iter_mut() {
            *v = (c * (*v as f64 * g / c).tanh()) as f32;
        }
    }
    let wet = resample_sinc(&up, 2, 1);
    let channels: Vec<Vec<f32>> = chs
        .iter()
        .zip(&wet)
        .map(|(d, w)| (0..d.len()).map(|i| (d[i] as f64 * (1.0 - mix) + w.get(i).map_or(0.0, |&x| x as f64) * mix) as f32).collect())
        .collect();
    Processed { info: format!("peak {} dB", to_fixed1(stats(&channels).peak)), channels }
}

#[derive(Debug, Clone, Copy)]
pub struct ShapeParams {
    pub attack: f64,
    pub tail: f64,
}

impl ShapeParams {
    pub fn from_json(p: &Value) -> Self {
        ShapeParams { attack: num(p, "attack").unwrap_or(0.0), tail: num(p, "tail").unwrap_or(0.0) }
    }
}

/// Transient shaper: `attack` and `tail` from -100 to 100, one gain for all channels.
pub fn shape(chs: &[Vec<f32>], sr: u32, p: &ShapeParams) -> Processed {
    let sr = sr as f64;
    let n = chs[0].len();
    let k = |ms: f64| (-1.0 / (sr * ms / 1000.0)).exp();
    let (fast_up, slow_up, down, slow_down, smooth) = (k(0.5), k(18.0), k(60.0), k(450.0), k(1.5));
    let a_amt = p.attack / 100.0;
    let t_amt = p.tail / 100.0 * 1.6;
    let floor = 1e-5;
    let mut out = vec![vec![0f32; n]; chs.len()];
    let (mut ef, mut es, mut rf, mut rs, mut gs) = (0f64, 0f64, 0f64, 0f64, 0f64);
    for i in 0..n {
        let mut x = 0f64;
        for d in chs {
            let v = (d[i] as f64).abs();
            if v > x {
                x = v;
            }
        }
        ef = if x > ef { x + (ef - x) * fast_up } else { x + (ef - x) * down };
        es = if x > es { x + (es - x) * slow_up } else { x + (es - x) * down };
        rf = if x > rf { x } else { x + (rf - x) * down };
        rs = if x > rs { x } else { x + (rs - x) * slow_down };
        let hit = (db(ef + floor) - db(es + floor)).max(0.0);
        let ring = (db(rs + floor) - db(rf + floor)).max(0.0);
        let want = (a_amt * hit + t_amt * ring).min(18.0).max(-36.0);
        gs = want + (gs - want) * smooth;
        let gain = lin(gs);
        for (c, d) in chs.iter().enumerate() {
            out[c][i] = (d[i] as f64 * gain) as f32;
        }
    }
    Processed { info: format!("peak {} dB", to_fixed1(stats(&out).peak)), channels: out }
}

#[derive(Debug, Clone, Copy)]
pub struct LofiParams {
    pub bits: f64,
    pub rate: f64,
}

impl LofiParams {
    pub fn from_json(p: &Value) -> Self {
        LofiParams { bits: num(p, "bits").unwrap_or(12.0), rate: num(p, "rate").unwrap_or(26.0) }
    }
}

/// Sample-and-hold to a lower rate (`rate` in kHz) and round to `bits`.
pub fn lofi(chs: &[Vec<f32>], sr: u32, p: &LofiParams) -> Processed {
    let srf = sr as f64;
    let step = (srf / (p.rate * 1000.0).min(srf).max(500.0)).max(1.0);
    let q = 2f64.powf(p.bits.min(24.0).max(2.0) - 1.0);
    let crush = p.bits < 24.0;
    let channels = chs
        .iter()
        .map(|d| {
            let (mut next, mut held) = (0f64, 0f64);
            d.iter()
                .enumerate()
                .map(|(i, &x)| {
                    if i as f64 >= next {
                        held = if crush { jround(x as f64 * q) / q } else { x as f64 };
                        next += step;
                    }
                    held as f32
                })
                .collect()
        })
        .collect();
    Processed { channels, info: format!("{}-bit at {} kHz", js_num(p.bits), to_fixed1(srf / step / 1000.0)) }
}

const NOTES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub midi: i64,
    pub name: String,
    pub hz: f64,
    pub cents: i64,
}

/// The nearest note to a frequency.
pub fn note_at(hz: f64) -> Note {
    let m = 69.0 + 12.0 * (hz / 440.0).log2();
    let midi = jround(m) as i64;
    Note {
        midi,
        name: format!("{}{}", NOTES[midi.rem_euclid(12) as usize], midi.div_euclid(12) - 1),
        hz: 440.0 * 2f64.powf((midi - 69) as f64 / 12.0),
        cents: jround((m - midi as f64) * 100.0) as i64,
    }
}

/// The pitch a sound holds in Hz (YIN, median over pitched frames), or None.
pub fn detect_pitch(d: &[f32], sr: u32) -> Option<f64> {
    let dd = if sr >= 32000 { 2 } else { 1 };
    let r = sr as f64 / dd as f64;
    let mlen = (d.len() / dd).min(jround(r * 12.0) as usize);
    let m: Vec<f32> = (0..mlen).map(|i| if dd == 2 { ((d[2 * i] as f64 + d[2 * i + 1] as f64) / 2.0) as f32 } else { d[i] }).collect();
    let max_lag = (r / 25.0).floor() as usize;
    let min_lag = 2.max((r / 2000.0).floor() as usize);
    let w = max_lag * 2;
    if mlen < w + max_lag {
        return None;
    }
    let mut diff = vec![0f32; max_lag + 1];
    let mut found = Vec::new();
    let mut top = 0f64;
    let hop = 256.max((mlen - w - max_lag) / 60);
    let mut frames = Vec::new();
    let mut s = 0;
    while s + w + max_lag <= mlen {
        let mut e = 0f64;
        for i in 0..w {
            e += m[s + i] as f64 * m[s + i] as f64;
        }
        frames.push((s, e));
        if e > top {
            top = e;
        }
        s += hop;
    }
    for &(s, e) in &frames {
        if e < top * 0.01 {
            continue;
        }
        let mut run = 0f64;
        diff[0] = 1.0;
        for t in 1..=max_lag {
            let mut sum = 0f64;
            for i in 0..w {
                let v = m[s + i] as f64 - m[s + i + t] as f64;
                sum += v * v;
            }
            run += sum;
            diff[t] = (if run != 0.0 { sum * t as f64 / run } else { 1.0 }) as f32;
        }
        let mut lag = None;
        let mut t = min_lag;
        while t < max_lag {
            if (diff[t] as f64) < 0.12 {
                while t + 1 < max_lag && diff[t + 1] < diff[t] {
                    t += 1;
                }
                lag = Some(t);
                break;
            }
            t += 1;
        }
        let Some(lag) = lag else { continue };
        let (a, b, c) = (diff[lag - 1] as f64, diff[lag] as f64, diff[lag + 1] as f64);
        let den = a - 2.0 * b + c;
        found.push(r / (lag as f64 + if den != 0.0 { (a - c) / (2.0 * den) } else { 0.0 }));
    }
    if (found.len() as f64) < (frames.len() as f64 * 0.2).max(1.0) {
        return None;
    }
    found.sort_by(f64::total_cmp);
    Some(found[found.len() >> 1])
}

/// Sample positions where a new sound starts. `sensitivity` runs from 1 to 10.
pub fn onsets(d: &[f32], sr: u32, sensitivity: f64) -> Vec<usize> {
    const N: usize = 1024;
    const H: usize = 256;
    let half = N / 2;
    let frames = if d.len() >= N { (d.len() - N) / H + 1 } else { 0 };
    let win = hann(N, N);
    let (mut re, mut im) = (vec![0f32; N], vec![0f32; N]);
    let mut prev = vec![0f32; half];
    let mut flux = vec![0f32; frames];
    for f in 0..frames {
        for i in 0..N {
            re[i] = (d[f * H + i] as f64 * win[i] as f64) as f32;
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        let mut sum = 0f64;
        for k in 1..half {
            let m = (100.0 * (re[k] as f64 * re[k] as f64 + im[k] as f64 * im[k] as f64).sqrt()).ln_1p();
            if m > prev[k] as f64 {
                sum += m - prev[k] as f64;
            }
            prev[k] = m as f32;
        }
        flux[f] = if f > 0 { sum as f32 } else { 0.0 };
    }
    let mut total = 0f64;
    for &v in &flux {
        total += v as f64;
    }
    let floor = total / frames.max(1) as f64;
    let k = 4.2 - sensitivity * 0.32;
    let gap = (sr as f64 * 0.05 / H as f64).ceil() as i64;
    let mut out = Vec::new();
    let mut last = -gap;
    let srf = sr as f64;
    for f in 1..frames.saturating_sub(1) {
        let (mut local, mut count) = (0f64, 0f64);
        for j in f.saturating_sub(12)..frames.min(f + 12) {
            local += flux[j] as f64;
            count += 1.0;
        }
        let x = flux[f] as f64;
        if x > k * local / count + 0.2 * floor && flux[f] >= flux[f - 1] && flux[f] > flux[f + 1] && f as i64 - last >= gap {
            let lo = f * H;
            let hi = d.len().min(lo + N + H);
            let mut peak = 0f64;
            for &v in &d[lo..hi] {
                let v = (v as f64).abs();
                if v > peak {
                    peak = v;
                }
            }
            let mut pos = lo;
            for i in lo..hi {
                if (d[i] as f64).abs() > 0.2 * peak {
                    pos = lo.max((i as i64 - jround(srf * 0.002) as i64).max(0) as usize);
                    break;
                }
            }
            out.push(pos);
            last = f as i64;
        }
    }
    out
}

/// Replace clicks with straight lines. Returns the channels and how many clicks were repaired.
pub fn declick(chs: &[Vec<f32>], sr: u32, sensitivity: f64) -> (Vec<Vec<f32>>, usize) {
    let max_run = jround(sr as f64 * 0.003) as usize;
    const W: usize = 1024;
    let thr = 26.0 - sensitivity * 2.2;
    let mut repaired = 0;
    let channels = chs
        .iter()
        .map(|d| {
            let n = d.len();
            let mut out = d.clone();
            let mut e = vec![0f32; n];
            for i in 2..n {
                e[i] = (d[i] as f64 - 2.0 * d[i - 1] as f64 + d[i - 2] as f64).abs() as f32;
            }
            let mut sum = 0f64;
            for &v in &e[..n.min(W)] {
                sum += v as f64;
            }
            let mut i = 2;
            while i + 2 < n {
                let a = i.saturating_sub(W / 2);
                let b = n.min(i + W / 2);
                if i > W / 2 && b < n {
                    sum += e[b] as f64 - e[a - 1] as f64;
                }
                if e[i] as f64 <= thr * (sum / W as f64) + 1e-5 {
                    i += 1;
                    continue;
                }
                let mut end = i;
                while end + 1 + 2 < n && end - i < max_run && e[end + 1] as f64 > 0.5 * thr * (sum / W as f64) {
                    end += 1;
                }
                let from = i.saturating_sub(3).max(1);
                let to = (n - 2).min(end + 3);
                let (y0, y1) = (out[from - 1] as f64, d[to + 1] as f64);
                for k in from..=to {
                    out[k] = (y0 + (y1 - y0) * (k - from + 1) as f64 / (to - from + 2) as f64) as f32;
                }
                repaired += 1;
                i = to + 1;
            }
            out
        })
        .collect();
    (channels, repaired)
}

fn declick_info(repaired: usize) -> String {
    format!("{repaired} click{} repaired", if repaired == 1 { "" } else { "s" })
}

/// Fold the last `n` samples into the first `n` with an equal-power crossfade and drop them.
pub fn loop_crossfade(chs: &[Vec<f32>], n: usize) -> Vec<Vec<f32>> {
    let len = chs[0].len();
    let n = n.min(len >> 1);
    chs.iter()
        .map(|d| {
            let mut out = d[..len - n].to_vec();
            for i in 0..n {
                let t = (i as f64 + 0.5) / n as f64;
                out[i] = (d[i] as f64 * (t * PI / 2.0).sin() + d[len - n + i] as f64 * (t * PI / 2.0).cos()) as f32;
            }
            out
        })
        .collect()
}

/// First and last sample of the mono mix at or above `threshold`, padded by 5 ms.
pub fn sound_bounds(chs: &[Vec<f32>], sr: u32, threshold: f64) -> Option<(usize, usize)> {
    let m = mix_mono(chs);
    let (mut a, mut b) = (0, m.len());
    while a < b && (m[a] as f64).abs() < threshold {
        a += 1;
    }
    while b > a && (m[b - 1] as f64).abs() < threshold {
        b -= 1;
    }
    if a >= b {
        return None;
    }
    let pad = jround(sr as f64 * 0.005) as usize;
    Some((a.saturating_sub(pad), m.len().min(b + pad)))
}

/// Equal-power (sin² / cos²) fade over samples a..b, in place.
pub fn fade(chs: &mut [Vec<f32>], a: usize, b: usize, fade_in: bool) {
    if b <= a {
        return;
    }
    let n = b - a;
    for d in chs.iter_mut() {
        for i in 0..n {
            let t = i as f64 / n as f64;
            let g = if fade_in { (t * PI / 2.0).sin() } else { (t * PI / 2.0).cos() };
            d[a + i] = (d[a + i] as f64 * (g * g)) as f32;
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PrepParams {
    pub silence_db: f64,
    pub target_db: f64,
    pub fade_in_ms: f64,
    pub fade_out_ms: f64,
}

impl Default for PrepParams {
    fn default() -> Self {
        PrepParams { silence_db: -45.0, target_db: -1.0, fade_in_ms: 2.0, fade_out_ms: 10.0 }
    }
}

impl PrepParams {
    pub fn from_json(p: &Value) -> Self {
        let d = Self::default();
        PrepParams {
            silence_db: num(p, "silenceDb").unwrap_or(d.silence_db),
            target_db: num(p, "targetDb").unwrap_or(d.target_db),
            fade_in_ms: num(p, "fadeInMs").unwrap_or(d.fade_in_ms),
            fade_out_ms: num(p, "fadeOutMs").unwrap_or(d.fade_out_ms),
        }
    }
}

/// Sampler prep: remove DC, trim silent ends, normalize the peak, add short edge fades.
pub fn prep(chs: &[Vec<f32>], sr: u32, p: &PrepParams) -> Vec<Vec<f32>> {
    let mut out: Vec<Vec<f32>> = chs
        .iter()
        .map(|d| {
            let mut m = 0f64;
            for &v in d {
                m += v as f64;
            }
            m /= d.len().max(1) as f64;
            d.iter().map(|&v| (v as f64 - m) as f32).collect()
        })
        .collect();
    if let Some((a, b)) = sound_bounds(&out, sr, lin(p.silence_db)) {
        out = out.into_iter().map(|d| d[a..b].to_vec()).collect();
    }
    let mut peak = 0f64;
    for d in &out {
        for &v in d {
            peak = peak.max((v as f64).abs());
        }
    }
    if peak != 0.0 {
        let k = lin(p.target_db) / peak;
        for d in out.iter_mut() {
            for v in d.iter_mut() {
                *v = (*v as f64 * k) as f32;
            }
        }
    }
    let len = out[0].len();
    let srf = sr as f64;
    fade(&mut out, 0, len.min(jround(srf * p.fade_in_ms / 1000.0).max(0.0) as usize), true);
    fade(&mut out, (len as i64 - jround(srf * p.fade_out_ms / 1000.0) as i64).max(0) as usize, len, false);
    out
}

/// Result of `run`: processed channels (empty when the op returns none), the op's other results
/// as JSON under the keys worker.js returns, and file bytes for `render`.
#[derive(Debug, Clone)]
pub struct Out {
    pub channels: Vec<Vec<f32>>,
    pub info: Value,
    pub bytes: Option<Vec<u8>>,
}

impl Default for Out {
    fn default() -> Self {
        Out { channels: vec![], info: json!({}), bytes: None }
    }
}

fn processed(p: Processed) -> Out {
    Out { channels: p.channels, info: json!({ "info": p.info }), bytes: None }
}

fn sensitivity(p: &Value) -> f64 {
    num(p, "sensitivity").unwrap_or(5.0)
}

/// Runs a worker.js op. `p` is the op's arguments as worker.js receives them, minus
/// `chs`/`data`/`sr`; ops that take mono `data` read it from `chs[0]`. Non-finite numbers
/// (a silent sound's `lufs`) come back as JSON null. `mix`, `spec` and `specData` are not
/// handled here: see `arrange::render_mix`, `spectrogram` and `spectrogram_peak`.
pub fn run(op: &str, chs: Vec<Vec<f32>>, sr: u32, p: &Value) -> Result<Out, String> {
    if chs.is_empty() {
        return Err(format!("{op}: no channels"));
    }
    let params = p.get("p").unwrap_or(&Value::Null);
    Ok(match op {
        "compress" => processed(compress(&chs, sr, &CompParams::from_json(params))),
        "maximize" => processed(maximize(&chs, sr, &MaxParams::from_json(params))),
        "denoise" => {
            let profile: Vec<f32> = p
                .get("profile")
                .and_then(Value::as_array)
                .ok_or("denoise: profile missing")?
                .iter()
                .map(|v| v.as_f64().unwrap_or(f64::NAN) as f32)
                .collect();
            processed(denoise(&chs, sr, &profile, &NrParams::from_json(params)))
        }
        "resample" => {
            let rate = |k: &str| num(p, k).map(|v| v as u32).ok_or(format!("resample: {k} missing"));
            Out { channels: resample_sinc(&chs, rate("from")?, rate("to")?), ..Default::default() }
        }
        "measure" => {
            let m = measure(&chs, sr);
            Out {
                info: json!({ "lufs": finite_or_null(m.lufs), "peak": finite_or_null(m.peak), "truePeak": finite_or_null(m.true_peak) }),
                ..Default::default()
            }
        }
        "stretchPitch" => Out {
            channels: stretch_pitch(&chs, sr, num(p, "length").unwrap_or(1.0), num(p, "semitones").unwrap_or(0.0)),
            ..Default::default()
        },
        "declick" => {
            let (channels, n) = declick(&chs, sr, sensitivity(p));
            Out { channels, info: json!({ "info": declick_info(n) }), bytes: None }
        }
        "saturate" => processed(saturate(&chs, sr, &SatParams::from_json(params))),
        "shape" => processed(shape(&chs, sr, &ShapeParams::from_json(params))),
        "lofi" => processed(lofi(&chs, sr, &LofiParams::from_json(params))),
        "pitch" => Out { info: json!({ "hz": detect_pitch(&chs[0], sr).map_or(Value::Null, finite_or_null) }), ..Default::default() },
        "onsets" => Out { info: json!({ "positions": onsets(&chs[0], sr, sensitivity(p)) }), ..Default::default() },
        "render" => {
            let rate = num(p, "rate").map_or(sr, |v| v as u32);
            let bits = num(p, "bits").map_or(16, |v| v as u32);
            let mut c = if truthy(p.get("mono")) && chs.len() > 1 { vec![mix_mono(&chs)] } else { chs };
            if truthy(p.get("prep")) {
                c = prep(&c, sr, &PrepParams::from_json(&p["prep"]));
            }
            if rate != sr {
                c = resample_sinc(&c, sr, rate);
            }
            let seed = num(p, "ditherSeed").map_or_else(random_seed, |v| v as u32);
            Out { bytes: Some(encode_wav_seeded(&c, rate, bits, seed)?), ..Default::default() }
        }
        "noiseProfile" => {
            let at = |k: &str| num(p, k).map(|v| v.max(0.0) as usize);
            let prof = noise_profile(&chs, at("a").unwrap_or(0), at("b").unwrap_or(usize::MAX));
            Out { info: json!({ "profile": prof }), ..Default::default() }
        }
        "estimateProfile" => Out { info: json!({ "profile": estimate_profile(&chs) }), ..Default::default() },
        "loopCrossfade" => Out { channels: loop_crossfade(&chs, num(p, "n").unwrap_or(0.0).max(0.0) as usize), ..Default::default() },
        _ => return Err(format!("unknown op {op}")),
    })
}
