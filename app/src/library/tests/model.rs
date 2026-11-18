//! With the CLAP model: a real index run, live watching, and a benchmark run by hand.

use crate::library::paths::model_dir;
use crate::library::search::SearchQuery;
use crate::library::store::dot;
use crate::library::Library;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use super::{Tmp, noise, set, write_wav};

fn wait(seconds: u64, cond: impl Fn() -> bool) -> bool {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    while std::time::Instant::now() < end {
        if cond() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    cond()
}

/// A library with the real model, watcher and worker, on a temporary data folder. None, with
/// a note printed, where the model has not been built.
fn running(root: &Path) -> Option<Arc<Library>> {
    let model = model_dir(None);
    if !model.join("audio.onnx").exists() {
        eprintln!("skipped: the CLAP model is not exported to {}", model.display());
        return None;
    }
    let (lib, rx) = Library::create(root.join("home"), model).unwrap();
    lib.update_settings(&json!({"projects_dir": root.join("Projects"), "rescan_on_open": false})).unwrap();
    let l = lib.clone();
    std::thread::spawn(move || l.boot(rx));
    assert!(wait(120, || lib.status()["model"] != "loading"));
    assert_eq!(lib.status()["model"], "ready", "{}", lib.status()["error"]);
    Some(lib)
}

fn gaussian(seed: u64, n: usize) -> Vec<f32> {
    let u = noise(seed, 2 * n);
    u.chunks_exact(2).map(|p| {
        let (a, b) = ((p[0] + 1.0) / 2.0 + 1e-7, (p[1] + 1.0) / 2.0);
        (-2.0 * a.ln()).sqrt() * (std::f32::consts::TAU * b).cos()
    }).collect()
}

#[test]
fn a_real_index_run_hears_the_whole_file() {
    let t = Tmp::new();
    let folder = t.0.join("sounds");
    let tone = |seconds: usize, tail: bool| -> Vec<f32> {
        let mut y: Vec<f32> = (0..48_000 * seconds).map(|i| 0.3 * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin()).collect();
        if tail {
            let n = y.len();
            for (v, g) in y[n - 48_000 * 20..].iter_mut().zip(gaussian(0, 48_000 * 20)) {
                *v = 0.3 * g;
            }
        }
        y
    };
    write_wav(&folder.join("short.wav"), &tone(2, false), 48_000, 32);
    write_wav(&folder.join("long_tone_then_noise.wav"), &tone(120, true), 48_000, 32);
    write_wav(&folder.join("long_tone.wav"), &tone(120, false), 48_000, 32);
    let Some(lib) = running(&t.0) else { return };
    lib.add_folder(&folder.to_string_lossy()).unwrap();
    assert!(wait(120, || lib.store.read().unwrap().items.len() >= 3));
    let all = lib.search(&SearchQuery { sort: Some("name".into()), ..Default::default() }, None, None, None).unwrap();
    let rows: HashMap<String, Value> = all.as_array().unwrap().iter().map(|r| (r["name"].as_str().unwrap().to_string(), r.clone())).collect();
    let mut found: Vec<&str> = rows.keys().map(String::as_str).collect();
    found.sort();
    assert_eq!(found, vec!["long_tone.wav", "long_tone_then_noise.wav", "short.wav"]);
    let unhashed: i64 = lib.db().query_row("SELECT COUNT(*) FROM files WHERE sha1 IS NULL OR sha1 = ''", [], |r| r.get(0)).unwrap();
    assert_eq!(unhashed, 0);
    // Peaks cover the whole file: the noisy last 20 s of 120 s is the last sixth of the bins.
    let pk: Vec<f64> = rows["long_tone_then_noise.wav"]["peaks"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    assert!(mean(&pk[pk.len() - 15..]) > mean(&pk[..60]) * 1.5);
    // The noisy ending changes the embedding, which it could not when only the opening was heard.
    let a = lib.embedding_of(rows["long_tone.wav"]["path"].as_str().unwrap()).unwrap();
    let b = lib.embedding_of(rows["long_tone_then_noise.wav"]["path"].as_str().unwrap()).unwrap();
    assert!((dot(&a, &a).sqrt() - 1.0).abs() < 1e-4);
    assert!(dot(&a, &b) < 0.999);
}

#[test]
fn finder_style_changes_keep_metadata() {
    let t = Tmp::new();
    let folder = t.0.join("sounds");
    std::fs::create_dir(&folder).unwrap();
    let f = folder.to_string_lossy().into_owned();
    let Some(lib) = running(&t.0) else { return };
    lib.add_folder(&f).unwrap();
    assert!(wait(30, || lib.watcher.lock().unwrap().is_some()));
    // The watch request reaches the watcher thread on its own time.
    std::thread::sleep(std::time::Duration::from_secs(1));
    let write = |p: &Path, seed: u64| write_wav(p, &noise(seed, 48_000).iter().map(|v| v * 0.1).collect::<Vec<_>>(), 48_000, 32);
    let indexed = |p: &Path| lib.embedding_of(&p.to_string_lossy()).is_some();
    let meta = |p: &Path| {
        let c = lib.catalog_get(&p.to_string_lossy());
        (c["tags"].clone(), c["rating"].clone())
    };
    let kept = (json!(["kyoto"]), json!(5));
    let paths = |pid: i64| lib.project_paths(pid);

    let a = folder.join("rain.wav");
    write(&a, 1);
    assert!(wait(30, || indexed(&a)), "a new file is indexed without a rescan");
    let pid = lib.create_project("kit").unwrap()["id"].as_i64().unwrap();
    lib.set_project_folder(pid, &f).unwrap();
    lib.catalog_set(&a.to_string_lossy(), &json!({"tags": ["kyoto"], "rating": 5})).unwrap();
    lib.project_add(pid, &a.to_string_lossy()).unwrap();

    // Renamed in Finder.
    let b = folder.join("rain kyoto.wav");
    std::fs::rename(&a, &b).unwrap();
    assert!(wait(30, || indexed(&b) && !indexed(&a)));
    assert_eq!(meta(&b), kept);
    assert_eq!(paths(pid), set(&[&b.to_string_lossy()]));

    // Saved over by another app: a temporary file, then a rename onto the original.
    let stamp = || -> Option<f64> { lib.db().query_row("SELECT mtime FROM files WHERE path=? AND emb IS NOT NULL", [b.to_string_lossy()], |r| r.get(0)).ok() };
    let before = stamp();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let tmp = folder.join(".rain kyoto.wav.tmp");
    write(&tmp, 2);
    std::fs::rename(&tmp, &b).unwrap();
    assert!(wait(30, || stamp().is_some() && stamp() != before), "the new content is re-indexed");
    assert_eq!(meta(&b), kept);
    assert_eq!(paths(pid), set(&[&b.to_string_lossy()]));

    // Moved into a subfolder.
    let c = folder.join("takes/rain kyoto.wav");
    std::fs::create_dir(folder.join("takes")).unwrap();
    std::fs::rename(&b, &c).unwrap();
    assert!(wait(30, || indexed(&c) && !indexed(&b)));
    assert_eq!(meta(&c), kept);
    assert_eq!(paths(pid), set(&[&c.to_string_lossy()]));

    // Deleted, then put back as from the Trash.
    let data = std::fs::read(&c).unwrap();
    std::fs::remove_file(&c).unwrap();
    assert!(wait(30, || !indexed(&c)));
    assert_eq!(lib.all_tags(), json!([]));
    std::fs::write(&c, data).unwrap();
    assert!(wait(30, || indexed(&c)));
    assert_eq!(meta(&c), kept);
    assert_eq!(paths(pid), set(&[&c.to_string_lossy()]));
}

/// Times a fresh library on a real folder: model load, import until idle, text and similar
/// searches, the first Map. Prints one JSON line. Run with
/// `ATHAR_BENCH_DIR=<folder> cargo test --release bench_ -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_a_fresh_library_on_a_real_folder() {
    let Ok(dir) = std::env::var("ATHAR_BENCH_DIR") else { return };
    let t = Tmp::new();
    let t0 = std::time::Instant::now();
    let Some(lib) = running(&t.0) else { return };
    let model_s = t0.elapsed().as_secs_f64();
    let t1 = std::time::Instant::now();
    lib.add_folder(&dir).unwrap();
    // Idle: nothing queued, and the count unchanged for two seconds.
    let mut last = (usize::MAX, std::time::Instant::now());
    loop {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let n = lib.store.read().unwrap().items.len();
        let pending = lib.status()["pending_jobs"].as_u64().unwrap_or(0);
        if n != last.0 || pending > 0 {
            last = (n, std::time::Instant::now());
        } else if last.1.elapsed().as_secs_f64() >= 2.0 {
            break;
        }
    }
    let import_s = last.1.duration_since(t1).as_secs_f64();
    let n = last.0;
    let queries = ["deep punchy kick drum", "rain on a window", "warm analog pad", "vocal chop", "metallic impact", "glitchy texture",
        "808 bass", "bright hi-hat", "dark drone", "riser", "snare with reverb", "plucked synth", "crowd noise", "wind", "laser zap",
        "piano chord", "dusty vinyl loop", "sub bass", "clap", "whoosh"];
    let mut text_ms = vec![];
    let mut tops = vec![];
    for q in queries {
        let s = std::time::Instant::now();
        let v = lib.embed_text(q).unwrap();
        let r = lib.search(&SearchQuery { limit: Some(200), ..Default::default() }, Some(v), None, Some(q)).unwrap();
        text_ms.push(s.elapsed().as_secs_f64() * 1000.0);
        tops.push(r.as_array().unwrap().iter().take(5).map(|x| x["name"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>());
    }
    let all = lib.search(&SearchQuery { sort: Some("name".into()), limit: Some(100000), ..Default::default() }, None, None, None).unwrap();
    let paths: Vec<String> = all.as_array().unwrap().iter().step_by((n / 20).max(1)).take(20).map(|x| x["path"].as_str().unwrap().to_string()).collect();
    let mut similar_ms = vec![];
    for p in &paths {
        let s = std::time::Instant::now();
        let v = lib.embedding_of(p).unwrap();
        lib.search(&SearchQuery { limit: Some(200), ..Default::default() }, Some(v), Some(p), None).unwrap();
        similar_ms.push(s.elapsed().as_secs_f64() * 1000.0);
    }
    let s = std::time::Instant::now();
    let map = lib.sound_map();
    let map_s = s.elapsed().as_secs_f64();
    println!("BENCH {}", json!({"files": n, "model_s": model_s, "import_s": import_s, "text_ms": text_ms, "similar_ms": similar_ms,
        "map_s": map_s, "map_points": map.as_array().map_or(0, |a| a.len()), "queries": queries, "tops": tops, "similar_paths": paths}));
}
