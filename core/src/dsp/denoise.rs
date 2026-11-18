//! Spectral noise reduction: learn or estimate the noise's average spectrum (the "noise
//! print"), then turn down every frequency bin that does not rise above it.

use super::{Processed, lin, mix_mono, num};
use crate::spectrum::{Stft, hann};
use serde_json::Value;

/// Frame length of the noise reduction. A noise print has `NR_N / 2 + 1` bins.
pub const NR_N: usize = 2048;
const HOP: usize = 512;
const BINS: usize = NR_N / 2 + 1;

fn stft() -> Stft {
    Stft::new(&hann(NR_N, true))
}

/// Magnitudes of the frame of `d` that starts at `start`.
fn frame_mags(stft: &mut Stft, d: &[f32], start: usize) -> Vec<f32> {
    stft.analyse(d, start as i64).iter().map(|c| (c.re as f64).hypot(c.im as f64) as f32).collect()
}

/// The noise print of samples a..b of the mono mix: their average magnitude spectrum.
pub fn noise_profile(chs: &[Vec<f32>], a: usize, b: usize) -> Vec<f32> {
    let parts: Vec<&[f32]> = chs
        .iter()
        .map(|c| {
            let e = b.min(c.len());
            &c[a.min(e)..e]
        })
        .collect();
    let d = mix_mono(&parts);
    let mut stft = stft();
    let mut sum = vec![0f64; BINS];
    let mut count = 0usize;
    // Whole frames only, but at least one, zero-filled when the span is short.
    let mut start = 0usize;
    while start == 0 || start + NR_N <= d.len() {
        for (s, m) in sum.iter_mut().zip(frame_mags(&mut stft, &d, start)) {
            *s += m as f64;
        }
        count += 1;
        start += HOP;
    }
    sum.iter().map(|&s| (s / count as f64) as f32).collect()
}

/// A noise print without a selection to learn from: in each bin, the level that 10% of up to
/// 400 frames spread over the sound stay under.
pub fn estimate_profile(chs: &[Vec<f32>]) -> Vec<f32> {
    let d = mix_mono(chs);
    let mut stft = stft();
    let total = d.len().saturating_sub(NR_N) / HOP + 1;
    let count = total.min(400);
    let frames: Vec<Vec<f32>> = (0..count).map(|f| frame_mags(&mut stft, &d, (f * total / count) * HOP)).collect();
    let mut column = vec![0f32; count];
    (0..BINS)
        .map(|k| {
            for (c, frame) in column.iter_mut().zip(&frames) {
                *c = frame[k];
            }
            column.sort_by(f32::total_cmp);
            column[count / 10]
        })
        .collect()
}

/// Noise reduction settings: the most a bin is turned down in dB, and how far above the noise
/// print a bin must rise to pass (a factor on the print).
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

/// Spectral noise reduction against a noise print from `noise_profile` or `estimate_profile`.
/// Each bin gets a Wiener-style gain from its level over the print, which falls no faster than
/// an 80 ms release and is smoothed across neighbouring bins.
pub fn denoise(chs: &[Vec<f32>], sr: u32, profile: &[f32], p: &NrParams) -> Result<Processed, String> {
    if profile.len() != BINS {
        return Err(format!("the noise print has {} bins, not {BINS}", profile.len()));
    }
    let floor = lin(-p.reduction);
    let release = (-(HOP as f64) / (sr as f64 * 0.08)).exp();
    let noise_power: Vec<f64> = profile
        .iter()
        .map(|&v| {
            let v = v as f64 * p.sensitivity + 1e-12;
            v * v
        })
        .collect();
    let mut stft = stft();
    let (mut gain, mut smooth) = (vec![0f64; BINS], vec![0f64; BINS]);
    let channels = chs
        .iter()
        .map(|d| {
            let n = d.len();
            // Overlap-add buffer with a frame of room before and after the sound.
            let mut buf = vec![0f32; n + 2 * NR_N];
            let mut prev = vec![1f64; BINS];
            let mut start = HOP as i64 - NR_N as i64;
            while start < n as i64 {
                let spectrum = stft.analyse(d, start);
                for k in 0..BINS {
                    let snr = (spectrum[k].re as f64 * spectrum[k].re as f64 + spectrum[k].im as f64 * spectrum[k].im as f64) / noise_power[k];
                    let wiener = snr / (snr + 1.0);
                    let mut g = floor.max(wiener * wiener);
                    if g < prev[k] {
                        g = g.max(prev[k] * release);
                    }
                    prev[k] = g;
                    gain[k] = g;
                }
                for k in 0..BINS {
                    smooth[k] = (gain[k.saturating_sub(1)] + gain[k] * 2.0 + gain[(k + 1).min(BINS - 1)]) / 4.0;
                }
                for (c, &g) in spectrum.iter_mut().zip(&smooth) {
                    *c *= g as f32;
                }
                let at = (start + NR_N as i64) as usize;
                for (b, &v) in buf[at..at + NR_N].iter_mut().zip(stft.synthesize()) {
                    *b += v;
                }
                start += HOP as i64;
            }
            // Four squared Hann windows overlap to 1.5.
            buf[NR_N..NR_N + n].iter().map(|&v| (v as f64 / 1.5) as f32).collect()
        })
        .collect();
    Ok(Processed { channels, info: format!("reduced noise by up to {} dB", p.reduction) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::testing::{noise, sine};
    use crate::dsp::{db, rms};

    fn nr(reduction: f64) -> NrParams {
        NrParams { reduction, sensitivity: 1.5 }
    }

    #[test]
    fn a_print_has_one_level_per_bin() {
        let chs = vec![sine(1000.0, 48000, 0.5, 0.5)];
        let print = noise_profile(&chs, 0, 24000);
        assert_eq!(print.len(), NR_N / 2 + 1);
        // 1 kHz at 48 kHz is bin 42.7 of 2048.
        let loudest = (0..print.len()).max_by(|&a, &b| print[a].total_cmp(&print[b])).unwrap();
        assert!((42..=43).contains(&loudest), "{loudest}");
        // A Hann-windowed sine of level 0.5 peaks near 0.5 * 2048 / 4.
        assert!(print[loudest] > 180.0 && print[loudest] < 260.0, "{}", print[loudest]);
    }

    #[test]
    fn prints_of_short_or_missing_sound() {
        assert_eq!(noise_profile(&[vec![0.1f32; 100]], 10, 50).len(), NR_N / 2 + 1);
        assert_eq!(noise_profile(&[vec![0.1f32; 100]], 500, 900), vec![0f32; NR_N / 2 + 1]);
        assert_eq!(noise_profile(&[], 0, 100), vec![0f32; NR_N / 2 + 1]);
        assert_eq!(estimate_profile(&[vec![0f32; 10]]), vec![0f32; NR_N / 2 + 1]);
        assert_eq!(estimate_profile(&[]), vec![0f32; NR_N / 2 + 1]);
    }

    #[test]
    fn the_estimate_follows_the_quiet_frames() {
        let hiss: Vec<f32> = noise(96000, 3).iter().map(|v| v * 0.01).collect();
        let mut d = hiss.clone();
        for (x, t) in d[48000..].iter_mut().zip(sine(440.0, 48000, 1.0, 0.5)) {
            *x += t;
        }
        let estimate = estimate_profile(&[d]);
        let learned = noise_profile(&[hiss], 0, 48000);
        // Away from the tone, the estimate (a low percentile) sits a little under the mean.
        let ratio = |k: usize| estimate[k] as f64 / learned[k] as f64;
        let mean: f64 = (200..1000).map(ratio).sum::<f64>() / 800.0;
        assert!(mean > 0.3 && mean < 1.0, "{mean}");
    }

    #[test]
    fn noise_goes_down_and_the_tone_stays() {
        let hiss: Vec<f32> = noise(96000, 7).iter().map(|v| v * 0.01).collect();
        let print = noise_profile(std::slice::from_ref(&hiss), 0, 48000);
        let cleaned = denoise(std::slice::from_ref(&hiss), 48000, &print, &nr(24.0)).unwrap();
        assert_eq!(cleaned.info, "reduced noise by up to 24 dB");
        assert_eq!(cleaned.channels[0].len(), hiss.len());
        let drop = db(rms(&cleaned.channels)) - db(rms(std::slice::from_ref(&hiss)));
        assert!(drop < -6.0, "{drop}");

        let tone = sine(1000.0, 48000, 2.0, 0.5);
        let mixed: Vec<f32> = tone.iter().zip(&hiss).map(|(a, b)| a + b).collect();
        let out = &denoise(&[mixed], 48000, &print, &nr(24.0)).unwrap().channels[0];
        let middle = 20000..70000;
        let error: Vec<f32> = middle.clone().map(|i| out[i] - tone[i]).collect();
        // What is left besides the tone is quieter than the hiss was.
        assert!(db(rms(&[error])) < db(rms(&[hiss])) - 6.0);
    }

    #[test]
    fn nothing_to_reduce_passes_through() {
        // A zero print passes every bin, which shows the overlap-add rebuilds the sound.
        let d = sine(700.0, 48000, 0.3, 0.4);
        let out = &denoise(std::slice::from_ref(&d), 48000, &vec![0f32; NR_N / 2 + 1], &nr(12.0)).unwrap().channels[0];
        for i in 0..d.len() {
            assert!((out[i] - d[i]).abs() < 1e-4, "sample {i}: {} against {}", out[i], d[i]);
        }
    }

    #[test]
    fn a_print_of_the_wrong_size_is_refused() {
        assert!(denoise(&[vec![0f32; 100]], 48000, &[0.0; 10], &nr(12.0)).is_err());
        assert!(denoise(&[], 48000, &vec![0f32; NR_N / 2 + 1], &nr(12.0)).unwrap().channels.is_empty());
    }
}
