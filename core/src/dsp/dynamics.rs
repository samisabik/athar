//! Effects that shape level: compressor, maximizer, saturation, transient shaper, lo-fi.
//! Each settings struct has a `from_json` that reads the keys the effect's panel sends; missing
//! keys take the panel's defaults.

use super::loudness::true_peak_envelope;
use super::{Processed, db, frames, lin, num, peak, resample_sinc, rms, round_half_up};
use serde_json::Value;

/// Compressor settings: levels in dB, times in milliseconds.
#[derive(Debug, Clone, Copy)]
pub struct CompParams {
    pub threshold: f64,
    pub ratio: f64,
    pub knee: f64,
    pub attack: f64,
    pub release: f64,
    pub makeup: f64,
    /// Ignore `makeup` and use `auto_makeup`.
    pub auto_makeup: bool,
}

impl CompParams {
    pub fn from_json(p: &Value) -> Self {
        CompParams {
            threshold: num(p, "threshold").unwrap_or(-24.0),
            ratio: num(p, "ratio").unwrap_or(4.0),
            knee: num(p, "knee").unwrap_or(6.0),
            attack: num(p, "attack").unwrap_or(10.0),
            release: num(p, "release").unwrap_or(150.0),
            makeup: num(p, "makeup").unwrap_or(0.0),
            auto_makeup: p.get("autoMakeup").and_then(Value::as_bool).unwrap_or(true),
        }
    }
}

/// Static compressor curve: output level in dB for an input level in dB.
pub fn comp_curve(x: f64, p: &CompParams) -> f64 {
    let over = x - p.threshold;
    if 2.0 * over < -p.knee {
        return x;
    }
    if p.knee > 0.0 && 2.0 * over.abs() <= p.knee {
        let y = over + p.knee / 2.0;
        return x + (1.0 / p.ratio - 1.0) * (y * y) / (2.0 * p.knee);
    }
    p.threshold + over / p.ratio
}

/// The makeup gain in dB used when none is set: half the distance from the threshold up to
/// where the curve puts a full-scale signal.
pub fn auto_makeup(p: &CompParams) -> f64 {
    -(p.threshold - comp_curve(0.0, p)) * 0.5
}

/// One-pole smoothing coefficient for a time constant in milliseconds.
fn smoothing(ms: f64, sr: f64) -> f64 {
    (-1.0 / (ms / 1000.0 * sr)).exp()
}

/// Feed-forward compressor, stereo-linked peak detector, smoothing in dB.
pub fn compress(chs: &[Vec<f32>], sr: u32, p: &CompParams) -> Processed {
    let sr = sr as f64;
    let n = frames(chs);
    let mut out: Vec<Vec<f32>> = chs.iter().map(|d| vec![0f32; d.len()]).collect();
    let attack = smoothing(p.attack.max(0.05), sr);
    let release = smoothing(p.release.max(1.0), sr);
    let makeup = if p.auto_makeup { auto_makeup(p) } else { p.makeup };
    // `reduction` is the smoothed gain reduction in dB.
    let (mut reduction, mut max_reduction) = (0f64, 0f64);
    for i in 0..n {
        let mut x = 0f64;
        for d in chs {
            x = x.max((d[i] as f64).abs());
        }
        let level = db(x);
        let target = level - comp_curve(level, p);
        let k = if target > reduction { attack } else { release };
        reduction = k * reduction + (1.0 - k) * target;
        max_reduction = max_reduction.max(reduction);
        let g = lin(makeup - reduction);
        for (c, d) in chs.iter().enumerate() {
            out[c][i] = (d[i] as f64 * g) as f32;
        }
    }
    Processed { channels: out, info: format!("max reduction {max_reduction:.1} dB, makeup {makeup:+.1} dB") }
}

/// Maximizer settings: gain and ceiling in dB, release in milliseconds.
#[derive(Debug, Clone, Copy)]
pub struct MaxParams {
    pub gain: f64,
    pub ceiling: f64,
    pub release: f64,
}

impl MaxParams {
    pub fn from_json(p: &Value) -> Self {
        MaxParams {
            gain: num(p, "gain").unwrap_or(6.0),
            ceiling: num(p, "ceiling").unwrap_or(-1.0),
            release: num(p, "release").unwrap_or(60.0),
        }
    }
}

/// For every position i, the smallest of `v[i..i + window]` (shorter at the end).
fn sliding_min(v: &[f32], window: usize) -> Vec<f32> {
    let n = v.len();
    let mut out = vec![0f32; n];
    // Indices of candidates for the minimum, their values rising from `head` to `tail`.
    let mut queue = vec![0usize; n];
    let (mut head, mut tail, mut next) = (0usize, 0usize, 0usize);
    for i in 0..n {
        while next < n.min(i + window) {
            while tail > head && v[queue[tail - 1]] >= v[next] {
                tail -= 1;
            }
            queue[tail] = next;
            tail += 1;
            next += 1;
        }
        while queue[head] < i {
            head += 1;
        }
        out[i] = v[queue[head]];
    }
    out
}

/// Gain followed by a look-ahead true-peak limiter at the ceiling.
pub fn maximize(chs: &[Vec<f32>], sr: u32, p: &MaxParams) -> Processed {
    let sr = sr as f64;
    let n = frames(chs);
    let (gain, ceiling) = (lin(p.gain), lin(p.ceiling));
    let lookahead = ((sr * 0.005).round() as usize).max(1);
    // The gain each sample needs to stay under the ceiling, held from two look-aheads before it.
    let mut needed = true_peak_envelope(chs);
    for r in needed.iter_mut() {
        let x = *r as f64 * gain;
        *r = (if x > ceiling { ceiling / x } else { 1.0 }) as f32;
    }
    let mut held = sliding_min(&needed, 2 * lookahead);
    // Drop at once, recover at the release rate.
    let release = smoothing(p.release.max(1.0), sr);
    let mut g = 1f64;
    for h in held.iter_mut() {
        let v = *h as f64;
        g = if v < g { v } else { g + (v - g) * (1.0 - release) };
        *h = g as f32;
    }
    // A moving average one look-ahead long rounds the corners of the drops.
    let mut out: Vec<Vec<f32>> = chs.iter().map(|d| vec![0f32; d.len()]).collect();
    let first = held.first().copied().unwrap_or(1.0);
    let mut ring = vec![first; lookahead];
    let mut acc = lookahead as f64 * first as f64;
    let mut max_limiting = 0f64;
    for i in 0..n {
        acc += held[i] as f64 - ring[i % lookahead] as f64;
        ring[i % lookahead] = held[i];
        let smooth = acc / lookahead as f64;
        max_limiting = max_limiting.max(-db(smooth));
        for (c, d) in chs.iter().enumerate() {
            out[c][i] = (d[i] as f64 * gain * smooth).clamp(-ceiling, ceiling) as f32;
        }
    }
    Processed {
        info: format!("peak {:.1} dB, RMS {:.1} dB, max limiting {max_limiting:.1} dB", db(peak(&out)), db(rms(&out))),
        channels: out,
    }
}

/// Saturation settings: drive and ceiling in dB, mix from 0 to 100.
#[derive(Debug, Clone, Copy)]
pub struct SatParams {
    pub drive: f64,
    pub ceiling: f64,
    pub mix: f64,
}

impl SatParams {
    pub fn from_json(p: &Value) -> Self {
        SatParams {
            drive: num(p, "drive").unwrap_or(6.0),
            ceiling: num(p, "ceiling").unwrap_or(-1.0),
            mix: num(p, "mix").unwrap_or(100.0),
        }
    }
}

/// What the soft clipper does to a level in dB.
pub fn sat_curve(in_db: f64, drive: f64, ceiling: f64) -> f64 {
    let c = lin(ceiling);
    db(c * (lin(in_db + drive) / c).tanh())
}

/// Drive into a tanh soft clipper at twice the sample rate, which keeps the harmonics it adds
/// from folding back; `mix` blends the dry sound back in.
pub fn saturate(chs: &[Vec<f32>], p: &SatParams) -> Processed {
    let (drive, ceiling) = (lin(p.drive), lin(p.ceiling));
    let mix = (p.mix / 100.0).clamp(0.0, 1.0);
    let mut up = resample_sinc(chs, 1, 2);
    for d in up.iter_mut() {
        for v in d.iter_mut() {
            *v = (ceiling * (*v as f64 * drive / ceiling).tanh()) as f32;
        }
    }
    let wet = resample_sinc(&up, 2, 1);
    let channels: Vec<Vec<f32>> = chs
        .iter()
        .zip(&wet)
        .map(|(d, w)| (0..d.len()).map(|i| (d[i] as f64 * (1.0 - mix) + w.get(i).map_or(0.0, |&x| x as f64) * mix) as f32).collect())
        .collect();
    Processed { info: format!("peak {:.1} dB", db(peak(&channels))), channels }
}

/// Transient shaper settings, each from -100 to 100.
#[derive(Debug, Clone, Copy)]
pub struct ShapeParams {
    pub attack: f64,
    pub tail: f64,
}

impl ShapeParams {
    pub fn from_json(p: &Value) -> Self {
        ShapeParams { attack: num(p, "attack").unwrap_or(0.0), tail: num(p, "tail").unwrap_or(0.0) }
    }
}

/// Transient shaper: raises or lowers the hits and the ringing between them, one gain for all
/// channels. A hit is where a fast envelope runs ahead of a slow one; ringing is where a
/// slow-falling envelope stays above a fast-falling one.
pub fn shape(chs: &[Vec<f32>], sr: u32, p: &ShapeParams) -> Processed {
    let sr = sr as f64;
    let n = frames(chs);
    let (fast_up, slow_up, down, slow_down, smooth) = (smoothing(0.5, sr), smoothing(18.0, sr), smoothing(60.0, sr), smoothing(450.0, sr), smoothing(1.5, sr));
    let attack = p.attack / 100.0;
    let tail = p.tail / 100.0 * 1.6;
    let floor = 1e-5;
    let mut out = vec![vec![0f32; n]; chs.len()];
    let (mut rise_fast, mut rise_slow, mut fall_fast, mut fall_slow, mut gain_db) = (0f64, 0f64, 0f64, 0f64, 0f64);
    for i in 0..n {
        let mut x = 0f64;
        for d in chs {
            x = x.max((d[i] as f64).abs());
        }
        rise_fast = x + (rise_fast - x) * if x > rise_fast { fast_up } else { down };
        rise_slow = x + (rise_slow - x) * if x > rise_slow { slow_up } else { down };
        fall_fast = if x > fall_fast { x } else { x + (fall_fast - x) * down };
        fall_slow = if x > fall_slow { x } else { x + (fall_slow - x) * slow_down };
        let hit = (db(rise_fast + floor) - db(rise_slow + floor)).max(0.0);
        let ring = (db(fall_slow + floor) - db(fall_fast + floor)).max(0.0);
        let want = (attack * hit + tail * ring).clamp(-36.0, 18.0);
        gain_db = want + (gain_db - want) * smooth;
        let gain = lin(gain_db);
        for (c, d) in chs.iter().enumerate() {
            out[c][i] = (d[i] as f64 * gain) as f32;
        }
    }
    Processed { info: format!("peak {:.1} dB", db(peak(&out))), channels: out }
}

/// Lo-fi settings: bit depth, and sample rate in kHz.
#[derive(Debug, Clone, Copy)]
pub struct LofiParams {
    pub bits: f64,
    pub rate: f64,
}

impl LofiParams {
    pub fn from_json(p: &Value) -> Self {
        LofiParams { bits: num(p, "bits").unwrap_or(12.0), rate: num(p, "rate").unwrap_or(26.0) }
    }
}

/// Sample-and-hold to a lower rate and round to fewer bits, with the aliasing left in.
pub fn lofi(chs: &[Vec<f32>], sr: u32, p: &LofiParams) -> Processed {
    let sr = sr as f64;
    // Samples held per new value: the rate stays between 500 Hz and the sound's own.
    let step = (sr / (p.rate * 1000.0).min(sr).max(500.0)).max(1.0);
    let levels = 2f64.powf(p.bits.clamp(2.0, 24.0) - 1.0);
    let crush = p.bits < 24.0;
    let channels = chs
        .iter()
        .map(|d| {
            let (mut next, mut held) = (0f64, 0f64);
            d.iter()
                .enumerate()
                .map(|(i, &x)| {
                    if i as f64 >= next {
                        held = if crush { round_half_up(x as f64 * levels) / levels } else { x as f64 };
                        next += step;
                    }
                    held as f32
                })
                .collect()
        })
        .collect();
    Processed { channels, info: format!("{}-bit at {:.1} kHz", p.bits, sr / step / 1000.0) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::testing::sine;
    use serde_json::json;

    fn comp(threshold: f64, ratio: f64, knee: f64) -> CompParams {
        CompParams { threshold, ratio, knee, attack: 1.0, release: 50.0, makeup: 0.0, auto_makeup: false }
    }

    #[test]
    fn settings_default_to_the_panels() {
        let c = CompParams::from_json(&json!({}));
        assert_eq!((c.threshold, c.ratio, c.knee, c.attack, c.release, c.makeup, c.auto_makeup), (-24.0, 4.0, 6.0, 10.0, 150.0, 0.0, true));
        let c = CompParams::from_json(&json!({"threshold": -10, "autoMakeup": false, "makeup": 3.5}));
        assert_eq!((c.threshold, c.makeup, c.auto_makeup), (-10.0, 3.5, false));
        assert_eq!(MaxParams::from_json(&json!({"gain": 2})).gain, 2.0);
        assert_eq!(SatParams::from_json(&json!({})).mix, 100.0);
        assert_eq!(LofiParams::from_json(&json!({})).bits, 12.0);
    }

    #[test]
    fn compressor_curve() {
        let hard = comp(-20.0, 4.0, 0.0);
        assert_eq!(comp_curve(-30.0, &hard), -30.0);
        assert_eq!(comp_curve(-20.0, &hard), -20.0);
        assert_eq!(comp_curve(0.0, &hard), -15.0);
        assert_eq!(auto_makeup(&hard), 2.5);
        // A knee joins both lines without a step.
        let soft = comp(-20.0, 4.0, 6.0);
        assert!((comp_curve(-23.0, &soft) + 23.0).abs() < 1e-12);
        assert!((comp_curve(-17.0, &soft) - (-20.0 + 3.0 / 4.0)).abs() < 1e-12);
        assert!(comp_curve(-20.0, &soft) < -20.0);
    }

    #[test]
    fn compressor_settles_on_the_curve() {
        let chs = vec![sine(1000.0, 48000, 1.0, 1.0)];
        let out = compress(&chs, 48000, &comp(-20.0, 4.0, 0.0));
        let late = peak(&[out.channels[0][40000..].to_vec()]);
        // The detector follows the waveform, so the settled level sits a little above the curve.
        assert!(db(late) > -15.0 && db(late) < -9.0, "{}", db(late));
        assert!(out.info.starts_with("max reduction 1"), "{}", out.info);
        assert!(out.info.ends_with("makeup +0.0 dB"), "{}", out.info);
        let quiet = vec![sine(1000.0, 48000, 0.1, 0.01)];
        assert_eq!(compress(&quiet, 48000, &comp(-20.0, 4.0, 0.0)).channels, quiet);
    }

    #[test]
    fn sliding_minimum_looks_ahead() {
        assert_eq!(sliding_min(&[5.0, 3.0, 4.0, 1.0, 2.0, 6.0], 2), vec![3.0, 3.0, 1.0, 1.0, 2.0, 6.0]);
        assert_eq!(sliding_min(&[2.0, 1.0, 3.0], 10), vec![1.0, 1.0, 3.0]);
        assert!(sliding_min(&[], 4).is_empty());
    }

    #[test]
    fn maximizer_holds_the_ceiling() {
        let chs = vec![sine(220.0, 48000, 0.5, 0.25), sine(330.0, 48000, 0.5, 0.5)];
        let out = maximize(&chs, 48000, &MaxParams { gain: 12.0, ceiling: -1.0, release: 60.0 });
        let top = peak(&out.channels);
        assert!(top <= lin(-1.0) + 1e-6 && top > lin(-1.5), "{top}");
        assert!(rms(&out.channels) > rms(&chs));
        // Below the ceiling it is only gain.
        let quiet = vec![sine(220.0, 48000, 0.1, 0.1)];
        let out = maximize(&quiet, 48000, &MaxParams { gain: 6.0, ceiling: -1.0, release: 60.0 });
        for (a, b) in out.channels[0].iter().zip(&quiet[0]) {
            assert!((*a as f64 - *b as f64 * lin(6.0)).abs() < 1e-6);
        }
    }

    #[test]
    fn saturation_curve_and_mix() {
        assert!((sat_curve(-60.0, 0.0, 0.0) + 60.0).abs() < 0.01);
        assert!(sat_curve(20.0, 0.0, -3.0) <= -3.0 && sat_curve(20.0, 0.0, -3.0) > -3.01);
        let chs = vec![sine(440.0, 48000, 0.2, 0.9)];
        let dry = saturate(&chs, &SatParams { drive: 24.0, ceiling: -6.0, mix: 0.0 });
        assert_eq!(dry.channels, chs);
        let wet = saturate(&chs, &SatParams { drive: 24.0, ceiling: -6.0, mix: 100.0 });
        assert_eq!(wet.channels[0].len(), chs[0].len());
        // Band-limiting the clipped wave lets it ring a little over the ceiling.
        let top = db(peak(&wet.channels));
        assert!(top > -7.0 && top < -4.0, "{top}");
    }

    #[test]
    fn shaper_at_zero_changes_nothing_and_attack_lifts_hits() {
        let mut hit = vec![0f32; 4800];
        hit.extend(sine(200.0, 48000, 0.3, 0.5));
        let chs = vec![hit];
        assert_eq!(shape(&chs, 48000, &ShapeParams { attack: 0.0, tail: 0.0 }).channels, chs);
        let more = shape(&chs, 48000, &ShapeParams { attack: 100.0, tail: 0.0 });
        let less = shape(&chs, 48000, &ShapeParams { attack: -100.0, tail: 0.0 });
        let start = |c: &[Vec<f32>]| peak(&[c[0][4800..5800].to_vec()]);
        assert!(start(&more.channels) > 0.55 && start(&less.channels) < 0.45);
    }

    #[test]
    fn lofi_holds_and_quantizes() {
        let chs = vec![sine(100.0, 48000, 0.1, 0.9)];
        let out = lofi(&chs, 48000, &LofiParams { bits: 4.0, rate: 12.0 });
        assert_eq!(out.info, "4-bit at 12.0 kHz");
        for block in out.channels[0].chunks_exact(4) {
            assert!(block.iter().all(|&v| v == block[0]));
            assert_eq!((block[0] * 8.0).fract(), 0.0);
        }
        let clean = lofi(&chs, 48000, &LofiParams { bits: 24.0, rate: 48.0 });
        assert_eq!(clean.channels, chs);
    }

    #[test]
    fn effects_accept_no_channels() {
        assert!(compress(&[], 48000, &comp(-20.0, 4.0, 0.0)).channels.is_empty());
        assert!(maximize(&[], 48000, &MaxParams::from_json(&json!({}))).channels.is_empty());
        assert!(shape(&[], 48000, &ShapeParams { attack: 50.0, tail: 0.0 }).channels.is_empty());
        assert!(saturate(&[], &SatParams::from_json(&json!({}))).channels.is_empty());
        assert!(lofi(&[], 48000, &LofiParams { bits: 8.0, rate: 8.0 }).channels.is_empty());
    }
}
