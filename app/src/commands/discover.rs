//! Discover: searching the online archives, previewing a found sound and adding it to a project.

use super::{progress, s, Files, Lib};
use crate::discover::{self, Amount};
use crate::library::{Library, Res};
use crate::sounds::Sound;
use athar_core::decode::{decode, Decoded};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How much of a found sound a preview fetches: about PREVIEW_FETCH_SECONDS of it when its
/// length is known, and at most PREVIEW_BYTES. Adding it to a project fetches all of it.
const PREVIEW_BYTES: u64 = 3 * 1024 * 1024;
const PREVIEW_FETCH_SECONDS: f64 = 20.0;
/// A file that cannot be read from its start alone is fetched whole for a preview, up to this
/// size.
const PREVIEW_WHOLE_BYTES: u64 = 50 * 1024 * 1024;
/// A preview plays at most this many seconds.
const PREVIEW_SECONDS: usize = 120;
/// Preview folders of earlier runs are cleared once they are this old.
const STALE: Duration = Duration::from_secs(24 * 60 * 60);

fn freesound_key(lib: &Library) -> String {
    lib.settings()["freesound_key"].as_str().unwrap_or("").to_string()
}

#[tauri::command(async)]
pub fn discover_sources(lib: Lib) -> Value {
    discover::sources(&freesound_key(&lib))
}

#[tauri::command(async)]
pub fn discover_search(lib: Lib, req: Value) -> Res<Value> {
    discover::search(s(&req, "source"), s(&req, "query"), req["page"].as_u64().unwrap_or(1), &freesound_key(&lib))
}

#[tauri::command(async)]
pub fn discover_waveform(req: Value) -> Value {
    discover::waveform(s(&req, "id"), 48)
}

#[tauri::command(async)]
pub fn discover_files(req: Value) -> Res<Value> {
    discover::files(s(&req, "source"), s(&req, "id"))
}

/// Where this run fetches previews to. Each file is deleted as soon as it is decoded, and the
/// folder when the app quits, so nothing a preview fetches stays on disk.
fn preview_dir() -> PathBuf {
    std::env::temp_dir().join(format!("athar-preview-{}", std::process::id()))
}

pub fn remove_preview_dir() {
    let _ = std::fs::remove_dir_all(preview_dir());
}

/// Removes preview folders left by earlier runs that quit before they cleaned up. A folder in
/// recent use may belong to another Athar that is running, and stays.
pub fn clear_old_previews() {
    let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) else { return };
    let mine = preview_dir();
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|age| age > STALE));
        if old && e.file_name().to_string_lossy().starts_with("athar-preview-") && e.path() != mine {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// A found sound fetched from its archive and decoded for the engine, to preview it: the start of
/// a long file, or all of a short one. Progress goes out as `discover-progress` events.
#[tauri::command(async)]
pub fn discover_preview(app: tauri::AppHandle, files: Files, url: String, duration: Option<f64>) -> Res<Value> {
    let tell = progress(&app, &url);
    let dir = preview_dir();
    let fetch = |amount: Amount| -> Res<(Decoded, bool)> {
        let (path, partial) = discover::fetch(&url, &dir, "preview", amount, &tell)?;
        let d = decode(&path);
        let _ = std::fs::remove_file(&path);
        Ok((d.map_err(|_| "That sound could not be played from the archive".to_string())?, partial))
    };
    let start = |total: u64| match duration.filter(|d| *d > 0.0) {
        Some(d) if total > 0 => ((total as f64 * PREVIEW_FETCH_SECONDS / d) as u64).clamp(256 * 1024, PREVIEW_BYTES),
        _ => PREVIEW_BYTES,
    };
    // Some formats cannot be read from their start alone (an MP4 can keep its index at the end);
    // those are fetched whole, when they are small enough to decode in one go.
    let whole = Amount::Whole(PREVIEW_WHOLE_BYTES, "That sound is too large to preview. Add it to a project to hear it");
    let (mut d, partial) = match fetch(Amount::Start(&start)) {
        Ok((d, partial)) if d.frames() > 0 => (d, partial),
        Ok((_, true)) | Err(_) => fetch(whole)?,
        Ok(r) => r,
    };
    let cap = PREVIEW_SECONDS * d.sr as usize;
    let partial = partial || d.frames() > cap;
    d.channels.iter_mut().for_each(|c| c.truncate(cap));
    let duration = d.duration();
    Ok(json!({"key": files.upload(Sound::new(d.channels, d.sr)), "duration": duration, "partial": partial}))
}

/// Downloads a found sound into a project's folder and adds it to the project, with where it
/// came from and its licence written into its note.
#[tauri::command(async)]
pub fn discover_save(app: tauri::AppHandle, lib: Lib, req: Value) -> Res<Value> {
    let pid = req["project"].as_i64().unwrap_or(0);
    let dest = lib.project_folder_ready(pid)?.ok_or("That project has no folder yet. Choose one on the Projects page")?;
    let url = s(&req, "url");
    let path = discover::download(url, Path::new(&dest), s(&req, "title"), &progress(&app, url))?;
    let p = path.to_string_lossy().into_owned();
    let note: String = s(&req, "note").chars().take(2000).collect();
    let tags: Vec<&str> = ["discovered", s(&req, "source")].into_iter().filter(|t| !t.is_empty()).collect();
    lib.catalog_set(&p, &json!({"memo": note, "tags": tags}))?;
    lib.index_file(&p);
    lib.project_add(pid, &p)?;
    Ok(json!({"path": p}))
}
