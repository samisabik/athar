//! Sample rate conversion for audio that is kept or exported: edits, the Arranger's mix and
//! every render. (`decode::resample` is the faster FFT resampler used for analysis and
//! playback.)

use std::f64::consts::PI;

/// Zero crossings of the sinc on each side of its centre.
const ZEROS: f64 = 16.0;
const KAISER_BETA: f64 = 9.0;
/// Most filter phases kept; ratios that need more use the nearest phase.
const MAX_PHASES: u64 = 4096;

/// Modified Bessel function of the first kind, order zero.
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..40 {
        let y = x / (2 * k) as f64;
        term *= y * y;
        sum += term;
    }
    sum
}

/// Kaiser-windowed sinc at `x` zero crossings from the centre.
fn kernel(x: f64) -> f64 {
    let r = x / ZEROS;
    if r >= 1.0 {
        return 0.0;
    }
    let sinc = if x != 0.0 { (PI * x).sin() / (PI * x) } else { 1.0 };
    sinc * bessel_i0(KAISER_BETA * (1.0 - r * r).sqrt())
}

/// Kaiser-windowed sinc resampler, cutoff at 94% of the lower Nyquist. The rates only matter as
/// a ratio. A zero rate returns the input unchanged.
pub fn resample_sinc(channels: &[Vec<f32>], from: u32, to: u32) -> Vec<Vec<f32>> {
    if from == to || from == 0 || to == 0 || channels.is_empty() {
        return channels.to_vec();
    }
    let len = channels[0].len();
    let n = (len as f64 * to as f64 / from as f64).round() as usize;
    let mut out = vec![vec![0f32; n]; channels.len()];
    if len == 0 {
        return out;
    }
    let g = crate::gcd(from as u64, to as u64);
    let (up, down) = (to as u64 / g, from as u64 / g);
    let cut = (to as f64 / from as f64).min(1.0) * 0.94;
    let half = (ZEROS / cut).ceil() as i64;
    let taps = (2 * half) as usize;
    let phases = up.min(MAX_PHASES);
    // One row of `taps` weights per phase, each row normalized to unity gain.
    let mut weights = vec![0f32; phases as usize * taps];
    for (p, row) in weights.chunks_exact_mut(taps).enumerate() {
        let mut sum = 0.0;
        for (q, w) in row.iter_mut().enumerate() {
            let v = kernel(((q as i64 - half + 1) as f64 - p as f64 / phases as f64).abs() * cut);
            *w = v as f32;
            sum += v;
        }
        for w in row.iter_mut() {
            *w = (*w as f64 / sum) as f32;
        }
    }
    for (d, o) in channels.iter().zip(out.iter_mut()) {
        for (i, oi) in o.iter_mut().enumerate() {
            // Output sample i sits at input position i * down / up: sample j plus phase p / up.
            let pos = i as u64 * down;
            let mut j = pos / up;
            let mut p = pos - j * up;
            if phases != up {
                p = (p as f64 / up as f64 * phases as f64).round() as u64;
                if p == phases {
                    p = 0;
                    j += 1;
                }
            }
            let base = j as i64 - half + 1;
            let row = &weights[p as usize * taps..(p as usize + 1) * taps];
            let mut s = 0.0;
            if base >= 0 && base as usize + taps <= len {
                let b = base as usize;
                for q in 0..taps {
                    s += row[q] as f64 * d[b + q] as f64;
                }
            } else {
                for q in 0..taps {
                    let k = base + q as i64;
                    if k >= 0 && (k as usize) < len {
                        s += row[q] as f64 * d[k as usize] as f64;
                    }
                }
            }
            *oi = s as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::testing::sine;

    #[test]
    fn length_follows_the_ratio() {
        let chs = vec![vec![0f32; 4410]; 2];
        let out = resample_sinc(&chs, 44100, 48000);
        assert_eq!((out.len(), out[0].len(), out[1].len()), (2, 4800, 4800));
        assert_eq!(resample_sinc(&chs, 44100, 22050)[0].len(), 2205);
        assert_eq!(resample_sinc(&[vec![]], 44100, 48000), vec![Vec::<f32>::new()]);
        assert!(resample_sinc(&[], 44100, 48000).is_empty());
    }

    #[test]
    fn a_constant_keeps_its_level() {
        for (from, to) in [(44100, 48000), (48000, 44100), (48000, 16000), (1, 2), (100000, 94387)] {
            let out = resample_sinc(&[vec![0.5f32; 4000]], from, to);
            for &v in &out[0][400..out[0].len() - 400] {
                assert!((v - 0.5).abs() < 1e-4, "{from} to {to}: {v}");
            }
        }
    }

    #[test]
    fn a_tone_keeps_its_level_and_pitch() {
        let out = &resample_sinc(&[sine(1000.0, 44100, 0.5, 0.8)], 44100, 48000)[0];
        let want = sine(1000.0, 48000, 0.5, 0.8);
        for i in 1000..out.len() - 1000 {
            assert!((out[i] - want[i]).abs() < 1e-3, "sample {i}: {} against {}", out[i], want[i]);
        }
    }

    #[test]
    fn equal_or_zero_rates_change_nothing() {
        let chs = vec![vec![0.1f32, -0.2, 0.3]];
        assert_eq!(resample_sinc(&chs, 48000, 48000), chs);
        assert_eq!(resample_sinc(&chs, 0, 48000), chs);
        assert_eq!(resample_sinc(&chs, 48000, 0), chs);
    }
}
