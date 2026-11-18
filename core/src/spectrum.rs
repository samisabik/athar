//! The Hann window and windowed Fourier frames.

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::f64::consts::PI;
use std::sync::Arc;

/// Hann window of `n` points. The periodic form (one period is `n` samples) is the one that
/// overlap-adds to a constant and that librosa uses; the symmetric form ends on zero.
pub(crate) fn hann(n: usize, periodic: bool) -> Vec<f64> {
    let period = if periodic { n } else { n - 1 } as f64;
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / period).cos()).collect()
}

/// Windowed Fourier frames of an f32 signal, with the plan and buffers made once.
pub(crate) struct Stft {
    window: Vec<f32>,
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Option<Arc<dyn ComplexToReal<f32>>>,
    frame: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
}

impl Stft {
    /// Frames as long as `window`, which must have an even length.
    pub fn new(window: &[f64]) -> Self {
        let forward = RealFftPlanner::<f32>::new().plan_fft_forward(window.len());
        let (frame, spectrum) = (forward.make_input_vec(), forward.make_output_vec());
        Stft { window: window.iter().map(|&w| w as f32).collect(), forward, inverse: None, frame, spectrum }
    }

    /// The spectrum (bins 0 to half the frame length, inclusive) of the windowed frame that
    /// starts at sample `start`. Samples outside the signal count as zero.
    pub fn analyse(&mut self, signal: &[f32], start: i64) -> &mut [Complex<f32>] {
        let n = self.window.len() as i64;
        let from = start.clamp(0, signal.len() as i64);
        let to = (start + n).clamp(from, signal.len() as i64);
        self.frame.fill(0.0);
        let at = (from - start).clamp(0, n) as usize;
        for ((f, &x), &w) in self.frame[at..].iter_mut().zip(&signal[from as usize..to as usize]).zip(&self.window[at..]) {
            *f = x * w;
        }
        self.forward.process(&mut self.frame, &mut self.spectrum).expect("buffers come from the plan");
        &mut self.spectrum
    }

    /// The frame the spectrum left by `analyse` (and changed by the caller) stands for, windowed
    /// again for overlap-add.
    pub fn synthesize(&mut self) -> &[f32] {
        let n = self.window.len();
        let inverse = self.inverse.get_or_insert_with(|| RealFftPlanner::<f32>::new().plan_fft_inverse(n));
        // A real signal has no imaginary part at 0 Hz and at Nyquist; rounding may leave one.
        self.spectrum[0].im = 0.0;
        self.spectrum[n / 2].im = 0.0;
        inverse.process(&mut self.spectrum, &mut self.frame).expect("buffers come from the plan");
        for (f, &w) in self.frame.iter_mut().zip(&self.window) {
            *f = *f / n as f32 * w;
        }
        &self.frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_hann_overlap_adds_to_a_constant() {
        let w = hann(8, true);
        for i in 0..4 {
            assert!((w[i] + w[i + 4] - 1.0).abs() < 1e-12);
        }
        let s = hann(9, false);
        assert!(s[0].abs() < 1e-12 && s[8].abs() < 1e-12 && (s[4] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_sine_lands_in_its_bin() {
        let n = 64;
        let signal: Vec<f32> = (0..n).map(|i| (2.0 * PI * 8.0 * i as f64 / n as f64).sin() as f32).collect();
        let mut stft = Stft::new(&vec![1.0; n]);
        let spectrum = stft.analyse(&signal, 0);
        let loudest = (0..=n / 2).max_by(|&a, &b| spectrum[a].norm().total_cmp(&spectrum[b].norm())).unwrap();
        assert_eq!(loudest, 8);
        assert!((spectrum[8].norm() - n as f32 / 2.0).abs() < 1e-3);
    }

    #[test]
    fn frames_past_the_ends_are_zero_filled() {
        let signal = vec![1f32; 16];
        let mut stft = Stft::new(&[1.0; 16]);
        assert!((stft.analyse(&signal, 0)[0].re - 16.0).abs() < 1e-4);
        assert!((stft.analyse(&signal, -12)[0].re - 4.0).abs() < 1e-4);
        assert!((stft.analyse(&signal, 10)[0].re - 6.0).abs() < 1e-4);
        assert!(stft.analyse(&signal, 40)[0].re.abs() < 1e-6);
        assert!(stft.analyse(&signal, -40)[0].re.abs() < 1e-6);
    }

    #[test]
    fn synthesis_undoes_analysis() {
        let signal: Vec<f32> = (0..32).map(|i| ((i * 7) % 11) as f32 / 11.0 - 0.5).collect();
        let mut stft = Stft::new(&[1.0; 32]);
        stft.analyse(&signal, 0);
        for (a, b) in stft.synthesize().iter().zip(&signal) {
            assert!((a - b).abs() < 1e-5);
        }
    }
}
