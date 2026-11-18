//! The catalog: tags, notes, ratings and origins of sounds, and the regions marked in them.

use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use super::paths::nfc;
use super::{Library, Res, now, report};

/// What the catalog holds about one sound.
pub(super) struct CatalogRow {
    pub(super) tags: Vec<String>,
    pub(super) memo: String,
    pub(super) rating: i64,
    pub(super) origin: Option<String>,
}

impl Library {
    /// The whole catalog by path, as search reads it.
    pub(super) fn catalog_all(&self) -> Arc<HashMap<String, CatalogRow>> {
        let mut cache = self.catalog.lock().unwrap();
        cache
            .get_or_insert_with(|| {
                let rows = self.rows("SELECT path, tags, memo, rating, origin FROM catalog", [], |r| {
                    let tags: Option<String> = r.get(1)?;
                    let row = CatalogRow {
                        tags: tags.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default(),
                        memo: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                        rating: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                        origin: r.get(4)?,
                    };
                    Ok((r.get::<_, String>(0)?, row))
                });
                Arc::new(rows.into_iter().collect())
            })
            .clone()
    }

    pub(super) fn catalog_get(&self, path: &str) -> Value {
        let row = self.one("SELECT tags, memo, rating FROM catalog WHERE path=?", [path], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<i64>>(2)?))
        });
        let (tags, memo, rating) = row.unwrap_or_default();
        let tags: Vec<String> = tags.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        json!({"path": path, "tags": tags, "memo": memo.unwrap_or_default(), "rating": rating.unwrap_or(0)})
    }

    pub fn catalog_set(&self, path: &str, changes: &Value) -> Res<Value> {
        let t = now();
        self.write(|db| {
            ensure_catalog_row(db, path, t)?;
            if let Some(r) = changes.get("rating").filter(|v| !v.is_null()) {
                let r = r.as_f64().unwrap_or(0.0).clamp(0.0, 5.0) as i64;
                db.execute("UPDATE catalog SET rating=?, updated=? WHERE path=?", params![r, t, path])?;
            }
            if let Some(m) = changes.get("memo").and_then(|v| v.as_str()) {
                db.execute("UPDATE catalog SET memo=?, updated=? WHERE path=?", params![m, t, path])?;
            }
            if let Some(tags) = changes.get("tags").and_then(|v| v.as_array()) {
                let tags = clean_tags(tags.iter().filter_map(|t| t.as_str()));
                db.execute("UPDATE catalog SET tags=?, updated=? WHERE path=?", params![serde_json::to_string(&tags).unwrap(), t, path])?;
            }
            Ok(())
        })?;
        Ok(self.catalog_get(path))
    }

    pub(super) fn catalog_copy(&self, src: &str, dst: &str) {
        let c = self.catalog_get(src);
        let mine = self.catalog_get(dst);
        let has = |v: &Value| v["tags"].as_array().is_some_and(|a| !a.is_empty()) || v["memo"].as_str().is_some_and(|m| !m.is_empty()) || v["rating"].as_i64().unwrap_or(0) > 0;
        if !has(&c) || has(&mine) {
            return;
        }
        report("copying tags to a new file", self.catalog_set(dst, &c));
    }

    pub(super) fn set_origin(&self, path: &str, origin: &str) {
        if origin.is_empty() || nfc(origin) == nfc(path) {
            return;
        }
        let done = self.write(|db| {
            ensure_catalog_row(db, path, now())?;
            db.execute("UPDATE catalog SET origin=? WHERE path=? AND origin IS NULL", [origin, path])
        });
        report("noting where a file came from", done);
    }

    pub fn all_tags(&self) -> Value {
        let mut counts: HashMap<String, i64> = HashMap::new();
        for t in self.rows("SELECT c.tags FROM catalog c JOIN files f ON f.path = c.path WHERE c.tags IS NOT NULL", [], |r| r.get::<_, String>(0)) {
            for tag in serde_json::from_str::<Vec<String>>(&t).unwrap_or_default() {
                *counts.entry(tag).or_default() += 1;
            }
        }
        let mut v: Vec<_> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Value::Array(v.into_iter().map(|(t, n)| json!({"tag": t, "count": n})).collect())
    }

    pub fn regions_get(&self, path: &str) -> Value {
        self.one("SELECT data FROM regions WHERE path=?", [path], |r| r.get::<_, String>(0)).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!([]))
    }

    pub fn regions_set(&self, path: &str, regions: &Value) -> Res<Value> {
        let mut clean = Vec::new();
        for r in regions.as_array().ok_or("regions must be a list")? {
            let (a, b) = (r["start"].as_f64().ok_or("start")?, r["end"].as_f64().ok_or("end")?);
            if b > a && a >= 0.0 {
                let name: String = r["name"].as_str().unwrap_or("").chars().take(80).collect();
                clean.push(json!({"start": a, "end": b, "name": name}));
            }
        }
        let clean = Value::Array(clean);
        if clean.as_array().is_some_and(Vec::is_empty) {
            self.exec("DELETE FROM regions WHERE path=?", [path])?;
        } else {
            self.exec("INSERT OR REPLACE INTO regions VALUES (?, ?)", params![path, clean.to_string()])?;
        }
        Ok(clean)
    }
}

pub(super) fn clean_tags<'a>(tags: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut set: Vec<String> = tags.map(|t| nfc(&t.trim().to_lowercase())).filter(|t| !t.is_empty()).collect();
    set.sort();
    set.dedup();
    set
}

pub(super) fn ensure_catalog_row(db: &Connection, path: &str, updated: f64) -> rusqlite::Result<()> {
    db.execute("INSERT OR IGNORE INTO catalog(path, updated) VALUES (?, ?)", params![path, updated])?;
    Ok(())
}
