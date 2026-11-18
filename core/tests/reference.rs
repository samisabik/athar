//! Checks the core against the files in tests/reference, so the vectors and descriptors stored
//! in existing libraries stay valid. The .npy files and reference.json are frozen outputs of
//! the retired Python engine (called "Python" below); they cannot be regenerated, only kept.
//!
//! The tests that embed need the model in build/model, which packaging/ builds locally. They
//! fail when it is absent, unless ATHAR_SKIP_MODEL_TESTS is set.

use athar_core::features::{clap_features, FRAMES, N_MELS};
use ndarray::ArrayD;
use ndarray_npy::read_npy;
use std::path::PathBuf;

fn reference(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/reference").join(name)
}

const SOUNDS: [&str; 5] = ["kick_44k_mono", "tone_440_48k", "noise_burst_44k", "chirp_stereo_16s", "loop_120bpm_22k"];

#[test]
fn features_match_python() {
    for name in SOUNDS {
        // Same input as Python's first clip: its 48 kHz audio. Long files are cut into clips
        // first, which this test does not cover.
        let y: ndarray::Array1<f32> = read_npy(reference(&format!("{name}.load48k.npy"))).unwrap();
        let y = y.to_vec();
        if y.len() > 15 * 48_000 {
            continue;
        }
        let want: ArrayD<f32> = read_npy(reference(&format!("{name}.features.npy"))).unwrap();
        let want = want.as_slice().unwrap();
        let got = clap_features(&y);
        assert_eq!(got.len(), FRAMES * N_MELS);
        assert_eq!(want.len(), got.len());
        let worst = got.iter().zip(want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        println!("{name}: largest difference {worst:.6} dB");
        assert!(worst < 1e-3, "{name}: features differ by {worst} dB");
    }
}

fn model() -> Option<athar_core::embed::Clap> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../build/model");
    if !dir.join("audio.onnx").exists() {
        // A missing model must not read as a pass, so skipping is asked for by name.
        assert!(std::env::var_os("ATHAR_SKIP_MODEL_TESTS").is_some(), "no model in {}: run packaging/export_model.py, or set ATHAR_SKIP_MODEL_TESTS", dir.display());
        return None;
    }
    Some(athar_core::embed::Clap::load(&dir).unwrap())
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[test]
fn embeddings_match_python() {
    let Some(clap) = model() else { return };
    for name in SOUNDS {
        let y: ndarray::Array1<f32> = read_npy(reference(&format!("{name}.load48k.npy"))).unwrap();
        if y.len() > 15 * 48_000 {
            continue;
        }
        let got = clap.embed_sound(&[y.to_vec()]).unwrap();
        let want: ndarray::Array1<f32> = read_npy(reference(&format!("{name}.embedding.npy"))).unwrap();
        let c = cosine(&got, want.as_slice().unwrap());
        println!("{name}: cosine {c:.6}");
        assert!(c > 0.9999, "{name}: cosine {c}");
    }
    let queries = ["deep punchy kick drum", "crisp snare", "warm synth pad", "rain on a window", "a person talking"];
    let got = clap.embed_texts(&queries).unwrap();
    let want: ndarray::Array2<f32> = read_npy(reference("queries.embedding.npy")).unwrap();
    for (i, q) in queries.iter().enumerate() {
        let c = cosine(&got[i], want.row(i).as_slice().unwrap());
        println!("{q:?}: cosine {c:.6}");
        assert!(c > 0.9999, "{q}: cosine {c}");
    }
}

/// How far `got` is from `want` in dB, counting only frequencies below `hz` at 48 kHz. Python
/// resampled with libsoxr, whose filter rolls off near Nyquist differently from the core's.
fn snr_below(got: &[f32], want: &[f32], hz: f64) -> f64 {
    let n = want.len();
    let fft = realfft::RealFftPlanner::<f64>::new().plan_fft_forward(n);
    let spectrum = |x: &[f32]| {
        let mut buf: Vec<f64> = x.iter().map(|&v| v as f64).collect();
        let mut out = fft.make_output_vec();
        fft.process(&mut buf, &mut out).unwrap();
        out
    };
    let (g, w) = (spectrum(got), spectrum(want));
    let bins = (hz * n as f64 / 48_000.0) as usize;
    let signal: f64 = w[..bins].iter().map(|c| c.norm_sqr()).sum();
    let error: f64 = g[..bins].iter().zip(&w[..bins]).map(|(a, b)| (a - b).norm_sqr()).sum();
    10.0 * (signal / error.max(1e-300)).log10()
}

#[test]
fn files_match_python() {
    use athar_core::decode::{clips, decode, load_audio};
    let clap = model();
    let index: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(reference("reference.json")).unwrap()).unwrap();
    for name in SOUNDS {
        let info = &index["files"][name];
        let d = decode(&reference(&format!("{name}.wav"))).unwrap();
        assert_eq!(d.sr as u64, info["sr"].as_u64().unwrap(), "{name}: rate");
        assert_eq!(d.channels.len() as u64, info["channels"].as_u64().unwrap(), "{name}: channels");
        assert!((d.duration() - info["duration"].as_f64().unwrap()).abs() < 1e-6, "{name}: duration");
        let y = load_audio(&d).unwrap();
        let want: ndarray::Array1<f32> = read_npy(reference(&format!("{name}.load48k.npy"))).unwrap();
        assert_eq!(y.len(), want.len(), "{name}: 48 kHz length");
        let snr = snr_below(&y, want.as_slice().unwrap(), 0.9 * d.sr.min(48_000) as f64 / 2.0);
        let c = clips(&d, &y).unwrap();
        assert_eq!(c.len() as u64, info["clips"].as_u64().unwrap(), "{name}: clips");
        println!("{name}: 48 kHz audio {snr:.1} dB from Python's below 90% of Nyquist");
        assert!(snr > 60.0, "{name}: 48 kHz audio only {snr} dB from Python's");
        if let Some(clap) = &clap {
            let got = clap.embed_sound(&c).unwrap();
            let emb: ndarray::Array1<f32> = read_npy(reference(&format!("{name}.embedding.npy"))).unwrap();
            let cos = cosine(&got, emb.as_slice().unwrap());
            println!("{name}: embedding cosine {cos:.6}");
            assert!(cos > 0.9999, "{name}: cosine {cos}");
        }
    }
}

#[test]
fn analysis_matches_python() {
    use athar_core::analysis::{analyze, peaks};
    use athar_core::decode::{decode, load_audio};
    let index: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(reference("reference.json")).unwrap()).unwrap();
    for name in SOUNDS {
        let info = &index["files"][name];
        let want = &info["analysis"];
        let d = decode(&reference(&format!("{name}.wav"))).unwrap();
        let y = load_audio(&d).unwrap();
        let a = analyze(&y, d.duration(), &format!("{name}.wav")).unwrap();
        println!("{name}: {a:?}");
        println!("{:>width$}  python: {want}", "", width = name.len());
        let close = |got: f64, key: &str, tol: f64| {
            let w = want[key].as_f64().unwrap();
            assert!((got - w).abs() <= tol * w.abs().max(1.0), "{name}: {key} {got} against {w}");
        };
        // Python resampled with libsoxr and the core with rubato. Their filters differ near
        // Nyquist, which moves the inter-sample peaks of full-band noise by up to 0.02 dB.
        close(a.peak_db, "peak_db", 0.02);
        close(a.rms_db, "rms_db", 1e-3);
        close(a.crest, "crest", 5e-3);
        // Brightness averages every frame's centroid, and in near-silent frames the centroid is
        // that of rounding noise: librosa itself moves 1148.5 to 1149.8 on the loop when the
        // resampled input differs by 4e-7. Within 3% is the port's measure here.
        close(a.brightness, "brightness", 0.03);
        assert_eq!(a.onsets as u64, want["onsets"].as_u64().unwrap(), "{name}: onsets");
        assert_eq!(a.is_loop as u64, want["is_loop"].as_u64().unwrap(), "{name}: is_loop");
        assert_eq!(a.pitch_hz.is_some(), !want["pitch_hz"].is_null(), "{name}: pitch found");
        if let Some(p) = a.pitch_hz {
            close(p, "pitch_hz", 1e-3);
        }
        assert_eq!(a.tempo.is_some(), !want["tempo"].is_null(), "{name}: tempo found");
        if let Some(t) = a.tempo {
            close(t, "tempo", 1e-6);
        }
        let p = peaks(&d, &y);
        let wp: Vec<u8> = info["peaks"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u8).collect();
        let worst = p.iter().zip(&wp).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(worst <= 2, "{name}: peaks differ by {worst}");
    }
}
