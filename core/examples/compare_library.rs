//! Runs the Rust core over sounds already indexed by the Python app and compares the results
//! with what Python stored: embeddings, analysis and peaks. Reads the database read-only.
//!
//!   cargo run --release --example compare_library -- [how many, default 200]

use athar_core::analysis::{analyze, peaks};
use athar_core::decode::{clips, decode, load_audio};
use athar_core::embed::Clap;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let limit: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(200);
    let home = std::env::var("HOME")?;
    let db = Connection::open_with_flags(format!("{home}/Library/Application Support/Athar/index.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let clap = Clap::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../build/model"))?;
    let mut stmt = db.prepare(
        "SELECT path, emb, brightness, pitch_hz, tempo, onsets, is_loop, peaks FROM files
         WHERE emb IS NOT NULL ORDER BY sha1 LIMIT ?",
    )?;
    type Row = (String, Vec<u8>, f64, Option<f64>, Option<f64>, i64, i64, Vec<u8>);
    let rows: Vec<Row> = stmt
        .query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)))?
        .filter_map(Result::ok)
        .filter(|r| Path::new(&r.0).exists())
        .collect();
    let (mut n, mut failed, mut cos_min, mut cos_sum) = (0, 0, 1.0f32, 0.0f32);
    let (mut onsets_same, mut pitch_same, mut tempo_same, mut loop_same, mut bright_close, mut peaks_close) = (0, 0, 0, 0, 0, 0);
    let t = Instant::now();
    for (path, emb, bright, pitch, tempo, onsets, is_loop, pk) in &rows {
        let p = Path::new(path);
        let Ok(d) = decode(p) else {
            failed += 1;
            println!("cannot decode {}", p.file_name().unwrap_or_default().to_string_lossy());
            continue;
        };
        let y = load_audio(&d)?;
        let v = clap.embed_sound(&clips(&d, &y)?)?;
        let stored: Vec<f32> = emb.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let c: f32 = v.iter().zip(&stored).map(|(a, b)| a * b).sum();
        let a = analyze(&y, d.duration(), &p.file_name().unwrap_or_default().to_string_lossy())?;
        n += 1;
        cos_sum += c;
        cos_min = cos_min.min(c);
        onsets_same += (a.onsets as i64 == *onsets) as usize;
        pitch_same += match (a.pitch_hz, pitch) {
            (None, None) => 1,
            (Some(x), Some(y)) => ((x - y).abs() / y < 0.01) as usize,
            _ => 0,
        };
        tempo_same += (a.tempo.map(|v| (v * 100.0).round()) == tempo.map(|v| (v * 100.0).round())) as usize;
        loop_same += (a.is_loop as i64 == *is_loop) as usize;
        bright_close += ((a.brightness - bright).abs() / bright.max(1.0) < 0.03) as usize;
        let pr = peaks(&d, &y);
        peaks_close += pr.iter().zip(pk).all(|(a, b)| (*a as i32 - *b as i32).abs() <= 2) as usize;
        if c < 0.999 {
            println!("embedding cosine {c:.4}: {}", p.file_name().unwrap_or_default().to_string_lossy());
        }
    }
    let pct = |k: usize| 100.0 * k as f64 / n.max(1) as f64;
    println!("\n{n} sounds in {:.1} s ({failed} could not be decoded)", t.elapsed().as_secs_f32());
    println!("embedding cosine: mean {:.5}, lowest {cos_min:.5}", cos_sum / n.max(1) as f32);
    println!("same onset count {:.0}%, same pitch {:.0}%, same tempo {:.0}%, same loop flag {:.0}%", pct(onsets_same), pct(pitch_same), pct(tempo_same), pct(loop_same));
    println!("brightness within 3% {:.0}%, waveform overview within 2/255 {:.0}%", pct(bright_close), pct(peaks_close));
    Ok(())
}
