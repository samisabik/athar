//! The editor's audio processing: effects, repairs, measurements, the spectrogram, time
//! stretch, and the render to a WAV file. Channels are slices of equal-length f32 vectors and
//! the arithmetic runs in f64.
//!
//! Nothing here feeds the library's stored values; `analysis` does that with its own detectors
//! (see the crate documentation).

#![allow(clippy::needless_range_loop)]

mod denoise;
mod detect;
mod dynamics;
mod loudness;
mod repair;
mod resample;
mod spectrogram;
mod stretch;
mod wav;

pub use denoise::{NR_N, NrParams, denoise, estimate_profile, noise_profile};
pub use detect::{detect_pitch, onsets};
pub use dynamics::{
    CompParams, LofiParams, MaxParams, SatParams, ShapeParams, auto_makeup, comp_curve, compress, lofi, maximize, sat_curve, saturate, shape,
};
pub use loudness::{Measure, measure};
pub use repair::{PrepParams, declick, fade, loop_crossfade, prep, sound_bounds};
pub use resample::resample_sinc;
pub use spectrogram::{spectrogram, spectrogram_peak};
pub use stretch::stretch_pitch;
pub use wav::{RenderArgs, encode_wav, render_wav};

use serde_json::Value;

/// What an effect returns: the processed channels and a one-line summary of what it did.
#[derive(Debug, Clone)]
pub struct Processed {
    pub channels: Vec<Vec<f32>>,
    pub info: String,
}

/// A number from the JSON object the page sends as an effect's settings.
fn num(p: &Value, key: &str) -> Option<f64> {
    p.get(key).and_then(Value::as_f64)
}

/// Rounds halves towards positive infinity. Used where a sample is quantized, so that files
/// keep the values earlier versions wrote; lengths and positions use `f64::round`.
fn round_half_up(x: f64) -> f64 {
    let floor = x.floor();
    if x - floor >= 0.5 { floor + 1.0 } else { floor }
}

fn frames(chs: &[Vec<f32>]) -> usize {
    chs.first().map_or(0, Vec::len)
}

/// Average of all channels.
pub fn mix_mono<S: AsRef<[f32]>>(chs: &[S]) -> Vec<f32> {
    let Some(first) = chs.first() else { return Vec::new() };
    if chs.len() == 1 {
        return first.as_ref().to_vec();
    }
    let n = first.as_ref().len();
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

/// Linear level to dB, floored at -240 dB.
pub fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

/// dB to linear level.
pub fn lin(d: f64) -> f64 {
    10f64.powf(d / 20.0)
}

/// The highest absolute sample over all channels.
fn peak(chs: &[Vec<f32>]) -> f64 {
    chs.iter().flatten().fold(0f64, |m, &x| m.max((x as f64).abs()))
}

/// RMS level over all channels.
fn rms(chs: &[Vec<f32>]) -> f64 {
    let sum: f64 = chs.iter().flatten().map(|&x| x as f64 * x as f64).sum();
    (sum / (chs.len() * frames(chs)).max(1) as f64).sqrt()
}

#[cfg(test)]
mod testing {
    use std::f64::consts::PI;

    pub fn sine(hz: f64, sr: u32, seconds: f64, level: f64) -> Vec<f32> {
        (0..(sr as f64 * seconds) as usize).map(|i| (level * (2.0 * PI * hz * i as f64 / sr as f64).sin()) as f32).collect()
    }

    /// Repeatable noise between -1 and 1.
    pub fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut r = seed.max(1);
        (0..n)
            .map(|_| {
                r ^= r << 13;
                r ^= r >> 17;
                r ^= r << 5;
                (r as f64 / 2147483648.0 - 1.0) as f32
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_mix_averages_channels() {
        assert_eq!(mix_mono(&[vec![1.0f32, 0.0], vec![0.0, -1.0]]), vec![0.5, -0.5]);
        assert_eq!(mix_mono(&[vec![0.25f32]]), vec![0.25]);
        assert!(mix_mono::<Vec<f32>>(&[]).is_empty());
    }

    #[test]
    fn decibels_round_trip() {
        assert!((db(lin(-6.0)) + 6.0).abs() < 1e-12);
        assert_eq!(db(0.0), -240.0);
    }

    #[test]
    fn halves_round_up() {
        assert_eq!(round_half_up(2.5), 3.0);
        assert_eq!(round_half_up(-2.5), -2.0);
        assert_eq!(round_half_up(-2.6), -3.0);
        assert_eq!(round_half_up(7.0), 7.0);
    }

    #[test]
    fn levels_cover_all_channels() {
        let chs = vec![vec![0.5f32, -0.5], vec![0.0, 1.0]];
        assert_eq!(peak(&chs), 1.0);
        assert!((rms(&chs) - (1.5f64 / 4.0).sqrt()).abs() < 1e-12);
        assert_eq!(rms(&[]), 0.0);
    }
}
