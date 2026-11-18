//! Athar's audio core. It has two halves, and each has its own pitch detector, onset detector
//! and resampler on purpose:
//!
//! - `decode`, `features`, `embed` and `analysis` produce what the library stores for every
//!   file: the CLAP vector, the descriptors and the waveform overview. Libraries indexed by
//!   earlier versions hold the same values, so these modules must keep their output; the
//!   frozen files in tests/reference guard that.
//! - `dsp` and `arrange` serve the editor and the Arranger: effects, measurements on a
//!   selection, the spectrogram, and the render that ends in a WAV file. They are free to
//!   change as long as they sound right.
//!
//! `spectrum` holds the Hann window and the short-time Fourier frames both halves use.

pub mod analysis;
pub mod arrange;
pub mod decode;
pub mod dsp;
pub mod embed;
pub mod features;
mod spectrum;

/// Greatest common divisor, for reducing a pair of sample rates to a ratio.
pub(crate) fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}
