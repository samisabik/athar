//! The descriptors the library stores for every file and filters and sorts by: level,
//! brightness, pitch, tempo, onset count, whether it is a loop, and the waveform overview.
//!
//! Libraries indexed by earlier versions (first by a Python engine built on librosa) hold the
//! same values, so the functions here reproduce librosa's algorithms with librosa's defaults,
//! down to where it rounds to f32. Do not "improve" a constant or a rounding step: stored
//! values would stop matching new ones. tests/reference.rs checks them against frozen outputs.
//!
//! The editor's detectors in `dsp` are separate and free to differ.

use crate::decode::{resample, Decoded, ANALYSIS_SECONDS};
use crate::embed::Error;
use crate::spectrum::hann;
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

/// Magnitude spectra of a centred, zero-padded STFT (n_fft 2048, hop 512, Hann window).
fn stft_mag(y: &[f64]) -> Vec<Vec<f64>> {
    let half = N_FFT / 2;
    let mut padded = vec![0.0; half];
    padded.extend_from_slice(y);
    padded.resize(padded.len() + half, 0.0);
    let frames = 1 + (padded.len() - N_FFT) / HOP;
    let win = hann(N_FFT, true);
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

/// The median of values sorted in rising order.
fn median(sorted: &[f64]) -> Option<f64> {
    let k = sorted.len();
    match k {
        0 => None,
        _ if k % 2 == 1 => Some(sorted[k / 2]),
        _ => Some((sorted[k / 2 - 1] + sorted[k / 2]) / 2.0),
    }
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
    let w = hann(win, true);
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
    // The best lag among those slower than 320 BPM.
    let first = (0..win).find(|&l| bpm(l) < 320.0).unwrap_or(0);
    let best = (first..win)
        .map(|l| (l, (1e6 * tg[l]).ln_1p() - 0.5 * (bpm(l).log2() - 120f64.log2()).powi(2)))
        .fold((0, f64::NEG_INFINITY), |b, c| if c.1 > b.1 { c } else { b })
        .0;
    bpm(best.max(1))
}

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
    let typical = median(&peaks).unwrap_or(0.0);
    let tail = (0..n).rev().find(|&i| is_max[i] && cum[i] >= 0.5 * typical).unwrap_or(n - 1);
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

/// A tempo written in the name: the first run of two or three digits followed by "bpm" in any
/// case, with or without a space (the regex `(?<!\d)(\d{2,3})\s*bpm`). That first number
/// decides: when it is outside 50 to 220 the name gives no tempo, whatever follows.
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

/// The pitch of a tonal sound in Hz, from its first second at 22.05 kHz. None for noisy sounds
/// (spectral flatness of 0.05 or more), for less than 4096 samples, and when the pitch moves by
/// a semitone or more or leaves 32 to 1900 Hz.
fn pitch_of(y22: &[f64]) -> Option<f64> {
    let seg = &y22[..y22.len().min(22_050)];
    let tonal = seg.len() >= 4096 && spectral_flatness(&stft_mag(seg)) < 0.05;
    if !tonal {
        return None;
    }
    let f0: Vec<f64> = yin(seg).into_iter().filter(|v| v.is_finite()).collect();
    let mut sorted = f0.clone();
    sorted.sort_by(f64::total_cmp);
    let pitch = median(&sorted)?;
    let midi: Vec<f64> = f0.iter().map(|&f| hz_to_midi(f)).collect();
    let centre = mean(&midi);
    let spread = (midi.iter().map(|v| (v - centre).powi(2)).sum::<f64>() / midi.len() as f64).sqrt();
    (pitch > 32.0 && pitch < 1900.0 && spread < 1.0).then_some(pitch)
}

/// The tempo of a loop, from its onset envelope and length. A loop is cut to a whole number of
/// beats, so its length implies a tempo; it is accepted only when the sound has a clear pulse
/// and that tempo is within 4% of the detected one, or of twice or half of it.
fn loop_tempo(env: &[f64], duration: f64) -> Option<f64> {
    let detected = tempo(env);
    let b = beats(env, detected);
    // How much stronger the onsets on the beats are than the average.
    let pulse = if b.len() >= 4 { mean(&b.iter().map(|&i| env[i]).collect::<Vec<_>>()) / (mean(env) + 1e-9) } else { 0.0 };
    if !(pulse > 2.0 && detected > 0.0) {
        return None;
    }
    let implied = [1.0, 2.0, 0.5].iter().find_map(|k| {
        let want = detected * k;
        [4.0, 8.0, 12.0, 16.0, 24.0, 32.0, 48.0, 64.0]
            .iter()
            .map(|count| count * 60.0 / duration)
            .filter(|&c| (60.0..=200.0).contains(&c) && (c - want).abs() / c < 0.04)
            .min_by(|a, b| (a - want).abs().total_cmp(&(b - want).abs()))
    })?;
    Some((implied * 100.0).round() / 100.0)
}

/// The descriptors of one file. `y48` is its first ANALYSIS_SECONDS at 48 kHz mono, `duration`
/// its whole length in seconds; `name` can state a tempo ("... 120bpm") or say "loop".
pub fn analyze(y48: &[f32], duration: f64, name: &str) -> Result<Analysis, Error> {
    let peak = y48.iter().map(|v| v.abs() as f64).fold(0.0, f64::max);
    let peak = if peak == 0.0 { 1e-9 } else { peak };
    let rms = (y48.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / y48.len().max(1) as f64).sqrt();
    let rms = if rms == 0.0 { 1e-9 } else { rms };
    let y22: Vec<f64> = resample(y48, 48_000, 22_050)?.iter().map(|&v| v as f64).collect();
    let mags = stft_mag(&y22);
    let env = onset_strength(&mags);
    let onsets = count_onsets(&env);

    let rhythmic = duration >= 1.5 && onsets >= 4 && env.iter().any(|&v| v != 0.0);
    let from_length = if rhythmic { loop_tempo(&env, duration) } else { None };
    // A tempo in the name wins over the detected one, and the word "loop" is taken at its word.
    let (tempo, is_loop) = match name_bpm(name) {
        Some(named) => (Some(named), true),
        None => (from_length, from_length.is_some() || (duration >= 1.0 && name.to_lowercase().contains("loop"))),
    };

    Ok(Analysis {
        peak_db: 20.0 * peak.log10(),
        rms_db: 20.0 * rms.log10(),
        crest: peak / rms,
        brightness: spectral_centroid(&mags),
        pitch_hz: pitch_of(&y22),
        tempo,
        onsets,
        is_loop,
    })
}

const PEAK_BINS: usize = 120;

/// The waveform overview stored for each file: PEAK_BINS bytes, the loudest point of each slice
/// of the whole file, scaled so the loudest slice is 255.
pub fn peaks(d: &Decoded, y48: &[f32]) -> Vec<u8> {
    let vals: Vec<f64> = if d.duration() <= ANALYSIS_SECONDS {
        // Slices as numpy's array_split cuts them: the first len % bins hold one extra sample.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_sorted_values() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0]), Some(3.0));
        assert_eq!(median(&[1.0, 2.0, 10.0]), Some(2.0));
        assert_eq!(median(&[1.0, 2.0, 4.0, 10.0]), Some(3.0));
    }

    #[test]
    fn tempo_in_a_name() {
        assert_eq!(name_bpm("Break 120bpm.wav"), Some(120.0));
        assert_eq!(name_bpm("break_95 BPM_dry.wav"), Some(95.0));
        assert_eq!(name_bpm("kick.wav"), None);
        assert_eq!(name_bpm("bpm.wav"), None);
        // Four digits are not a tempo, and the search goes on after them.
        assert_eq!(name_bpm("1200bpm then 90bpm"), Some(90.0));
        // The first two or three digit number decides, even when it is out of range.
        assert_eq!(name_bpm("300bpm or 100bpm"), None);
        assert_eq!(name_bpm("7bpm 140bpm"), Some(140.0));
    }

    #[test]
    fn peaks_of_a_short_file() {
        let mut y = vec![0f32; 1210];
        y[5] = -0.5;
        y[1205] = 0.25;
        let d = Decoded { channels: vec![y.clone()], sr: 48_000 };
        let p = peaks(&d, &y);
        assert_eq!(p.len(), PEAK_BINS);
        assert_eq!((p[0], p[1], p[119]), (255, 0, 127));
        assert_eq!(peaks(&Decoded { channels: vec![vec![]], sr: 48_000 }, &[]), vec![0; PEAK_BINS]);
    }

    #[test]
    fn silence_and_a_tone() {
        let silent = analyze(&vec![0f32; 48_000], 1.0, "silence.wav").unwrap();
        assert_eq!((silent.onsets, silent.pitch_hz, silent.tempo, silent.is_loop), (0, None, None, false));
        assert!(silent.peak_db.is_finite() && silent.rms_db.is_finite() && silent.crest.is_finite() && silent.brightness.is_finite());

        let tone: Vec<f32> = (0..96_000).map(|i| (0.5 * (2.0 * std::f64::consts::PI * 220.0 * i as f64 / 48_000.0).sin()) as f32).collect();
        let a = analyze(&tone, 2.0, "tone loop 100bpm.wav").unwrap();
        assert!((a.peak_db + 6.02).abs() < 0.01 && (a.crest - 2f64.sqrt()).abs() < 0.01);
        assert!((a.pitch_hz.unwrap() - 220.0).abs() < 1.0, "{:?}", a.pitch_hz);
        assert_eq!((a.tempo, a.is_loop), (Some(100.0), true));
        assert!(analyze(&tone, 2.0, "tone loop.wav").unwrap().is_loop);
        assert!(!analyze(&tone, 2.0, "tone.wav").unwrap().is_loop);
    }
}
