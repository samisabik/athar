//! Search the Athar library from the terminal with the Rust core. Reads the app's database
//! read-only and plays the best match with afplay.
//!
//!   cargo run --release --example search -- deep punchy kick drum
//!   cargo run --release --example search -- --like ~/Samples/kick.wav
//!   cargo run --release --example search -- --quiet rain on a window

use athar_core::decode::{clips, decode, load_audio};
use athar_core::embed::Clap;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let quiet = args.iter().any(|a| a == "--quiet");
    args.retain(|a| a != "--quiet");
    let like = args.first().map(|a| a == "--like").unwrap_or(false);
    if args.is_empty() || (like && args.len() < 2) {
        eprintln!("usage: search [--quiet] WORDS...  |  search [--quiet] --like FILE");
        std::process::exit(2);
    }
    let home = std::env::var("HOME")?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let model = std::env::var("ATHAR_MODEL").map(PathBuf::from).unwrap_or(root.join("build/model"));

    let t = Instant::now();
    let clap = Clap::load(&model)?;
    println!("model loaded from {} in {:.1} s", model.display(), t.elapsed().as_secs_f32());

    let t = Instant::now();
    let query = if like {
        let d = decode(Path::new(&args[1]))?;
        let y = load_audio(&d)?;
        let c = clips(&d, &y)?;
        let v = clap.embed_sound(&c)?;
        println!("{} decoded and embedded ({} clip{}) in {} ms", args[1], c.len(), if c.len() > 1 { "s" } else { "" }, t.elapsed().as_millis());
        v
    } else {
        let words = args.join(" ");
        let v = clap.embed_texts(&[&words])?.remove(0);
        println!("\"{words}\" embedded in {} ms", t.elapsed().as_millis());
        v
    };

    let t = Instant::now();
    let db = Connection::open_with_flags(format!("{home}/Library/Application Support/Athar/index.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = db.prepare("SELECT path, duration, emb FROM files WHERE emb IS NOT NULL")?;
    let mut hits: Vec<(f32, String, f64)> = stmt
        .query_map([], |r| {
            let blob: Vec<u8> = r.get(2)?;
            let emb = blob.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            let score = emb.zip(&query).map(|(a, b)| a * b).sum::<f32>();
            Ok((score, r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
        })?
        .filter_map(Result::ok)
        .collect();
    let total = hits.len();
    hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!("{total} sounds ranked in {} ms\n", t.elapsed().as_millis());

    let skip = if like { 1 } else { 0 };
    for (score, path, dur) in hits.iter().skip(skip).take(12) {
        let p = Path::new(path);
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        let folder = p.parent().and_then(|f| f.file_name()).unwrap_or_default().to_string_lossy();
        println!("  {score:.3}  {dur:>6.2}s  {name}  ({folder})");
    }
    if let Some((_, path, _)) = hits.get(skip) {
        if !quiet {
            println!("\nplaying {}", Path::new(path).file_name().unwrap_or_default().to_string_lossy());
            Command::new("afplay").arg(path).status()?;
        }
    }
    Ok(())
}
