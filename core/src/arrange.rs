//! The Arranger's mixdown: clips placed on a timeline, summed and saved as one WAV file.

use crate::dsp::{db, encode_wav, lin, mix_mono, resample_sinc};
use std::f64::consts::FRAC_PI_2;

/// Fade lengths in seconds for one clip.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Fade {
    pub fade_in: f64,
    pub fade_out: f64,
}

/// One clip of the mix: its channels at rate `sr`, a linear gain, where it starts in seconds,
/// and its fades. A mono clip goes to both sides of a stereo mix.
#[derive(Debug, Clone, Default)]
pub struct MixLane {
    pub chs: Vec<Vec<f32>>,
    pub sr: u32,
    pub gain: f64,
    pub at: f64,
    pub fade: Fade,
}

/// How a mix is rendered: mixed at rate `sr` for `seconds`, saved at `rate` and `bits`.
#[derive(Debug, Clone, Copy)]
pub struct MixArgs {
    pub sr: u32,
    pub seconds: f64,
    pub rate: u32,
    pub bits: u32,
    pub mono: bool,
}

/// The saved mix: WAV bytes, and how far over full scale the mix went in dB (0 when it fit).
#[derive(Debug, Clone)]
pub struct Mix {
    pub wav: Vec<u8>,
    pub over: f64,
}

/// Sums lanes into `seconds` of sound at rate `sr`; stereo when any lane is.
fn mixdown(lanes: &[MixLane], sr: u32, seconds: f64) -> Vec<Vec<f32>> {
    let srf = sr as f64;
    let n = ((seconds * srf).round() as usize).max(1);
    let count = if lanes.iter().any(|l| l.chs.len() > 1) { 2 } else { 1 };
    let mut channels = vec![vec![0f32; n]; count];
    for l in lanes {
        if l.chs.is_empty() {
            continue;
        }
        let resampled;
        let chs = if l.sr == sr {
            &l.chs
        } else {
            resampled = resample_sinc(&l.chs, l.sr, sr);
            &resampled
        };
        let len = chs[0].len() as i64;
        let start = (l.at * srf).round() as i64;
        let fade_in = (l.fade.fade_in * srf).round() as i64;
        let fade_out = (l.fade.fade_out * srf).round() as i64;
        // Samples of the clip that fall inside the mix.
        let (from, to) = ((-start).max(0), len.min(n as i64 - start));
        for (c, o) in channels.iter_mut().enumerate() {
            let d = &chs[c.min(chs.len() - 1)];
            for i in from..to {
                let mut g = l.gain;
                if i < fade_in {
                    g *= (i as f64 / fade_in as f64 * FRAC_PI_2).sin();
                }
                if i >= len - fade_out {
                    g *= ((i - (len - fade_out)) as f64 / fade_out as f64 * FRAC_PI_2).cos();
                }
                let at = (start + i) as usize;
                o[at] = (o[at] as f64 + d[i as usize] as f64 * g) as f32;
            }
        }
    }
    channels
}

/// Turns the sound down to -0.1 dBFS, in place, when it goes over full scale. Returns how far
/// over it was in dB, 0 when it fit.
fn fit_peak(channels: &mut [Vec<f32>]) -> f64 {
    let peak = channels.iter().flatten().fold(0f64, |m, &v| m.max((v as f64).abs()));
    if peak <= 1.0 {
        return 0.0;
    }
    let k = lin(-0.1) / peak;
    for v in channels.iter_mut().flatten() {
        *v = (*v as f64 * k) as f32;
    }
    db(peak)
}

/// The whole save of a mix: mix, fold to mono if asked, convert the rate, fit the peak, encode.
pub fn render_mix(lanes: &[MixLane], a: &MixArgs) -> Result<Mix, String> {
    let mut c = mixdown(lanes, a.sr, a.seconds);
    if a.mono && c.len() > 1 {
        c = vec![mix_mono(&c)];
    }
    if a.rate != a.sr {
        c = resample_sinc(&c, a.sr, a.rate);
    }
    let over = fit_peak(&mut c);
    let wav = encode_wav(&c, a.rate, a.bits)?;
    Ok(Mix { wav, over })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(chs: Vec<Vec<f32>>, at: f64, gain: f64) -> MixLane {
        MixLane { chs, sr: 1000, gain, at, fade: Fade::default() }
    }

    #[test]
    fn clips_land_at_their_time_with_their_gain() {
        let mix = mixdown(&[lane(vec![vec![1.0; 100]], 0.5, 0.5), lane(vec![vec![1.0; 100]], 0.55, 0.25)], 1000, 1.0);
        assert_eq!((mix.len(), mix[0].len()), (1, 1000));
        assert_eq!((mix[0][499], mix[0][500], mix[0][549], mix[0][550], mix[0][599], mix[0][600], mix[0][649], mix[0][650]), (0.0, 0.5, 0.5, 0.75, 0.75, 0.25, 0.25, 0.0));
    }

    #[test]
    fn a_mono_clip_goes_to_both_sides() {
        let mix = mixdown(&[lane(vec![vec![1.0; 10]], 0.0, 1.0), lane(vec![vec![0.5; 10], vec![-0.5; 10]], 0.0, 1.0)], 1000, 0.01);
        assert_eq!(mix, vec![vec![1.5; 10], vec![0.5; 10]]);
    }

    #[test]
    fn clips_are_cut_at_the_edges_of_the_mix() {
        let mix = mixdown(&[lane(vec![vec![1.0; 100]], -0.05, 1.0), lane(vec![vec![1.0; 100]], 0.95, 1.0), lane(vec![], 0.0, 1.0), lane(vec![vec![1.0; 10]], 5.0, 1.0)], 1000, 1.0);
        assert_eq!(mix[0].iter().filter(|&&v| v == 1.0).count(), 100);
        assert_eq!((mix[0][49], mix[0][50], mix[0][949], mix[0][950]), (1.0, 0.0, 0.0, 1.0));
        assert_eq!(mixdown(&[], 1000, 0.0), vec![vec![0.0]]);
    }

    #[test]
    fn fades_shape_the_ends_and_rates_are_converted() {
        let mut l = lane(vec![vec![1.0; 1000]], 0.0, 1.0);
        l.fade = Fade { fade_in: 0.1, fade_out: 0.2 };
        let mix = mixdown(std::slice::from_ref(&l), 1000, 1.0);
        assert_eq!((mix[0][0], mix[0][100], mix[0][799]), (0.0, 1.0, 1.0));
        assert!((mix[0][50] as f64 - (0.5 * FRAC_PI_2).sin()).abs() < 1e-6);
        assert!((mix[0][900] as f64 - (0.5 * FRAC_PI_2).cos()).abs() < 1e-6);
        assert!(mix[0][999] < 0.01);
        l.fade = Fade::default();
        let mix = mixdown(&[l], 2000, 1.0);
        assert!((mix[0][1000] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn a_mix_over_full_scale_is_turned_down() {
        let mut chs = vec![vec![0.5f32, -2.0], vec![1.0, 0.25]];
        let over = fit_peak(&mut chs);
        assert!((over - db(2.0)).abs() < 1e-12);
        assert!((chs[0][1] as f64 + lin(-0.1)).abs() < 1e-6 && (chs[0][0] as f64 - lin(-0.1) / 4.0).abs() < 1e-6);
        let mut quiet = vec![vec![1.0f32, -0.5]];
        assert_eq!(fit_peak(&mut quiet), 0.0);
        assert_eq!(quiet, vec![vec![1.0, -0.5]]);
    }

    #[test]
    fn render_reports_the_overshoot_and_encodes() {
        let lanes = [lane(vec![vec![1.0; 100], vec![1.0; 100]], 0.0, 2.0)];
        let mix = render_mix(&lanes, &MixArgs { sr: 1000, seconds: 0.1, rate: 1000, bits: 24, mono: true }).unwrap();
        assert!((mix.over - db(2.0)).abs() < 1e-9);
        assert_eq!(mix.wav.len(), 44 + 300);
        let stereo = render_mix(&lanes, &MixArgs { sr: 1000, seconds: 0.1, rate: 2000, bits: 16, mono: false }).unwrap();
        assert_eq!(stereo.wav.len(), 44 + 200 * 4);
        assert!(render_mix(&lanes, &MixArgs { sr: 1000, seconds: 0.1, rate: 1000, bits: 12, mono: false }).is_err());
    }
}
