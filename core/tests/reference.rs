//! Checks the Rust core against outputs of the Python engine (tests/reference, written by
//! packaging/make_reference.py).

use athar_core::features::{clap_features, FRAMES, N_MELS};
use ndarray::ArrayD;
use ndarray_npy::read_npy;
use std::path::PathBuf;

pub fn reference(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/reference").join(name)
}

pub const SOUNDS: [&str; 5] = ["kick_44k_mono", "tone_440_48k", "noise_burst_44k", "chirp_stereo_16s", "loop_120bpm_22k"];

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

fn model_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../build/model")
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[test]
fn embeddings_match_python() {
    let clap = athar_core::embed::Clap::load(&model_dir()).unwrap();
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

#[test]
fn files_match_python() {
    use athar_core::decode::{clips, decode, load_audio};
    let clap = athar_core::embed::Clap::load(&model_dir()).unwrap();
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
        let worst = y.iter().zip(want.iter()).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        let c = clips(&d, &y).unwrap();
        assert_eq!(c.len() as u64, info["clips"].as_u64().unwrap(), "{name}: clips");
        let got = clap.embed_sound(&c).unwrap();
        let emb: ndarray::Array1<f32> = read_npy(reference(&format!("{name}.embedding.npy"))).unwrap();
        let cos = cosine(&got, emb.as_slice().unwrap());
        println!("{name}: 48 kHz audio within {worst:.2e}, embedding cosine {cos:.6}");
        assert!(worst < 1e-4, "{name}: 48 kHz audio differs by {worst}");
        assert!(cos > 0.9999, "{name}: cosine {cos}");
    }
}
