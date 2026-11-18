//! Indexing: walking folders, analysing and embedding files, and following files that move.

use athar_core::analysis::{analyze, peaks};
use athar_core::decode::{clips, decode, load_audio};
use rayon::prelude::*;
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use super::paths::{basename, is_audio, is_inside, mtime_of};
use super::store::{Meta, emb_blob};
use super::{Embed, Library, Res, has_row, now, query_one, query_rows, report};

const BATCH: usize = 32;

const HASH_SPAN: u64 = 1 << 20;

struct FileRow {
    meta: Meta,
    mtime: f64,
    size: i64,
    sha1: Option<String>,
    /// Why the file could not be indexed. Kept with the file's time and size, so it is not
    /// tried again until it changes.
    error: Option<String>,
}

enum Prepared {
    Ok(FileRow, Vec<f32>),
    Failed(FileRow),
    Skip,
}

impl Library {
    /// Walks `dir`, the library folder `folder` or a folder inside it: files that are new or
    /// changed are indexed, and files that are gone are forgotten.
    pub(super) fn scan(&self, folder: &str, dir: &str, embed: Embed) {
        if !Path::new(dir).is_dir() {
            return;
        }
        let others: HashSet<String> = self.folders().into_iter().filter(|f| f != folder).collect();
        let known: HashMap<String, (f64, i64)> = self
            .rows("SELECT path, mtime, size FROM files WHERE folder=?", [folder], |r| {
                Ok((r.get::<_, String>(0)?, (r.get::<_, Option<f64>>(1)?.unwrap_or(0.0), r.get::<_, Option<i64>>(2)?.unwrap_or(0))))
            })
            .into_iter()
            .filter(|(p, _)| dir == folder || is_inside(p, dir))
            .collect();
        let mut seen = HashSet::new();
        let mut todo = Vec::new();
        let mut stack = vec![PathBuf::from(dir)];
        let mut unreadable = Vec::new();
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                unreadable.push(dir.to_string_lossy().into_owned());
                continue;
            };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let p = e.path();
                let ps = p.to_string_lossy().into_owned();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if !others.contains(&ps) {
                        stack.push(p);
                    }
                    continue;
                }
                if !is_audio(&p) {
                    continue;
                }
                let Ok(md) = std::fs::metadata(&p) else { continue };
                seen.insert(ps.clone());
                if known.get(&ps) != Some(&(mtime_of(&md), md.size() as i64)) {
                    todo.push(ps);
                }
            }
        }
        for gone in known.keys().filter(|k| !seen.contains(*k)) {
            if !unreadable.iter().any(|u| gone == u || is_inside(gone, u)) {
                self.forget(gone, false);
            }
        }
        todo.sort();
        // Files indexed before recording dates were kept get theirs now.
        let undated: Vec<String> = self
            .rows("SELECT path FROM files WHERE folder=? AND recorded IS NULL AND emb IS NOT NULL", [folder], |r| r.get(0))
            .into_iter()
            .filter(|p| seen.contains(p) && todo.binary_search(p).is_err())
            .collect();
        for p in undated {
            let rec = recorded_date(&p);
            report("dating a file", self.exec("UPDATE files SET recorded=? WHERE path=?", params![rec, p]));
            let mut s = self.store.write().unwrap();
            if let Some(&i) = s.pos.get(&p) {
                s.items[i].meta.recorded = rec;
            }
        }
        self.index(folder, todo, embed);
    }

    pub(super) fn index(&self, folder: &str, paths: Vec<String>, embed: Embed) {
        {
            let mut s = self.status.lock().unwrap();
            s.folder = Some(folder.to_string());
            s.queued = paths.len();
            s.done = 0;
        }
        for (b, batch) in paths.chunks(BATCH).enumerate() {
            if !self.folders().iter().any(|f| f == folder) {
                return;
            }
            self.status.lock().unwrap().current = batch.first().map(|p| basename(p));
            // Decoding, analysis and embedding run side by side; the model sessions are shared out.
            let prepared: Vec<Prepared> = batch.par_iter().map(|p| prepare(folder, p, embed)).collect();
            self.save_batch(prepared);
            self.status.lock().unwrap().done = ((b + 1) * BATCH).min(paths.len());
        }
    }

    /// Saves a batch of prepared files in one transaction, then shows them in the store.
    fn save_batch(&self, batch: Vec<Prepared>) {
        let mut indexed = Vec::new();
        let mut failed = Vec::new();
        let saved = self.write(|db| {
            for p in batch {
                match p {
                    Prepared::Skip => {}
                    Prepared::Failed(row) => {
                        eprintln!("library: {} was not indexed: {}", row.meta.path, row.error.as_deref().unwrap_or(""));
                        save_row(db, &row, None)?;
                        failed.push(row.meta.path);
                    }
                    Prepared::Ok(row, emb) => {
                        if !Path::new(&row.meta.path).exists() {
                            continue;
                        }
                        let before: Option<String> = query_one(db, "SELECT sha1 FROM files WHERE path=?", [&row.meta.path], |r| r.get(0)).flatten();
                        save_row(db, &row, Some(&emb))?;
                        let replaced = before.is_some() && before != row.sha1;
                        let moved_from = reattach(db, &row.meta.path, row.sha1.as_deref(), replaced)?;
                        indexed.push((row.meta, emb, moved_from));
                    }
                }
            }
            Ok(())
        });
        if report("saving indexed files", saved).is_none() {
            return;
        }
        for (meta, emb, moved_from) in indexed {
            if let Some(old) = moved_from {
                self.move_in_arrangement(&old, &meta.path);
            }
            let cat = self.category_of(&emb);
            self.store.write().unwrap().put(meta, emb, cat);
        }
        let mut s = self.store.write().unwrap();
        for p in failed {
            s.drop_path(&p);
        }
    }

    /// Remove a file from the index. Its tags, note, rating and project places are kept, and its
    /// content hash remembered, so they come back if the file reappears here or elsewhere.
    /// With `purge` they go too.
    pub(super) fn forget(&self, path: &str, purge: bool) {
        let done = self.write(|db| {
            let sha: Option<String> = query_one(db, "SELECT sha1 FROM files WHERE path=?", [path], |r| r.get(0)).flatten();
            db.execute("DELETE FROM files WHERE path=?", [path])?;
            if purge {
                for t in ["catalog", "regions", "project_items", "gone"] {
                    db.execute(&format!("DELETE FROM {t} WHERE path=?"), [path])?;
                }
            } else if let Some(s) = sha {
                db.execute("INSERT OR REPLACE INTO gone VALUES (?, ?, ?)", params![path, s, now()])?;
            }
            Ok(())
        });
        report("forgetting a file", done);
        self.store.write().unwrap().drop_path(path);
    }
}

/// Identity of a file's content: its size plus its first and last megabyte.
pub(super) fn file_hash(path: &str, size: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut h = sha1_smol::Sha1::new();
    h.update(size.to_string().as_bytes());
    let mut f = std::fs::File::open(path)?;
    let mut head = Vec::new();
    (&mut f).take(HASH_SPAN).read_to_end(&mut head)?;
    h.update(&head);
    if size > 2 * HASH_SPAN {
        f.seek(SeekFrom::Start(size - HASH_SPAN))?;
        let mut tail = Vec::new();
        f.take(HASH_SPAN).read_to_end(&mut tail)?;
        h.update(&tail);
    }
    Ok(h.digest().to_string())
}

/// Recording time from a WAV's BWF 'bext' chunk, otherwise the file's creation time.
fn recorded_date(path: &str) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let lower = path.to_lowercase();
    if lower.ends_with(".wav") || lower.ends_with(".wave") || lower.ends_with(".bwf") {
        if let Ok(mut f) = std::fs::File::open(path) {
            let mut head = [0u8; 12];
            if f.read_exact(&mut head).is_ok() && (&head[..4] == b"RIFF" || &head[..4] == b"RF64") && &head[8..12] == b"WAVE" {
                for _ in 0..64 {
                    let mut hdr = [0u8; 8];
                    if f.read_exact(&mut hdr).is_err() {
                        break;
                    }
                    let size = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as u64;
                    if &hdr[..4] == b"bext" {
                        let mut data = vec![0u8; size.min(340) as usize];
                        let _ = f.read_exact(&mut data);
                        if data.len() >= 338 {
                            let d: String = String::from_utf8_lossy(&data[320..330]).trim_matches(['\0', ' ']).to_string();
                            let t: String = String::from_utf8_lossy(&data[330..338]).trim_matches(['\0', ' ']).to_string();
                            if d.len() == 10 && d.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                                let t5: String = t.chars().take(5).collect::<String>().replace(['-', '.'], ":");
                                return Some(format!("{} {}", d.replace([':', '/'], "-"), t5).trim().to_string());
                            }
                        }
                        break;
                    }
                    if f.seek(SeekFrom::Current((size + (size & 1)) as i64)).is_err() {
                        break;
                    }
                }
            }
        }
    }
    let md = std::fs::metadata(path).ok()?;
    let t = md.created().or_else(|_| md.modified()).ok()?;
    let dt: chrono::DateTime<chrono::Local> = t.into();
    Some(dt.format("%Y-%m-%d %H:%M").to_string())
}

/// Decode, analyse and embed one file; runs on the rayon pool.
fn prepare(folder: &str, p: &str, embed: Embed) -> Prepared {
    let Ok(md) = std::fs::metadata(p) else { return Prepared::Skip };
    let base = FileRow { meta: Meta { path: p.into(), folder: folder.into(), ..Default::default() }, mtime: mtime_of(&md), size: md.size() as i64, sha1: None, error: None };
    let run = || -> Res<(FileRow, Vec<f32>)> {
        let d = decode(Path::new(p)).map_err(|e| e.to_string())?;
        let y = load_audio(&d).map_err(|e| e.to_string())?;
        let a = analyze(&y, d.duration(), &basename(p)).map_err(|e| e.to_string())?;
        let emb = embed(&clips(&d, &y).map_err(|e| e.to_string())?)?;
        let meta = Meta {
            path: p.into(),
            folder: folder.into(),
            duration: Some(d.duration()),
            sr: Some(d.sr as i64),
            channels: Some(d.channels.len() as i64),
            rms_db: Some(a.rms_db),
            brightness: Some(a.brightness),
            pitch_hz: a.pitch_hz,
            tempo: a.tempo,
            is_loop: a.is_loop,
            peaks: peaks(&d, &y),
            recorded: recorded_date(p),
        };
        Ok((FileRow { meta, mtime: base.mtime, size: base.size, sha1: file_hash(p, base.size as u64).ok(), error: None }, emb))
    };
    match run() {
        Ok((row, emb)) => Prepared::Ok(row, emb),
        Err(e) => Prepared::Failed(FileRow { error: Some(e), ..base }),
    }
}

fn save_row(db: &Connection, row: &FileRow, emb: Option<&[f32]>) -> rusqlite::Result<()> {
    let m = &row.meta;
    db.execute(
        "INSERT OR REPLACE INTO files (path, folder, mtime, size, sha1, duration, sr, channels, rms_db, brightness, pitch_hz,
         tempo, is_loop, emb, peaks, error, recorded)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            m.path, m.folder, row.mtime, row.size, row.sha1, m.duration, m.sr, m.channels, m.rms_db, m.brightness, m.pitch_hz,
            m.tempo, m.is_loop as i64, emb.map(emb_blob), if row.error.is_some() { None } else { Some(&m.peaks) }, row.error, m.recorded
        ],
    )?;
    Ok(())
}

/// True when both paths lead to one file: two spellings of a name, or two links to it.
fn same_file(a: &str, b: &str) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Moves the row that `table` keeps for `old` to `new`. A row already at `new` stays, and the
/// one for `old` with it, unless `overwrite` is set.
pub(super) fn rekey(db: &Connection, table: &str, old: &str, new: &str, overwrite: bool) -> rusqlite::Result<()> {
    if overwrite {
        db.execute(&format!("DELETE FROM {table} WHERE path=?"), [new])?;
    }
    db.execute(&format!("UPDATE OR IGNORE {table} SET path=? WHERE path=?"), [new, old])?;
    Ok(())
}

/// Gives `new` the places `old` has in projects; where `new` is in a project already, it
/// keeps its own.
pub(super) fn move_project_places(db: &Connection, old: &str, new: &str) -> rusqlite::Result<()> {
    db.execute("UPDATE OR IGNORE project_items SET path=? WHERE path=?", [new, old])?;
    db.execute("DELETE FROM project_items WHERE path=?", [old])?;
    Ok(())
}

/// Give a newly indexed file the tags, regions and project places of a vanished file with
/// the same content, and return that file's path. Its own tags give way only when replaced
/// is true: other content was indexed at this path before, so the vanished file was moved
/// over it.
fn reattach(db: &Connection, path: &str, sha1: Option<&str>, replaced: bool) -> rusqlite::Result<Option<String>> {
    db.execute("DELETE FROM gone WHERE path=?", [path])?;
    let Some(sha1) = sha1 else { return Ok(None) };
    for old in query_rows(db, "SELECT path FROM gone WHERE sha1=? ORDER BY at DESC", [sha1], |r| r.get::<_, String>(0)) {
        db.execute("DELETE FROM gone WHERE path=?", [&old])?;
        // Still there, the vanished file came back and this one is a copy of it. A path that
        // leads to this very file is another spelling of its name, so the file was renamed.
        if Path::new(&old).exists() && !same_file(&old, path) {
            continue;
        }
        for t in ["catalog", "regions"] {
            rekey(db, t, &old, path, replaced && has_row(db, t, &old))?;
        }
        move_project_places(db, &old, path)?;
        return Ok(Some(old));
    }
    Ok(None)
}
