//! Signal descriptors used for filtering and sorting: a port of `analyze` and `peaks` in
//! athar/engine.py, and of the librosa functions they call (spectral centroid and flatness,
//! onset detection, YIN pitch, tempo estimation and beat tracking), with librosa's defaults.

use crate::decode::{resample, Decoded, ANALYSIS_SECONDS};
use crate::embed::Error;
use realfft::RealFftPlanner;

const SR: f64 = 22_050.0;
const N_FFT: usize = 2048;
const HOP: usize = 512;
/// The smallest positive f32, numpy's `tiny` for the float32 arrays librosa works on.
const TINY: f64 = f32::MIN_POSITIVE as f64;

#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub peak_db: f64,
    pub rms_db: f64,
    pub crest: f64,
    pub brightness: f64,
    pub pitch_hz: Option<f64>,
    pub tempo: Option<f64>,
    pub onsets: usize,
    pub is_loop: bool,
}

fn periodic_hann(n: usize) -> Vec<f64> {
    (0..n).map(|k| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * k as f64 / n as f64).cos()).collect()
}

/// Magnitude spectra of a centred, zero-padded STFT (n_fft 2048, hop 512, Hann window).
fn stft_mag(y: &[f64]) -> Vec<Vec<f64>> {
    let half = N_FFT / 2;
    let mut padded = vec![0.0; half];
    padded.extend_from_slice(y);
    padded.resize(padded.len() + half, 0.0);
    let frames = 1 + (padded.len() - N_FFT) / HOP;
    let win = periodic_hann(N_FFT);
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(N_FFT);
    let (mut buf, mut spec) = (fft.make_input_vec(), fft.make_output_vec());
    (0..frames)
        .map(|f| {
            for k in 0..N_FFT {
                buf[k] = padded[f * HOP + k] * win[k];
            }
            fft.process(&mut buf, &mut spec).expect("FFT sizes are fixed");
            // librosa stores the spectrum as complex64 and its magnitude as float32. Keeping
            // that rounding matters in near-silent frames, where it decides whether a frame
            // counts at all.
            spec.iter().map(|c| ((c.re as f32).hypot(c.im as f32)) as f64).collect()
        })
        .collect()
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 }
}

fn spectral_centroid(mags: &[Vec<f64>]) -> f64 {
    let freq: Vec<f64> = (0..=N_FFT / 2).map(|k| k as f64 * SR / N_FFT as f64).collect();
    let per_frame: Vec<f64> = mags
        .iter()
        .map(|m| {
            let s: f64 = m.iter().sum();
            let s = if s < TINY { 1.0 } else { s };
            m.iter().zip(&freq).map(|(a, f)| a / s * f).sum()
        })
        .collect();
    mean(&per_frame)
}

fn spectral_flatness(mags: &[Vec<f64>]) -> f64 {
    let per_frame: Vec<f64> = mags
        .iter()
        .map(|m| {
            let p: Vec<f64> = m.iter().map(|a| (a * a).max(1e-10)).collect();
            let g = (p.iter().map(|v| v.ln()).sum::<f64>() / p.len() as f64).exp();
            g / mean(&p)
        })
        .collect();
    mean(&per_frame)
}

/// librosa.onset.onset_strength: mean positive change of a 128-band mel spectrogram in dB,
/// shifted to line up with the frames.
fn onset_strength(mags: &[Vec<f64>]) -> Vec<f64> {
    let mel = crate::features::mel(SR, N_FFT, 128, 0.0, SR / 2.0);
    let mut db: Vec<Vec<f64>> = mags
        .iter()
        .map(|m| {
            mel.iter()
                .map(|row| {
                    let e: f64 = row.iter().zip(m).map(|(w, a)| w * a * a).sum();
                    10.0 * e.max(1e-10).log10()
                })
                .collect()
        })
        .collect();
    let top = db.iter().flatten().cloned().fold(f64::NEG_INFINITY, f64::max) - 80.0;
    db.iter_mut().flatten().for_each(|v| *v = v.max(top));
    let t = db.len();
    let mut env = vec![0.0; t];
    // lag 1, plus n_fft / (2 hop) = 2 frames of centring.
    for i in 3..t {
        let (cur, prev) = (&db[i - 2], &db[i - 3]);
        env[i] = cur.iter().zip(prev).map(|(c, p)| (c - p).max(0.0)).sum::<f64>() / cur.len() as f64;
    }
    env
}

/// librosa.onset.onset_detect with its defaults, counted.
fn count_onsets(env: &[f64]) -> usize {
    if env.is_empty() {
        return 0;
    }
    let lo = env.iter().cloned().fold(f64::INFINITY, f64::min);
    let x: Vec<f64> = env.iter().map(|v| v - lo).collect();
    let hi = x.iter().cloned().fold(0.0, f64::max);
    let x: Vec<f64> = x.iter().map(|v| v / (hi + TINY)).collect();
    if !x.iter().any(|&v| v != 0.0) {
        return 0;
    }
    // pre_max 30 ms, post_max 1 frame, pre_avg 100 ms, post_avg 100 ms + 1, wait 30 ms.
    let f = |s: f64| (s * SR / HOP as f64).floor() as usize;
    let (pre_max, post_max, pre_avg, post_avg, wait, delta) = (f(0.03), 1, f(0.10), f(0.10) + 1, f(0.03), 0.07);
    let n = x.len();
    let max_of = |a: usize, b: usize| x[a..b].iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mean_of = |a: usize, b: usize| x[a..b].iter().sum::<f64>() / (b - a) as f64;
    let mut count = 0;
    let first = x[0] >= max_of(0, post_max.min(n)) && x[0] >= mean_of(0, post_avg.min(n)) + delta;
    let mut i = if first {
        count += 1;
        wait + 1
    } else {
        1
    };
    while i < n {
        if x[i] != max_of(i.saturating_sub(pre_max), (i + post_max).min(n)) || x[i] < mean_of(i.saturating_sub(pre_avg), (i + post_avg).min(n)) + delta {
            i += 1;
            continue;
        }
        count += 1;
        i += wait + 1;
    }
    count
}

/// librosa.yin(fmin=30, fmax=2000, sr=22050): one frequency per 512-sample hop.
fn yin(y: &[f64]) -> Vec<f64> {
    let (frame, hop) = (2048usize, 512usize);
    let min_period = (SR / 2000.0).floor() as usize;
    let max_period = ((SR / 30.0).ceil() as usize).min(frame - 1);
    let mut padded = vec![0.0; frame / 2];
    padded.extend_from_slice(y);
    padded.resize(padded.len() + frame / 2, 0.0);
    let frames = 1 + (padded.len() - frame) / hop;
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let x = &padded[f * hop..f * hop + frame];
        let acf: Vec<f64> = (0..=max_period).map(|k| x[..frame - k].iter().zip(&x[k..]).map(|(a, b)| a * b).sum()).collect();
        let mut energy = vec![0.0; frame];
        let mut run = 0.0;
        for (e, v) in energy.iter_mut().zip(x) {
            run += v * v;
            *e = run;
        }
        energy[0] = 0.0;
        let mut d = vec![0.0; max_period + 1];
        for k in 1..=max_period {
            d[k] = 2.0 * (acf[0] - acf[k]) - energy[k - 1];
        }
        let mut cum = vec![0.0; max_period];
        let mut run = 0.0;
        for k in 1..=max_period {
            run += d[k];
            cum[k - 1] = run / k as f64;
        }
        let yf: Vec<f64> = (min_period..=max_period).map(|k| d[k] / (cum[k - 1] + TINY)).collect();
        let m = yf.len();
        let shift = |j: usize| {
            if j == 0 || j + 1 >= m {
                return 0.0;
            }
            let a = yf[j + 1] + yf[j - 1] - 2.0 * yf[j];
            let b = (yf[j + 1] - yf[j - 1]) / 2.0;
            if b.abs() >= a.abs() { 0.0 } else { -b / a }
        };
        let trough = |j: usize| {
            if j == 0 {
                yf[0] < yf[1]
            } else if j + 1 == m {
                yf[j] < yf[j - 1]
            } else {
                yf[j] < yf[j - 1] && yf[j] <= yf[j + 1]
            }
        };
        let period = (0..m).find(|&j| trough(j) && yf[j] < 0.1).unwrap_or_else(|| {
            (0..m).fold(0, |best, j| if yf[j] < yf[best] { j } else { best })
        });
        out.push(SR / (min_period as f64 + period as f64 + shift(period)));
    }
    out
}

fn hz_to_midi(f: f64) -> f64 {
    12.0 * (f.log2() - 440f64.log2()) + 69.0
}

/// librosa.feature.tempo: the autocorrelation tempogram over 8 s windows, averaged, weighted by
/// a log-normal prior around 120 BPM.
fn tempo(env: &[f64]) -> f64 {
    let win = ((8.0 * SR) as usize) / HOP;
    let n = env.len();
    let half = win / 2;
    let mut padded = vec![0.0; half];
    padded.extend_from_slice(env);
    let edge = *env.last().unwrap_or(&0.0);
    padded.extend((0..half).map(|i| edge * (half - i) as f64 / half as f64));
    let w = periodic_hann(win);
    let mut tg = vec![0.0; win];
    let mut frame = vec![0.0; win];
    for t in 0..n {
        for k in 0..win {
            frame[k] = padded[t + k] * w[k];
        }
        let ac: Vec<f64> = (0..win).map(|lag| frame[..win - lag].iter().zip(&frame[lag..]).map(|(a, b)| a * b).sum()).collect();
        let peak = ac.iter().map(|v| v.abs()).fold(0.0, f64::max);
        let peak = if peak < TINY { 1.0 } else { peak };
        for (g, v) in tg.iter_mut().zip(&ac) {
            *g += v / peak;
        }
    }
    tg.iter_mut().for_each(|g| *g /= n.max(1) as f64);
    let bpm = |lag: usize| if lag == 0 { f64::INFINITY } else { 60.0 * SR / (HOP as f64 * lag as f64) };
    let max_idx = (0..win).find(|&l| bpm(l) < 320.0).unwrap_or(0);
    (0..win)
        .filter(|&l| l >= max_idx)
        .map(|l| (l, (1e6 * tg[l]).ln_1p() - 0.5 * (bpm(l).log2() - 120f64.log2()).powi(2)))
        .fold((0, f64::NEG_INFINITY), |b, c| if c.1 > b.1 { c } else { b })
        .0
        .max(1)
        .pipe(bpm)
}

trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl<T> Pipe for T {}

/// librosa.beat.beat_track's dynamic-programming tracker at a fixed tempo, returning beat frames.
fn beats(env: &[f64], bpm: f64) -> Vec<usize> {
    let n = env.len();
    if n < 2 || !env.iter().any(|&v| v != 0.0) {
        return vec![];
    }
    let fpb = ((SR / HOP as f64) * 60.0 / bpm).round_ties_even();
    let m = mean(env);
    let sd = (env.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt();
    let onsets: Vec<f64> = env.iter().map(|v| v / (sd + TINY)).collect();
    let r = fpb as i64;
    let window: Vec<f64> = (-r..=r).map(|k| (-0.5 * (k as f64 * 32.0 / fpb).powi(2)).exp()).collect();
    let kk = window.len();
    let local: Vec<f64> = (0..n)
        .map(|i| {
            let lo = (i + kk / 2 + 1).saturating_sub(n);
            let hi = (i + kk / 2).min(kk);
            (lo..hi).map(|k| window[k] * onsets[i + kk / 2 - k]).sum()
        })
        .collect();
    let thresh = 0.01 * local.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mut backlink = vec![-1i64; n];
    let mut cum = vec![0.0; n];
    let mut first = true;
    let reach = (fpb / 2.0).round_ties_even() as i64;
    for i in 0..n {
        let (mut best, mut at) = (f64::NEG_INFINITY, -1i64);
        let mut loc = i as i64 - reach;
        let stop = (i as f64 - 2.0 * fpb - 1.0) as i64;
        while loc > stop {
            if loc < 0 {
                break;
            }
            let s = cum[loc as usize] - 100.0 * (((i as i64 - loc) as f64).ln() - fpb.ln()).powi(2);
            if s > best {
                best = s;
                at = loc;
            }
            loc -= 1;
        }
        cum[i] = if at >= 0 { local[i] + best } else { local[i] };
        if first && local[i] < thresh {
            backlink[i] = -1;
        } else {
            backlink[i] = at;
            first = false;
        }
    }
    let is_max: Vec<bool> = (0..n)
        .map(|i| if i == 0 { false } else if i == n - 1 { cum[i] > cum[i - 1] } else { cum[i] > cum[i - 1] && cum[i] >= cum[i + 1] })
        .collect();
    let mut peaks: Vec<f64> = (0..n).filter(|&i| is_max[i]).map(|i| cum[i]).collect();
    peaks.sort_by(f64::total_cmp);
    let median = match peaks.len() {
        0 => 0.0,
        k if k % 2 == 1 => peaks[k / 2],
        k => (peaks[k / 2 - 1] + peaks[k / 2]) / 2.0,
    };
    let tail = (0..n).rev().find(|&i| is_max[i] && cum[i] >= 0.5 * median).unwrap_or(n - 1);
    let mut on = vec![false; n];
    let mut b = tail as i64;
    while b >= 0 {
        on[b as usize] = true;
        b = backlink[b as usize];
    }
    let picked: Vec<f64> = (0..n).filter(|&i| on[i]).map(|i| local[i]).collect();
    let w = [0.0, 0.5, 1.0, 0.5, 0.0];
    let full: Vec<f64> = (0..picked.len() + 4)
        .map(|j| (0..5).filter(|&k| j >= k && j - k < picked.len()).map(|k| w[k] * picked[j - k]).sum())
        .collect();
    let smooth = &full[2.min(full.len())..(n + 2).min(full.len())];
    let threshold = 0.5 * mean(&smooth.iter().map(|v| v * v).collect::<Vec<_>>()).sqrt();
    let mut i = 0;
    while i < n && local[i] <= threshold {
        on[i] = false;
        i += 1;
    }
    let mut i = n as i64 - 1;
    while i >= 0 && local[i as usize] <= threshold {
        on[i as usize] = false;
        i -= 1;
    }
    (0..n).filter(|&i| on[i]).collect()
}

/// A tempo written in the name, as the regex `(?<!\d)(\d{2,3})\s*bpm` (any case) finds it.
fn name_bpm(name: &str) -> Option<f64> {
    let s = name.to_lowercase();
    let b = s.as_bytes();
    let mut from = 0;
    while let Some(p) = s[from..].find("bpm").map(|p| p + from) {
        from = p + 3;
        let mut end = p;
        while end > 0 && b[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        let mut start = end;
        while start > 0 && b[start - 1].is_ascii_digit() {
            start -= 1;
        }
        let len = end - start;
        if (2..=3).contains(&len) {
            let v: u32 = s[start..end].parse().ok()?;
            return (50..=220).contains(&v).then_some(v as f64);
        }
    }
    None
}

/// `analyze` in athar/engine.py: y is the first ANALYSIS_SECONDS of the sound at 48 kHz mono.
pub fn analyze(y48: &[f32], duration: f64, name: &str) -> Result<Analysis, Error> {
    let peak = y48.iter().map(|v| v.abs() as f64).fold(0.0, f64::max);
    let peak = if peak == 0.0 { 1e-9 } else { peak };
    let rms = (y48.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / y48.len().max(1) as f64).sqrt();
    let rms = if rms == 0.0 { 1e-9 } else { rms };
    let y22: Vec<f64> = resample(y48, 48_000, 22_050)?.iter().map(|&v| v as f64).collect();
    let mags = stft_mag(&y22);
    let brightness = spectral_centroid(&mags);
    let env = onset_strength(&mags);
    let onsets = count_onsets(&env);

    let mut pitch_hz = None;
    let seg = &y22[..y22.len().min(22_050)];
    if seg.len() >= 4096 && spectral_flatness(&stft_mag(seg)) < 0.05 {
        let f0: Vec<f64> = yin(seg).into_iter().filter(|v| v.is_finite()).collect();
        if !f0.is_empty() {
            let mut sorted = f0.clone();
            sorted.sort_by(f64::total_cmp);
            let k = sorted.len();
            let p = if k % 2 == 1 { sorted[k / 2] } else { (sorted[k / 2 - 1] + sorted[k / 2]) / 2.0 };
            let midi: Vec<f64> = f0.iter().map(|&f| hz_to_midi(f)).collect();
            let mm = mean(&midi);
            let spread = (midi.iter().map(|v| (v - mm).powi(2)).sum::<f64>() / midi.len() as f64).sqrt();
            if p > 32.0 && p < 1900.0 && spread < 1.0 {
                pitch_hz = Some(p);
            }
        }
    }

    let (mut tempo_out, mut is_loop) = (None, false);
    let nb = name_bpm(name);
    if duration >= 1.5 && onsets >= 4 && env.iter().any(|&v| v != 0.0) {
        let t = tempo(&env);
        let b = beats(&env, t);
        let pulse = if b.len() >= 4 { mean(&b.iter().map(|&i| env[i]).collect::<Vec<_>>()) / (mean(&env) + 1e-9) } else { 0.0 };
        // A loop is cut to a whole number of beats, so its length implies a tempo. Accept it
        // only when that agrees with the detected beat.
        if pulse > 2.0 && t > 0.0 {
            'k: for k in [1.0, 2.0, 0.5] {
                let cands: Vec<f64> = [4.0, 8.0, 12.0, 16.0, 24.0, 32.0, 48.0, 64.0]
                    .iter()
                    .map(|n| n * 60.0 / duration)
                    .filter(|&c| (60.0..=200.0).contains(&c) && (c - t * k).abs() / c < 0.04)
                    .collect();
                if let Some(best) = cands.iter().cloned().min_by(|a, b| (a - t * k).abs().total_cmp(&(b - t * k).abs())) {
                    tempo_out = Some((best * 100.0).round() / 100.0);
                    is_loop = true;
                    break 'k;
                }
            }
        }
    }
    if let Some(b) = nb {
        tempo_out = Some(b);
        is_loop = true;
    } else if duration >= 1.0 && name.to_lowercase().contains("loop") {
        is_loop = true;
    }

    Ok(Analysis {
        peak_db: 20.0 * peak.log10(),
        rms_db: 20.0 * rms.log10(),
        crest: peak / rms,
        brightness,
        pitch_hz,
        tempo: tempo_out,
        onsets,
        is_loop,
    })
}

pub const PEAK_BINS: usize = 120;

/// The waveform overview stored for each file: PEAK_BINS bytes, the loudest point of each slice
/// of the whole file, scaled so the loudest slice is 255.
pub fn peaks(d: &Decoded, y48: &[f32]) -> Vec<u8> {
    let vals: Vec<f64> = if d.duration() <= ANALYSIS_SECONDS {
        // np.array_split: the first len % bins slices hold one extra sample.
        let n = y48.len();
        if n == 0 {
            return vec![0; PEAK_BINS];
        }
        let (base, extra) = (n / PEAK_BINS, n % PEAK_BINS);
        let mut at = 0;
        (0..PEAK_BINS)
            .map(|i| {
                let len = base + usize::from(i < extra);
                let m = y48[at..at + len].iter().map(|v| v.abs() as f64).fold(0.0, f64::max);
                at += len;
                m
            })
            .collect()
    } else {
        let total = d.frames().max(1);
        let mut v = vec![0.0f64; PEAK_BINS];
        for i in 0..d.frames() {
            let m = d.channels.iter().map(|c| c[i].abs() as f64).fold(0.0, f64::max);
            let b = (i * PEAK_BINS / total).min(PEAK_BINS - 1);
            v[b] = v[b].max(m);
        }
        v
    };
    let top = vals.iter().cloned().fold(0.0, f64::max);
    let top = if top == 0.0 { 1.0 } else { top };
    vals.iter().map(|v| (v / top * 255.0) as u8).collect()
}
