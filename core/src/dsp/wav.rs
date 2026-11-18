//! WAV encoding, and the render that ends every export.

use super::{PrepParams, mix_mono, num, prep, resample_sinc, round_half_up};
use serde_json::Value;
use std::borrow::Cow;

/// The speaker positions WAVE_FORMAT_EXTENSIBLE names for a channel count (0: unspecified).
fn channel_mask(channels: usize) -> u32 {
    match channels {
        1 => 0x4,
        2 => 0x3,
        4 => 0x33,
        6 => 0x3f,
        8 => 0x63f,
        _ => 0,
    }
}

fn random_seed() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.finish() as u32
}

/// WAV file bytes: 16-bit PCM with triangular dither, 24-bit PCM, or 32-bit float.
pub fn encode_wav(channels: &[Vec<f32>], sr: u32, bits: u32) -> Result<Vec<u8>, String> {
    encode_wav_seeded(channels, sr, bits, random_seed())
}

/// `encode_wav` with the state of the dither's xorshift generator given.
fn encode_wav_seeded(channels: &[Vec<f32>], sr: u32, bits: u32, seed: u32) -> Result<Vec<u8>, String> {
    if !matches!(bits, 16 | 24 | 32) {
        return Err(format!("unsupported bit depth {bits}"));
    }
    if channels.is_empty() {
        return Err("no channels".into());
    }
    let (n, ch, bps, float) = (channels[0].len(), channels.len(), (bits / 8) as usize, bits == 32);
    // Float samples and more than two channels need the extensible header and a fact chunk.
    let extensible = float || ch > 2;
    let size = n * ch * bps;
    let pad = size & 1;
    let head = if extensible { 80 } else { 44 };
    let mut v = Vec::with_capacity(head + size + pad);
    let u16le = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    let u32le = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    v.extend_from_slice(b"RIFF");
    u32le(&mut v, (head - 8 + size + pad) as u32);
    v.extend_from_slice(b"WAVEfmt ");
    u32le(&mut v, if extensible { 40 } else { 16 });
    u16le(&mut v, if extensible { 0xfffe } else { 1 });
    u16le(&mut v, ch as u16);
    u32le(&mut v, sr);
    u32le(&mut v, sr.wrapping_mul((ch * bps) as u32));
    u16le(&mut v, (ch * bps) as u16);
    u16le(&mut v, bits as u16);
    if extensible {
        u16le(&mut v, 22);
        u16le(&mut v, bits as u16);
        u32le(&mut v, channel_mask(ch));
        // The sub-format GUID: PCM (1) or IEEE float (3), then the fixed tail.
        u32le(&mut v, if float { 3 } else { 1 });
        u16le(&mut v, 0);
        u16le(&mut v, 0x10);
        v.extend_from_slice(&[0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71]);
        v.extend_from_slice(b"fact");
        u32le(&mut v, 4);
        u32le(&mut v, n as u32);
    }
    v.extend_from_slice(b"data");
    u32le(&mut v, size as u32);
    if float {
        for i in 0..n {
            for d in channels {
                v.extend_from_slice(&d[i].to_le_bytes());
            }
        }
    } else if bits == 16 {
        let mut r = seed.max(1);
        let mut rand = || {
            r ^= r << 13;
            r ^= r >> 17;
            r ^= r << 5;
            r as f64 / 4294967296.0
        };
        for i in 0..n {
            for d in channels {
                let dither = (rand() - rand()) / 32768.0;
                let x = (d[i] as f64 + dither).clamp(-1.0, 1.0);
                let q = round_half_up(if x < 0.0 { x * 32768.0 } else { x * 32767.0 }) as i16;
                v.extend_from_slice(&q.to_le_bytes());
            }
        }
    } else {
        for i in 0..n {
            for d in channels {
                let x = (d[i] as f64).clamp(-1.0, 1.0);
                let q = round_half_up(if x < 0.0 { x * 8388608.0 } else { x * 8388607.0 }) as i32;
                v.extend_from_slice(&q.to_le_bytes()[..3]);
            }
        }
    }
    v.resize(head + size + pad, 0);
    Ok(v)
}

/// How a sound is written to a file.
#[derive(Debug, Clone, Copy)]
pub struct RenderArgs {
    /// The file's sample rate.
    pub rate: u32,
    pub bits: u32,
    /// Fold all channels into one.
    pub mono: bool,
    /// Prepare the sound for a sampler first (see `prep`).
    pub prep: Option<PrepParams>,
}

impl RenderArgs {
    /// Reads the export settings the page sends: `rate` (missing or 0 keeps `sr`), `bits`
    /// (24 when missing), `mono`, and `prep` as an object of `PrepParams` keys or null.
    pub fn from_json(p: &Value, sr: u32) -> Self {
        RenderArgs {
            rate: num(p, "rate").map(|r| r as u32).filter(|&r| r > 0).unwrap_or(sr),
            bits: num(p, "bits").map_or(24, |b| b as u32),
            mono: p.get("mono").and_then(Value::as_bool).unwrap_or(false),
            prep: p.get("prep").filter(|v| v.is_object()).map(PrepParams::from_json),
        }
    }
}

/// A sound at rate `sr` as WAV bytes: folded to mono if asked, prepared, converted to the
/// file's rate, encoded.
pub fn render_wav(chs: &[Vec<f32>], sr: u32, a: &RenderArgs) -> Result<Vec<u8>, String> {
    if chs.is_empty() {
        return Err("no channels".into());
    }
    if a.rate == 0 {
        return Err("unsupported sample rate 0".into());
    }
    let mut c = Cow::Borrowed(chs);
    if a.mono && chs.len() > 1 {
        c = Cow::Owned(vec![mix_mono(chs)]);
    }
    if let Some(p) = &a.prep {
        c = Cow::Owned(prep(&c, sr, p).0);
    }
    if a.rate != sr {
        c = Cow::Owned(resample_sinc(&c, sr, a.rate));
    }
    encode_wav(&c, a.rate, a.bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn u32_at(v: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([v[at], v[at + 1], v[at + 2], v[at + 3]])
    }

    #[test]
    fn pcm_headers_and_sizes() {
        let w = encode_wav(&[vec![0f32; 5]], 44100, 16).unwrap();
        assert_eq!(w.len(), 44 + 10);
        assert_eq!((&w[..4], &w[8..16], &w[36..40]), (&b"RIFF"[..], &b"WAVEfmt "[..], &b"data"[..]));
        assert_eq!(u32_at(&w, 4) as usize, w.len() - 8);
        assert_eq!(u32_at(&w, 24), 44100);
        assert_eq!(u32_at(&w, 40), 10);
        // Three 24-bit mono samples are 9 bytes, padded to an even chunk.
        let w = encode_wav(&[vec![0f32; 3]], 48000, 24).unwrap();
        assert_eq!(w.len(), 44 + 9 + 1);
        assert_eq!(u32_at(&w, 40), 9);
        assert_eq!(u32_at(&w, 4) as usize, w.len() - 8);
        let w = encode_wav(&[vec![0f32; 4], vec![0f32; 4]], 48000, 24).unwrap();
        assert_eq!(w.len(), 44 + 24);
        assert_eq!(u32_at(&w, 28), 48000 * 6);
    }

    #[test]
    fn float_and_multichannel_files_use_the_extensible_header() {
        let w = encode_wav(&[vec![0.25f32, -1.5]], 48000, 32).unwrap();
        assert_eq!(w.len(), 80 + 8);
        assert_eq!(u16::from_le_bytes([w[20], w[21]]), 0xfffe);
        assert_eq!(&w[60..64], b"fact");
        assert_eq!(u32_at(&w, 68), 2);
        assert_eq!(&w[72..76], b"data");
        assert_eq!(f32::from_le_bytes([w[80], w[81], w[82], w[83]]), 0.25);
        assert_eq!(f32::from_le_bytes([w[84], w[85], w[86], w[87]]), -1.5);
        assert_eq!(encode_wav(&vec![vec![0f32; 2]; 4], 48000, 16).unwrap().len(), 80 + 16);
    }

    #[test]
    fn samples_are_quantized_to_full_scale() {
        let w = encode_wav(&[vec![1.0f32, -1.0, 0.0, 2.0]], 48000, 24).unwrap();
        assert_eq!(&w[44..56], &[0xff, 0xff, 0x7f, 0x00, 0x00, 0x80, 0, 0, 0, 0xff, 0xff, 0x7f]);
    }

    #[test]
    fn dither_follows_its_seed_and_stays_within_one_step() {
        let chs = [vec![0.1f32; 64]];
        let a = encode_wav_seeded(&chs, 48000, 16, 7).unwrap();
        assert_eq!(a, encode_wav_seeded(&chs, 48000, 16, 7).unwrap());
        assert_ne!(a, encode_wav_seeded(&chs, 48000, 16, 8).unwrap());
        for s in a[44..].chunks_exact(2) {
            let q = i16::from_le_bytes([s[0], s[1]]) as f64;
            assert!((q - 0.1f32 as f64 * 32767.0).abs() <= 1.5, "{q}");
        }
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(encode_wav(&[vec![0f32; 4]], 48000, 8).is_err());
        assert!(encode_wav(&[], 48000, 16).is_err());
        let args = RenderArgs { rate: 0, bits: 24, mono: false, prep: None };
        assert!(render_wav(&[vec![0f32; 4]], 48000, &args).is_err());
        assert!(render_wav(&[], 48000, &RenderArgs { rate: 48000, ..args }).is_err());
    }

    #[test]
    fn render_folds_resamples_and_encodes() {
        let chs = vec![vec![0.5f32; 480], vec![-0.5f32; 480]];
        let args = RenderArgs { rate: 24000, bits: 32, mono: true, prep: None };
        let w = render_wav(&chs, 48000, &args).unwrap();
        assert_eq!(w.len(), 80 + 240 * 4);
        assert_eq!(u32_at(&w, 24), 24000);
        assert_eq!(u16::from_le_bytes([w[22], w[23]]), 1);
        let plain = render_wav(&chs, 48000, &RenderArgs { rate: 48000, bits: 24, mono: false, prep: None }).unwrap();
        assert_eq!(plain, encode_wav(&chs, 48000, 24).unwrap());
    }

    #[test]
    fn render_settings_from_the_page() {
        let a = RenderArgs::from_json(&json!({"rate": 0, "bits": 16, "mono": true, "prep": null}), 44100);
        assert_eq!((a.rate, a.bits, a.mono, a.prep.is_none()), (44100, 16, true, true));
        let a = RenderArgs::from_json(&json!({"rate": 48000, "prep": {"targetDb": -3}}), 44100);
        assert_eq!((a.rate, a.bits, a.mono), (48000, 24, false));
        assert_eq!(a.prep.unwrap().target_db, -3.0);
    }
}
