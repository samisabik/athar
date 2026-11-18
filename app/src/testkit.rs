//! What tests across the app share: a folder of their own and sound files to put in it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A folder for one test's files, removed afterwards.
pub struct Tmp(pub PathBuf);

impl Tmp {
    pub fn new() -> Self {
        let p = std::env::temp_dir().join(format!("athar-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Tmp(std::fs::canonicalize(&p).unwrap())
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A mono WAV of `samples`: 32-bit float when `bits` is 32, otherwise integers of that depth.
pub fn write_wav(path: &Path, samples: &[f32], sr: u32, bits: u16) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let float = bits == 32;
    let spec = hound::WavSpec { channels: 1, sample_rate: sr, bits_per_sample: bits, sample_format: if float { hound::SampleFormat::Float } else { hound::SampleFormat::Int } };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let scale = ((1i64 << (bits - 1)) - 1) as f32;
    for &v in samples {
        if float {
            w.write_sample(v).unwrap();
        } else {
            w.write_sample((v.clamp(-1.0, 1.0) * scale).round() as i32).unwrap();
        }
    }
    w.finalize().unwrap();
}

pub fn wav_bytes(samples: &[f32], sr: u32, bits: u16) -> Vec<u8> {
    let t = Tmp::new();
    let p = t.0.join("x.wav");
    write_wav(&p, samples, sr, bits);
    std::fs::read(p).unwrap()
}

/// A 16-bit sine of `seconds` written as `name` in `dir`; returns its path.
pub fn write_tone(dir: &Path, name: &str, sr: u32, seconds: f64) -> String {
    let path = dir.join(name);
    let samples: Vec<f32> = (0..(sr as f64 * seconds) as usize).map(|i| 0.25 * (i as f32 * 0.1).sin()).collect();
    write_wav(&path, &samples, sr, 16);
    path.to_string_lossy().into_owned()
}
