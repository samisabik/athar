//! Reading sound files, and bringing them to the 48 kHz mono that CLAP and the analysis take.

use crate::embed::Error;
use crate::features::{SECONDS, SR};
use rubato::audioadapter_buffers::owned::InterleavedOwned;
use rubato::{Fft, FixedSync, Resampler, WindowFunction};
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// How much of a file is analysed, from its start.
pub const ANALYSIS_SECONDS: f64 = 30.0;
const MAX_CLIPS: usize = 6;
/// The most frames reserved up front on a container's word: an hour at 192 kHz.
const MAX_RESERVED_FRAMES: u64 = 3600 * 192_000;

pub struct Decoded {
    /// One vector of samples per channel.
    pub channels: Vec<Vec<f32>>,
    pub sr: u32,
}

impl Decoded {
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn duration(&self) -> f64 {
        self.frames() as f64 / self.sr as f64
    }

    /// The mean of all channels over `from..to` frames.
    pub fn mono(&self, from: usize, to: usize) -> Vec<f32> {
        let to = to.min(self.frames());
        let from = from.min(to);
        let k = self.channels.len() as f32;
        (from..to).map(|i| self.channels.iter().map(|c| c[i]).sum::<f32>() / k).collect()
    }
}

/// Any sound file as samples: symphonia first, then macOS's afconvert for what symphonia cannot
/// read (WavPack, some FLAC and AIFF variants). Samples that are not finite numbers, which
/// float files can hold, come out as zero.
pub fn decode(path: &Path) -> Result<Decoded, Error> {
    let file = File::open(path)?;
    let ext = path.extension().and_then(|e| e.to_str());
    decode_stream(file, ext).or_else(|first| {
        static CONVERSIONS: AtomicU64 = AtomicU64::new(0);
        let tmp = std::env::temp_dir().join(format!("athar-{}-{}.wav", std::process::id(), CONVERSIONS.fetch_add(1, Ordering::Relaxed)));
        let converted = std::process::Command::new("afconvert").args(["-f", "WAVE", "-d", "LEF32"]).arg(path).arg(&tmp).output().is_ok_and(|o| o.status.success());
        let out = if converted { File::open(&tmp).map_err(Error::from).and_then(|f| decode_stream(f, Some("wav"))) } else { Err(first) };
        let _ = std::fs::remove_file(&tmp);
        out
    })
}

fn decode_stream(file: File, ext: Option<&str>) -> Result<Decoded, Error> {
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = ext {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe().probe(&hint, mss, FormatOptions::default(), MetadataOptions::default()).map_err(|e| format!("reading the container: {e}"))?;
    let track = format.default_track(TrackType::Audio).ok_or("no audio track")?;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or("no audio parameters")?;
    let mut decoder = symphonia::default::get_codecs().make_audio_decoder(params, &AudioDecoderOptions::default())?;
    let track_id = track.id;
    // Room for the length the container states, within reason: the header can be wrong.
    let expected = track.num_frames.unwrap_or(0).min(MAX_RESERVED_FRAMES) as usize;
    let mut channels: Vec<Vec<f32>> = Vec::new();
    let mut packet_samples: Vec<f32> = Vec::new();
    let mut sr = 0u32;
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            // Some streams end with an end-of-file error instead of a clean end.
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format!("reading a packet: {e}").into()),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(audio) => {
                sr = audio.spec().rate();
                let count = audio.spec().channels().count();
                if count == 0 {
                    continue;
                }
                if channels.is_empty() {
                    channels = (0..count).map(|_| Vec::with_capacity(expected)).collect();
                }
                packet_samples.resize(audio.samples_interleaved(), 0.0);
                audio.copy_to_slice_interleaved(&mut packet_samples);
                for frame in packet_samples.chunks_exact(count) {
                    for (channel, &v) in channels.iter_mut().zip(frame) {
                        channel.push(if v.is_finite() { v } else { 0.0 });
                    }
                }
            }
            Err(SymError::DecodeError(_)) => continue,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format!("decoding: {e}").into()),
        }
    }
    if channels.is_empty() {
        return Err("no audio decoded".into());
    }
    Ok(Decoded { channels, sr })
}

fn resampling(e: impl std::fmt::Debug) -> Error {
    format!("resampling failed: {e:?}").into()
}

/// Fixed-ratio FFT resampling with rubato, trimmed or padded to ceil(len * to / from) samples as
/// librosa does. This is the resampler for analysis and playback; audio that is edited or
/// exported goes through `dsp::resample_sinc`.
pub fn resample(y: &[f32], from: u32, to: u32) -> Result<Vec<f32>, Error> {
    if from == to {
        return Ok(y.to_vec());
    }
    let want = (y.len() as f64 * (to as f64 / from as f64)).ceil() as usize;
    if y.is_empty() {
        return Ok(Vec::new());
    }
    let gcd = crate::gcd(from as u64, to as u64) as usize;
    let (from, to) = (from as usize, to as usize);
    // FFT blocks of about 300 frames at the lower rate, in an even number of the smallest blocks
    // the ratio allows so that the filter delay is a whole number of frames at both rates. A
    // block this size cuts off near 95% of the lower Nyquist, close to soxr's high quality filter.
    let low = from.min(to) / gcd;
    let chunk = ((300.0 / low as f64 / 2.0).round() as usize).max(1) * 2 * low;
    let fixed = if from < to { FixedSync::Input } else { FixedSync::Output };
    let mut r = Fft::<f64>::new_custom(from, to, chunk, 1, 1, WindowFunction::BlackmanHarris2, fixed).map_err(resampling)?;
    let input = InterleavedOwned::new_from(y.iter().map(|&v| v as f64).collect(), 1, y.len()).map_err(resampling)?;
    let mut out: Vec<f32> = r.process_all(&input, y.len(), None).map_err(resampling)?.take_data().into_iter().map(|v| v as f32).collect();
    out.resize(want, 0.0);
    Ok(out)
}

/// The first ANALYSIS_SECONDS of the file as 48 kHz mono.
pub fn load_audio(d: &Decoded) -> Result<Vec<f32>, Error> {
    let frames = (d.sr as f64 * ANALYSIS_SECONDS).round() as usize;
    resample(&d.mono(0, frames), d.sr, SR as u32)
}

/// What CLAP hears of a file: the sound itself (`y48`) when it is short, otherwise up to
/// MAX_CLIPS ten-second clips spread evenly from start to end.
pub fn clips(d: &Decoded, y48: &[f32]) -> Result<Vec<Vec<f32>>, Error> {
    let duration = d.duration();
    let clip = SECONDS as f64;
    if duration <= clip * 1.5 {
        return Ok(vec![y48.to_vec()]);
    }
    let n = MAX_CLIPS.min((duration / clip).ceil() as usize);
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let s = if n == 1 { 0.0 } else { (duration - clip) * k as f64 / (n - 1) as f64 };
        let from = (s * d.sr as f64) as usize;
        let c = d.mono(from, from + SECONDS * d.sr as usize);
        if !c.is_empty() {
            out.push(resample(&c, d.sr, SR as u32)?);
        }
    }
    if out.is_empty() {
        out.push(y48.to_vec());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::encode_wav;

    fn temp_wav(name: &str, channels: &[Vec<f32>], sr: u32, bits: u32) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("athar-test-{}-{name}.wav", std::process::id()));
        std::fs::write(&path, encode_wav(channels, sr, bits).unwrap()).unwrap();
        path
    }

    #[test]
    fn channels_come_back_as_written() {
        let chs = vec![vec![0.25f32, -0.5, 0.75, 0.0, 1.0], vec![-0.125, 0.5, -1.0, 0.0625, 0.0]];
        let path = temp_wav("stereo", &chs, 44_100, 32);
        let d = decode(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!((d.sr, d.frames()), (44_100, 5));
        assert_eq!(d.channels, chs);
        assert_eq!(d.mono(1, 3), vec![0.0, -0.125]);
    }

    #[test]
    fn samples_that_are_not_numbers_become_silence() {
        let chs = vec![vec![0.5f32, f32::NAN, -0.5, f32::INFINITY, f32::NEG_INFINITY, 0.25]];
        let path = temp_wav("nan", &chs, 48_000, 32);
        let d = decode(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(d.channels, vec![vec![0.5, 0.0, -0.5, 0.0, 0.0, 0.25]]);
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let e = decode(Path::new("/nonexistent/athar/missing.wav")).err().unwrap();
        assert_eq!(e.downcast_ref::<std::io::Error>().map(std::io::Error::kind), Some(std::io::ErrorKind::NotFound));
    }

    #[test]
    fn resampling_keeps_length_and_level() {
        assert_eq!(resample(&[0.5; 100], 48_000, 48_000).unwrap(), vec![0.5; 100]);
        assert!(resample(&[], 44_100, 48_000).unwrap().is_empty());
        let out = resample(&vec![0.5f32; 24_000], 24_000, 48_000).unwrap();
        assert_eq!(out.len(), 48_000);
        assert!(out[4800..43_200].iter().all(|v| (v - 0.5).abs() < 1e-3));
        assert_eq!(resample(&vec![0f32; 1001], 48_000, 22_050).unwrap().len(), 460);
    }

    #[test]
    fn long_files_are_heard_in_clips() {
        let short = Decoded { channels: vec![vec![0.1f32; 48_000 * 12]], sr: 48_000 };
        let y = load_audio(&short).unwrap();
        assert_eq!(y.len(), 48_000 * 12);
        assert_eq!(clips(&short, &y).unwrap().len(), 1);
        let long = Decoded { channels: vec![vec![0.1f32; 8_000 * 35]], sr: 8_000 };
        let y = load_audio(&long).unwrap();
        assert_eq!(y.len(), 48_000 * 30);
        let c = clips(&long, &y).unwrap();
        assert_eq!(c.len(), 4);
        assert!(c.iter().all(|clip| clip.len() == 48_000 * 10));
    }
}
