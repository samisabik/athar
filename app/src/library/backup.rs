//! Backups of everything written about the sounds, and restoring them.

use rusqlite::params;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use super::catalog::{clean_tags, ensure_catalog_row};
use super::paths::{expand, unique_path};
use super::projects::SP404_PADS;
use super::{Library, Res, now, report};

const BACKUPS_KEPT: usize = 10;

impl Library {
    /// Everything written about the sounds: tags, notes, ratings, regions and projects.
    fn backup_json(&self) -> Value {
        let catalog = self.rows("SELECT path, tags, memo, rating FROM catalog", [], |r| {
            Ok(json!({"path": r.get::<_, String>(0)?, "tags": r.get::<_, Option<String>>(1)?, "memo": r.get::<_, Option<String>>(2)?, "rating": r.get::<_, Option<i64>>(3)?}))
        });
        let regions = self.rows("SELECT path, data FROM regions", [], |r| Ok(json!({"path": r.get::<_, String>(0)?, "data": r.get::<_, String>(1)?})));
        let projects: Vec<Value> = self
            .projects()
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| {
                let id = p["id"].as_i64().unwrap_or(0);
                json!({"name": p["name"], "folder": p["folder"], "paths": self.project_order(id), "pads": self.project_pads(id)})
            })
            .collect();
        let made = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
        json!({"athar_backup": 1, "made": made, "catalog": catalog, "projects": projects, "regions": regions})
    }

    /// The backup as a dated file beside the index: the first launch of each day writes one,
    /// and the last BACKUPS_KEPT are kept.
    pub(super) fn daily_backup(&self) {
        let dir = self.data_dir.join("backups");
        let today = dir.join(format!("catalog-{}.json", chrono::Local::now().format("%Y-%m-%d")));
        if !today.exists() {
            let data = self.backup_json();
            if ["catalog", "projects"].iter().all(|k| data[k].as_array().is_some_and(Vec::is_empty)) {
                return;
            }
            if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&today, data.to_string())) {
                eprintln!("library: today's backup was not written: {e}");
            }
        }
        let mut old: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("catalog-"))).collect())
            .unwrap_or_default();
        old.sort();
        if old.len() > BACKUPS_KEPT {
            for p in &old[..old.len() - BACKUPS_KEPT] {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    /// The backup as one file in dest.
    pub fn export_backup(&self, dest: &str) -> Res<String> {
        let d = expand(dest);
        if !d.is_dir() {
            return Err(format!("Not a folder: {}", d.display()));
        }
        let target = unique_path(&d.join(format!("Athar backup {}.json", chrono::Local::now().format("%Y-%m-%d"))))?;
        std::fs::write(&target, self.backup_json().to_string()).map_err(|e| format!("Could not write the backup: {e}"))?;
        Ok(target.to_string_lossy().into_owned())
    }

    /// Tags, notes, ratings, regions and projects from a backup file. What the file holds wins
    /// for the sounds it names; everything else is kept. The file is checked in full first.
    pub fn restore_backup(&self, file: &str) -> Res<Value> {
        let text = std::fs::read_to_string(expand(file)).map_err(|e| format!("That file could not be read as a backup: {e}"))?;
        let data: Value = serde_json::from_str(&text).map_err(|e| format!("That file could not be read as a backup: {e}"))?;
        if !data["catalog"].is_array() {
            return Err("That file is not an Athar backup".into());
        }
        let (catalog_rows, project_rows, region_rows) = (backup_rows(&data, "catalog")?, backup_rows(&data, "projects")?, backup_rows(&data, "regions")?);
        if project_rows.iter().any(|p| p.get("paths").is_some_and(|x| !x.is_array())) {
            return Err(damaged("a project's paths are not a list"));
        }
        let dated = dated_backup_projects(&data)?;
        let mut sounds = Vec::new();
        for r in catalog_rows {
            let tags: Vec<String> = match &r["tags"] {
                Value::String(t) => serde_json::from_str(t).unwrap_or_default(),
                Value::Array(a) => a.iter().filter_map(|t| t.as_str().map(String::from)).collect(),
                _ => vec![],
            };
            let tags = clean_tags(tags.iter().map(String::as_str));
            let memo = r["memo"].as_str().unwrap_or("").to_string();
            let rating = r["rating"].as_i64().unwrap_or(0).clamp(0, 5);
            if let Some(p) = r["path"].as_str() {
                if !tags.is_empty() || !memo.is_empty() || rating > 0 {
                    sounds.push((p.to_string(), serde_json::to_string(&tags).unwrap(), memo, rating));
                }
            }
        }
        let mut projects = Vec::new();
        for p in project_rows {
            let name = p["name"].as_str().unwrap_or("").trim().to_string();
            if name.is_empty() {
                continue;
            }
            let dated = p["id"].as_i64().and_then(|id| dated.get(&id));
            let paths: Vec<String> = match p["paths"].as_array() {
                Some(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
                None => dated.map(|d| d.0.clone()).unwrap_or_default(),
            };
            let pads: HashMap<String, i64> = match p["pads"].as_object() {
                Some(o) => o.iter().filter_map(|(k, v)| v.as_i64().map(|n| (k.clone(), n))).collect(),
                None => dated.map(|d| d.1.clone()).unwrap_or_default(),
            };
            let pads: HashMap<String, i64> = pads.into_iter().filter(|(_, n)| (0..SP404_PADS).contains(n)).collect();
            projects.push((name, p["folder"].as_str().map(String::from), paths, pads));
        }
        let regions: Vec<(String, String)> = region_rows
            .iter()
            .filter_map(|r| Some((r["path"].as_str()?.to_string(), if r["data"].is_string() { r["data"].as_str()?.to_string() } else { r["data"].to_string() })))
            .collect();
        let t = now();
        let ids = self.write(|db| {
            for (path, tags, memo, rating) in &sounds {
                ensure_catalog_row(db, path, t)?;
                db.execute("UPDATE catalog SET tags=?, memo=?, rating=?, updated=? WHERE path=?", params![tags, memo, rating, t, path])?;
            }
            for (path, blob) in &regions {
                db.execute("INSERT OR REPLACE INTO regions VALUES (?, ?)", params![path, blob])?;
            }
            let mut ids = Vec::new();
            for (name, folder, paths, pads) in &projects {
                let pid = Self::project_named(db, name)?;
                for path in paths {
                    Self::add_project_item(db, pid, path)?;
                }
                Self::place_in_order(db, pid, paths)?;
                if !pads.is_empty() {
                    Self::place_on_pads(db, pid, pads.iter())?;
                }
                ids.push((pid, folder.clone()));
            }
            Ok(ids)
        })?;
        for (pid, folder) in ids {
            if let Some(f) = folder.filter(|f| Path::new(f).is_dir()) {
                if self.project_folder(pid).is_none() {
                    report("giving a restored project its folder", self.set_project_folder(pid, &f));
                }
            }
        }
        Ok(json!({"sounds": sounds.len(), "projects": projects.len(), "regions": regions.len()}))
    }
}

fn damaged(why: &str) -> String {
    format!("That backup is damaged, so nothing was restored: {why}")
}

/// The records a backup holds under `key`; none when the key is absent.
fn backup_rows(data: &Value, key: &str) -> Res<Vec<Value>> {
    match &data[key] {
        Value::Null => Ok(vec![]),
        Value::Array(a) if a.iter().all(Value::is_object) => Ok(a.clone()),
        _ => Err(damaged(&format!("{key} must be a list of records"))),
    }
}

/// Daily backups once listed the members of projects as records of their own. For each
/// project id in such a file: its paths in the order they were added, and its pads.
fn dated_backup_projects(data: &Value) -> Res<HashMap<i64, (Vec<String>, HashMap<String, i64>)>> {
    let items = backup_rows(data, "project_items")?;
    if items.iter().any(|it| !it["project_id"].is_i64() || !it["path"].is_string()) {
        return Err(damaged("a project item has no project or path"));
    }
    let mut members: HashMap<i64, Vec<(f64, String)>> = HashMap::new();
    let mut pads: HashMap<i64, HashMap<String, i64>> = HashMap::new();
    for it in items {
        let pid = it["project_id"].as_i64().unwrap_or(-1);
        let path = it["path"].as_str().unwrap_or("").to_string();
        members.entry(pid).or_default().push((it["added"].as_f64().unwrap_or(0.0), path.clone()));
        if let Some(pad) = it["pad"].as_i64() {
            pads.entry(pid).or_default().insert(path, pad);
        }
    }
    Ok(members
        .into_iter()
        .map(|(pid, mut m)| {
            m.sort_by(|a, b| a.0.total_cmp(&b.0));
            (pid, (m.into_iter().map(|x| x.1).collect(), pads.remove(&pid).unwrap_or_default()))
        })
        .collect())
}
