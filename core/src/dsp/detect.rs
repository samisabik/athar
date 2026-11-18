//! The editor's pitch and onset detectors, which work on a selection at its own sample rate.
//! The values the library stores come from `analysis` instead.

use crate::spectrum::{Stft, hann};

/// Lowest pitch looked for, in Hz.
const PITCH_FLOOR: f64 = 25.0;
const PITCH_CEILING: f64 = 2000.0;
/// A dip of the normalized difference under this counts as a period.
const YIN_THRESHOLD: f64 = 0.12;

/// The pitch a sound holds in Hz, or None: YIN on up to 60 frames of the first 12 seconds, the
/// median of the frames that have a pitch, when at least a fifth of them do. Rates from 32 kHz
/// up are halved first.
pub fn detect_pitch(d: &[f32], sr: u32) -> Option<f64> {
    let halve = sr >= 32000;
    let rate = if halve { sr as f64 / 2.0 } else { sr as f64 };
    let limit = (rate * 12.0).round() as usize;
    let m: Vec<f32> =
        if halve { d.chunks_exact(2).take(limit).map(|p| ((p[0] as f64 + p[1] as f64) / 2.0) as f32).collect() } else { d[..d.len().min(limit)].to_vec() };
    let max_lag = (rate / PITCH_FLOOR).floor() as usize;
    let min_lag = ((rate / PITCH_CEILING).floor() as usize).max(2);
    let window = max_lag * 2;
    if m.len() < window + max_lag || max_lag <= min_lag {
        return None;
    }
    let hop = ((m.len() - window - max_lag) / 60).max(256);
    let energies: Vec<(usize, f64)> =
        (0..=m.len() - window - max_lag).step_by(hop).map(|s| (s, m[s..s + window].iter().map(|&v| v as f64 * v as f64).sum())).collect();
    let loudest = energies.iter().fold(0f64, |top, e| top.max(e.1));
    // The cumulative-mean-normalized difference of YIN, by lag.
    let mut diff = vec![0f32; max_lag + 1];
    let mut found = Vec::new();
    for &(s, energy) in &energies {
        if energy < loudest * 0.01 {
            continue;
        }
        let mut running = 0f64;
        diff[0] = 1.0;
        for lag in 1..=max_lag {
            let mut sum = 0f64;
            for i in 0..window {
                let v = m[s + i] as f64 - m[s + i + lag] as f64;
                sum += v * v;
            }
            running += sum;
            diff[lag] = (if running != 0.0 { sum * lag as f64 / running } else { 1.0 }) as f32;
        }
        // The first dip under the threshold, followed down to its bottom.
        let Some(mut lag) = (min_lag..max_lag).find(|&t| (diff[t] as f64) < YIN_THRESHOLD) else { continue };
        while lag + 1 < max_lag && diff[lag + 1] < diff[lag] {
            lag += 1;
        }
        // A parabola through the bottom and its neighbours places the period between samples.
        let (a, b, c) = (diff[lag - 1] as f64, diff[lag] as f64, diff[lag + 1] as f64);
        let curve = a - 2.0 * b + c;
        found.push(rate / (lag as f64 + if curve != 0.0 { (a - c) / (2.0 * curve) } else { 0.0 }));
    }
    if (found.len() as f64) < (energies.len() as f64 * 0.2).max(1.0) {
        return None;
    }
    found.sort_by(f64::total_cmp);
    Some(found[found.len() / 2])
}

/// Sample positions where a new sound starts: peaks of the spectral flux (the rise of the log
/// spectrum from one 1024-sample frame to the next) that stand out from the frames around
/// them, at least 50 ms apart, each moved back to where the waveform starts to rise.
/// `sensitivity` runs from 1 to 10.
pub fn onsets(d: &[f32], sr: u32, sensitivity: f64) -> Vec<usize> {
    const N: usize = 1024;
    const HOP: usize = 256;
    let frames = if d.len() >= N { (d.len() - N) / HOP + 1 } else { 0 };
    let mut stft = Stft::new(&hann(N, true));
    let mut prev = vec![0f32; N / 2];
    let mut flux = vec![0f32; frames];
    for f in 0..frames {
        let spectrum = stft.analyse(d, (f * HOP) as i64);
        let mut rise = 0f64;
        for k in 1..N / 2 {
            let m = (100.0 * (spectrum[k].re as f64).hypot(spectrum[k].im as f64)).ln_1p();
            if m > prev[k] as f64 {
                rise += m - prev[k] as f64;
            }
            prev[k] = m as f32;
        }
        flux[f] = if f > 0 { rise as f32 } else { 0.0 };
    }
    let mean = flux.iter().map(|&v| v as f64).sum::<f64>() / frames.max(1) as f64;
    let factor = 4.2 - sensitivity * 0.32;
    let gap = (sr as f64 * 0.05 / HOP as f64).ceil() as i64;
    let lead = (sr as f64 * 0.002).round() as usize;
    let mut out = Vec::new();
    let mut last = -gap;
    for f in 1..frames.saturating_sub(1) {
        let around = &flux[f.saturating_sub(12)..frames.min(f + 12)];
        let local = around.iter().map(|&v| v as f64).sum::<f64>();
        let stands_out = flux[f] as f64 > factor * local / around.len() as f64 + 0.2 * mean;
        if !(stands_out && flux[f] >= flux[f - 1] && flux[f] > flux[f + 1] && f as i64 - last >= gap) {
            continue;
        }
        // The onset is 2 ms before the waveform first passes a fifth of the frame's peak.
        let lo = f * HOP;
        let hi = d.len().min(lo + N + HOP);
        let peak = d[lo..hi].iter().fold(0f64, |m, &v| m.max((v as f64).abs()));
        let rise = (lo..hi).find(|&i| (d[i] as f64).abs() > 0.2 * peak);
        out.push(rise.map_or(lo, |i| i.saturating_sub(lead).max(lo)));
        last = f as i64;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::testing::{noise, sine};

    #[test]
    fn a_tone_has_its_pitch() {
        for (hz, sr) in [(110.0, 48000), (440.0, 44100), (440.0, 22050), (1000.0, 96000)] {
            let got = detect_pitch(&sine(hz, sr, 1.0, 0.5), sr).unwrap();
            assert!((got - hz).abs() < hz * 0.005, "{hz} Hz at {sr}: {got}");
        }
    }

    #[test]
    fn noise_silence_and_short_sounds_have_none() {
        assert_eq!(detect_pitch(&noise(48000, 5), 48000), None);
        assert_eq!(detect_pitch(&vec![0f32; 48000], 48000), None);
        assert_eq!(detect_pitch(&sine(440.0, 48000, 0.05, 0.5), 48000), None);
        assert_eq!(detect_pitch(&[], 48000), None);
        assert_eq!(detect_pitch(&[0.5], 48000), None);
    }

    /// Short noise bursts at the given sample positions.
    fn bursts(at: &[usize], len: usize) -> Vec<f32> {
        let mut d = vec![0f32; len];
        for &a in at {
            for (i, v) in noise(2400, a as u32).into_iter().enumerate() {
                d[a + i] = v * 0.8 * (-(i as f32) / 600.0).exp();
            }
        }
        d
    }

    #[test]
    fn bursts_are_found_where_they_start() {
        let at = [12000, 24000, 40000, 61000];
        let found = onsets(&bursts(&at, 72000), 48000, 5.0);
        assert_eq!(found.len(), at.len(), "{found:?}");
        for (f, a) in found.iter().zip(at) {
            assert!(*f <= a && a - *f <= 300, "{f} for {a}");
        }
    }

    #[test]
    fn nothing_to_find() {
        assert!(onsets(&vec![0f32; 48000], 48000, 5.0).is_empty());
        assert!(onsets(&sine(440.0, 48000, 1.0, 0.5)[4800..], 48000, 5.0).is_empty());
        assert!(onsets(&[], 48000, 5.0).is_empty());
        assert!(onsets(&[0.5; 100], 48000, 5.0).is_empty());
    }
}
