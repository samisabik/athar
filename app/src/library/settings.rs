//! Settings: the defaults, and the choices saved over them.

use serde_json::{json, Value};
use std::path::PathBuf;
use super::paths::home;
use super::{Library, Res, report};

const OLD_NAME_PATTERN: &str = "{project}_{category}_{n:03d}_{name}";

impl Library {
    /// The settings that were chosen; every other setting follows its default.
    pub(super) fn saved_settings(&self) -> serde_json::Map<String, Value> {
        self.meta_get("settings").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// Settings were once saved whole, so a library from before the default naming pattern
    /// changed holds the old default as if it had been chosen.
    pub(super) fn retire_old_name_pattern(&self) {
        let mut saved = self.saved_settings();
        if saved.get("name_pattern").is_some_and(|v| v == OLD_NAME_PATTERN) {
            saved.remove("name_pattern");
            report("retiring the old naming pattern", self.meta_set("settings", &Value::Object(saved).to_string()));
        }
    }

    pub(super) fn default_projects_dir(&self) -> PathBuf {
        // Started on its own data folder (tests, trials), Athar keeps projects in there too.
        if std::env::var_os("ATHAR_HOME").is_some() { self.data_dir.join("Projects") } else { home().join("Music/Athar Projects") }
    }

    pub fn settings(&self) -> Value {
        let mut st = default_settings();
        for (k, v) in self.saved_settings() {
            if st.get(&k).is_some() {
                st[k] = v;
            }
        }
        if st["projects_dir"].as_str().unwrap_or("").is_empty() {
            st["projects_dir"] = json!(self.default_projects_dir());
        }
        st
    }

    /// Only settings that differ from their default are saved, so a default that changes in a
    /// later version reaches every library that had not chosen otherwise.
    pub fn update_settings(&self, changes: &Value) -> Res<Value> {
        let defaults = default_settings();
        let mut saved = self.saved_settings();
        if let Some(ch) = changes.as_object() {
            for (k, v) in ch {
                let Some(d) = defaults.get(k) else { continue };
                let coerced = match d {
                    Value::Bool(_) => json!(v.as_bool().unwrap_or_else(|| v.as_f64().map(|n| n != 0.0).unwrap_or(false))),
                    Value::Number(n) if n.is_i64() => json!(v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).ok_or(format!("{k} must be a number"))?.round() as i64),
                    Value::Number(_) => json!(v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).ok_or(format!("{k} must be a number"))?),
                    _ => json!(v.as_str().map(String::from).unwrap_or_else(|| v.to_string())),
                };
                saved.insert(k.clone(), coerced);
            }
        }
        let projects_dir = json!(self.default_projects_dir());
        saved.retain(|k, v| defaults.get(k).is_some_and(|d| d != v) && !(k == "projects_dir" && *v == projects_dir));
        self.meta_set("settings", &Value::Object(saved).to_string())?;
        Ok(self.settings())
    }
}

fn default_settings() -> Value {
    json!({
        "text_size": 13, "show_cat": true, "cat_skin": "sukey", "accent": "#7a3dff", "autoplay": false,
        "preview_volume": 80, "default_sort": "name",
        "rescan_on_open": true, "export_rate": 0, "export_bits": 24, "export_mono": false, "edit_suffix": "_edit",
        "export_format": "wav", "smooth_edits": true, "edit_fade_ms": 3, "loudness_target": -16.0,
        "prep_target_db": -1.0, "prep_fade_in_ms": 2, "prep_fade_out_ms": 10, "silence_db": -45,
        "slice_gap_ms": 250, "sp404_mono": false, "name_pattern": "{project}_{name}", "projects_dir": "",
        "theme": "system", "freesound_key": ""
    })
}
