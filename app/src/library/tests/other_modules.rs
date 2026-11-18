//! Saving a rendered sound into the library, kept here for the library on a temporary folder it needs.

use athar_core::decode::decode;
use serde_json::json;
use std::path::Path;
use super::{lab, wav_bytes};

#[test]
fn saving_writes_flac_mp3_and_wav() {
    let l = lab();
    let dest = l.root.join("saved");
    std::fs::create_dir(&dest).unwrap();
    let tone: Vec<f32> = (0..48_000).map(|i| 0.3 * (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin()).collect();
    let wav = wav_bytes(&tone, 48_000, 24);
    for (fmt, magic) in [("flac", &b"fLaC"[..]), ("mp3", &b"ID3"[..]), ("wav", &b"RIFF"[..])] {
        let path = crate::commands::save::save_wav(&l.lib, &wav, &json!({"dest_dir": dest, "name": "tone.wav", "format": fmt})).unwrap();
        assert_eq!(path, dest.join(format!("tone.{fmt}")).to_string_lossy());
        let bytes = std::fs::read(&path).unwrap();
        let mpeg_frame = fmt == "mp3" && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0;
        assert!(bytes.starts_with(magic) || mpeg_frame, "{fmt} file");
        let d = decode(Path::new(&path)).unwrap();
        assert_eq!(d.sr, 48_000, "{fmt}");
        assert!((d.duration() - 1.0).abs() < 0.06, "{fmt} lasts {}", d.duration());
    }
    // STREAMINFO: 20 bits of rate, 3 of channels, 5 of bits per sample.
    let flac = std::fs::read(dest.join("tone.flac")).unwrap();
    assert_eq!((((flac[20] & 1) << 4) | (flac[21] >> 4)) + 1, 24);
    assert!(crate::commands::save::save_wav(&l.lib, &wav, &json!({"dest_dir": dest, "name": "x", "format": "exe"})).is_err());
}
