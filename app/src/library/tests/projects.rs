//! Projects: their folders, order and pads.

use crate::library::paths::{basename, is_inside};
use rusqlite::params;
use serde_json::json;
use std::path::Path;
use super::{lab, set};

#[test]
fn a_project_with_a_folder_copies_what_is_added() {
    let l = lab();
    let src = l.add("kick.wav", 0);
    l.tag(&src, &["keeper"], 4);
    let kit = l.path("kit");
    std::fs::create_dir(&kit).unwrap();
    let pid = l.lib.create_project("kit").unwrap()["id"].as_i64().unwrap();
    l.lib.set_project_folder(pid, &kit).unwrap();
    let copy = l.lib.project_add(pid, &src).unwrap();
    assert_eq!(copy, l.path("kit/kick.wav"));
    assert!(Path::new(&copy).exists() && Path::new(&src).exists());
    assert_eq!(l.meta(&copy).0, vec!["keeper"]);
    // The same sound a second time reuses the copy.
    assert_eq!(l.lib.project_add(pid, &src).unwrap(), copy);
    let inside: Vec<String> = std::fs::read_dir(&kit).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(inside, vec!["kick.wav"]);
    assert_eq!(l.lib.project_paths(pid), set(&[&copy]));
}

#[test]
fn setting_a_folder_moves_existing_members_into_it() {
    let l = lab();
    let pid = l.lib.create_project("kit").unwrap()["id"].as_i64().unwrap();
    let a = l.add("a.wav", 1);
    l.lib.project_add(pid, &a).unwrap();
    let kit = l.path("kit");
    std::fs::create_dir(&kit).unwrap();
    l.lib.set_project_folder(pid, &kit).unwrap();
    assert_eq!(l.lib.project_paths(pid), set(&[&l.path("kit/a.wav")]));
    assert!(Path::new(&a).exists());
}

#[test]
fn adding_to_a_project_fails_cleanly_when_its_folder_is_gone() {
    let l = lab();
    let src = l.add("kick.wav", 0);
    let kit = l.path("kit");
    std::fs::create_dir(&kit).unwrap();
    let pid = l.lib.create_project("kit").unwrap()["id"].as_i64().unwrap();
    l.lib.set_project_folder(pid, &kit).unwrap();
    std::fs::remove_dir(&kit).unwrap();
    assert!(l.lib.project_add(pid, &src).is_err());
    assert!(l.lib.project_paths(pid).is_empty());
}

#[test]
fn project_order_is_kept_and_new_sounds_go_last() {
    let l = lab();
    let pid = l.kit("kit");
    let [a, b, c] = [("a.wav", 0), ("b.wav", 1), ("c.wav", 2)].map(|(n, s)| l.add(n, s));
    for p in [&a, &b, &c] {
        l.lib.project_add(pid, p).unwrap();
    }
    assert_eq!(l.lib.project_order(pid), json!([a, b, c]));
    l.lib.set_project_order(pid, &[c.clone(), a.clone(), b.clone()]);
    assert_eq!(l.lib.project_order(pid), json!([c, a, b]));
    let d = l.add("0-first-by-name.wav", 9);
    l.lib.project_add(pid, &d).unwrap();
    assert_eq!(l.lib.project_order(pid), json!([c, a, b, d]));
}

#[test]
fn pads_hold_one_sound_each_and_follow_the_project() {
    let l = lab();
    let pid = l.kit("kit");
    let (a, b) = (l.add("a.wav", 1), l.add("b.wav", 2));
    l.lib.project_add(pid, &a).unwrap();
    l.lib.project_add(pid, &b).unwrap();
    l.lib.set_project_pads(pid, &json!({&a: 0, &b: 17})).unwrap();
    assert_eq!(l.lib.project_pads(pid), json!({&a: 0, &b: 17}));
    assert!(l.lib.set_project_pads(pid, &json!({&a: 3, &b: 3})).is_err());
    assert!(l.lib.set_project_pads(pid, &json!({&a: 160})).is_err());
    assert!(l.lib.set_project_pads(pid, &json!({"/not/in/the/project.wav": 1})).is_err());
    assert_eq!(l.lib.project_pads(pid), json!({&a: 0, &b: 17}));
    l.lib.set_project_pads(pid, &json!({&b: 5})).unwrap();
    assert_eq!(l.lib.project_pads(pid), json!({&b: 5}));
    l.lib.project_remove(pid, &b);
    assert_eq!(l.lib.project_pads(pid), json!({}));
}

#[test]
fn a_project_folder_that_left_the_library_rejoins_it() {
    let l = lab();
    let src = l.add("kick.wav", 0);
    let kit = l.root.join("elsewhere");
    std::fs::create_dir(&kit).unwrap();
    let kit = kit.to_string_lossy().into_owned();
    let pid = l.lib.create_project("kit").unwrap()["id"].as_i64().unwrap();
    l.lib.db().execute("UPDATE projects SET folder=? WHERE id=?", params![kit, pid]).unwrap();
    assert!(!l.lib.folders().contains(&kit));
    l.lib.project_add(pid, &src).unwrap();
    assert!(l.lib.folders().contains(&kit));
}

#[test]
fn the_same_name_and_size_with_another_sound_is_not_reused() {
    let l = lab();
    let a = l.add("one/kick.wav", 1);
    let b = l.add("two/kick.wav", 2);
    assert_eq!(std::fs::metadata(&a).unwrap().len(), std::fs::metadata(&b).unwrap().len());
    let kit = l.path("kit");
    std::fs::create_dir(&kit).unwrap();
    let pid = l.lib.create_project("kit").unwrap()["id"].as_i64().unwrap();
    l.lib.set_project_folder(pid, &kit).unwrap();
    let (first, second) = (l.lib.project_add(pid, &a).unwrap(), l.lib.project_add(pid, &b).unwrap());
    assert_ne!(first, second);
    assert_eq!(std::fs::read_dir(&kit).unwrap().count(), 2);
}

#[test]
fn setting_a_folder_keeps_the_order() {
    let l = lab();
    let pid = l.lib.create_project("kit").unwrap()["id"].as_i64().unwrap();
    let (a, b) = (l.add("a.wav", 1), l.add("b.wav", 2));
    let a = l.lib.project_add(pid, &a).unwrap();
    let b = l.lib.project_add(pid, &b).unwrap();
    l.lib.set_project_order(pid, &[b, a]);
    let kit = l.path("kit");
    std::fs::create_dir(&kit).unwrap();
    l.lib.set_project_folder(pid, &kit).unwrap();
    let order: Vec<String> = l.lib.project_order(pid).as_array().unwrap().iter().map(|p| basename(p.as_str().unwrap())).collect();
    assert_eq!(order, vec!["b.wav", "a.wav"]);
    assert!(l.lib.project_paths(pid).iter().all(|p| is_inside(p, &kit)));
}

#[test]
fn a_folder_made_by_hand_in_the_projects_folder_becomes_a_project() {
    let l = lab();
    let base = l.path("projects");
    std::fs::create_dir_all(l.path("projects/by hand")).unwrap();
    l.lib.update_settings(&json!({"projects_dir": base})).unwrap();
    let inside = l.add("projects/by hand/a_b.wav", 0);
    l.lib.adopt_project_folders();
    let p = l.lib.projects().as_array().unwrap().iter().find(|p| p["name"] == "by hand").cloned().unwrap();
    assert_eq!(p["folder"], l.path("projects/by hand"));
    assert_eq!(l.lib.project_paths(p["id"].as_i64().unwrap()), set(&[&inside]));
    // Deleted in the app, it stays deleted although its folder is still on disk.
    l.lib.delete_project(p["id"].as_i64().unwrap());
    l.lib.adopt_project_folders();
    assert!(!l.lib.projects().as_array().unwrap().iter().any(|q| q["name"] == "by hand"));
}
