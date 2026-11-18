//! The saved arrangement.

use serde_json::{json, Value};
use super::{lab};

fn clip(path: &str, over: Value) -> Value {
    let mut c = json!({"path": path, "name": "x", "duration": 1.0, "sr": 48000, "at": 0.5, "in": 0.1, "out": 0.2});
    c.as_object_mut().unwrap().extend(over.as_object().unwrap().clone());
    c
}

fn lane(clips: Vec<Value>, over: Value) -> Value {
    let mut l = json!({"name": "Drums", "gain": -3, "mute": false, "solo": true, "clips": clips});
    l.as_object_mut().unwrap().extend(over.as_object().unwrap().clone());
    l
}

#[test]
fn the_arrangement_is_kept_and_starts_empty() {
    let l = lab();
    assert_eq!(l.lib.arrangement_get()["lanes"], json!([]));
    let p = l.add("kick.wav", 0);
    l.lib.arrangement_set(&json!({"loop": false, "lanes": [lane(vec![clip(&p, json!({}))], json!({}))]})).unwrap();
    let a = l.lib.arrangement_get();
    assert_eq!(a["loop"], false);
    assert_eq!(a["lanes"], json!([lane(vec![clip(&p, json!({"name": "kick.wav"}))], json!({"gain": -3.0}))]));
}

#[test]
fn arrangement_values_are_held_in_range() {
    let l = lab();
    let p = l.add("kick.wav", 0);
    let mut lanes = vec![lane(vec![clip(&p, json!({"at": -1})), clip(&p, json!({"at": 1e9, "in": 0.8, "out": 0.9}))], json!({"gain": 40, "name": "  "}))];
    lanes.extend(std::iter::repeat_n(lane(vec![], json!({})), 40));
    let a = l.lib.arrangement_set(&json!({"lanes": lanes})).unwrap();
    assert_eq!(a["loop"], true);
    assert_eq!(a["lanes"].as_array().unwrap().len(), 16);
    let first = &a["lanes"][0];
    assert_eq!((first["gain"].as_f64(), first["name"].as_str()), (Some(6.0), Some("Lane 1")));
    let ats: Vec<f64> = first["clips"].as_array().unwrap().iter().map(|c| c["at"].as_f64().unwrap()).collect();
    assert_eq!(ats, vec![0.0, 600.0]);
    // Fades that would meet in the middle are cut back to the length of the clip.
    assert_eq!(first["clips"][1]["in"], 0.8);
    assert!((first["clips"][1]["out"].as_f64().unwrap() - 0.2).abs() < 1e-9);
    assert_eq!(a["lanes"][1]["clips"], json!([]));
}

#[test]
fn the_arrangement_rejects_a_clip_without_a_path() {
    let l = lab();
    assert!(l.lib.arrangement_set(&json!({"lanes": [{"clips": [{"at": 0}]}]})).is_err());
}

#[test]
fn an_arrangement_of_one_sound_per_lane_becomes_lanes_of_clips() {
    let l = lab();
    let p = l.add("kick.wav", 0);
    let old = json!({"bpm": 120, "grid": false, "lanes": [{"path": p, "name": "kick.wav", "duration": 0.1, "sr": 48000, "gain": -6, "mute": true, "solo": false, "hits": [0, 0.5]}]});
    l.lib.meta_set("arrangement", &old.to_string()).unwrap();
    let a = l.lib.arrangement_get();
    let lanes: Vec<_> = a["lanes"].as_array().unwrap().iter().map(|x| (x["name"].as_str().unwrap().to_string(), x["gain"].as_f64().unwrap(), x["mute"].as_bool().unwrap())).collect();
    assert_eq!(lanes, vec![("kick".to_string(), -6.0, true)]);
    let clips: Vec<_> = a["lanes"][0]["clips"].as_array().unwrap().iter().map(|c| (c["path"].as_str().unwrap().to_string(), c["at"].as_f64().unwrap(), c["in"].as_f64().unwrap(), c["out"].as_f64().unwrap())).collect();
    assert_eq!(clips, vec![(p.clone(), 0.0, 0.0, 0.0), (p.clone(), 0.5, 0.0, 0.0)]);
}

#[test]
fn the_arrangement_follows_a_renamed_file() {
    let l = lab();
    let (p, other) = (l.add("kick.wav", 1), l.add("snare.wav", 2));
    l.lib.arrangement_set(&json!({"lanes": [lane(vec![clip(&p, json!({})), clip(&other, json!({}))], json!({}))]})).unwrap();
    let new = l.lib.rename(&p, "boom").unwrap();
    let clips: Vec<_> = l.lib.arrangement_get()["lanes"][0]["clips"].as_array().unwrap().iter().map(|c| (c["path"].as_str().unwrap().to_string(), c["name"].as_str().unwrap().to_string())).collect();
    assert_eq!(clips, vec![(new, "boom.wav".to_string()), (other, "snare.wav".to_string())]);
}

#[test]
fn the_arrangement_follows_a_moved_folder() {
    let l = lab();
    let p = l.add("drums/kick.wav", 0);
    l.lib.arrangement_set(&json!({"lanes": [lane(vec![clip(&p, json!({}))], json!({}))]})).unwrap();
    let (old, new) = (l.path("drums"), l.path("kit"));
    std::fs::rename(&old, &new).unwrap();
    l.events(&[&old, &new]);
    assert_eq!(l.lib.arrangement_get()["lanes"][0]["clips"][0]["path"], l.path("kit/kick.wav"));
}
