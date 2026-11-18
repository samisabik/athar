//! The library's behaviour. Each test runs on its own temporary data folder and sound folder.
//! Most index with a stand-in embedding; the two that index and watch for real load the CLAP
//! model, and pass without checking anything where the model has not been built.

mod arrangement;
mod backup;
mod catalog;
mod export;
mod index;
mod model;
mod other_modules;
mod paths;
mod projects;
mod search;
mod settings;

use crate::library::paths::basename;
use crate::library::search::SearchQuery;
use crate::library::store::{EMB_DIM, dot};
use crate::library::{Job, Library, Res};
use crate::testkit::{Tmp, wav_bytes, write_wav};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

/// Values between -1 and 1 that depend only on the seed.
fn xorshift(seed: u64) -> impl Iterator<Item = f32> {
    let mut s = seed | 1;
    std::iter::repeat_with(move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    })
}

/// An embedding that depends only on the audio, unit length, without the model.
fn stand_in(clips: &[Vec<f32>]) -> Res<Vec<f32>> {
    let bytes: Vec<u8> = clips.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    let d = sha1_smol::Sha1::from(&bytes).digest().bytes();
    let v: Vec<f32> = xorshift(u64::from_le_bytes(d[..8].try_into().unwrap())).take(EMB_DIM).collect();
    let norm = dot(&v, &v).sqrt();
    Ok(v.into_iter().map(|x| x / norm).collect())
}

fn noise(seed: u64, frames: usize) -> Vec<f32> {
    xorshift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)).take(frames).collect()
}

fn names(rows: &[Value]) -> Vec<String> {
    let mut n: Vec<String> = rows.iter().map(|r| basename(r["path"].as_str().unwrap())).collect();
    n.sort();
    n
}

fn set(paths: &[&str]) -> HashSet<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

/// A library on a temporary data folder with one sound folder, and no model, watcher or worker:
/// jobs wait in `rx` until `drain` runs them with the stand-in embedding.
struct Lab {
    lib: Arc<Library>,
    rx: Receiver<Job>,
    root: PathBuf,
    folder: String,
    _tmp: Tmp,
}

fn lab() -> Lab {
    let tmp = Tmp::new();
    let root = tmp.0.clone();
    let folder = root.join("sounds");
    std::fs::create_dir(&folder).unwrap();
    let (lib, rx) = Library::create(root.join("home"), root.join("no model")).unwrap();
    lib.update_settings(&json!({"projects_dir": root.join("Projects")})).unwrap();
    let folder = folder.to_string_lossy().into_owned();
    lib.db().execute("INSERT INTO folders VALUES (?)", [&folder]).unwrap();
    Lab { lib, rx, root, folder, _tmp: tmp }
}

impl Lab {
    fn path(&self, rel: &str) -> String {
        Path::new(&self.folder).join(rel).to_string_lossy().into_owned()
    }

    /// A tenth of a second of 16-bit noise at 48 kHz.
    fn write(&self, rel: &str, seed: u64) -> String {
        let p = self.path(rel);
        write_wav(Path::new(&p), &noise(seed, 4800).iter().map(|v| v * 0.1).collect::<Vec<_>>(), 48_000, 16);
        p
    }

    fn add(&self, rel: &str, seed: u64) -> String {
        let p = self.write(rel, seed);
        self.index(&[&p]);
        p
    }

    fn index(&self, paths: &[&str]) {
        self.lib.index(&self.folder, paths.iter().map(|p| p.to_string()).collect(), &stand_in);
    }

    fn drain(&self) {
        while let Ok(job) = self.rx.try_recv() {
            self.lib.run_job(job, &stand_in);
        }
    }

    /// What the folder watcher does when it reports these paths as changed.
    fn events(&self, paths: &[&str]) {
        self.lib.changed.lock().unwrap().extend(paths.iter().map(|p| p.to_string()));
        self.lib.flush_changes();
        self.drain();
    }

    fn tag(&self, path: &str, tags: &[&str], rating: i64) {
        self.lib.catalog_set(path, &json!({"tags": tags, "rating": rating})).unwrap();
    }

    fn meta(&self, path: &str) -> (Vec<String>, i64) {
        let c = self.lib.catalog_get(path);
        (serde_json::from_value(c["tags"].clone()).unwrap(), c["rating"].as_i64().unwrap())
    }

    /// A project whose folder is the sound folder, so adding a sound keeps it where it is.
    fn kit(&self, name: &str) -> i64 {
        let pid = self.lib.create_project(name).unwrap()["id"].as_i64().unwrap();
        self.lib.set_project_folder(pid, &self.folder).unwrap();
        self.drain();
        pid
    }

    fn scan(&self) {
        self.lib.scan(&self.folder, &self.folder, &stand_in);
    }

    fn count(&self) -> usize {
        self.lib.store.read().unwrap().items.len()
    }

    fn search(&self, q: SearchQuery, words: Option<&str>) -> Vec<Value> {
        self.lib.search(&q, None, None, words).unwrap().as_array().unwrap().clone()
    }

    fn by_name(&self) -> Vec<Value> {
        self.search(SearchQuery { sort: Some("name".into()), ..Default::default() }, None)
    }
}
