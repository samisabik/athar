//! SP-404MKII project files: PADCONF.BIN and one SMPL/BANKb-pp.SMP per used pad.
//!
//! Roland does not publish these formats. The layouts follow the SparkyMK2 notes
//! (github.com/TuscanPL/SparkyMK2, docs/re/02-files.md and 03-parameters.md), checked against a
//! project exported by an SP-404MKII on firmware 5.x. All integers are big-endian.

use md5::{Digest, Md5};
use std::collections::BTreeMap;
use unicode_normalization::UnicodeNormalization;

pub const PADS: usize = 160;
pub const SR: u32 = 48_000;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}

#[cfg(test)]
mod python_parity {
    use super::*;
    const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata");

    #[test]
    fn padconf_matches_python() {
        let mut pads = BTreeMap::new();
        pads.insert(0, (1000, "Kick Deep".to_string(), Some(120.0)));
        pads.insert(17, (5000, "Snare é long name that goes on".to_string(), None));
        pads.insert(159, (64, "Last".to_string(), Some(250.0)));
        let want = std::fs::read(format!("{DIR}/padconf.bin")).unwrap();
        assert_eq!(padconf_bytes("Night drive é", &pads, 92.5), want);
    }

    #[test]
    fn picture_matches_python() {
        assert_eq!(blank_picture(), std::fs::read(format!("{DIR}/picture.bmp")).unwrap());
    }

    #[test]
    fn smp_matches_python_apart_from_padding() {
        let want = std::fs::read(format!("{DIR}/smp.bin")).unwrap();
        let got = smp_bytes(&[vec![0.5, -0.25, 0.999, -1.0], vec![0.1, 0.2, 0.3, 0.4]]);
        assert_eq!(got.len(), want.len());
        assert_eq!(got[..0x30], want[..0x30]);
        assert_eq!(got[512..], want[512..]);
    }
}
