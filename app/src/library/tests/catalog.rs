//! Regions, and what a saved edit takes from its source.

use crate::library::search::SearchQuery;
use serde_json::json;
use super::{lab};

#[test]
fn regions_are_kept_and_follow_a_move() {
    let l = lab();
    let a = l.add("take.wav", 0);
    let saved = l.lib.regions_set(&a, &json!([{"start": 0.5, "end": 1.25, "name": "hit"}, {"start": 3, "end": 2}])).unwrap();
    assert_eq!(saved, json!([{"start": 0.5, "end": 1.25, "name": "hit"}]));
    let b = l.path("take 2.wav");
    std::fs::rename(&a, &b).unwrap();
    l.events(&[&a, &b]);
    assert_eq!(l.lib.regions_get(&b), saved);
    assert_eq!(l.lib.regions_get(&a), json!([]));
    l.lib.regions_set(&b, &json!([])).unwrap();
    assert_eq!(l.lib.regions_get(&b), json!([]));
}

#[test]
fn a_saved_edit_inherits_metadata_once() {
    let l = lab();
    let src = l.add("rain.wav", 0);
    l.tag(&src, &["kyoto"], 4);
    let dst = l.add("rain_edit.wav", 2);
    l.lib.after_save(&dst, &src, 0).unwrap();
    l.drain();
    assert_eq!(l.meta(&dst), (vec!["kyoto".to_string()], 4));
    let rows = l.search(SearchQuery::default(), Some("rain_edit"));
    assert_eq!(rows.iter().find(|r| r["path"] == dst.as_str()).unwrap()["origin"], json!(src));
    l.lib.catalog_set(&dst, &json!({"tags": ["mine"]})).unwrap();
    l.lib.after_save(&dst, &src, 0).unwrap();
    assert_eq!(l.meta(&dst).0, vec!["mine"]);
}
