//! Indexing, and the tags and project places that follow a file when it is replaced, moved, deleted or renamed.

use crate::library::Library;
use crate::library::index::file_hash;
use crate::library::paths::nfc;
use serde_json::json;
use super::{Lab, lab, names, set, stand_in};
use unicode_normalization::UnicodeNormalization;

#[test]
fn an_atomic_replace_keeps_tags_and_project() {
    let l = lab();
    let pid = l.kit("kit");
    let p = l.add("kick.wav", 0);
    l.tag(&p, &["keeper"], 5);
    l.lib.project_add(pid, &p).unwrap();
    let tmp = l.write("kick.wav.tmp", 7);
    std::fs::rename(&tmp, &p).unwrap();
    l.events(&[&tmp, &p]);
    assert_eq!(l.meta(&p), (vec!["keeper".to_string()], 5));
    assert_eq!(l.lib.project_paths(pid), set(&[&p]));
    assert_eq!(l.count(), 1);
    let sha: Option<String> = l.lib.db().query_row("SELECT sha1 FROM files WHERE path=?", [&p], |r| r.get(0)).unwrap();
    assert_eq!(sha, file_hash(&p, std::fs::metadata(&p).unwrap().len()).ok(), "the new content is indexed");
}

#[test]
fn a_file_moved_over_another_brings_its_own_tags() {
    let l = lab();
    let a = l.add("a.wav", 1);
    let b = l.add("b.wav", 2);
    l.tag(&a, &["from-a"], 4);
    l.tag(&b, &["from-b"], 1);
    std::fs::rename(&a, &b).unwrap();
    l.events(&[&a, &b]);
    assert_eq!(l.meta(&b), (vec!["from-a".to_string()], 4));
    assert_eq!(l.count(), 1);
}

#[test]
fn a_folder_move_rekeys_everything() {
    let l = lab();
    let pid = l.kit("kit");
    let a = l.add("old/a.wav", 0);
    l.tag(&a, &["x"], 3);
    l.lib.project_add(pid, &a).unwrap();
    let (old, new) = (l.path("old"), l.path("new"));
    std::fs::rename(&old, &new).unwrap();
    l.events(&[&old, &new]);
    let moved = l.path("new/a.wav");
    assert_eq!(l.meta(&moved), (vec!["x".to_string()], 3));
    assert_eq!(l.lib.project_paths(pid), set(&[&moved]));
    assert!(l.lib.embedding_of(&moved).is_some() && l.lib.embedding_of(&a).is_none());
}

#[test]
fn a_file_moved_while_athar_was_closed_gets_its_tags_back() {
    let l = lab();
    let pid = l.kit("kit");
    let old = l.add("kick.wav", 0);
    l.tag(&old, &["keeper"], 5);
    l.lib.project_add(pid, &old).unwrap();
    let new = l.path("drums/kick 01.wav");
    std::fs::create_dir(l.path("drums")).unwrap();
    std::fs::rename(&old, &new).unwrap();
    l.scan();
    assert_eq!(l.meta(&new), (vec!["keeper".to_string()], 5));
    assert_eq!(l.lib.project_paths(pid), set(&[&new]));
    let gone: i64 = l.lib.db().query_row("SELECT COUNT(*) FROM gone", [], |r| r.get(0)).unwrap();
    assert_eq!(gone, 0);
}

#[test]
fn a_deleted_file_keeps_its_tags_for_its_return() {
    let l = lab();
    let p = l.add("rain.wav", 0);
    l.tag(&p, &["kyoto"], 4);
    let data = std::fs::read(&p).unwrap();
    std::fs::remove_file(&p).unwrap();
    l.events(&[&p]);
    assert_eq!(l.count(), 0);
    assert_eq!(l.lib.all_tags(), json!([]));
    std::fs::write(&p, data).unwrap();
    l.index(&[&p]);
    assert_eq!(l.meta(&p), (vec!["kyoto".to_string()], 4));
    assert_eq!(l.lib.all_tags(), json!([{"tag": "kyoto", "count": 1}]));
}

#[test]
fn a_missing_folder_is_offline_and_kept() {
    let l = lab();
    let p = l.add("rain.wav", 0);
    l.tag(&p, &["kyoto"], 0);
    let size = std::fs::metadata(&p).unwrap().len();
    let away = format!("{}-away", l.folder);
    std::fs::rename(&l.folder, &away).unwrap();
    assert_eq!(l.lib.folder_counts(), json!([{"path": l.folder, "files": 1, "indexed": 1, "bytes": size, "offline": true}]));
    assert!(l.by_name().is_empty());
    assert_eq!(l.count(), 1);
    std::fs::rename(&away, &l.folder).unwrap();
    assert_eq!(names(&l.by_name()), vec!["rain.wav"]);
}

#[test]
fn forgetting_for_good_drops_tags_and_project_places() {
    let l = lab();
    let pid = l.kit("kit");
    let p = l.add("bad.wav", 0);
    l.tag(&p, &["x"], 0);
    l.lib.project_add(pid, &p).unwrap();
    l.lib.forget(&p, true);
    assert_eq!(l.meta(&p), (vec![], 0));
    assert!(l.lib.project_paths(pid).is_empty());
}

#[test]
fn a_name_in_decomposed_unicode_is_one_sound_and_keeps_its_tags() {
    let l = lab();
    // The file system lists a name in the form it was written with, here e followed by an accent.
    let p = l.write(&"café.wav".nfd().collect::<String>(), 0);
    assert_ne!(p, nfc(&p));
    l.events(&[&p]);
    l.tag(&p, &["keeper"], 3);
    l.scan();
    l.events(&[&p]);
    assert_eq!(l.by_name().iter().map(|r| r["path"].as_str().unwrap()).collect::<Vec<_>>(), vec![p.as_str()]);
    assert_eq!(l.meta(&p), (vec!["keeper".to_string()], 3));
}

#[test]
fn a_sound_indexed_under_another_spelling_of_its_name_keeps_its_tags() {
    let l = lab();
    let p = l.write(&"café.wav".nfd().collect::<String>(), 0);
    // Version 2.0.1 indexed a watched file under the composed form of its name.
    let composed = nfc(&p);
    l.index(&[&composed]);
    l.tag(&composed, &["keeper"], 3);
    l.scan();
    assert_eq!(l.by_name().iter().map(|r| r["path"].as_str().unwrap()).collect::<Vec<_>>(), vec![p.as_str()]);
    assert_eq!(l.meta(&p), (vec!["keeper".to_string()], 3));
}

#[test]
fn a_changed_folder_is_walked_without_the_rest_of_the_library() {
    let l = lab();
    let (new, _elsewhere) = (l.write("new/a.wav", 0), l.write("elsewhere/b.wav", 1));
    l.events(&[&l.path("new")]);
    assert_eq!(names(&l.by_name()), vec!["a.wav"]);
    assert!(l.lib.embedding_of(&new).is_some());
    // What left the folder is forgotten, and only there.
    let kept = l.add("elsewhere/c.wav", 2);
    std::fs::remove_file(&new).unwrap();
    std::fs::remove_file(&kept).unwrap();
    l.lib.scan(&l.folder, &l.path("new"), &stand_in);
    assert!(l.lib.embedding_of(&new).is_none() && l.lib.embedding_of(&kept).is_some());
}

#[test]
fn a_folder_waits_for_one_scan_at_a_time() {
    let l = lab();
    let pending = || l.lib.status()["pending_jobs"].as_u64().unwrap();
    l.lib.rescan();
    l.lib.rescan();
    assert_eq!(pending(), 1);
    l.drain();
    assert_eq!(pending(), 0);
    l.lib.rescan();
    assert_eq!(pending(), 1);
}

#[test]
fn nothing_waits_when_the_worker_never_started() {
    let l = lab();
    let Lab { lib, rx, .. } = l;
    drop(rx);
    lib.rescan();
    assert_eq!(lib.status()["pending_jobs"], 0);
}

#[test]
fn a_file_that_cannot_be_embedded_is_recorded_and_not_tried_again() {
    let l = lab();
    let p = l.write("rain.wav", 0);
    l.lib.index(&l.folder, vec![p.clone()], &|_| Err("the model gave up".into()));
    assert_eq!(l.count(), 0);
    let error: Option<String> = l.lib.db().query_row("SELECT error FROM files WHERE path=?", [&p], |r| r.get(0)).unwrap();
    assert_eq!(error.as_deref(), Some("the model gave up"));
    l.scan();
    assert_eq!(l.count(), 0, "unchanged, it is left alone");
    std::fs::remove_file(&p).unwrap();
    l.write("rain.wav", 1);
    l.scan();
    assert_eq!(l.count(), 1, "changed, it is tried again");
}

#[test]
fn an_embedding_from_another_model_is_dropped_on_opening() {
    let l = lab();
    let (a, b) = (l.add("a.wav", 0), l.add("b.wav", 1));
    l.lib.db().execute("UPDATE files SET emb=x'0000803f' WHERE path=?", [&a]).unwrap();
    let (again, _rx) = Library::create(l.root.join("home"), l.root.join("no model")).unwrap();
    assert!(again.embedding_of(&a).is_none() && again.embedding_of(&b).is_some());
    again.scan(&l.folder, &l.folder, &stand_in);
    assert!(again.embedding_of(&a).is_some(), "the next scan indexes it again");
}
