//! Backups and restoring them.

use serde_json::{json, Value};
use std::path::PathBuf;
use super::{lab, set};

#[test]
fn the_daily_backup_restores_regions_and_project_order() {
    let l = lab();
    let pid = l.kit("kit");
    let (a, b) = (l.add("a.wav", 1), l.add("b.wav", 2));
    l.lib.project_add(pid, &a).unwrap();
    l.lib.project_add(pid, &b).unwrap();
    l.lib.set_project_order(pid, &[b.clone(), a.clone()]);
    l.lib.regions_set(&a, &json!([{"start": 0.01, "end": 0.05, "name": "hit"}])).unwrap();
    l.lib.daily_backup();
    let file = std::fs::read_dir(l.root.join("home/backups")).unwrap().flatten().next().unwrap().path();
    l.lib.regions_set(&a, &json!([])).unwrap();
    l.lib.delete_project(pid);
    l.lib.restore_backup(&file.to_string_lossy()).unwrap();
    assert_eq!(l.lib.regions_get(&a)[0]["name"], "hit");
    assert_eq!(l.lib.project_order(l.lib.projects()[0]["id"].as_i64().unwrap()), json!([b, a]));
}

#[test]
fn a_backup_is_written_once_per_day() {
    let l = lab();
    let p = l.add("rain.wav", 0);
    l.tag(&p, &["kyoto"], 0);
    l.lib.daily_backup();
    l.lib.daily_backup();
    let files: Vec<PathBuf> = std::fs::read_dir(l.root.join("home/backups")).unwrap().flatten().map(|e| e.path()).collect();
    assert_eq!(files.len(), 1);
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("catalog-") && name.ends_with(".json"));
    assert!(std::fs::read_to_string(&files[0]).unwrap().contains("kyoto"));
}

#[test]
fn a_backup_round_trips() {
    let l = lab();
    let pid = l.kit("kit");
    let (a, b) = (l.add("a.wav", 1), l.add("b.wav", 2));
    l.lib.project_add(pid, &a).unwrap();
    l.lib.project_add(pid, &b).unwrap();
    l.tag(&a, &["keep"], 4);
    l.lib.catalog_set(&a, &json!({"memo": "under the bridge"})).unwrap();
    l.lib.set_project_order(pid, &[b.clone(), a.clone()]);
    l.lib.set_project_pads(pid, &json!({&a: 16})).unwrap();
    let file = l.lib.export_backup(&l.root.to_string_lossy()).unwrap();
    let data: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(data["athar_backup"], 1);
    l.lib.catalog_set(&a, &json!({"tags": [], "rating": 0, "memo": ""})).unwrap();
    l.lib.delete_project(pid);
    assert_eq!(l.lib.restore_backup(&file).unwrap(), json!({"sounds": 1, "projects": 1, "regions": 0}));
    let c = l.lib.catalog_get(&a);
    assert_eq!((c["tags"].clone(), c["rating"].clone(), c["memo"].clone()), (json!(["keep"]), json!(4), json!("under the bridge")));
    let restored = l.lib.projects()[0]["id"].as_i64().unwrap();
    assert_eq!(l.lib.project_order(restored), json!([b, a]));
    assert_eq!(l.lib.project_pads(restored), json!({&a: 16}));
}

#[test]
fn restore_refuses_a_file_that_is_not_a_backup() {
    let l = lab();
    let bad = l.root.join("x.json");
    std::fs::write(&bad, "[1, 2, 3]").unwrap();
    assert!(l.lib.restore_backup(&bad.to_string_lossy()).is_err());
    std::fs::write(&bad, "not json").unwrap();
    assert!(l.lib.restore_backup(&bad.to_string_lossy()).is_err());
}

#[test]
fn restore_brings_regions_back() {
    let l = lab();
    let a = l.add("a.wav", 0);
    l.tag(&a, &["x"], 0);
    l.lib.regions_set(&a, &json!([{"start": 0.01, "end": 0.05, "name": "hit"}])).unwrap();
    let file = l.lib.export_backup(&l.root.to_string_lossy()).unwrap();
    l.lib.regions_set(&a, &json!([])).unwrap();
    assert_eq!(l.lib.restore_backup(&file).unwrap()["regions"], 1);
    assert_eq!(l.lib.regions_get(&a)[0]["name"], "hit");
}

#[test]
fn restore_reads_the_dated_backups_too() {
    let l = lab();
    let pid = l.kit("kit");
    let (a, b) = (l.add("a.wav", 1), l.add("b.wav", 2));
    l.lib.project_add(pid, &a).unwrap();
    l.lib.project_add(pid, &b).unwrap();
    l.tag(&a, &["keep"], 0);
    l.lib.set_project_pads(pid, &json!({&b: 2})).unwrap();
    let rows = |sql: &str| -> Vec<Value> {
        let db = l.lib.db();
        let mut st = db.prepare(sql).unwrap();
        let cols: Vec<String> = st.column_names().map(|c| c.to_string()).collect();
        st.query_map([], |r| {
            let mut o = serde_json::Map::new();
            for (i, c) in cols.iter().enumerate() {
                let v = match r.get_ref(i)? {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(n) => json!(n),
                    rusqlite::types::ValueRef::Real(f) => json!(f),
                    rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    rusqlite::types::ValueRef::Blob(_) => Value::Null,
                };
                o.insert(c.clone(), v);
            }
            Ok(Value::Object(o))
        })
        .unwrap()
        .flatten()
        .collect()
    };
    let dated = json!({
        "catalog": rows("SELECT path, tags, memo, rating, updated FROM catalog"),
        "projects": [{"id": pid, "name": "kit"}],
        "project_items": rows("SELECT project_id, path, added, pad FROM project_items"),
    });
    let file = l.root.join("catalog-2026-11-13.json");
    std::fs::write(&file, dated.to_string()).unwrap();
    l.lib.delete_project(pid);
    l.lib.catalog_set(&a, &json!({"tags": []})).unwrap();
    assert_eq!(l.lib.restore_backup(&file.to_string_lossy()).unwrap()["projects"], 1);
    assert_eq!(l.meta(&a).0, vec!["keep"]);
    let restored = l.lib.projects()[0]["id"].as_i64().unwrap();
    assert_eq!(l.lib.project_paths(restored), set(&[&a, &b]));
    assert_eq!(l.lib.project_pads(restored), json!({&b: 2}));
}

#[test]
fn a_damaged_backup_restores_nothing() {
    let l = lab();
    let a = l.add("a.wav", 0);
    l.tag(&a, &["mine"], 0);
    let bad = l.root.join("bad.json");
    std::fs::write(&bad, json!({"catalog": [{"path": a, "tags": ["theirs"], "rating": 3}, "not a row"], "projects": [{"name": "kit", "paths": ["x"]}]}).to_string()).unwrap();
    assert!(l.lib.restore_backup(&bad.to_string_lossy()).is_err());
    assert_eq!(l.meta(&a).0, vec!["mine"]);
    assert_eq!(l.lib.projects(), json!([]));
}
