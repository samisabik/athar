//! Time stretch and pitch shift.

use super::{frames, mix_mono, resample_sinc};
use crate::spectrum::hann;

/// The waveform search runs on the mono mix averaged over this many samples.
const DECIMATION: usize = 4;

/// WSOLA time stretch without pitch change; `factor` is new length over old length. 40 ms
/// grains are overlap-added at half their length, each taken from where the source lines up
/// best with the end of the previous grain, within 12 ms of where the stretch puts it.
fn stretch(chs: &[Vec<f32>], sr: u32, factor: f64) -> Vec<Vec<f32>> {
    let len = frames(chs);
    let sr = sr as f64;
    let grain = (sr * 0.04 / 2.0).round() as usize * 2;
    if len == 0 || grain == 0 || (factor - 1.0).abs() < 1e-4 {
        return chs.to_vec();
    }
    let hop = grain / 2;
    let reach = (sr * 0.012 / DECIMATION as f64).round() as i64;
    let span = grain / DECIMATION;
    let out_len = ((len as f64 * factor).round() as usize).max(1);
    let window = hann(grain, true);
    let mono = mix_mono(chs);
    let mut search = vec![0f32; len.div_ceil(DECIMATION) + span + reach as usize + 2];
    for (s, block) in search.iter_mut().zip(mono.chunks_exact(DECIMATION)) {
        *s = (block.iter().map(|&v| v as f64).sum::<f64>() / DECIMATION as f64) as f32;
    }
    let mut out = vec![vec![0f32; out_len + grain]; chs.len()];
    let mut weight = vec![0f32; out_len + grain];
    let mut prev = 0usize;
    let mut m = 0usize;
    while m * hop < out_len {
        let at = m * hop;
        let mut pos = (at as f64 / factor).round() as i64;
        if m > 0 {
            // What would follow the previous grain in the source, and the place near `pos` that
            // looks most like it.
            let natural = (search.len() - span - 1).min(((prev + hop) as f64 / DECIMATION as f64).round() as usize);
            let ideal = (pos as f64 / DECIMATION as f64).round() as i64;
            let (mut best, mut best_score) = (0i64, f64::NEG_INFINITY);
            for lag in -reach..=reach {
                let c = ideal + lag;
                if c < 0 || c as usize + span >= search.len() {
                    continue;
                }
                let c = c as usize;
                let (mut dot, mut energy) = (0f64, 1e-9f64);
                for i in 0..span {
                    let v = search[c + i] as f64;
                    dot += v * search[natural + i] as f64;
                    energy += v * v;
                }
                let score = dot / energy.sqrt();
                if score > best_score {
                    best_score = score;
                    best = lag;
                }
            }
            pos += best * DECIMATION as i64;
        }
        let pos = pos.min(len as i64 - grain as i64).max(0) as usize;
        let take = grain.min(len - pos);
        // The first grain starts at full level instead of fading in.
        let w = |i: usize| if m == 0 && i < hop { 1.0 } else { window[i] };
        for (d, o) in chs.iter().zip(out.iter_mut()) {
            for i in 0..take {
                o[at + i] = (o[at + i] as f64 + d[pos + i] as f64 * w(i)) as f32;
            }
        }
        for i in 0..take {
            weight[at + i] = (weight[at + i] as f64 + w(i)) as f32;
        }
        prev = pos;
        m += 1;
    }
    out.into_iter()
        .map(|mut o| {
            o.truncate(out_len);
            for (v, &w) in o.iter_mut().zip(&weight) {
                if w as f64 > 1e-6 {
                    *v = (*v as f64 / w as f64) as f32;
                }
            }
            o
        })
        .collect()
}

/// Stretch to `length` (new over old) and shift pitch by `semitones`, independently: a stretch
/// by both factors, then a resample that undoes the pitch factor's share of the length.
pub fn stretch_pitch(chs: &[Vec<f32>], sr: u32, length: f64, semitones: f64) -> Vec<Vec<f32>> {
    let ratio = 2f64.powf(semitones / 12.0);
    let stretched = stretch(chs, sr, length * ratio);
    if (ratio - 1.0).abs() < 1e-6 { stretched } else { resample_sinc(&stretched, (ratio * 100000.0).round() as u32, 100000) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::testing::sine;
    use crate::dsp::{db, rms};

    /// Zero crossings per second, a stand-in for pitch on a sine.
    fn crossings(d: &[f32], sr: u32) -> f64 {
        let n = d.windows(2).filter(|w| (w[0] <= 0.0) != (w[1] <= 0.0)).count();
        n as f64 / (d.len() as f64 / sr as f64)
    }

    #[test]
    fn length_changes_and_pitch_stays() {
        let chs = vec![sine(440.0, 48000, 1.0, 0.5), sine(440.0, 48000, 1.0, 0.25)];
        for factor in [0.5, 0.8, 1.5, 2.0] {
            let out = stretch_pitch(&chs, 48000, factor, 0.0);
            assert_eq!(out.len(), 2);
            assert_eq!(out[0].len(), (48000.0 * factor) as usize, "{factor}");
            assert_eq!(out[1].len(), out[0].len());
            assert!((crossings(&out[0], 48000) - 880.0).abs() < 8.0, "{factor}: {}", crossings(&out[0], 48000));
            assert!((db(rms(&[out[0].clone()])) - db(rms(&[chs[0].clone()]))).abs() < 0.5, "{factor}");
        }
    }

    #[test]
    fn pitch_changes_and_length_stays() {
        let chs = vec![sine(440.0, 48000, 1.0, 0.5)];
        let up = stretch_pitch(&chs, 48000, 1.0, 12.0);
        assert!((up[0].len() as i64 - 48000).abs() <= 1, "{}", up[0].len());
        assert!((crossings(&up[0], 48000) - 1760.0).abs() < 16.0, "{}", crossings(&up[0], 48000));
        let down = stretch_pitch(&chs, 48000, 1.0, -12.0);
        assert!((down[0].len() as i64 - 48000).abs() <= 1, "{}", down[0].len());
        assert!((crossings(&down[0], 48000) - 440.0).abs() < 8.0, "{}", crossings(&down[0], 48000));
    }

    #[test]
    fn no_change_and_tiny_inputs() {
        let chs = vec![sine(440.0, 48000, 0.1, 0.5)];
        assert_eq!(stretch_pitch(&chs, 48000, 1.0, 0.0), chs);
        assert_eq!(stretch_pitch(&[vec![]], 48000, 2.0, 0.0), vec![Vec::<f32>::new()]);
        assert!(stretch_pitch(&[], 48000, 2.0, 0.0).is_empty());
        // Shorter than one grain.
        assert_eq!(stretch_pitch(&[vec![0.5f32; 100]], 48000, 2.0, 0.0)[0].len(), 200);
        assert_eq!(stretch_pitch(&[vec![0.5f32]], 48000, 0.2, 0.0)[0].len(), 1);
    }
}
