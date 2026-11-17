//! Reading sound files and bringing them to CLAP's 48 kHz mono, as `load_audio` and `clips` in
//! athar/engine.py do with soundfile and librosa.

use crate::embed::Error;
use crate::features::{SECONDS, SR};
use rubato::audioadapter_buffers::owned::InterleavedOwned;
use rubato::{Fft, FixedSync, Resampler, WindowFunction};
use std::fs::File;
use std::path::Path;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// How much of a file is analysed, from its start.
pub const ANALYSIS_SECONDS: f64 = 30.0;
const MAX_CLIPS: usize = 6;

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

/// Any sound file as samples: symphonia first, then macOS's afconvert for whatever symphonia
/// cannot read (WavPack, some FLAC and AIFF variants), as the Python engine does.
pub fn decode(path: &Path) -> Result<Decoded, Error> {
    match decode_native(path) {
        Ok(d) => Ok(d),
        Err(first) => {
            let tmp = std::env::temp_dir().join(format!("athar-{}-{}.wav", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
            let ok = std::process::Command::new("afconvert")
                .args(["-f", "WAVE", "-d", "LEF32"])
                .arg(path)
                .arg(&tmp)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            let out = if ok { decode_native(&tmp) } else { Err(first) };
            let _ = std::fs::remove_file(&tmp);
            out
        }
    }
}

fn decode_native(path: &Path) -> Result<Decoded, Error> {
    let mss = MediaSourceStream::new(Box::new(File::open(path)?), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe().probe(&hint, mss, FormatOptions::default(), MetadataOptions::default()).map_err(|e| format!("reading the container: {e}"))?;
    let track = format.default_track(TrackType::Audio).ok_or("no audio track")?;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or("no audio parameters")?;
    let mut decoder = symphonia::default::get_codecs().make_audio_decoder(params, &AudioDecoderOptions::default())?;
    let track_id = track.id;
    let mut interleaved: Vec<f32> = Vec::new();
    let mut buf: Vec<f32> = Vec::new();
    let (mut sr, mut nch) = (0u32, 0usize);
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
                nch = audio.spec().channels().count();
                buf.resize(audio.samples_interleaved(), 0.0);
                audio.copy_to_slice_interleaved(&mut buf);
                interleaved.extend_from_slice(&buf);
            }
            Err(SymError::DecodeError(_)) => continue,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format!("decoding: {e}").into()),
        }
    }
    if nch == 0 {
        return Err("no audio decoded".into());
    }
    let channels = (0..nch).map(|c| interleaved.iter().skip(c).step_by(nch).copied().collect()).collect();
    Ok(Decoded { channels, sr })
}

fn resampling(e: impl std::fmt::Debug) -> Error {
    format!("resampling failed: {e:?}").into()
}

/// Fixed-ratio FFT resampling with rubato, trimmed or padded to ceil(len * to / from) samples as
/// librosa does.
pub fn resample(y: &[f32], from: u32, to: u32) -> Result<Vec<f32>, Error> {
    if from == to {
        return Ok(y.to_vec());
    }
    let want = (y.len() as f64 * (to as f64 / from as f64)).ceil() as usize;
    if y.is_empty() {
        return Ok(Vec::new());
    }
    let (from, to) = (from as usize, to as usize);
    let (mut gcd, mut b) = (from, to);
    while b != 0 {
        (gcd, b) = (b, gcd % b);
    }
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

/// The first ANALYSIS_SECONDS of the file as 48 kHz mono, as `load_audio`.
pub fn load_audio(d: &Decoded) -> Result<Vec<f32>, Error> {
    let frames = (d.sr as f64 * ANALYSIS_SECONDS).round() as usize;
    resample(&d.mono(0, frames), d.sr, SR as u32)
}

/// What CLAP hears of a file, as `clips`: the sound itself when it is short, otherwise up to
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
