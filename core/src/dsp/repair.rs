//! Repairs and edge treatment: click removal, loop crossfade, fades, silence trimming and the
//! sampler prep that combines them.

use super::{frames, lin, mix_mono, num, peak};
use serde_json::Value;
use std::f64::consts::FRAC_PI_2;

/// Samples the click detector averages around each sample.
const DECLICK_WINDOW: usize = 1024;

/// Replaces clicks with straight lines. A click is a run of up to 3 ms where the waveform's
/// second difference stands far above its average over the samples around it. Returns the
/// channels and how many clicks were repaired. `sensitivity` runs from 1 to 10.
pub fn declick(chs: &[Vec<f32>], sr: u32, sensitivity: f64) -> (Vec<Vec<f32>>, usize) {
    let max_run = (sr as f64 * 0.003).round() as usize;
    let factor = 26.0 - sensitivity * 2.2;
    let mut repaired = 0;
    let channels = chs
        .iter()
        .map(|d| {
            let (out, n) = declick_channel(d, max_run, factor);
            repaired += n;
            out
        })
        .collect();
    (channels, repaired)
}

fn declick_channel(d: &[f32], max_run: usize, factor: f64) -> (Vec<f32>, usize) {
    let n = d.len();
    let mut out = d.to_vec();
    // How sharply the waveform bends at each sample, and its running total.
    let mut bend = vec![0f32; n];
    for i in 2..n {
        bend[i] = (d[i] as f64 - 2.0 * d[i - 1] as f64 + d[i - 2] as f64).abs() as f32;
    }
    let mut total = vec![0f64; n + 1];
    for i in 0..n {
        total[i + 1] = total[i] + bend[i] as f64;
    }
    // The average bend over the window centred on `i`, which stops at the ends of the sound.
    let average = |i: usize| {
        let from = i.saturating_sub(DECLICK_WINDOW / 2).min(n.saturating_sub(DECLICK_WINDOW));
        (total[n.min(from + DECLICK_WINDOW)] - total[from]) / DECLICK_WINDOW as f64
    };
    let mut repaired = 0;
    let mut i = 2;
    while i + 2 < n {
        let usual = average(i);
        if bend[i] as f64 <= factor * usual + 1e-5 {
            i += 1;
            continue;
        }
        let mut end = i;
        while end + 3 < n && end - i < max_run && bend[end + 1] as f64 > 0.5 * factor * usual {
            end += 1;
        }
        // A line from the sample before the click to the one after it, 3 samples wider each side.
        let from = i.saturating_sub(3).max(1);
        let to = (n - 2).min(end + 3);
        let (y0, y1) = (out[from - 1] as f64, d[to + 1] as f64);
        for k in from..=to {
            out[k] = (y0 + (y1 - y0) * (k - from + 1) as f64 / (to - from + 2) as f64) as f32;
        }
        repaired += 1;
        i = to + 1;
    }
    (out, repaired)
}

/// Folds the last `n` samples into the first `n` with an equal-power crossfade and drops them,
/// so the end of a loop runs into its start without a step.
pub fn loop_crossfade(chs: &[Vec<f32>], n: usize) -> Vec<Vec<f32>> {
    let len = frames(chs);
    let n = n.min(len / 2);
    chs.iter()
        .map(|d| {
            let mut out = d[..len - n].to_vec();
            for i in 0..n {
                let t = (i as f64 + 0.5) / n as f64 * FRAC_PI_2;
                out[i] = (d[i] as f64 * t.sin() + d[len - n + i] as f64 * t.cos()) as f32;
            }
            out
        })
        .collect()
}

/// The span from the first to the last sample of the mono mix at or above the linear level
/// `threshold`, padded by 5 ms. None when nothing reaches it.
pub fn sound_bounds(chs: &[Vec<f32>], sr: u32, threshold: f64) -> Option<(usize, usize)> {
    let m = mix_mono(chs);
    let quiet = |v: &f32| (*v as f64).abs() < threshold;
    let a = m.iter().position(|v| !quiet(v))?;
    let b = m.len() - m.iter().rev().position(|v| !quiet(v))?;
    let pad = (sr as f64 * 0.005).round() as usize;
    Some((a.saturating_sub(pad), m.len().min(b + pad)))
}

/// Fade over samples a..b, in place: the gain follows sin² up for a fade in, cos² down for a
/// fade out. A span that runs past the end of the sound is faded as far as the sound goes.
pub fn fade(chs: &mut [Vec<f32>], a: usize, b: usize, fade_in: bool) {
    let n = b.saturating_sub(a);
    for d in chs.iter_mut() {
        for (i, v) in d.iter_mut().skip(a).take(n).enumerate() {
            let t = i as f64 / n as f64 * FRAC_PI_2;
            let g = if fade_in { t.sin() } else { t.cos() };
            *v = (*v as f64 * (g * g)) as f32;
        }
    }
}

/// Sampler prep settings.
#[derive(Debug, Clone, Copy)]
pub struct PrepParams {
    /// The linear level under which the ends of the sound count as silence.
    pub silence: f64,
    /// The peak level the sound is normalized to, in dB.
    pub target_db: f64,
    pub fade_in_ms: f64,
    pub fade_out_ms: f64,
}

impl Default for PrepParams {
    fn default() -> Self {
        PrepParams { silence: lin(-45.0), target_db: -1.0, fade_in_ms: 2.0, fade_out_ms: 10.0 }
    }
}

impl PrepParams {
    /// Reads the keys a batch export sends (`silenceDb`, `targetDb`, `fadeInMs`, `fadeOutMs`);
    /// missing ones take the defaults.
    pub fn from_json(p: &Value) -> Self {
        let d = Self::default();
        PrepParams {
            silence: num(p, "silenceDb").map_or(d.silence, lin),
            target_db: num(p, "targetDb").unwrap_or(d.target_db),
            fade_in_ms: num(p, "fadeInMs").unwrap_or(d.fade_in_ms),
            fade_out_ms: num(p, "fadeOutMs").unwrap_or(d.fade_out_ms),
        }
    }
}

/// Sampler prep: remove DC, trim the silent ends, normalize the peak, add short edge fades.
/// Also returns the span of the input that was kept, when silence was found around a sound.
pub fn prep(chs: &[Vec<f32>], sr: u32, p: &PrepParams) -> (Vec<Vec<f32>>, Option<(usize, usize)>) {
    let mut out: Vec<Vec<f32>> = chs
        .iter()
        .map(|d| {
            let mean = d.iter().map(|&v| v as f64).sum::<f64>() / d.len().max(1) as f64;
            d.iter().map(|&v| (v as f64 - mean) as f32).collect()
        })
        .collect();
    let kept = sound_bounds(&out, sr, p.silence);
    if let Some((a, b)) = kept {
        out = out.into_iter().map(|d| d[a..b].to_vec()).collect();
    }
    let top = peak(&out);
    if top != 0.0 {
        let k = lin(p.target_db) / top;
        for v in out.iter_mut().flatten() {
            *v = (*v as f64 * k) as f32;
        }
    }
    let len = frames(&out);
    let samples = |ms: f64| (sr as f64 * ms / 1000.0).round().max(0.0) as usize;
    fade(&mut out, 0, len.min(samples(p.fade_in_ms)), true);
    fade(&mut out, len.saturating_sub(samples(p.fade_out_ms)), len, false);
    (out, kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::testing::sine;
    use std::f64::consts::PI;

    /// A decaying 60 Hz tone with a burst of full-scale alternating samples at `at`.
    fn tone_with_burst(at: usize) -> Vec<f32> {
        let mut d: Vec<f32> = (0..48000).map(|i| (0.5 * (-(i as f64) / 12000.0).exp() * (2.0 * PI * 60.0 * i as f64 / 48000.0).sin()) as f32).collect();
        for i in 0..50 {
            d[at + i] += if i % 2 == 0 { 0.9 } else { -0.9 };
        }
        d
    }

    #[test]
    fn a_clean_sine_is_left_alone() {
        for hz in [60.0, 440.0, 5000.0] {
            let chs = vec![sine(hz, 48000, 1.0, 0.8)];
            let (out, repaired) = declick(&chs, 48000, 10.0);
            assert_eq!(repaired, 0, "{hz} Hz");
            assert_eq!(out, chs);
        }
    }

    #[test]
    fn a_click_is_bridged() {
        let clean = sine(100.0, 48000, 0.5, 0.5);
        let mut d = clean.clone();
        d[10000] += 0.7;
        d[17000] -= 0.6;
        let (out, repaired) = declick(&[d.clone()], 48000, 5.0);
        assert_eq!(repaired, 2);
        for i in 0..d.len() {
            assert!((out[0][i] - clean[i]).abs() < 0.01, "sample {i}: {} against {}", out[0][i], clean[i]);
            if !(9990..10010).contains(&i) && !(16990..17010).contains(&i) {
                assert_eq!(out[0][i], d[i], "sample {i} changed");
            }
        }
    }

    /// The burst used to poison the detector's running average when it sat at sample 1024:
    /// every later sample then passed for a click and most of the file was flattened.
    #[test]
    fn a_burst_is_repaired_alike_wherever_it_sits() {
        let changed = |at: usize| {
            let d = tone_with_burst(at);
            let (out, repaired) = declick(std::slice::from_ref(&d), 48000, 5.0);
            let changed: Vec<usize> = (0..d.len()).filter(|&i| out[0][i] != d[i]).collect();
            assert!(changed.iter().all(|&i| i + 10 >= at && i < at + 60), "burst at {at}: changes from {} to {}", changed[0], changed[changed.len() - 1]);
            (repaired, changed.len())
        };
        let early = changed(1000);
        assert!(early.0 >= 1 && early.1 >= 50, "{early:?}");
        assert_eq!(early, changed(3000));
        assert_eq!(early, changed(20000));
    }

    #[test]
    fn repairs_do_not_shift_the_detector() {
        // Many clicks in a row: each is judged against the samples around it, however many
        // were repaired before it.
        let clean = sine(100.0, 48000, 1.0, 0.5);
        let mut d = clean.clone();
        let at: Vec<usize> = (0..20).map(|k| 2000 + k * 2000).collect();
        for &a in &at {
            d[a] += 0.7;
        }
        let (out, repaired) = declick(&[d], 48000, 5.0);
        assert_eq!(repaired, at.len());
        for i in 0..clean.len() {
            assert!((out[0][i] - clean[i]).abs() < 0.01, "sample {i}");
        }
    }

    #[test]
    fn declick_of_tiny_inputs() {
        assert_eq!(declick(&[], 48000, 5.0), (vec![], 0));
        for n in 0..6 {
            let chs = vec![vec![0.5f32; n]];
            assert_eq!(declick(&chs, 48000, 5.0), (chs, 0));
        }
    }

    #[test]
    fn loop_crossfade_shortens_and_meets_the_end() {
        let chs = vec![(0..1000).map(|i| i as f32 / 1000.0).collect::<Vec<f32>>()];
        let out = loop_crossfade(&chs, 100);
        assert_eq!(out[0].len(), 900);
        // The start now continues from where the kept part ends.
        assert!((out[0][0] - 0.9).abs() < 0.01 && (out[0][99] - 0.099).abs() < 0.01);
        assert_eq!(out[0][100..], chs[0][100..900]);
        assert_eq!(loop_crossfade(&chs, 0), chs);
        assert_eq!(loop_crossfade(&chs, 5000)[0].len(), 500);
        assert!(loop_crossfade(&[], 10).is_empty());
    }

    #[test]
    fn bounds_of_a_sound_in_silence() {
        let mut d = vec![0f32; 48000];
        d[10000..20000].fill(0.5);
        assert_eq!(sound_bounds(&[d.clone()], 48000, 0.1), Some((10000 - 240, 20000 + 240)));
        assert_eq!(sound_bounds(&[d.clone()], 48000, 0.6), None);
        assert_eq!(sound_bounds(&[vec![0.5f32; 100]], 48000, 0.1), Some((0, 100)));
        assert_eq!(sound_bounds(&[vec![]], 48000, 0.1), None);
        assert_eq!(sound_bounds(&[], 48000, 0.1), None);
        // The threshold itself counts as sound.
        assert_eq!(sound_bounds(&[vec![0.0, 0.5, 0.0]], 1000, 0.5), Some((0, 3)));
    }

    #[test]
    fn fades_are_equal_power_and_stay_inside_the_sound() {
        let mut chs = vec![vec![1f32; 100]];
        fade(&mut chs, 0, 50, true);
        assert_eq!(chs[0][0], 0.0);
        assert!((chs[0][25] - 0.5).abs() < 1e-6);
        assert_eq!(chs[0][50..], [1f32; 50]);
        fade(&mut chs, 80, 100, false);
        assert_eq!(chs[0][80], 1.0);
        assert!((chs[0][90] - 0.5).abs() < 1e-6 && chs[0][99] < 0.01);
        let mut chs = vec![vec![1f32; 10]];
        fade(&mut chs, 5, 20, false);
        assert!(chs[0][9] > 0.5 && chs[0][9] < 1.0);
        fade(&mut chs, 50, 60, true);
        fade(&mut chs, 8, 3, true);
        assert_eq!(chs[0][..5], [1f32; 5]);
    }

    #[test]
    fn prep_trims_normalizes_and_fades() {
        let mut d = vec![0.1f32; 48000];
        for (v, t) in d[12000..36000].iter_mut().zip(sine(440.0, 48000, 0.5, 0.25)) {
            *v += t;
        }
        let p = PrepParams { silence: 0.01, target_db: -6.0, fade_in_ms: 2.0, fade_out_ms: 10.0 };
        let (out, kept) = prep(&[d.clone(), d], 48000, &p);
        let (a, b) = kept.unwrap();
        assert!((11760..=11770).contains(&a) && (36230..=36240).contains(&b), "{a} to {b}");
        assert_eq!((out.len(), out[0].len()), (2, b - a));
        assert!((peak(&out) - lin(-6.0)).abs() < 1e-6);
        assert_eq!((out[0][0], out[1][0]), (0.0, 0.0));
        assert!(out[0][b - a - 1].abs() < 1e-4);
        // The offset is gone: the padding around the tone is silent.
        assert!(out[0][..200].iter().all(|v| v.abs() < 1e-3));
    }

    #[test]
    fn prep_of_silence_and_nothing() {
        let (out, kept) = prep(&[vec![0f32; 1000]], 48000, &PrepParams::default());
        assert_eq!((out, kept), (vec![vec![0f32; 1000]], None));
        let (out, kept) = prep(&[], 48000, &PrepParams::default());
        assert!(out.is_empty() && kept.is_none());
        let (out, kept) = prep(&[vec![]], 48000, &PrepParams::default());
        assert_eq!((out, kept), (vec![vec![]], None));
    }

    #[test]
    fn prep_settings_from_the_page() {
        let p = PrepParams::from_json(&serde_json::json!({"silenceDb": -20, "fadeOutMs": 50}));
        assert!((p.silence - 0.1).abs() < 1e-12);
        assert_eq!((p.target_db, p.fade_in_ms, p.fade_out_ms), (-1.0, 2.0, 50.0));
    }
}
