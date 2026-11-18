//! The saved arrangement: lanes of clips placed in time.

use serde_json::{json, Value};
use std::path::Path;
use super::paths::basename;
use super::{Library, Res, report};

const ARRANGE_MAX_LANES: usize = 16;

const ARRANGE_MAX_CLIPS: usize = 64;

const ARRANGE_MAX_SECONDS: f64 = 600.0;

impl Library {
    pub fn arrangement_get(&self) -> Value {
        self.meta_get("arrangement").and_then(|s| serde_json::from_str(&s).ok()).map(|a| lanes_of_clips(&a)).unwrap_or(json!({"loop": true, "lanes": []}))
    }

    pub fn arrangement_set(&self, data: &Value) -> Res<Value> {
        let data = &lanes_of_clips(data);
        let mut lanes = Vec::new();
        for (n, lane) in data["lanes"].as_array().cloned().unwrap_or_default().iter().take(ARRANGE_MAX_LANES).enumerate() {
            let mut clips = Vec::new();
            for clip in lane["clips"].as_array().cloned().unwrap_or_default().iter().take(ARRANGE_MAX_CLIPS) {
                let path = clip["path"].as_str().ok_or("clip without a path")?.to_string();
                let duration = clip["duration"].as_f64().unwrap_or(0.0).max(0.0);
                let fade_in = clip["in"].as_f64().unwrap_or(0.0).max(0.0).min(duration);
                let fade_out = clip["out"].as_f64().unwrap_or(0.0).max(0.0).min(duration - fade_in);
                clips.push(json!({
                    "path": path, "name": basename(&path), "duration": duration, "sr": clip["sr"].as_i64().unwrap_or(0),
                    "at": clip["at"].as_f64().unwrap_or(0.0).clamp(0.0, ARRANGE_MAX_SECONDS), "in": fade_in, "out": fade_out,
                }));
            }
            let name: String = lane["name"].as_str().unwrap_or("").trim().chars().take(60).collect();
            lanes.push(json!({
                "name": if name.is_empty() { format!("Lane {}", n + 1) } else { name },
                "gain": lane["gain"].as_f64().unwrap_or(0.0).clamp(-24.0, 6.0),
                "mute": lane["mute"].as_bool().unwrap_or(false), "solo": lane["solo"].as_bool().unwrap_or(false), "clips": clips,
            }));
        }
        let clean = json!({"loop": data["loop"].as_bool().unwrap_or(true), "lanes": lanes});
        self.meta_set("arrangement", &clean.to_string())?;
        Ok(clean)
    }

    pub(super) fn move_in_arrangement(&self, src: &str, dst: &str) {
        let mut a = self.arrangement_get();
        let mut changed = false;
        if let Some(lanes) = a["lanes"].as_array_mut() {
            for lane in lanes {
                if let Some(clips) = lane["clips"].as_array_mut() {
                    for c in clips {
                        if c["path"] == src {
                            c["path"] = json!(dst);
                            c["name"] = json!(basename(dst));
                            changed = true;
                        }
                    }
                }
            }
        }
        if changed {
            report("moving a file in the arrangement", self.meta_set("arrangement", &a.to_string()));
        }
    }
}

/// An arrangement kept from when each lane was one sound placed several times, as it is now:
/// named lanes holding clips. Each old lane becomes a lane named after its sound.
fn lanes_of_clips(data: &Value) -> Value {
    let old = data["lanes"].as_array().cloned().unwrap_or_default();
    if old.iter().all(|l| l.get("clips").is_some()) {
        return data.clone();
    }
    let lanes: Vec<Value> = old
        .iter()
        .map(|lane| {
            if lane.get("clips").is_some() {
                return lane.clone();
            }
            let name = lane["name"].as_str().unwrap_or("");
            let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let clips: Vec<Value> = lane["hits"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|at| json!({"path": lane["path"], "name": name, "duration": lane["duration"].as_f64().unwrap_or(0.0), "sr": lane["sr"].as_i64().unwrap_or(0), "at": at, "in": 0.0, "out": 0.0}))
                .collect();
            json!({"name": stem, "gain": lane["gain"].as_f64().unwrap_or(0.0), "mute": lane["mute"].as_bool().unwrap_or(false), "solo": lane["solo"].as_bool().unwrap_or(false), "clips": clips})
        })
        .collect();
    json!({"loop": data["loop"].as_bool().unwrap_or(true), "lanes": lanes})
}
