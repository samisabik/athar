//! Checks dsp.rs and arrange.rs against athar/static/dsp.js and arrange.js. tests/dsp_ref.mjs
//! runs the JavaScript under node and writes the cases; the test is skipped when node is missing.

use athar_core::arrange::{MixArgs, MixLane, fit_peak, mixdown, render_mix};
use athar_core::dsp::{self, CompParams};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

const SAMPLE_TOL: f64 = 1e-4;
const REL_TOL: f64 = 1e-4;

struct Data(Vec<u8>);

impl Data {
    fn f32s(&self, r: &Value) -> Vec<f32> {
        assert_eq!(r["t"], "f32");
        let (off, len) = (r["off"].as_u64().unwrap() as usize, r["len"].as_u64().unwrap() as usize);
        self.0[off..off + 4 * len].chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect()
    }
    fn bytes(&self, r: &Value) -> Vec<u8> {
        let (off, len) = (r["off"].as_u64().unwrap() as usize, r["len"].as_u64().unwrap() as usize);
        self.0[off..off + len].to_vec()
    }
    fn chs(&self, v: &Value) -> Vec<Vec<f32>> {
        v.as_array().map_or(vec![], |a| a.iter().map(|r| self.f32s(r)).collect())
    }
}

#[derive(Default)]
struct Worst {
    sample: f64,
    scalar: f64,
    lsb: i64,
    pixel: i64,
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1.0)
}

fn compare_channels(name: &str, got: &[Vec<f32>], want: &[Vec<f32>], w: &mut Worst) {
    assert_eq!(got.len(), want.len(), "{name}: channel count");
    for (g, e) in got.iter().zip(want) {
        assert_eq!(g.len(), e.len(), "{name}: channel length");
        for (a, b) in g.iter().zip(e) {
            let d = (*a as f64 - *b as f64).abs();
            assert!(d <= SAMPLE_TOL, "{name}: sample differs by {d}");
            w.sample = w.sample.max(d);
        }
    }
}

fn compare_json(name: &str, key: &str, got: &Value, want: &Value, w: &mut Worst) {
    match (got, want) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            if key == "positions" || key == "midi" || key == "cents" || key == "bounds" {
                assert_eq!(a, b, "{name}: {key}");
            } else {
                let d = rel(a, b);
                assert!(d <= REL_TOL, "{name}: {key} {a} vs {b}");
                w.scalar = w.scalar.max(d);
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{name}: {key} length");
            for (x, y) in a.iter().zip(b) {
                compare_json(name, key, x, y, w);
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                compare_json(name, k, a.get(k).unwrap_or(&Value::Null), v, w);
            }
        }
        _ => assert_eq!(got, want, "{name}: {key}"),
    }
}

fn wav_samples(b: &[u8]) -> (usize, u16, Vec<f64>) {
    let head = if b[20..22] == [0xfe, 0xff] { 80 } else { 44 };
    let bits = u16::from_le_bytes([b[34], b[35]]);
    let size = u32::from_le_bytes(b[head - 4..head].try_into().unwrap()) as usize;
    let data = &b[head..head + size];
    let s = match bits {
        16 => data.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f64).collect(),
        24 => data.chunks_exact(3).map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f64).collect(),
        _ => data.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap()) as f64).collect(),
    };
    (head, bits, s)
}

fn compare_wav(name: &str, got: &[u8], want: &[u8], w: &mut Worst) {
    assert_eq!(got.len(), want.len(), "{name}: wav length");
    let (head, bits, gs) = wav_samples(got);
    assert_eq!(got[..head], want[..head], "{name}: wav header");
    let (_, _, ws) = wav_samples(want);
    for (a, b) in gs.iter().zip(&ws) {
        if bits == 32 {
            let d = (a - b).abs();
            assert!(d <= SAMPLE_TOL, "{name}: float sample differs by {d}");
            w.sample = w.sample.max(d);
        } else {
            let d = (a - b).abs() as i64;
            assert!(d <= 1, "{name}: sample differs by {d} LSB");
            w.lsb = w.lsb.max(d);
        }
    }
}

fn lanes(data: &Data, args: &Value) -> Vec<MixLane> {
    args["lanes"].as_array().unwrap().iter().map(|l| MixLane::from_json(l, data.chs(&l["chs"]))).collect()
}

fn generate() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dsp_parity");
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/dsp_ref.mjs");
    match Command::new("node").arg(&script).arg(&dir).output() {
        Ok(o) if o.status.success() => Some(dir),
        Ok(o) => panic!("dsp_ref.mjs failed: {}", String::from_utf8_lossy(&o.stderr)),
        Err(e) => {
            eprintln!("skipping dsp parity: node is not available ({e})");
            None
        }
    }
}

#[test]
fn dsp_matches_javascript() {
    let Some(dir) = generate() else { return };
    let data = Data(std::fs::read(dir.join("data.bin")).unwrap());
    let cases: Vec<Value> = serde_json::from_slice(&std::fs::read(dir.join("cases.json")).unwrap()).unwrap();
    let mut worst: BTreeMap<String, Worst> = BTreeMap::new();
    for c in &cases {
        let name = c["name"].as_str().unwrap();
        let sr = c["sr"].as_u64().unwrap() as u32;
        let chs = data.chs(&c["chs"]);
        let (a, out) = (&c["args"], &c["out"]);
        let group = match c["fn"].as_str().unwrap() {
            "run" => a["op"].as_str().unwrap().to_string(),
            f => f.to_string(),
        };
        let w = worst.entry(group).or_default();
        let want_chs = || data.chs(&out["channels"]);
        let n = |k: &str| a[k].as_f64().unwrap();
        match c["fn"].as_str().unwrap() {
            "run" => {
                let r = dsp::run(a["op"].as_str().unwrap(), chs, sr, &a["p"]).unwrap();
                compare_channels(name, &r.channels, &want_chs(), w);
                compare_json(name, "info", &r.info, &out["info"], w);
                if let Some(b) = out.get("bytes") {
                    compare_wav(name, &r.bytes.unwrap(), &data.bytes(b), w);
                }
            }
            "fft" => {
                let (mut re, mut im) = (chs[0].clone(), chs[1].clone());
                dsp::fft(&mut re, &mut im);
                compare_channels(name, &[re, im], &want_chs(), w);
            }
            "mixMono" => compare_channels(name, &[dsp::mix_mono(&chs)], &want_chs(), w),
            "stats" => {
                let s = dsp::stats(&chs);
                compare_json(name, "info", &serde_json::json!({ "peak": s.peak, "rms": s.rms }), &out["info"], w);
            }
            "compCurve" => {
                let p = CompParams::from_json(&a["p"]);
                let got = serde_json::json!({ "y": dsp::comp_curve(n("x"), &p), "makeup": dsp::auto_makeup(&p) });
                compare_json(name, "info", &got, &out["info"], w);
            }
            "satCurve" => {
                let y = dsp::sat_curve(n("x"), a["p"]["drive"].as_f64().unwrap(), a["p"]["ceiling"].as_f64().unwrap());
                compare_json(name, "info", &serde_json::json!({ "y": y }), &out["info"], w);
            }
            "spectrogramPeak" => {
                compare_json(name, "info", &serde_json::json!({ "peak": dsp::spectrogram_peak(&chs[0], sr) }), &out["info"], w)
            }
            "spectrogram" => {
                let px = dsp::spectrogram(&chs[0], sr, n("start"), n("spp"), n("PW") as usize, n("rows") as usize, n("peak"));
                let want = data.bytes(&out["bytes"]);
                assert_eq!(px.len(), want.len(), "{name}: pixel count");
                for (g, e) in px.iter().zip(&want) {
                    let d = (*g as i64 - *e as i64).abs();
                    assert!(d <= 1, "{name}: pixel differs by {d}");
                    w.pixel = w.pixel.max(d);
                }
            }
            "loudness" => {
                let l = dsp::loudness(&chs, sr);
                let got = if l.is_finite() { serde_json::json!({ "lufs": l }) } else { serde_json::json!({ "lufs": null }) };
                compare_json(name, "info", &got, &out["info"], w);
            }
            "truePeakEnvelope" => compare_channels(name, &[dsp::true_peak_envelope(&chs)], &want_chs(), w),
            "stretch" => compare_channels(name, &dsp::stretch(&chs, sr, n("factor")), &want_chs(), w),
            "noteAt" => {
                let t = dsp::note_at(n("hz"));
                let got = serde_json::json!({ "midi": t.midi, "name": t.name, "hz": t.hz, "cents": t.cents });
                compare_json(name, "info", &got, &out["info"], w);
            }
            "soundBounds" => {
                let b = dsp::sound_bounds(&chs, sr, n("threshold")).map(|(a, b)| vec![a, b]);
                compare_json(name, "info", &serde_json::json!({ "bounds": b }), &out["info"], w);
            }
            "fade" => {
                let mut c = chs;
                dsp::fade(&mut c, n("a") as usize, n("b") as usize, a["fadeIn"].as_bool().unwrap());
                compare_channels(name, &c, &want_chs(), w);
            }
            "prep" => compare_channels(name, &dsp::prep(&chs, sr, &dsp::PrepParams::from_json(&a["p"])), &want_chs(), w),
            "encodeWav" => {
                let b = dsp::encode_wav_seeded(&chs, sr, n("bits") as u32, n("seed") as u32).unwrap();
                compare_wav(name, &b, &data.bytes(&out["bytes"]), w);
            }
            "mix" => {
                let m = render_mix(&lanes(&data, a), &MixArgs::from_json(a)).unwrap();
                compare_wav(name, &m.wav, &data.bytes(&out["bytes"]), w);
                compare_json(name, "info", &serde_json::json!({ "over": m.over }), &out["info"], w);
            }
            "mixdown" => compare_channels(name, &mixdown(&lanes(&data, a), sr, n("seconds")), &want_chs(), w),
            "fitPeak" => {
                let mut c = chs;
                let over = fit_peak(&mut c);
                compare_channels(name, &c, &want_chs(), w);
                compare_json(name, "info", &serde_json::json!({ "over": over }), &out["info"], w);
            }
            f => panic!("{name}: no Rust case for {f}"),
        }
    }
    println!("{} cases", cases.len());
    for (k, w) in &worst {
        println!("{k:18} sample {:.3e}  scalar(rel) {:.3e}  wav {} LSB  pixel {}", w.sample, w.scalar, w.lsb, w.pixel);
    }
}
