//! Settings.

use crate::library::Library;
use serde_json::json;
use super::{lab};

#[test]
fn only_chosen_settings_are_saved() {
    let l = lab();
    l.lib.update_settings(&json!({"text_size": 15, "autoplay": false, "unknown": 1})).unwrap();
    let mut saved: Vec<String> = l.lib.saved_settings().keys().cloned().collect();
    saved.sort();
    assert_eq!(saved, vec!["projects_dir", "text_size"]);
    assert_eq!(l.lib.settings()["text_size"], 15);
    let default_dir = l.lib.default_projects_dir();
    let st = l.lib.update_settings(&json!({"text_size": 13, "projects_dir": default_dir})).unwrap();
    assert!(l.lib.saved_settings().is_empty());
    assert_eq!((st["text_size"].clone(), st["projects_dir"].clone()), (json!(13), json!(default_dir)));
}

#[test]
fn the_old_default_name_pattern_becomes_the_new_one() {
    let l = lab();
    // A library saved before the default changed holds the old default among its settings.
    l.lib.meta_set("settings", &json!({"name_pattern": "{project}_{category}_{n:03d}_{name}", "text_size": 15}).to_string()).unwrap();
    let (again, _rx) = Library::create(l.root.join("home"), l.root.join("no model")).unwrap();
    assert_eq!(again.settings()["name_pattern"], "{project}_{name}");
    assert_eq!(again.settings()["text_size"], 15);
    again.update_settings(&json!({"name_pattern": "{n:03d}_{name}"})).unwrap();
    assert_eq!(again.settings()["name_pattern"], "{n:03d}_{name}");
}
