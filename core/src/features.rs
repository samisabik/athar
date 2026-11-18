//! CLAP's audio input: 10 s at 48 kHz, repeated to fill, then a 64-band Slaney log-mel
//! spectrogram. This is what the model's own feature extractor computes in its "rand_trunc"
//! mode with "repeatpad" padding; stored vectors depend on it staying exactly so.

use crate::spectrum::hann;
use realfft::RealFftPlanner;
use std::sync::OnceLock;

pub const SR: usize = 48_000;
pub const SECONDS: usize = 10;
const N_FFT: usize = 1024;
const HOP: usize = 480;
pub const N_MELS: usize = 64;
pub const FRAMES: usize = SR * SECONDS / HOP + 1;
const FMIN: f64 = 50.0;
const FMAX: f64 = 14_000.0;

/// Slaney mel scale, as librosa's `hz_to_mel(htk=False)`.
fn hz_to_mel(f: f64) -> f64 {
    let f_sp = 200.0 / 3.0;
    let logstep = 6.4f64.ln() / 27.0;
    if f >= 1000.0 { 15.0 + (f / 1000.0).ln() / logstep } else { f / f_sp }
}

fn mel_to_hz(m: f64) -> f64 {
    let f_sp = 200.0 / 3.0;
    let logstep = 6.4f64.ln() / 27.0;
    if m >= 15.0 { 1000.0 * (logstep * (m - 15.0)).exp() } else { f_sp * m }
}

/// librosa.filters.mel(htk=False, norm="slaney"), rounded to f32 as librosa stores it: n_mels
/// rows of n_fft/2+1 weights.
pub(crate) fn mel(sr: f64, n_fft: usize, n_mels: usize, fmin: f64, fmax: f64) -> Vec<Vec<f64>> {
    let bins = n_fft / 2 + 1;
    let fft_freqs: Vec<f64> = (0..bins).map(|k| k as f64 * (sr / 2.0) / (bins - 1) as f64).collect();
    let (lo, hi) = (hz_to_mel(fmin), hz_to_mel(fmax));
    let mel_f: Vec<f64> = (0..n_mels + 2).map(|i| mel_to_hz(lo + (hi - lo) * i as f64 / (n_mels + 1) as f64)).collect();
    (0..n_mels)
        .map(|i| {
            let enorm = 2.0 / (mel_f[i + 2] - mel_f[i]);
            fft_freqs
                .iter()
                .map(|&f| {
                    let lower = (f - mel_f[i]) / (mel_f[i + 1] - mel_f[i]);
                    let upper = (mel_f[i + 2] - f) / (mel_f[i + 2] - mel_f[i + 1]);
                    ((lower.min(upper).max(0.0) * enorm) as f32) as f64
                })
                .collect()
        })
        .collect()
}

/// CLAP's filterbank: 48 kHz, 1024-point FFT, 64 bands from 50 Hz to 14 kHz.
fn mel_filters() -> &'static Vec<Vec<f64>> {
    static MEL: OnceLock<Vec<Vec<f64>>> = OnceLock::new();
    MEL.get_or_init(|| mel(SR as f64, N_FFT, N_MELS, FMIN, FMAX))
}

/// The model input for one clip of 48 kHz mono audio: FRAMES rows of N_MELS values.
pub fn clap_features(y: &[f32]) -> Vec<f32> {
    let n = SR * SECONDS;
    let y: Vec<f64> = y.iter().take(n).map(|&v| v as f64).collect();
    let mut x = Vec::with_capacity(n);
    if !y.is_empty() && y.len() < n {
        for _ in 0..n / y.len() {
            x.extend_from_slice(&y);
        }
    } else {
        x = y;
    }
    x.resize(n, 0.0);
    // numpy's "reflect" padding: the edge sample is not repeated.
    let half = N_FFT / 2;
    let mut padded = Vec::with_capacity(n + N_FFT);
    padded.extend((1..=half).rev().map(|i| x[i]));
    padded.extend_from_slice(&x);
    padded.extend((1..=half).map(|i| x[n - 1 - i]));

    let window = hann(N_FFT, true);
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(N_FFT);
    let mut frame = fft.make_input_vec();
    let mut spec = fft.make_output_vec();
    let mel = mel_filters();
    let mut out = Vec::with_capacity(FRAMES * N_MELS);
    let mut power = vec![0.0f64; N_FFT / 2 + 1];
    for f in 0..FRAMES {
        let start = f * HOP;
        for k in 0..N_FFT {
            frame[k] = padded[start + k] * window[k];
        }
        fft.process(&mut frame, &mut spec).expect("FFT sizes are fixed");
        for (p, c) in power.iter_mut().zip(&spec) {
            *p = c.norm_sqr();
        }
        for row in mel {
            let e: f64 = row.iter().zip(&power).map(|(w, p)| w * p).sum();
            out.push((10.0 * e.max(1e-10).log10()) as f32);
        }
    }
    out
}
