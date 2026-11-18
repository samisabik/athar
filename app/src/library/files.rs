//! Library folders and the files in them: adding and removing folders, renaming and trashing files.

use rusqlite::params;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, MAIN_SEPARATOR};
use super::index::{move_project_places, rekey};
use super::paths::{basename, expand, is_inside, mtime_of, nfc, safe_name};
use super::{Job, Library, Res, query_one, query_rows};

/// How many unreadable files a folder names; the rest are only counted.
const FAILED_NAMED: usize = 5;

impl Library {
    pub fn folders(&self) -> Vec<String> {
        self.rows("SELECT path FROM folders ORDER BY path", [], |r| r.get(0))
    }

    /// The library folder that holds `path`, and the path as the index keys it: the folder as
    /// it is stored, then the rest of `path` exactly as given. A file name keeps the spelling
    /// the file system reports, so the watcher and a scan agree on the key of a file whose
    /// name is not in NFC.
    pub(super) fn locate(&self, path: &str, folders: &[String]) -> Option<(String, String)> {
        let folder = folders.iter().filter(|f| is_inside(path, f)).max_by_key(|f| f.len())?;
        let root = folder.trim_end_matches(MAIN_SEPARATOR);
        // Normalizing neither adds nor removes a separator, so the rest of the path starts
        // after as many of them as the folder has.
        let rest = path.splitn(root.matches(MAIN_SEPARATOR).count() + 2, MAIN_SEPARATOR).last()?;
        Some((folder.clone(), format!("{root}{MAIN_SEPARATOR}{rest}")))
    }

    pub fn folder_counts(&self) -> Value {
        let folders = self.folders();
        let counts: HashMap<String, (i64, i64, i64, i64)> = self
            .rows("SELECT folder, COUNT(*), SUM(emb IS NOT NULL), COALESCE(SUM(size), 0), SUM(error IS NOT NULL) FROM files GROUP BY folder", [], |r| {
                Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            })
            .into_iter()
            .collect();
        // The first few files that could not be read, with the reason, for the folder's row to name.
        let mut failed: HashMap<String, Vec<Value>> = HashMap::new();
        for (folder, path, error) in self.rows("SELECT folder, path, error FROM files WHERE error IS NOT NULL ORDER BY path", [], |r| {
            Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        }) {
            let list = failed.entry(folder).or_default();
            if list.len() < FAILED_NAMED {
                list.push(json!({"path": path, "error": error}));
            }
        }
        Value::Array(
            folders
                .iter()
                .map(|f| {
                    let c = counts.get(f).copied().unwrap_or((0, 0, 0, 0));
                    json!({"path": f, "files": c.0, "indexed": c.1, "bytes": c.2, "failed": c.3, "failed_files": failed.get(f).cloned().unwrap_or_default(), "offline": !Path::new(f).is_dir()})
                })
                .collect(),
        )
    }

    pub fn add_folder(&self, path: &str) -> Res<String> {
        let p = std::fs::canonicalize(expand(path)).map_err(|_| format!("Not a folder: {path}"))?;
        if !p.is_dir() {
            return Err(format!("Not a folder: {}", p.display()));
        }
        let p = p.to_string_lossy().into_owned();
        for f in self.folders() {
            if is_inside(&p, &f) {
                return Err(format!("Already in the library as part of {f}"));
            }
            if is_inside(&f, &p) {
                return Err(format!("Contains the library folder {f}; remove that one first"));
            }
        }
        self.exec("INSERT OR IGNORE INTO folders(path) VALUES (?)", [&p])?;
        self.watch(&p);
        self.enqueue_scan(&p, &p);
        Ok(p)
    }

    pub fn remove_folder(&self, path: &str) -> Res<()> {
        let gone = self.write(|db| {
            db.execute("DELETE FROM folders WHERE path=?", [path])?;
            let gone: Vec<String> = query_rows(db, "SELECT path FROM files WHERE folder=?", [path], |r| r.get(0));
            db.execute("DELETE FROM files WHERE folder=?", [path])?;
            Ok(gone)
        });
        let gone = gone.map_err(|e| format!("The folder could not be removed: {e}"))?;
        // Only once the database has let go of it: on a failure the folder is still watched.
        if let Some(w) = self.watcher.lock().unwrap().as_ref() {
            let _ = w.send((false, path.to_string()));
        }
        self.unwatched.lock().unwrap().remove(path);
        let mut s = self.store.write().unwrap();
        for p in gone {
            s.drop_path(&p);
        }
        Ok(())
    }

    pub fn rescan(&self) {
        for f in self.folders() {
            self.enqueue_scan(&f, &f);
        }
    }

    pub fn index_file(&self, path: &str) {
        if let Some((folder, p)) = self.locate(path, &self.folders()) {
            self.enqueue(Job::Files(folder, vec![p]));
        }
    }

    pub fn subfolders(&self, path: &str) -> Value {
        let folders = self.folders();
        let (folder, path) = match folders.iter().find(|f| nfc(f) == nfc(path)) {
            Some(f) => (f.clone(), f.clone()),
            None => match self.locate(path, &folders) {
                Some(x) => x,
                None => return json!([]),
            },
        };
        let base = format!("{}{}", nfc(&path).trim_end_matches(MAIN_SEPARATOR), MAIN_SEPARATOR);
        let mut counts: HashMap<String, i64> = HashMap::new();
        let mut deeper: HashSet<String> = HashSet::new();
        for p in self.rows("SELECT path FROM files WHERE folder=?", [&folder], |r| r.get::<_, String>(0)) {
            let p = nfc(&p);
            let Some(rest) = p.strip_prefix(&base) else { continue };
            let parts: Vec<&str> = rest.split(MAIN_SEPARATOR).collect();
            if parts.len() > 1 {
                *counts.entry(parts[0].to_string()).or_default() += 1;
                if parts.len() > 2 {
                    deeper.insert(parts[0].to_string());
                }
            }
        }
        if let Ok(rd) = std::fs::read_dir(&path) {
            for e in rd.flatten() {
                let name = nfc(&e.file_name().to_string_lossy());
                if e.path().is_dir() && !name.starts_with('.') {
                    counts.entry(name).or_insert(0);
                }
            }
        }
        let mut names: Vec<_> = counts.into_iter().collect();
        names.sort_by_key(|(n, _)| n.to_lowercase());
        Value::Array(names.into_iter().map(|(n, c)| json!({"path": format!("{base}{n}"), "name": n, "files": c, "nested": deeper.contains(&n)})).collect())
    }

    pub(super) fn is_known(&self, path: &str) -> bool {
        if self.store.read().unwrap().pos.contains_key(path) {
            return true;
        }
        let real = std::fs::canonicalize(path).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| path.to_string());
        self.folders().iter().any(|f| {
            let rf = std::fs::canonicalize(f).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| f.clone());
            is_inside(&real, &rf)
        })
    }

    pub fn known_path(&self, path: &str) -> Res<()> {
        if !self.is_known(path) {
            return Err("Path is outside your library folders".into());
        }
        if !Path::new(path).is_file() {
            return Err("File not found".into());
        }
        Ok(())
    }

    pub fn trash(&self, path: &str) -> Res<()> {
        self.known_path(path)?;
        trash::delete(path).map_err(|e| format!("Could not move to Trash: {e}"))?;
        self.forget(path, true);
        Ok(())
    }

    pub fn rename(&self, path: &str, new_stem: &str) -> Res<String> {
        self.known_path(path)?;
        let src = Path::new(path);
        let stem = safe_name(new_stem).trim_matches(['.', ' ']).to_string();
        if stem.is_empty() {
            return Err("Name is empty".into());
        }
        let ext = src.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let target = src.with_file_name(format!("{stem}{ext}"));
        if target == src {
            return Ok(path.into());
        }
        if target.exists() {
            return Err(format!("{} already exists in that folder", basename(&target.to_string_lossy())));
        }
        std::fs::rename(src, &target).map_err(|e| e.to_string())?;
        let new = target.to_string_lossy().into_owned();
        let saved = std::fs::metadata(&target).map_err(|e| e.to_string()).and_then(|md| {
            self.write(|db| {
                let folder: Option<String> = query_one(db, "SELECT folder FROM files WHERE path=?", [path], |r| r.get(0)).flatten();
                for t in ["files", "catalog", "regions"] {
                    rekey(db, t, path, &new, true)?;
                }
                move_project_places(db, path, &new)?;
                db.execute("UPDATE files SET mtime=?, size=? WHERE path=?", params![mtime_of(&md), md.size() as i64, new])?;
                Ok(folder.unwrap_or_default())
            })
        });
        let folder = match saved {
            Ok(folder) => folder,
            Err(e) => {
                // The index still names the old file, so the file takes its old name back.
                let _ = std::fs::rename(&target, src);
                return Err(e);
            }
        };
        self.move_in_arrangement(path, &new);
        self.store.write().unwrap().rename(path, &new, &folder);
        Ok(new)
    }
}
