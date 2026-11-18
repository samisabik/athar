//! The editor's spectrogram, drawn as RGBA pixels.

use crate::spectrum::{Stft, hann};

/// FFT length: longer at high sample rates so the low end keeps its resolution.
fn fft_size(sr: u32) -> usize {
    if sr >= 88200 { 8192 } else { 4096 }
}

fn stft(sr: u32) -> Stft {
    Stft::new(&hann(fft_size(sr), false))
}

/// The loudest spectral level of mono samples in dB, from up to 256 frames spread across them.
/// It sets the top of the colour scale, so every tile of one sound is drawn alike.
pub fn spectrogram_peak(d: &[f32], sr: u32) -> f64 {
    let n = fft_size(sr);
    let half = n / 2;
    let frames = d.len().div_ceil(half).clamp(1, 256);
    let mut stft = stft(sr);
    let mut max = 0f64;
    for f in 0..frames {
        let centre = ((f as f64 + 0.5) / frames as f64 * d.len() as f64).floor() as i64;
        for c in &stft.analyse(d, centre - half as i64)[..half] {
            max = max.max(c.re as f64 * c.re as f64 + c.im as f64 * c.im as f64);
        }
    }
    20.0 * (max.sqrt() * 4.0 / n as f64 + 1e-9).log10()
}

/// The colour scale from quiet to loud, as RGB.
const STOPS: [[f64; 3]; 6] = [[18., 16., 30.], [52., 24., 96.], [122., 61., 255.], [214., 86., 160.], [255., 168., 92.], [255., 246., 214.]];

/// The colour at `t` from 0 (80 dB under the peak) to 1 (the peak).
fn color(t: f64) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0) * (STOPS.len() - 1) as f64;
    let i = (t.floor() as usize).min(STOPS.len() - 2);
    let f = t - i as f64;
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    [0, 1, 2].map(|c| (a[c] + (b[c] - a[c]) * f).round() as u8)
}

/// Spectrogram image of mono samples: `pw` x `rows` RGBA pixels, one FFT per column centred on
/// sample `start` and stepping `spp` samples, 20 Hz to Nyquist on a log axis from the bottom
/// row up, `peak` dB brightest.
pub fn spectrogram(d: &[f32], sr: u32, start: f64, spp: f64, pw: usize, rows: usize, peak: f64) -> Vec<u8> {
    let n = fft_size(sr);
    let half = n / 2;
    let mut stft = stft(sr);
    let mut mags = vec![0f32; half];
    let mut dbs = vec![-200f32; pw * rows];
    let (fmin, fmax) = (20f64, sr as f64 / 2.0);
    // The (fractional) FFT bin at the top edge of each row, and the bottom edge of the last.
    let edges: Vec<f64> = (0..=rows)
        .map(|y| ((half - 1) as f64).min(fmin * (fmax / fmin).powf(1.0 - y as f64 / rows as f64) / sr as f64 * n as f64))
        .collect();
    for x in 0..pw {
        let centre = (start + x as f64 * spp).floor();
        if centre >= d.len() as f64 {
            break;
        }
        for (m, c) in mags.iter_mut().zip(stft.analyse(d, centre as i64 - half as i64).iter()) {
            *m = ((c.re as f64).hypot(c.im as f64) * 4.0 / n as f64) as f32;
        }
        for y in 0..rows {
            let (top, bottom) = (edges[y], edges[y + 1]);
            // A row narrower than a bin interpolates between two bins; a wider one shows the
            // loudest bin it covers.
            let m = if top - bottom < 1.0 {
                let mid = (top + bottom) / 2.0;
                let k = mid.floor() as usize;
                let t = mid - k as f64;
                mags[k] as f64 * (1.0 - t) + mags[(k + 1).min(half - 1)] as f64 * t
            } else {
                mags[bottom.floor() as usize..=top.ceil() as usize].iter().fold(0f64, |m, &v| m.max(v as f64))
            };
            dbs[y * pw + x] = (20.0 * (m + 1e-9).log10()) as f32;
        }
    }
    let floor = peak - 80.0;
    let mut px = vec![0u8; pw * rows * 4];
    for (p, &v) in px.chunks_exact_mut(4).zip(&dbs) {
        let [r, g, b] = color((v as f64 - floor) / 80.0);
        p.copy_from_slice(&[r, g, b, 255]);
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::testing::sine;

    #[test]
    fn peak_of_a_full_scale_sine_is_near_zero() {
        // The 4 / n scale makes a full-scale sine read 0 dB through the Hann window when it
        // sits on a bin, and up to 1.4 dB less between two.
        for sr in [48000, 96000] {
            let pk = spectrogram_peak(&sine(1000.0, sr, 1.0, 1.0), sr);
            assert!(pk <= 0.01 && pk > -1.5, "{sr}: {pk}");
        }
        assert!(spectrogram_peak(&[], 48000) < -170.0);
        assert!(spectrogram_peak(&[0.0], 48000) < -170.0);
    }

    #[test]
    fn colors_run_from_the_first_stop_to_the_last() {
        assert_eq!(color(-1.0), [18, 16, 30]);
        assert_eq!(color(0.0), [18, 16, 30]);
        assert_eq!(color(0.2), [52, 24, 96]);
        assert_eq!(color(0.5), [168, 74, 208]);
        assert_eq!(color(2.0), [255, 246, 214]);
    }

    #[test]
    fn a_tone_lights_its_row() {
        let (sr, rows, pw) = (48000u32, 100usize, 8usize);
        let d = sine(1000.0, sr, 1.0, 1.0);
        let px = spectrogram(&d, sr, 10000.0, 2000.0, pw, rows, 0.0);
        assert_eq!(px.len(), pw * rows * 4);
        // Row y covers 20 * 1200^(1 - y / rows) Hz at its top edge: 1 kHz falls in row 44.
        let brightness = |y: usize| px[(y * pw + 3) * 4..(y * pw + 3) * 4 + 3].iter().map(|&v| v as u32).sum::<u32>();
        let brightest = (0..rows).max_by_key(|&y| brightness(y)).unwrap();
        assert!((43..=45).contains(&brightest), "{brightest}");
        let top = &px[(brightest * pw + 3) * 4..(brightest * pw + 3) * 4 + 4];
        assert!(top[0] == 255 && top[1] > 225 && top[3] == 255, "{top:?}");
        assert_eq!(&px[(5 * pw + 3) * 4..(5 * pw + 3) * 4 + 4], &[18, 16, 30, 255]);
    }

    #[test]
    fn columns_past_the_end_and_empty_images() {
        let d = sine(1000.0, 48000, 0.1, 1.0);
        let px = spectrogram(&d, 48000, 0.0, 4800.0, 3, 10, 0.0);
        // Only the first column starts inside the sound; the others stay at the floor colour.
        for y in 0..10 {
            assert_eq!(&px[(y * 3 + 1) * 4..(y * 3 + 1) * 4 + 4], &[18, 16, 30, 255]);
            assert_eq!(&px[(y * 3 + 2) * 4..(y * 3 + 2) * 4 + 4], &[18, 16, 30, 255]);
        }
        assert!(spectrogram(&d, 48000, 0.0, 100.0, 0, 10, 0.0).is_empty());
        assert!(spectrogram(&d, 48000, 0.0, 100.0, 10, 0, 0.0).is_empty());
        assert_eq!(spectrogram(&[], 48000, 0.0, 100.0, 2, 2, 0.0).len(), 16);
    }
}
