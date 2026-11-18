//! Search, subfolders, folder counts and the sound map.

use crate::library::search::SearchQuery;
use serde_json::{json, Value};
use std::collections::HashSet;
use super::{lab, names};

#[test]
fn search_hides_a_missing_file_without_dropping_its_tags() {
    let l = lab();
    let p = l.add("rain.wav", 0);
    l.tag(&p, &["kyoto"], 4);
    std::fs::remove_file(&p).unwrap();
    assert!(l.by_name().is_empty());
    assert_eq!(l.meta(&p), (vec!["kyoto".to_string()], 4));
}

#[test]
fn search_follows_catalog_writes() {
    let l = lab();
    let p = l.add("rain.wav", 0);
    l.tag(&p, &["one"], 0);
    let q = || SearchQuery { sort: Some("name".into()), ..Default::default() };
    assert_eq!(l.search(q(), Some("one"))[0]["tags"], json!(["one"]));
    l.lib.catalog_set(&p, &json!({"tags": ["two"]})).unwrap();
    assert!(l.search(q(), Some("one")).is_empty());
    assert_eq!(l.search(q(), Some("two"))[0]["tags"], json!(["two"]));
}

#[test]
fn subfolders_are_listed_with_counts() {
    let l = lab();
    l.add("top.wav", 0);
    l.add("Hits/Kicks/k1.wav", 1);
    l.add("Hits/Snares/s1.wav", 2);
    l.add("Hits/Snares/s2.wav", 3);
    l.add("Loops/l1.wav", 4);
    let row = |s: &Value| (s["name"].as_str().unwrap().to_string(), s["files"].as_i64().unwrap(), s["nested"].as_bool().unwrap());
    let top: Vec<_> = l.lib.subfolders(&l.folder).as_array().unwrap().iter().map(row).collect();
    assert_eq!(top, vec![("Hits".into(), 3, true), ("Loops".into(), 1, false)]);
    let hits: Vec<_> = l.lib.subfolders(&l.path("Hits")).as_array().unwrap().iter().map(|s| (s["name"].as_str().unwrap().to_string(), s["files"].as_i64().unwrap())).collect();
    assert_eq!(hits, vec![("Kicks".into(), 1), ("Snares".into(), 2)]);
    assert_eq!(l.lib.subfolders("/nowhere/at/all"), json!([]));
}

#[test]
fn search_narrows_to_a_subfolder_and_reports_it() {
    let l = lab();
    l.add("top.wav", 0);
    l.add("Hits/Snares/s1.wav", 1);
    l.add("Hits/Kicks/k1.wav", 2);
    let rows = l.search(SearchQuery { folder: Some(l.folder.clone()), under: Some(l.path("Hits")), sort: Some("name".into()), ..Default::default() }, None);
    assert_eq!(names(&rows), vec!["k1.wav", "s1.wav"]);
    let subs: HashSet<&str> = rows.iter().map(|r| r["sub"].as_str().unwrap()).collect();
    assert_eq!(subs, HashSet::from(["Hits/Kicks", "Hits/Snares"]));
    assert_eq!(l.by_name().last().unwrap()["sub"], "");
}

#[test]
fn words_match_the_subfolder_a_sound_sits_in() {
    let l = lab();
    l.add("Hits/Snares/one.wav", 1);
    l.add("Hits/Kicks/two.wav", 2);
    assert_eq!(names(&l.search(SearchQuery::default(), Some("snares"))), vec!["one.wav"]);
}

#[test]
fn the_sound_map_places_every_sound_inside_the_unit_square() {
    let l = lab();
    assert_eq!(l.lib.sound_map(), json!([]));
    let mut paths: Vec<String> = (0..14).map(|i| l.add(&format!("s{i}.wav"), i)).collect();
    let points = l.lib.sound_map();
    let pts = points.as_array().unwrap();
    let mut got: Vec<String> = pts.iter().map(|p| p["path"].as_str().unwrap().to_string()).collect();
    got.sort();
    paths.sort();
    assert_eq!(got, paths);
    let xy: Vec<(f64, f64)> = pts.iter().map(|p| (p["x"].as_f64().unwrap(), p["y"].as_f64().unwrap())).collect();
    assert!(xy.iter().all(|&(x, y)| (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)));
    assert!(xy.iter().any(|p| *p != xy[0]));
    assert_eq!(l.lib.sound_map(), points);
}

#[test]
fn a_folder_names_the_files_that_could_not_be_read() {
    let l = lab();
    l.add("a.wav", 0);
    let bad = l.path("broken.wav");
    std::fs::write(&bad, b"this is not a sound").unwrap();
    l.index(&[&bad]);
    let f = &l.lib.folder_counts()[0];
    assert_eq!((f["files"].as_i64(), f["indexed"].as_i64(), f["failed"].as_i64()), (Some(2), Some(1), Some(1)));
    assert_eq!(f["failed_files"][0]["path"], bad);
    assert!(!f["failed_files"][0]["error"].as_str().unwrap().is_empty());
    // A file that is fixed is no longer counted.
    l.add("broken.wav", 1);
    assert_eq!(l.lib.folder_counts()[0]["failed"], 0);
}

#[test]
fn a_write_the_database_refuses_is_reported() {
    let l = lab();
    let a = l.add("a.wav", 0);
    let pid = l.kit("Kit");
    l.lib.project_add(pid, &a).unwrap();
    l.lib.db().execute_batch("CREATE TRIGGER no_delete BEFORE DELETE ON project_items BEGIN SELECT RAISE(ABORT, 'refused'); END; CREATE TRIGGER no_order BEFORE UPDATE ON project_items BEGIN SELECT RAISE(ABORT, 'refused'); END; CREATE TRIGGER no_folder BEFORE DELETE ON folders BEGIN SELECT RAISE(ABORT, 'refused'); END;").unwrap();
    assert!(l.lib.project_remove(pid, &a).is_err());
    assert!(l.lib.set_project_order(pid, &[a.clone()]).is_err());
    assert!(l.lib.delete_project(pid).is_err());
    assert!(l.lib.remove_folder(&l.folder).is_err());
    // Nothing was dropped along the way: the sound is still listed.
    assert_eq!(l.count(), 1);
}

#[test]
fn folder_counts_include_the_size_on_disk() {
    let l = lab();
    let a = l.add("a.wav", 0);
    assert_eq!(l.lib.folder_counts()[0]["bytes"], std::fs::metadata(&a).unwrap().len());
}
