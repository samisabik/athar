//! Audio files written or handed to the page: decoded previews and SP-404MKII exports.

use athar_core::decode::{decode, resample};
use std::io::Cursor;
use std::path::{Path, PathBuf};

pub const PREVIEW_MAX_SECONDS: usize = 600;
const SP404_SR: u32 = 48_000;
const SP404_MAX_SECONDS: usize = 960;

fn wav_bytes(channels: &[Vec<f32>], sr: u32, bits16: bool) -> Result<Vec<u8>, String> {
    let spec = hound::WavSpec {
        channels: channels.len() as u16,
        sample_rate: sr,
        bits_per_sample: if bits16 { 16 } else { 32 },
        sample_format: if bits16 { hound::SampleFormat::Int } else { hound::SampleFormat::Float },
    };
    let mut cur = Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(&mut cur, spec).map_err(|e| e.to_string())?;
        let frames = channels.first().map_or(0, Vec::len);
        for i in 0..frames {
            for c in channels {
                if bits16 {
                    w.write_sample((c[i].clamp(-1.0, 1.0) * 32767.0).round() as i16).map_err(|e| e.to_string())?;
                } else {
                    w.write_sample(c[i]).map_err(|e| e.to_string())?;
                }
            }
        }
        w.finalize().map_err(|e| e.to_string())?;
    }
    Ok(cur.into_inner())
}

pub fn sp404_card() -> Option<String> {
    let mut cards: Vec<String> = std::fs::read_dir("/Volumes")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("IMPORT").is_dir() && p.join("EXPORT").is_dir())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    cards.sort_by_key(|p| !p.rsplit('/').next().unwrap_or("").contains("404"));
    cards.into_iter().next()
}

/// The SP-404MKII imports from IMPORT at the SD card root; use it when present.
pub fn sp404_dir(dest: &Path) -> PathBuf {
    let up = |p: &Path, n: &str| p.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(n));
    let mut d = dest.to_path_buf();
    if up(&d, "PROJECT") && d.parent().is_some_and(|p| up(p, "EXPORT")) {
        d = d.parent().unwrap().parent().unwrap().to_path_buf();
    } else if up(&d, "EXPORT") {
        d = d.parent().unwrap().to_path_buf();
    }
    let imp = d.join("IMPORT");
    if imp.is_dir() { imp } else { d }
}

/// A sound as the SP-404MKII holds it: one or two channels at 48 kHz, at most 16 minutes,
/// dithered (TPDF, one 16-bit step peak to peak) unless it already is 16-bit 48 kHz, so the cut
/// to 16 bits adds noise instead of distortion.
pub fn sp404_channels(path: &Path, mono: bool) -> Result<Vec<Vec<f32>>, String> {
    let d = decode(path).map_err(|e| e.to_string())?;
    let mut exact = hound::WavReader::open(path).map(|r| r.spec().bits_per_sample <= 16 && r.spec().sample_format == hound::SampleFormat::Int).unwrap_or(false);
    let mut chs: Vec<Vec<f32>> = if mono && d.channels.len() > 1 {
        exact = false;
        vec![d.mono(0, d.frames())]
    } else {
        d.channels.iter().take(2).cloned().collect()
    };
    if d.sr != SP404_SR {
        exact = false;
        chs = chs.iter().map(|c| resample(c, d.sr, SP404_SR)).collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    }
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15 ^ chs.first().map_or(0, |c| c.len() as u64);
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 40) as f32 / (1u64 << 24) as f32
    };
    for c in chs.iter_mut() {
        c.truncate(SP404_SR as usize * SP404_MAX_SECONDS);
        for v in c.iter_mut() {
            if !exact {
                *v += (rnd() - rnd()) / 32768.0;
            }
            *v = v.clamp(-1.0, 1.0);
        }
    }
    Ok(chs)
}

/// 48 kHz, 16-bit PCM WAV, the format the SP-404MKII imports from SD card.
pub fn write_sp404(wav: &[u8], target: &Path, mono: bool) -> Result<(), String> {
    let tmp = target.with_extension("tmp.wav");
    std::fs::write(&tmp, wav).map_err(|e| e.to_string())?;
    let chs = sp404_channels(&tmp, mono);
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(target, wav_bytes(&chs?, SP404_SR, true)?).map_err(|e| format!("Save failed: {e}"))
}

/// The editor's WAV rewritten as FLAC (16-bit when the WAV is, otherwise 24) or MP3 (320 kbps).
pub fn transcode(wav: &[u8], format: &str) -> Result<Vec<u8>, String> {
    let mut r = hound::WavReader::new(Cursor::new(wav)).map_err(|e| e.to_string())?;
    let spec = r.spec();
    let ch = spec.channels as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>().map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            let scale = (1u64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().map(|s| s.map(|v| v as f32 / scale)).collect::<Result<_, _>>().map_err(|e| e.to_string())?
        }
    };
    match format {
        "flac" => {
            // macOS's own FLAC encoder, from a 16-bit or 24-bit WAV.
            let bits = if spec.sample_format == hound::SampleFormat::Int && spec.bits_per_sample == 16 { 16 } else { 24 };
            let out_spec = hound::WavSpec { channels: spec.channels, sample_rate: spec.sample_rate, bits_per_sample: bits, sample_format: hound::SampleFormat::Int };
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            let tmp = std::env::temp_dir().join(format!("athar-{}-{stamp}", std::process::id()));
            let (wav_tmp, flac_tmp) = (tmp.with_extension("wav"), tmp.with_extension("flac"));
            let result = (|| {
                let mut w = hound::WavWriter::create(&wav_tmp, out_spec).map_err(|e| e.to_string())?;
                let full = ((1i64 << (bits - 1)) - 1) as f32;
                for v in &samples {
                    w.write_sample((v.clamp(-1.0, 1.0) * full).round() as i32).map_err(|e| e.to_string())?;
                }
                w.finalize().map_err(|e| e.to_string())?;
                let ok = std::process::Command::new("afconvert").args(["-f", "flac", "-d", "flac"]).arg(&wav_tmp).arg(&flac_tmp).status().map(|s| s.success()).unwrap_or(false);
                if !ok {
                    return Err("FLAC encoding failed".to_string());
                }
                std::fs::read(&flac_tmp).map_err(|e| e.to_string())
            })();
            let _ = std::fs::remove_file(&wav_tmp);
            let _ = std::fs::remove_file(&flac_tmp);
            result
        }
        "mp3" => {
            use mp3lame_encoder::{Bitrate, Builder, DualPcm, FlushNoGap, MonoPcm, Quality};
            let mut b = Builder::new().ok_or("MP3 encoder unavailable")?;
            let stereo = ch >= 2;
            b.set_num_channels(if stereo { 2 } else { 1 }).map_err(|e| format!("{e:?}"))?;
            b.set_sample_rate(spec.sample_rate).map_err(|e| format!("{e:?}"))?;
            b.set_brate(Bitrate::Kbps320).map_err(|e| format!("{e:?}"))?;
            b.set_quality(Quality::Best).map_err(|e| format!("{e:?}"))?;
            let mut enc = b.build().map_err(|e| format!("{e:?}"))?;
            let frames = samples.len() / ch.max(1);
            let clip = |v: f32| v.clamp(-1.0, 1.0);
            let left: Vec<f32> = (0..frames).map(|i| clip(samples[i * ch])).collect();
            let mut out = Vec::with_capacity(mp3lame_encoder::max_required_buffer_size(frames));
            let n = if stereo {
                let right: Vec<f32> = (0..frames).map(|i| clip(samples[i * ch + 1])).collect();
                enc.encode(DualPcm { left: &left, right: &right }, out.spare_capacity_mut())
            } else {
                enc.encode(MonoPcm(&left), out.spare_capacity_mut())
            }
            .map_err(|e| format!("{e:?}"))?;
            unsafe { out.set_len(n) };
            enc.flush_to_vec::<FlushNoGap>(&mut out).map_err(|e| format!("{e:?}"))?;
            Ok(out)
        }
        f => Err(format!("Cannot save as {f}")),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn transcodes_decode_back() {
        let sr = 44_100u32;
        let tone: Vec<f32> = (0..sr as usize).map(|i| 0.5 * (i as f32 * 440.0 * std::f32::consts::TAU / sr as f32).sin()).collect();
        let wav = super::wav_bytes(&[tone.clone(), tone.clone()], sr, true).unwrap();
        for fmt in ["flac", "mp3"] {
            let bytes = super::transcode(&wav, fmt).unwrap();
            let tmp = std::env::temp_dir().join(format!("athar_transcode_test.{fmt}"));
            std::fs::write(&tmp, &bytes).unwrap();
            let d = athar_core::decode::decode(&tmp).unwrap_or_else(|e| panic!("{fmt}: {e}"));
            let _ = std::fs::remove_file(&tmp);
            assert_eq!(d.channels.len(), 2, "{fmt}");
            assert_eq!(d.sr, sr, "{fmt}");
            let peak = d.channels[0].iter().fold(0.0f32, |a, v| a.max(v.abs()));
            assert!((peak - 0.5).abs() < 0.02, "{fmt}: peak {peak}");
            if fmt == "flac" {
                assert_eq!(d.frames(), tone.len());
                let worst = d.channels[0].iter().zip(&tone).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
                assert!(worst < 1e-4, "flac differs by {worst}");
            }
        }
    }
}
