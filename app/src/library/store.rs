//! The sounds held in memory for search: what is known about each, and its embedding.

use std::collections::HashMap;
use std::path::{Path, MAIN_SEPARATOR};
use super::paths::{basename, nfc};

pub(super) const EMB_DIM: usize = 512;

#[derive(Clone, Default)]
pub(super) struct Meta {
    pub(super) path: String,
    pub(super) folder: String,
    pub(super) duration: Option<f64>,
    pub(super) sr: Option<i64>,
    pub(super) channels: Option<i64>,
    pub(super) rms_db: Option<f64>,
    pub(super) brightness: Option<f64>,
    pub(super) pitch_hz: Option<f64>,
    pub(super) tempo: Option<f64>,
    pub(super) is_loop: bool,
    pub(super) peaks: Vec<u8>,
    pub(super) recorded: Option<String>,
}

pub(super) struct Item {
    pub(super) meta: Meta,
    pub(super) emb: Vec<f32>,
    /// Index into the category names.
    pub(super) cat: Option<usize>,
    /// Name and subfolder in lowercase NFC, for matching typed words.
    pub(super) words: String,
}

impl Item {
    pub(super) fn new(meta: Meta, emb: Vec<f32>, cat: Option<usize>) -> Self {
        let words = words_of(&meta.path, &meta.folder);
        Self { meta, emb, cat, words }
    }
}

#[derive(Default)]
pub(super) struct Store {
    pub(super) items: Vec<Item>,
    pub(super) pos: HashMap<String, usize>,
    pub(super) version: u64,
}

impl Store {
    pub(super) fn put(&mut self, meta: Meta, emb: Vec<f32>, cat: Option<usize>) {
        self.version += 1;
        match self.pos.get(&meta.path) {
            Some(&i) => self.items[i] = Item::new(meta, emb, cat),
            None => {
                self.pos.insert(meta.path.clone(), self.items.len());
                self.items.push(Item::new(meta, emb, cat));
            }
        }
    }

    pub(super) fn drop_path(&mut self, path: &str) {
        if let Some(i) = self.pos.remove(path) {
            self.version += 1;
            let last = self.items.len() - 1;
            self.items.swap_remove(i);
            if i != last {
                let moved = self.items[i].meta.path.clone();
                self.pos.insert(moved, i);
            }
        }
    }

    pub(super) fn rename(&mut self, old: &str, new: &str, folder: &str) {
        if old == new {
            return;
        }
        self.drop_path(new);
        if let Some(i) = self.pos.remove(old) {
            self.version += 1;
            let it = &mut self.items[i];
            it.meta.path = new.to_string();
            it.meta.folder = folder.to_string();
            it.words = words_of(new, folder);
            self.pos.insert(new.to_string(), i);
        }
    }
}

/// The folders between a library folder and a file in it; empty for a file at its top.
pub(super) fn sub_of(path: &str, folder: &str) -> String {
    if folder.is_empty() {
        return String::new();
    }
    let dir = Path::new(&nfc(path)).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let f = nfc(folder);
    let f = f.trim_end_matches(MAIN_SEPARATOR);
    dir.get(f.len() + 1..).unwrap_or("").to_string()
}

fn words_of(path: &str, folder: &str) -> String {
    nfc(&format!("{} {}", basename(path), sub_of(path, folder)).to_lowercase())
}

pub(super) fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

pub(super) fn row_meta(r: &rusqlite::Row) -> rusqlite::Result<(Meta, Vec<u8>)> {
    Ok((
        Meta {
            path: r.get("path")?,
            folder: r.get::<_, Option<String>>("folder")?.unwrap_or_default(),
            duration: r.get("duration")?,
            sr: r.get("sr")?,
            channels: r.get("channels")?,
            rms_db: r.get("rms_db")?,
            brightness: r.get("brightness")?,
            pitch_hz: r.get("pitch_hz")?,
            tempo: r.get("tempo")?,
            is_loop: r.get::<_, Option<i64>>("is_loop")?.unwrap_or(0) != 0,
            peaks: r.get::<_, Option<Vec<u8>>>("peaks")?.unwrap_or_default(),
            recorded: r.get("recorded")?,
        },
        r.get::<_, Vec<u8>>("emb")?,
    ))
}

pub(super) fn emb_of(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

pub(super) fn emb_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}
