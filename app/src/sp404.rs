//! What the SP-404MKII takes from its SD card: sounds as 48 kHz 16-bit WAVs in IMPORT, and
//! whole projects as PADCONF.BIN with one SMPL/BANKb-pp.SMP per used pad.
//!
//! Roland does not publish the project formats. The layouts follow the SparkyMK2 notes
//! (github.com/TuscanPL/SparkyMK2, docs/re/02-files.md and 03-parameters.md), checked against a
//! project exported by an SP-404MKII on firmware 5.x. All integers are big-endian. The files in
//! app/testdata that the tests compare with were written by Athar 1.x and loaded on the device.

use crate::export::{read_wav, wav16_bytes};
use athar_core::decode::{decode, resample};
use md5::{Digest, Md5};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

pub const PADS: usize = 160;
pub const SR: u32 = 48_000;
/// The longest sample the device holds: 16 minutes.
const MAX_SECONDS: usize = 960;
const SMP_HEADER: usize = 512;
const PADCONF_LEN: usize = 52_000;
const RECORDS: usize = 0xA0;
const NAMES: usize = 0x6C20;
const CHOPS: usize = 0x7B20;
const RECORD_WORDS: usize = 43;
const NAME_LEN: usize = 24;
const CHOP_WORDS: usize = 32;
pub const PROJECT_NAME_LEN: usize = 32;
const SIZE: usize = 0;
const START: usize = 1;
const END: usize = 2;
const BPM: usize = 9;
const LOOP_TOP: usize = 11;

/// Every empty pad in a device export holds this record. Field n is pad parameter 0x66 + n.
fn empty_pad() -> [i32; RECORD_WORDS] {
    let mut r = [0i32; RECORD_WORDS];
    r[..24].copy_from_slice(&[0, 0, 0, 127, 1, 0, 1, 0, 0, 9000, 0, 0, 0, 0, 0, 0, 10000, 1, 64, 0, 1, 2, 0, 100]);
    r
}

/// The 28 project setting words of a device export. Word 0 is the project tempo, words 12 to
/// 21 hold each bank's tempo shifted left by one, bit 0 for protect. The rest are kept as the
/// device wrote them.
fn settings(tempo: u32) -> [u32; 28] {
    let mut s = [9000, 0, 0, 64, 64, 64, 0, 1, 12, 2, 3, 13, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1179669, 2555904, 1638415, 1769477, 13];
    s[0] = tempo;
    for w in &mut s[12..22] {
        *w = tempo << 1;
    }
    s
}

/// The mounted SD card the device has formatted, if there is one.
pub fn card() -> Option<String> {
    card_in(Path::new("/Volumes"))
}

/// A card the SP-404MKII has formatted holds IMPORT and EXPORT at its root; one named for the
/// SP-404 wins when several are mounted.
pub fn card_in(volumes: &Path) -> Option<String> {
    let mut cards: Vec<String> = std::fs::read_dir(volumes)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("IMPORT").is_dir() && p.join("EXPORT").is_dir())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    cards.sort_by_key(|p| !p.rsplit('/').next().unwrap_or("").contains("404"));
    cards.into_iter().next()
}

/// Where sounds saved for the device go: it imports from IMPORT at the card's root, so that
/// folder is used when `dest` is the card, or EXPORT or EXPORT/PROJECT on it.
pub fn import_dir(dest: &Path) -> PathBuf {
    let named = |p: &Path, n: &str| p.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(n));
    let root = match dest.parent() {
        Some(up) if named(dest, "PROJECT") && named(up, "EXPORT") => up.parent().unwrap_or(up),
        Some(up) if named(dest, "EXPORT") => up,
        _ => dest,
    };
    let import = root.join("IMPORT");
    if import.is_dir() { import } else { root.to_path_buf() }
}

/// A sound as the SP-404MKII holds it: one or two channels at 48 kHz, at most 16 minutes,
/// dithered (TPDF, one 16-bit step peak to peak) so the cut to 16 bits adds noise instead of
/// distortion. `exact` says the samples already are 16-bit integers; they stay as they are
/// unless the sound has to be mixed down or resampled.
fn prepare(channels: &[Vec<f32>], sr: u32, mut exact: bool, mono: bool) -> Result<Vec<Vec<f32>>, String> {
    let mut chs: Vec<Vec<f32>> = if mono && channels.len() > 1 {
        exact = false;
        vec![athar_core::dsp::mix_mono(channels)]
    } else {
        channels.iter().take(2).cloned().collect()
    };
    if sr != SR {
        exact = false;
        chs = chs.iter().map(|c| resample(c, sr, SR)).collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    }
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15 ^ chs.first().map_or(0, |c| c.len() as u64);
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 40) as f32 / (1u64 << 24) as f32
    };
    for c in chs.iter_mut() {
        c.truncate(SR as usize * MAX_SECONDS);
        for v in c.iter_mut() {
            if !exact {
                *v += (rnd() - rnd()) / 32768.0;
            }
            *v = v.clamp(-1.0, 1.0);
        }
    }
    Ok(chs)
}

/// A sound file's samples as the device holds them (see `prepare`).
pub fn file_channels(path: &Path, mono: bool) -> Result<Vec<Vec<f32>>, String> {
    let d = decode(path).map_err(|e| e.to_string())?;
    let exact = hound::WavReader::open(path).is_ok_and(|r| r.spec().bits_per_sample == 16 && r.spec().sample_format == hound::SampleFormat::Int);
    prepare(&d.channels, d.sr, exact, mono)
}

/// Writes a rendered WAV as the file the device imports: 48 kHz, 16-bit.
pub fn write_wav(wav: &[u8], target: &Path, mono: bool) -> Result<(), String> {
    let wav = read_wav(wav)?;
    let chs = prepare(&wav.channels, wav.sr, wav.int16, mono)?;
    std::fs::write(target, wav16_bytes(&chs, SR)?).map_err(|e| format!("Save failed: {e}"))
}

pub fn ascii_text(s: &str, n: usize) -> String {
    s.nfkd().filter(|c| c.is_ascii()).map(|c| if (' '..='~').contains(&c) { c } else { '_' }).take(n).collect()
}

pub fn bank_file(pad: usize) -> String {
    format!("BANK{}-{:02}.SMP", pad / 16 + 1, pad % 16 + 1)
}

fn noise(n: usize) -> Vec<u8> {
    let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
    (0..n)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 32) as u8
        })
        .collect()
}

/// An SMP file from float samples, one or two channels at 48 kHz.
pub fn smp_bytes(chs: &[Vec<f32>]) -> Vec<u8> {
    let frames = chs.first().map_or(0, Vec::len);
    let mut pcm = Vec::with_capacity(frames * chs.len() * 2);
    for i in 0..frames {
        for c in chs {
            // Scaled by 32768, so 16-bit sources come back exact.
            let v = ((c[i] as f64) * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
            pcm.extend_from_slice(&v.to_be_bytes());
        }
    }
    let mut fields = Vec::with_capacity(16);
    for v in [(pcm.len() + SMP_HEADER - 8) as u32, SR, chs.len() as u32, 16] {
        fields.extend_from_slice(&v.to_be_bytes());
    }
    let mut head = b"RFWV".to_vec();
    head.extend_from_slice(&fields);
    head.extend_from_slice(&[0u8; 12]);
    head.extend_from_slice(&Md5::digest(&fields));
    let pad = SMP_HEADER - head.len();
    head.extend(noise(pad));
    head.extend(pcm);
    head
}

/// pads maps a pad number (0 = A1, 159 = J16) to (SMP file size, pad name, BPM).
pub fn padconf_bytes(name: &str, pads: &BTreeMap<usize, (usize, String, Option<f64>)>, tempo: f64) -> Vec<u8> {
    let mut b = vec![0u8; PADCONF_LEN];
    b[0..4].copy_from_slice(b"RFPD");
    b[4..8].copy_from_slice(&(RECORDS as u32).to_be_bytes());
    b[8..12].copy_from_slice(&[3, 0, 0, 0]);
    b[12..16].copy_from_slice(&0x7A80u32.to_be_bytes());
    for (i, w) in settings((tempo * 100.0).round() as u32).iter().enumerate() {
        b[0x10 + i * 4..0x14 + i * 4].copy_from_slice(&w.to_be_bytes());
    }
    let pname = ascii_text(name, PROJECT_NAME_LEN);
    b[0x80..0x80 + pname.len()].copy_from_slice(pname.as_bytes());
    for pad in 0..PADS {
        let mut rec = empty_pad();
        let mut label = vec![b' '; NAME_LEN - 1];
        if let Some((size, pn, bpm)) = pads.get(&pad) {
            rec[SIZE] = *size as i32;
            rec[END] = *size as i32;
            rec[START] = SMP_HEADER as i32;
            rec[LOOP_TOP] = SMP_HEADER as i32;
            if let Some(bpm) = bpm.filter(|b| (40.0..=200.0).contains(b)) {
                rec[BPM] = (bpm * 100.0).round() as i32;
            }
            label = ascii_text(pn, NAME_LEN - 1).into_bytes();
            label.resize(NAME_LEN - 1, 0);
        }
        let at = RECORDS + pad * RECORD_WORDS * 4;
        for (i, v) in rec.iter().enumerate() {
            b[at + i * 4..at + i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        let n = NAMES + pad * NAME_LEN;
        b[n..n + NAME_LEN - 1].copy_from_slice(&label);
        b[n + NAME_LEN - 1] = 0;
        let c = CHOPS + pad * CHOP_WORDS * 4;
        for i in 0..CHOP_WORDS {
            let v: i32 = if i < 16 { -1 } else { 0 };
            b[c + i * 4..c + i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
    }
    b
}

pub const PICTURES: [&str; 6] = ["startup_1", "startup_2", "screen_saver_1", "screen_saver_2", "screen_saver_3", "screen_saver_4"];

/// A 128 x 64, 1-bit BMP for the device screen, every pixel dark.
pub fn blank_picture() -> Vec<u8> {
    let mut b = b"BM".to_vec();
    for v in [1086u32, 0, 62, 40] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&128i32.to_le_bytes());
    b.extend_from_slice(&64i32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    for v in [0u32, 0, 0, 0, 0, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&[0, 0, 0, 0, 255, 255, 255, 0]);
    b.extend(vec![0u8; 1024]);
    b
}

/// The project name in a PADCONF.BIN, if it is one.
pub fn padconf_name(data: &[u8]) -> Option<String> {
    if data.len() < PADCONF_LEN || &data[..4] != b"RFPD" || data[8] != 3 {
        return None;
    }
    let raw = &data[0x80..0x80 + PROJECT_NAME_LEN];
    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    Some(String::from_utf8_lossy(&raw[..end]).trim().to_string())
}

/// Each used pad's SMP size and label, read back from a PADCONF.BIN.
#[cfg(test)]
pub fn pad_labels(data: &[u8]) -> BTreeMap<usize, (usize, String)> {
    let word = |at: usize| i32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
    (0..PADS)
        .filter_map(|pad| {
            let size = word(RECORDS + pad * RECORD_WORDS * 4 + SIZE * 4);
            let n = NAMES + pad * NAME_LEN;
            let raw = &data[n..n + NAME_LEN - 1];
            let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
            (size > 0).then(|| (pad, (size as usize, String::from_utf8_lossy(&raw[..end]).to_string())))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{wav_bytes, Tmp};

    const REFERENCE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata");

    #[test]
    fn smp_header_digest() {
        let d = smp_bytes(&[vec![0.5, -0.5, 0.0]]);
        assert_eq!(&d[..4], b"RFWV");
        assert_eq!(&d[0x20..0x30], Md5::digest(&d[4..20]).as_slice());
        assert_eq!(u32::from_be_bytes([d[4], d[5], d[6], d[7]]) as usize + 8, d.len());
        assert_eq!(&d[512..514], &16384i16.to_be_bytes());
    }

    #[test]
    fn padconf_round_trip() {
        let mut pads = BTreeMap::new();
        pads.insert(0, (1000, "Kick".into(), Some(120.0)));
        let b = padconf_bytes("Night drive", &pads, 90.0);
        assert_eq!(b.len(), PADCONF_LEN);
        assert_eq!(padconf_name(&b).as_deref(), Some("Night drive"));
        assert_eq!(bank_file(16), "BANK2-01.SMP");
    }

    #[test]
    fn padconf_matches_the_reference_file() {
        let mut pads = BTreeMap::new();
        pads.insert(0, (1000, "Kick Deep".to_string(), Some(120.0)));
        pads.insert(17, (5000, "Snare é long name that goes on".to_string(), None));
        pads.insert(159, (64, "Last".to_string(), Some(250.0)));
        let want = std::fs::read(format!("{REFERENCE}/padconf.bin")).unwrap();
        assert_eq!(padconf_bytes("Night drive é", &pads, 92.5), want);
    }

    #[test]
    fn the_blank_picture_matches_the_reference_file() {
        assert_eq!(blank_picture(), std::fs::read(format!("{REFERENCE}/picture.bmp")).unwrap());
    }

    #[test]
    fn smp_matches_the_reference_file_apart_from_its_padding() {
        let want = std::fs::read(format!("{REFERENCE}/smp.bin")).unwrap();
        let got = smp_bytes(&[vec![0.5, -0.25, 0.999, -1.0], vec![0.1, 0.2, 0.3, 0.4]]);
        assert_eq!(got.len(), want.len());
        assert_eq!(got[..0x30], want[..0x30]);
        assert_eq!(got[512..], want[512..]);
    }

    #[test]
    fn a_wav_for_the_device_is_16_bit_48k_and_dithered() {
        let t = Tmp::new();
        let out = t.0.join("out.wav");
        let quiet: Vec<f32> = (0..44_100).map(|i| 0.4 / 32768.0 * (std::f32::consts::TAU * 440.0 * i as f32 / 44_100.0).sin()).collect();
        write_wav(&wav_bytes(&quiet, 44_100, 32), &out, false).unwrap();
        let mut r = hound::WavReader::open(&out).unwrap();
        let spec = r.spec();
        assert_eq!((spec.sample_rate, spec.bits_per_sample, spec.sample_format), (48_000, 16, hound::SampleFormat::Int));
        assert!(r.samples::<i16>().any(|s| s.unwrap() != 0), "a tone below one step is kept as dithered signal");
        assert_eq!(std::fs::read_dir(&t.0).unwrap().count(), 1, "nothing else is left beside it");
    }

    #[test]
    fn a_16_bit_48k_sound_reaches_the_device_unchanged() {
        let t = Tmp::new();
        let out = t.0.join("out.wav");
        // Quiet and loud samples: every level must come back as it was.
        let exact: Vec<i16> = (0..48_000).map(|i| ((i * 37) % 65_536 - 32_768) as i16).collect();
        let src = wav16_bytes(&[exact.iter().map(|&v| v as f32 / 32768.0).collect()], SR).unwrap();
        write_wav(&src, &out, false).unwrap();
        let got: Vec<i16> = hound::WavReader::open(&out).unwrap().samples::<i16>().map(Result::unwrap).collect();
        assert_eq!(got, exact);
        std::fs::write(t.0.join("in.wav"), &src).unwrap();
        let held = file_channels(&t.0.join("in.wav"), false).unwrap();
        assert_eq!(held[0].iter().map(|v| (v * 32768.0) as i16).collect::<Vec<_>>(), exact);
    }

    #[test]
    fn the_card_is_the_volume_with_import_and_export() {
        let t = Tmp::new();
        assert_eq!(card_in(&t.0), None);
        std::fs::create_dir_all(t.0.join("Backup/Photos")).unwrap();
        for v in ["CARD", "SP404MKII"] {
            std::fs::create_dir_all(t.0.join(v).join("IMPORT")).unwrap();
            std::fs::create_dir(t.0.join(v).join("EXPORT")).unwrap();
        }
        let card = t.0.join("SP404MKII");
        assert_eq!(card_in(&t.0), Some(card.to_string_lossy().into_owned()));
        for inside in ["", "EXPORT", "EXPORT/PROJECT", "export/project"] {
            assert_eq!(import_dir(&card.join(inside)), card.join("IMPORT"), "from {inside:?}");
        }
        assert_eq!(import_dir(&t.0.join("Backup")), t.0.join("Backup"));
    }
}
