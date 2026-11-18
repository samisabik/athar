//! Level measurements: integrated loudness, sample peak and true peak.

use super::{db, frames, peak};
use std::f64::consts::PI;
use std::sync::OnceLock;

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

/// The K-weighting of BS.1770: a high shelf, then a high-pass. The constants are the standard's
/// 48 kHz filters expressed as frequency, gain and Q so they hold at any rate.
fn k_weight(d: &[f32], sr: f64) -> Vec<f32> {
    let k = (PI * 1681.974450955533 / sr).tan();
    let q = 0.7071752369554196;
    let vh = 10f64.powf(3.999843853973347 / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelved = biquad(
        d,
        (vh + vb * k / q + k * k) / a0,
        2.0 * (k * k - vh) / a0,
        (vh - vb * k / q + k * k) / a0,
        2.0 * (k * k - 1.0) / a0,
        (1.0 - k / q + k * k) / a0,
    );
    let k = (PI * 38.13547087602444 / sr).tan();
    let q = 0.5003270373238773;
    let a0 = 1.0 + k / q + k * k;
    biquad(&shelved, 1.0, -2.0, 1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0)
}

/// Integrated loudness in LUFS (ITU-R BS.1770-4): 400 ms blocks every 100 ms, gated at -70 LUFS
/// and at 10 LU under the mean of what is left. A sound shorter than one block is measured
/// whole, without gating. Silence gives negative infinity.
pub(crate) fn loudness(chs: &[Vec<f32>], sr: u32) -> f64 {
    let sr = sr as f64;
    let n = frames(chs);
    if n == 0 {
        return f64::NEG_INFINITY;
    }
    let full = (sr * 0.4).round() as usize;
    let block = n.min(full);
    let step = ((sr * 0.1).round() as usize).max(1);
    let weighted: Vec<Vec<f32>> = chs.iter().map(|d| k_weight(d, sr)).collect();
    // Mean square of each block, summed over the channels.
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

/// Taps on each side of the windowed sinc that finds the level between samples.
const TP_HALF: usize = 6;

/// Interpolation filters for the points a quarter, a half and three quarters of the way to the
/// next sample.
fn true_peak_taps() -> &'static [[f32; 2 * TP_HALF]; 3] {
    static TAPS: OnceLock<[[f32; 2 * TP_HALF]; 3]> = OnceLock::new();
    TAPS.get_or_init(|| {
        let mut taps = [[0f32; 2 * TP_HALF]; 3];
        for (phase, row) in taps.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                let x = j as f64 - TP_HALF as f64 + 1.0 - (phase + 1) as f64 / 4.0;
                *v = ((PI * x).sin() / (PI * x) * (0.5 + 0.5 * (PI * x / TP_HALF as f64).cos())) as f32;
            }
        }
        taps
    })
}

/// For every sample, the highest level the 4x-oversampled waveform of any channel reaches at or
/// just after it.
pub(crate) fn true_peak_envelope(chs: &[Vec<f32>]) -> Vec<f32> {
    let taps = true_peak_taps();
    let n = frames(chs);
    let mut env = vec![0f32; n];
    for d in chs {
        for i in 0..n {
            let mut m = (d[i] as f64).abs();
            if i + 1 >= TP_HALF && i + TP_HALF < n {
                let base = i + 1 - TP_HALF;
                for row in taps {
                    let mut v = 0f64;
                    for j in 0..2 * TP_HALF {
                        v += d[base + j] as f64 * row[j] as f64;
                    }
                    m = m.max(v.abs());
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
    /// Integrated loudness in LUFS; negative infinity for silence.
    pub lufs: f64,
    /// Sample peak in dB full scale.
    pub peak: f64,
    /// Peak of the 4x-oversampled waveform in dB full scale.
    pub true_peak: f64,
}

/// Integrated loudness, sample peak and true peak.
pub fn measure(chs: &[Vec<f32>], sr: u32) -> Measure {
    let true_peak = true_peak_envelope(chs).iter().fold(0f64, |m, &v| m.max(v as f64));
    Measure { lufs: loudness(chs, sr), peak: db(peak(chs)), true_peak: db(true_peak) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::testing::sine;

    #[test]
    fn full_scale_sine_loudness() {
        // BS.1770: a 997 Hz sine at 0 dBFS in one channel reads -3.01 LUFS, at any rate.
        for sr in [44100, 48000, 96000] {
            let l = loudness(&[sine(997.0, sr, 3.0, 1.0)], sr);
            assert!((l + 3.01).abs() < 0.02, "{sr}: {l}");
        }
        // The same tone in two channels is 3 dB louder, at -20 dB it is 20 LU quieter.
        let tone = sine(997.0, 48000, 3.0, 1.0);
        assert!((loudness(&[tone.clone(), tone], 48000) + 0.0).abs() < 0.02);
        assert!((loudness(&[sine(997.0, 48000, 3.0, 0.1)], 48000) + 23.01).abs() < 0.02);
    }

    #[test]
    fn silence_and_edges() {
        assert_eq!(loudness(&[vec![0f32; 48000]], 48000), f64::NEG_INFINITY);
        assert_eq!(loudness(&[vec![]], 48000), f64::NEG_INFINITY);
        assert_eq!(loudness(&[], 48000), f64::NEG_INFINITY);
        // Shorter than one block: measured whole.
        let short = loudness(&[sine(997.0, 48000, 0.1, 1.0)], 48000);
        assert!((short + 3.01).abs() < 0.2, "{short}");
        let one = measure(&[vec![0.5f32]], 48000);
        assert!((one.peak - db(0.5)).abs() < 1e-9 && one.true_peak == one.peak);
        let none = measure(&[], 48000);
        assert_eq!((none.lufs, none.peak, none.true_peak), (f64::NEG_INFINITY, -240.0, -240.0));
    }

    #[test]
    fn quiet_passages_are_gated_out() {
        let mut d = sine(997.0, 48000, 2.0, 0.5);
        let loud = loudness(&[d.clone()], 48000);
        d.extend(vec![0f32; 96000]);
        // Half of the sound is silent, which ungated would read 3 LU lower. Only the blocks
        // that straddle the end of the tone pull the reading down a little.
        assert!((loudness(&[d], 48000) - loud).abs() < 0.5);
    }

    #[test]
    fn true_peak_sees_between_samples() {
        // A quarter-rate sine sampled 45 degrees off its crests: samples reach 0.707, the wave 1.
        let d: Vec<f32> = (0..4800).map(|i| (PI * (i as f64 / 2.0 + 0.25)).sin() as f32).collect();
        let m = measure(&[d], 48000);
        assert!((m.peak + 3.01).abs() < 0.01, "{}", m.peak);
        assert!(m.true_peak.abs() < 0.1, "{}", m.true_peak);
    }
}
