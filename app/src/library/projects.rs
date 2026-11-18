//! Projects: named sets of sounds, each with its own folder, order and pads.

use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use super::index::file_hash;
use super::paths::{basename, expand, is_inside, nfc, safe_name, unique_path};
use super::{Library, Res, now, report};

pub(super) const SP404_PADS: i64 = 160;

impl Library {
    pub fn projects(&self) -> Value {
        Value::Array(self.rows(
            "SELECT p.id, p.name, p.folder, COUNT(f.path) n, COALESCE(SUM(f.size), 0) bytes FROM projects p
             LEFT JOIN project_items i ON i.project_id=p.id LEFT JOIN files f ON f.path=i.path GROUP BY p.id ORDER BY p.name",
            [],
            |r| {
                Ok(json!({"id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "folder": r.get::<_, Option<String>>(2)?,
                          "n": r.get::<_, i64>(3)?, "bytes": r.get::<_, i64>(4)?}))
            },
        ))
    }

    pub fn project_exists(&self, pid: i64) -> bool {
        self.one("SELECT 1 FROM projects WHERE id=?", [pid], |_| Ok(())).is_some()
    }

    pub(super) fn project_name(&self, pid: i64) -> Option<String> {
        self.one("SELECT name FROM projects WHERE id=?", [pid], |r| r.get(0))
    }

    pub(super) fn project_folder(&self, pid: i64) -> Option<String> {
        self.one("SELECT folder FROM projects WHERE id=?", [pid], |r| r.get::<_, Option<String>>(0)).flatten().filter(|f| !f.is_empty())
    }

    /// The project of that name, made if there is none.
    pub(super) fn project_named(db: &Connection, name: &str) -> rusqlite::Result<i64> {
        db.execute("INSERT OR IGNORE INTO projects(name) VALUES (?)", [name])?;
        db.query_row("SELECT id FROM projects WHERE name=?", [name], |r| r.get(0))
    }

    pub fn create_project(&self, name: &str) -> Res<Value> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Name required".into());
        }
        let fresh = !self.projects().as_array().unwrap().iter().any(|p| p["name"] == name);
        let id = self.write(|db| Self::project_named(db, name))?;
        let folder: Option<String> = self.one("SELECT folder FROM projects WHERE id=?", [id], |r| r.get(0)).flatten();
        let mut p = json!({"id": id, "name": name, "folder": folder});
        if fresh {
            match self.give_project_folder(id, name) {
                Ok(f) => p["folder"] = json!(f),
                Err(e) => p["folder_error"] = json!(e),
            }
        }
        Ok(p)
    }

    /// The project's own folder inside the projects folder from Settings; every sound added
    /// to the project is copied there.
    fn give_project_folder(&self, pid: i64, name: &str) -> Res<String> {
        let base = expand(self.settings()["projects_dir"].as_str().unwrap_or(""));
        let folder = unique_path(&base.join(safe_name(name)))?;
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let base = std::fs::canonicalize(&base).map_err(|e| e.to_string())?.to_string_lossy().into_owned();
        if !self.folders().iter().any(|f| nfc(f) == nfc(&base) || is_inside(&base, f) || is_inside(f, &base)) {
            self.add_folder(&base)?;
        }
        self.set_project_folder(pid, &folder.to_string_lossy())
    }

    pub(super) fn ensure_project_folders(&self) {
        let missing: Vec<(i64, String)> = self.rows("SELECT id, name FROM projects WHERE folder IS NULL OR folder = ''", [], |r| Ok((r.get(0)?, r.get(1)?)));
        for (id, name) in missing {
            report("making a project folder", self.give_project_folder(id, &name));
        }
    }

    fn dismissed_folders(&self) -> HashSet<String> {
        self.meta_get("projects_dismissed").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// A project whose folder was deleted outside the app leaves the list. A folder on a disk
    /// that is not mounted (its parent is missing too) keeps its project.
    pub fn drop_missing_projects(&self) {
        let gone: Vec<i64> = self
            .rows("SELECT id, folder FROM projects WHERE folder IS NOT NULL AND folder != ''", [], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .into_iter()
            .filter(|(_, f)| {
                let p = Path::new(f);
                !p.exists() && p.parent().is_some_and(Path::is_dir)
            })
            .map(|(id, _)| id)
            .collect();
        if gone.is_empty() {
            return;
        }
        let done = self.write(|db| gone.iter().try_for_each(|&id| Self::remove_project(db, id)));
        report("dropping projects whose folders are gone", done);
    }

    fn remove_project(db: &Connection, pid: i64) -> rusqlite::Result<()> {
        db.execute("DELETE FROM project_items WHERE project_id=?", [pid])?;
        db.execute("DELETE FROM projects WHERE id=?", [pid])?;
        Ok(())
    }

    /// A folder made by hand inside the projects folder becomes a project, with the sounds it holds.
    pub fn adopt_project_folders(&self) {
        let base = expand(self.settings()["projects_dir"].as_str().unwrap_or(""));
        let Ok(base) = std::fs::canonicalize(&base) else { return };
        let projects = self.projects();
        let list = projects.as_array().unwrap();
        let mut owned: HashSet<String> = list.iter().filter_map(|p| p["folder"].as_str()).map(nfc).collect();
        owned.extend(self.dismissed_folders().iter().map(|f| nfc(f)));
        let mut names: HashSet<String> = list.iter().filter_map(|p| p["name"].as_str().map(String::from)).collect();
        let Ok(rd) = std::fs::read_dir(&base) else { return };
        let mut found: Vec<String> = rd.flatten().filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.')).map(|e| e.path().to_string_lossy().into_owned()).collect();
        found.sort();
        for folder in found {
            if owned.contains(&nfc(&folder)) {
                continue;
            }
            let stem = basename(&folder);
            let (mut name, mut n) = (stem.clone(), 2);
            while names.contains(&name) {
                name = format!("{stem} ({n})");
                n += 1;
            }
            names.insert(name.clone());
            let Some(id) = report("adopting a project folder", self.write(|db| Self::project_named(db, &name))) else { continue };
            if self.set_project_folder(id, &folder).is_err() {
                continue;
            }
            let inside: Vec<String> = self.store.read().unwrap().items.iter().map(|it| &it.meta.path).filter(|p| is_inside(p, &folder)).cloned().collect();
            let done = self.write(|db| inside.iter().try_for_each(|p| Self::add_project_item(db, id, p)));
            report("adopting the sounds of a project folder", done);
        }
    }

    pub fn rename_project(&self, pid: i64, name: &str) -> Res<()> {
        if name.trim().is_empty() {
            return Err("Name required".into());
        }
        self.exec("UPDATE projects SET name=? WHERE id=?", params![name.trim(), pid]).map(|_| ()).map_err(|_| "A project with that name already exists".into())
    }

    pub fn delete_project(&self, pid: i64) -> Res<()> {
        if let Some(f) = self.project_folder(pid) {
            let mut gone: Vec<String> = self.dismissed_folders().into_iter().collect();
            gone.push(f);
            gone.sort();
            gone.dedup();
            report("remembering a deleted project", self.meta_set("projects_dismissed", &serde_json::to_string(&gone).unwrap()));
        }
        self.write(|db| Self::remove_project(db, pid)).map_err(|e| format!("The project could not be deleted: {e}"))
    }

    /// The project's folder, checked to exist and to be inside the library.
    pub fn project_folder_ready(&self, pid: i64) -> Res<Option<String>> {
        let Some(folder) = self.project_folder(pid) else { return Ok(None) };
        if !Path::new(&folder).is_dir() {
            return Err(format!("The project folder was not found: {folder}"));
        }
        if !self.folders().iter().any(|f| nfc(f) == nfc(&folder) || is_inside(&folder, f)) {
            self.add_folder(&folder)?;
        }
        Ok(Some(folder))
    }

    fn into_project_folder(&self, folder: &str, path: &str) -> Res<String> {
        if is_inside(path, folder) {
            return Ok(path.into());
        }
        let mut target = Path::new(folder).join(basename(path));
        let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        let same = target.is_file()
            && std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0) == size
            && file_hash(&target.to_string_lossy(), size).ok() == file_hash(path, size).ok();
        if !same {
            target = unique_path(&target)?;
            std::fs::copy(path, &target).map_err(|e| e.to_string())?;
        }
        let t = target.to_string_lossy().into_owned();
        self.catalog_copy(path, &t);
        self.set_origin(&t, path);
        self.index_file(&t);
        Ok(t)
    }

    pub fn set_project_folder(&self, pid: i64, path: &str) -> Res<String> {
        let p = std::fs::canonicalize(expand(path)).map_err(|_| format!("Not a folder: {path}"))?;
        if !p.is_dir() {
            return Err(format!("Not a folder: {}", p.display()));
        }
        let mut path = p.to_string_lossy().into_owned();
        if !self.folders().iter().any(|f| nfc(f) == nfc(&path) || is_inside(&path, f)) {
            path = self.add_folder(&path)?;
        }
        self.exec("UPDATE projects SET folder=? WHERE id=?", params![path, pid])?;
        let mut olds: Vec<String> = self.project_paths(pid).into_iter().collect();
        olds.sort();
        for old in olds {
            if is_inside(&old, &path) || !Path::new(&old).exists() {
                continue;
            }
            let new = self.into_project_folder(&path, &old)?;
            self.write(|db| {
                db.execute(
                    "INSERT OR IGNORE INTO project_items(project_id, path, added, pos, pad) SELECT project_id, ?, added, pos, pad FROM project_items WHERE project_id=? AND path=?",
                    params![new, pid, old],
                )?;
                db.execute("DELETE FROM project_items WHERE project_id=? AND path=?", params![pid, old])
            })?;
        }
        Ok(path)
    }

    pub fn project_add(&self, pid: i64, path: &str) -> Res<String> {
        if self.project_folder(pid).is_none() {
            let name = self.project_name(pid).ok_or("Project not found")?;
            self.give_project_folder(pid, &name)?;
        }
        let mut p = path.to_string();
        if let Some(folder) = self.project_folder_ready(pid)? {
            p = self.into_project_folder(&folder, path)?;
        }
        self.write(|db| Self::add_project_item(db, pid, &p))?;
        Ok(p)
    }

    pub(super) fn add_project_item(db: &Connection, pid: i64, path: &str) -> rusqlite::Result<()> {
        db.execute("INSERT OR IGNORE INTO project_items(project_id, path, added) VALUES (?, ?, ?)", params![pid, path, now()])?;
        Ok(())
    }

    pub fn project_remove(&self, pid: i64, path: &str) -> Res<()> {
        self.exec("DELETE FROM project_items WHERE project_id=? AND path=?", params![pid, path]).map(|_| ()).map_err(|e| format!("The sound could not be taken out of the project: {e}"))
    }

    pub(super) fn project_paths(&self, pid: i64) -> HashSet<String> {
        self.rows("SELECT path FROM project_items WHERE project_id=?", [pid], |r| r.get(0)).into_iter().collect()
    }

    pub fn project_order(&self, pid: i64) -> Value {
        Value::Array(self.rows("SELECT path FROM project_items WHERE project_id=? ORDER BY pos IS NULL, pos, path", [pid], |r| r.get(0).map(Value::String)))
    }

    pub fn set_project_order(&self, pid: i64, paths: &[String]) -> Res<()> {
        self.write(|db| Self::place_in_order(db, pid, paths)).map_err(|e| format!("The order could not be saved: {e}"))
    }

    pub(super) fn place_in_order(db: &Connection, pid: i64, paths: &[String]) -> rusqlite::Result<()> {
        for (n, p) in paths.iter().enumerate() {
            db.execute("UPDATE project_items SET pos=? WHERE project_id=? AND path=?", params![n as f64, pid, p])?;
        }
        Ok(())
    }

    pub fn project_pads(&self, pid: i64) -> Value {
        Value::Object(self.rows("SELECT path, pad FROM project_items WHERE project_id=? AND pad IS NOT NULL", [pid], |r| Ok((r.get::<_, String>(0)?, json!(r.get::<_, i64>(1)?)))).into_iter().collect())
    }

    /// Lays the project's pads out as given: every other sound of the project leaves its pad.
    pub(super) fn place_on_pads<'a>(db: &Connection, pid: i64, pads: impl Iterator<Item = (&'a String, &'a i64)>) -> rusqlite::Result<()> {
        db.execute("UPDATE project_items SET pad=NULL WHERE project_id=?", [pid])?;
        for (path, pad) in pads {
            db.execute("UPDATE project_items SET pad=? WHERE project_id=? AND path=?", params![pad, pid, path])?;
        }
        Ok(())
    }

    pub fn set_project_pads(&self, pid: i64, pads: &Value) -> Res<()> {
        let mut clean: Vec<(String, i64)> = Vec::new();
        for (k, v) in pads.as_object().ok_or("pads must be an object")? {
            if let Some(n) = v.as_i64() {
                if !(0..SP404_PADS).contains(&n) {
                    return Err(format!("Pads are numbered 0 to {}", SP404_PADS - 1));
                }
                clean.push((k.clone(), n));
            }
        }
        let used: HashSet<i64> = clean.iter().map(|c| c.1).collect();
        if used.len() != clean.len() {
            return Err("Two sounds are on the same pad".into());
        }
        let members = self.project_paths(pid);
        if let Some((p, _)) = clean.iter().find(|(p, _)| !members.contains(p)) {
            return Err(format!("Not in this project: {p}"));
        }
        self.write(|db| Self::place_on_pads(db, pid, clean.iter().map(|(p, n)| (p, n))))
    }

    /// Where a file saved from the editor lands, for a project or a chosen folder.
    pub fn save_target_dir(&self, project: i64, dest_dir: &str) -> Res<PathBuf> {
        if project != 0 {
            let name = self.project_name(project).ok_or("Project not found")?;
            let f = match self.project_folder_ready(project).map_err(|e| format!("The project folder cannot be used: {e}"))? {
                Some(f) => f,
                None => self.give_project_folder(project, &name)?,
            };
            return Ok(PathBuf::from(f));
        }
        let d = expand(dest_dir);
        if !d.is_dir() {
            return Err(format!("Not a folder: {}", d.display()));
        }
        Ok(d)
    }

    pub fn after_save(&self, target: &str, source: &str, project: i64) -> Res<()> {
        if !source.is_empty() && self.is_known(source) {
            self.catalog_copy(source, target);
            self.set_origin(target, source);
        }
        self.index_file(target);
        if project != 0 {
            self.project_add(project, target)?;
        }
        Ok(())
    }
}
