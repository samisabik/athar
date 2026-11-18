//! Rendered sounds written in other forms: FLAC and MP3 files, and the 16-bit WAV the SP-404MKII
//! imports. Renders arrive as WAV bytes from `athar_core::dsp::render_wav`.

use std::io::Cursor;

/// A WAV file's samples, one vector per channel.
pub struct Wav {
    pub channels: Vec<Vec<f32>>,
    pub sr: u32,
    /// Whether the file holds 16-bit integers, which a 16-bit copy keeps exactly.
    pub int16: bool,
}

pub fn read_wav(wav: &[u8]) -> Result<Wav, String> {
    let mut r = hound::WavReader::new(Cursor::new(wav)).map_err(|e| e.to_string())?;
    let spec = r.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>().map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            let scale = (1u64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().map(|s| s.map(|v| v as f32 / scale)).collect::<Result<_, _>>().map_err(|e| e.to_string())?
        }
    };
    let n = (spec.channels as usize).max(1);
    let channels = (0..n).map(|c| samples.iter().skip(c).step_by(n).copied().collect()).collect();
    Ok(Wav { channels, sr: spec.sample_rate, int16: spec.sample_format == hound::SampleFormat::Int && spec.bits_per_sample == 16 })
}

/// A WAV of integers `bits` deep. Full scale is 2^(bits - 1), as decoders read it, so samples
/// that came from integers of that depth are written back unchanged.
fn write_int_wav<W: std::io::Write + std::io::Seek>(to: W, channels: &[Vec<f32>], sr: u32, bits: u16) -> Result<(), String> {
    let spec = hound::WavSpec { channels: channels.len() as u16, sample_rate: sr, bits_per_sample: bits, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::new(to, spec).map_err(|e| e.to_string())?;
    let full = (1i64 << (bits - 1)) as f64;
    for i in 0..channels.first().map_or(0, Vec::len) {
        for c in channels {
            w.write_sample((c[i] as f64 * full).round().clamp(-full, full - 1.0) as i32).map_err(|e| e.to_string())?;
        }
    }
    w.finalize().map_err(|e| e.to_string())
}

pub fn wav16_bytes(channels: &[Vec<f32>], sr: u32) -> Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    write_int_wav(&mut out, channels, sr, 16)?;
    Ok(out.into_inner())
}

/// A rendered WAV rewritten as FLAC (16-bit when the WAV is, otherwise 24) or MP3 (320 kbps).
pub fn transcode(wav: &[u8], format: &str) -> Result<Vec<u8>, String> {
    let wav = read_wav(wav)?;
    match format {
        "flac" => flac(&wav),
        "mp3" => mp3(&wav),
        f => Err(format!("Cannot save as {f}")),
    }
}

/// Encoded by macOS's own afconvert, from a WAV in the temporary folder.
fn flac(wav: &Wav) -> Result<Vec<u8>, String> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("athar-{}-{stamp}", std::process::id()));
    let (wav_tmp, flac_tmp) = (tmp.with_extension("wav"), tmp.with_extension("flac"));
    let result = (|| {
        let file = std::io::BufWriter::new(std::fs::File::create(&wav_tmp).map_err(|e| e.to_string())?);
        write_int_wav(file, &wav.channels, wav.sr, if wav.int16 { 16 } else { 24 })?;
        let ok = std::process::Command::new("/usr/bin/afconvert").args(["-f", "flac", "-d", "flac"]).arg(&wav_tmp).arg(&flac_tmp).status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            return Err("FLAC encoding failed".to_string());
        }
        std::fs::read(&flac_tmp).map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_file(&wav_tmp);
    let _ = std::fs::remove_file(&flac_tmp);
    result
}

fn mp3(wav: &Wav) -> Result<Vec<u8>, String> {
    use mp3lame_encoder::{Bitrate, Builder, DualPcm, FlushNoGap, MonoPcm, Quality};
    let failed = |e: &dyn std::fmt::Debug| format!("MP3 encoding failed: {e:?}");
    let clipped = |c: &[f32]| -> Vec<f32> { c.iter().map(|v| v.clamp(-1.0, 1.0)).collect() };
    let left = clipped(wav.channels.first().ok_or("Nothing to encode")?);
    let right = wav.channels.get(1).map(|c| clipped(c));
    let mut b = Builder::new().ok_or("MP3 encoder unavailable")?;
    b.set_num_channels(if right.is_some() { 2 } else { 1 }).map_err(|e| failed(&e))?;
    b.set_sample_rate(wav.sr).map_err(|e| failed(&e))?;
    b.set_brate(Bitrate::Kbps320).map_err(|e| failed(&e))?;
    b.set_quality(Quality::Best).map_err(|e| failed(&e))?;
    let mut enc = b.build().map_err(|e| failed(&e))?;
    let mut out = Vec::with_capacity(mp3lame_encoder::max_required_buffer_size(left.len()));
    let n = match &right {
        Some(right) => enc.encode(DualPcm { left: &left, right }, out.spare_capacity_mut()),
        None => enc.encode(MonoPcm(&left), out.spare_capacity_mut()),
    }
    .map_err(|e| failed(&e))?;
    // The encoder wrote `n` bytes into the spare capacity it was given.
    unsafe { out.set_len(n) };
    enc.flush_to_vec::<FlushNoGap>(&mut out).map_err(|e| failed(&e))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::Tmp;

    #[test]
    fn transcodes_decode_back() {
        let sr = 44_100u32;
        let tone: Vec<f32> = (0..sr as usize).map(|i| 0.5 * (i as f32 * 440.0 * std::f32::consts::TAU / sr as f32).sin()).collect();
        let wav = wav16_bytes(&[tone.clone(), tone.clone()], sr).unwrap();
        let t = Tmp::new();
        for fmt in ["flac", "mp3"] {
            let file = t.0.join(format!("tone.{fmt}"));
            std::fs::write(&file, transcode(&wav, fmt).unwrap()).unwrap();
            let d = athar_core::decode::decode(&file).unwrap_or_else(|e| panic!("{fmt}: {e}"));
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

    #[test]
    fn sixteen_bit_samples_survive_a_read_and_write_at_any_level() {
        let all: Vec<i16> = (i16::MIN..=i16::MAX).step_by(7).collect();
        let wav = wav16_bytes(&[all.iter().map(|&v| v as f32 / 32768.0).collect()], 48_000).unwrap();
        let read = read_wav(&wav).unwrap();
        assert!(read.int16);
        let again = wav16_bytes(&read.channels, read.sr).unwrap();
        let back: Vec<i16> = hound::WavReader::new(Cursor::new(&again)).unwrap().samples::<i16>().map(Result::unwrap).collect();
        assert_eq!(back, all);
    }
}
