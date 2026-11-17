//! Port of the mixdown in `athar/static/arrange.js`.

use crate::dsp::{db, encode_wav_seeded, jround, lin, mix_mono, random_seed, resample_sinc};
use serde_json::Value;
use std::f64::consts::PI;

/// Fade lengths in seconds for one hit.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Fade {
    pub fade_in: f64,
    pub fade_out: f64,
}

/// One lane of the mix: its channels at rate `sr`, a linear gain, start times in seconds, and
/// optionally one fade per hit. A mono lane goes to both sides of a stereo mix.
#[derive(Debug, Clone, Default)]
pub struct MixLane {
    pub chs: Vec<Vec<f32>>,
    pub sr: u32,
    pub gain: f64,
    pub hits: Vec<f64>,
    pub fades: Vec<Option<Fade>>,
}

impl MixLane {
    /// Builds a lane from its worker.js JSON (`{ sr, gain, hits, fades: [{ in, out }] }`, without
    /// `chs`) and its channels.
    pub fn from_json(v: &Value, chs: Vec<Vec<f32>>) -> Self {
        let f = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
        MixLane {
            chs,
            sr: f(v, "sr").unwrap_or(0.0) as u32,
            gain: f(v, "gain").unwrap_or(1.0),
            hits: v.get("hits").and_then(Value::as_array).map_or(vec![], |a| a.iter().filter_map(Value::as_f64).collect()),
            fades: v.get("fades").and_then(Value::as_array).map_or(vec![], |a| {
                a.iter()
                    .map(|x| x.is_object().then(|| Fade { fade_in: f(x, "in").unwrap_or(0.0), fade_out: f(x, "out").unwrap_or(0.0) }))
                    .collect()
            }),
        }
    }
}

/// The non-lane arguments of `renderMix`. `dither_seed` fixes the 16-bit dither (None: random).
#[derive(Debug, Clone, Copy)]
pub struct MixArgs {
    pub sr: u32,
    pub seconds: f64,
    pub rate: u32,
    pub bits: u32,
    pub mono: bool,
    pub fade_end: bool,
    pub dither_seed: Option<u32>,
}

impl MixArgs {
    /// Reads `{ sr, seconds, rate, bits, mono, fadeEnd, ditherSeed? }` as worker.js receives them.
    pub fn from_json(v: &Value) -> Self {
        let f = |k: &str| v.get(k).and_then(Value::as_f64);
        let b = |k: &str| v.get(k).is_some_and(|x| x.as_bool().unwrap_or_else(|| x.as_f64().is_some_and(|n| n != 0.0)));
        let sr = f("sr").unwrap_or(44100.0) as u32;
        MixArgs {
            sr,
            seconds: f("seconds").unwrap_or(0.0),
            rate: f("rate").map_or(sr, |r| r as u32),
            bits: f("bits").map_or(16, |r| r as u32),
            mono: b("mono"),
            fade_end: b("fadeEnd"),
            dither_seed: f("ditherSeed").map(|s| s as u32),
        }
    }
}

/// The saved mix: WAV bytes, and how far over full scale the mix went in dB (0 when it fit).
#[derive(Debug, Clone)]
pub struct Mix {
    pub wav: Vec<u8>,
    pub over: f64,
}

/// Sums lanes into `seconds` of sound at rate `sr`; stereo when any lane is.
pub fn mixdown(lanes: &[MixLane], sr: u32, seconds: f64) -> Vec<Vec<f32>> {
    let srf = sr as f64;
    let n = (jround(seconds * srf) as i64).max(1) as usize;
    let count = if lanes.iter().any(|l| l.chs.len() > 1) { 2 } else { 1 };
    let mut channels = vec![vec![0f32; n]; count];
    for l in lanes {
        if l.chs.is_empty() {
            continue;
        }
        let resampled;
        let chs = if l.sr == sr {
            &l.chs
        } else {
            resampled = resample_sinc(&l.chs, l.sr, sr);
            &resampled
        };
        for (k, &at) in l.hits.iter().enumerate() {
            let len = chs[0].len() as i64;
            let start = jround(at * srf) as i64;
            let m = len.min(n as i64 - start);
            let f = l.fades.get(k).copied().flatten().unwrap_or_default();
            let fin = jround(f.fade_in * srf) as i64;
            let fout = jround(f.fade_out * srf) as i64;
            for (c, o) in channels.iter_mut().enumerate() {
                let d = &chs[c.min(chs.len() - 1)];
                for i in 0..m.max(0) {
                    let at = start + i;
                    let mut g = l.gain;
                    if fin != 0 || fout != 0 {
                        if i < fin {
                            g *= (i as f64 / fin as f64 * PI / 2.0).sin();
                        }
                        if i >= len - fout {
                            g *= ((i - (len - fout)) as f64 / fout as f64 * PI / 2.0).cos();
                        }
                    }
                    if at >= 0 && (at as usize) < n {
                        o[at as usize] = (o[at as usize] as f64 + d[i as usize] as f64 * g) as f32;
                    }
                }
            }
        }
    }
    channels
}

/// Turns the sound down to -0.1 dBFS, in place, when it goes over full scale. Returns how far
/// over it was in dB, 0 when it fit.
pub fn fit_peak(channels: &mut [Vec<f32>]) -> f64 {
    let mut peak = 0f64;
    for d in channels.iter() {
        for &v in d {
            let v = (v as f64).abs();
            if v > peak {
                peak = v;
            }
        }
    }
    if peak <= 1.0 {
        return 0.0;
    }
    let k = lin(-0.1) / peak;
    for d in channels.iter_mut() {
        for v in d.iter_mut() {
            *v = (*v as f64 * k) as f32;
        }
    }
    db(peak)
}

/// arrange.js `renderMix`: mix, fold to mono if asked, fade the cut end, convert the rate, fit
/// the peak, encode. A host passes the worker.js `mix` args as `MixArgs::from_json(&args)` and
/// one `MixLane::from_json(&args.lanes[i], channels_i)` per lane.
pub fn render_mix(lanes: &[MixLane], a: &MixArgs) -> Result<Mix, String> {
    let mut c = mixdown(lanes, a.sr, a.seconds);
    if a.mono && c.len() > 1 {
        c = vec![mix_mono(&c)];
    }
    if a.fade_end {
        let n = c[0].len();
        let f = n.min(jround(a.sr as f64 * 0.005) as usize);
        for d in c.iter_mut() {
            for i in 0..f {
                d[n - f + i] = (d[n - f + i] as f64 * (1.0 - (i + 1) as f64 / f as f64)) as f32;
            }
        }
    }
    if a.rate != a.sr {
        c = resample_sinc(&c, a.sr, a.rate);
    }
    let over = fit_peak(&mut c);
    let wav = encode_wav_seeded(&c, a.rate, a.bits, a.dither_seed.unwrap_or_else(random_seed))?;
    Ok(Mix { wav, over })
}
