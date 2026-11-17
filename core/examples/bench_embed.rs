//! Clips embedded per second with N model sessions side by side, on sounds from the library.
//!   cargo run --release --example bench_embed -- SESSIONS [FILES]

use athar_core::decode::{clips, decode, load_audio};
use athar_core::embed::Clap;
use rayon::prelude::*;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::time::Instant;

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap_or(0.0) / 1024.0
}

fn main() {
    let pool: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1);
    let n: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(120);
    let home = std::env::var("HOME").unwrap();
    let db = Connection::open_with_flags(format!("{home}/Library/Application Support/Athar/index.db"), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let paths: Vec<String> = db.prepare("SELECT path FROM files WHERE emb IS NOT NULL ORDER BY sha1 LIMIT ?").unwrap().query_map([n as i64], |r| r.get(0)).unwrap().flatten().collect();
    let all: Vec<Vec<f32>> = paths.par_iter().filter_map(|p| decode(Path::new(p)).ok().and_then(|d| load_audio(&d).ok().and_then(|y| clips(&d, &y).ok()))).flatten().collect();
    let before = rss_mb();
    let t = Instant::now();
    let model = std::env::var("BENCH_MODEL").map(std::path::PathBuf::from).unwrap_or(Path::new(env!("CARGO_MANIFEST_DIR")).join("../build/model"));
    let clap = Clap::load_with(&model, pool).unwrap();
    let load = t.elapsed().as_secs_f64();
    let mem = rss_mb() - before;
    let t = Instant::now();
    let _: Vec<Vec<f32>> = all.par_iter().map(|c| clap.embed_clip(c).unwrap()).collect();
    let dt = t.elapsed().as_secs_f64();
    println!("{pool} sessions: {:.1} ms a clip over {} clips ({:.0} clips/s), load {load:.1} s, model memory {mem:.0} MB", dt * 1000.0 / all.len() as f64, all.len(), all.len() as f64 / dt);
}
