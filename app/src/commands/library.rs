//! The library: folders, search, the map, tags and files, settings and backups.

use super::{done, s, Lib};
use crate::library::{self, Res, SearchQuery};
use crate::{mapnames, sounds};
use serde_json::{json, Value};
use tauri_plugin_dialog::DialogExt;

#[tauri::command(async)]
pub fn status(lib: Lib) -> Value {
    lib.status()
}

#[tauri::command(async)]
pub fn get_settings(lib: Lib) -> Value {
    let mut st = lib.settings();
    st["_data_dir"] = json!(lib.data_dir);
    st["_version"] = json!(env!("CARGO_PKG_VERSION"));
    st["_preview_max_s"] = json!(sounds::PREVIEW_MAX_SECONDS);
    st
}

#[tauri::command(async)]
pub fn set_settings(lib: Lib, req: Value) -> Res<Value> {
    lib.update_settings(&req)
}

#[tauri::command(async)]
pub fn folders(lib: Lib) -> Value {
    lib.folder_counts()
}

#[tauri::command(async)]
pub fn add_folder(lib: Lib, req: Value) -> Res<Value> {
    lib.add_folder(s(&req, "path")).map(|p| json!({"path": p}))
}

#[tauri::command(async)]
pub fn remove_folder(lib: Lib, req: Value) -> Value {
    lib.remove_folder(s(&req, "path"));
    done()
}

#[tauri::command(async)]
pub fn subfolders(lib: Lib, req: Value) -> Value {
    lib.subfolders(s(&req, "path"))
}

#[tauri::command(async)]
pub fn rescan(lib: Lib) -> Value {
    lib.rescan();
    done()
}

#[tauri::command(async)]
pub fn categories(lib: Lib) -> Value {
    lib.category_counts()
}

/// Sounds like the file `similar_to`, or matching the words of `query`, or all of them, within
/// the request's filters.
#[tauri::command]
pub async fn search(lib: Lib<'_>, req: SearchQuery) -> Res<Value> {
    let (vec, exclude, words) = if let Some(p) = req.similar_to.as_deref().filter(|p| !p.is_empty()) {
        (Some(lib.embedding_of(p).ok_or("That file is not indexed yet")?), Some(p.to_string()), None)
    } else if let Some(q) = req.query.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        (Some(lib.embed_text(q)?), None, Some(q.to_string()))
    } else {
        (None, None, None)
    };
    lib.search(&req, vec, exclude.as_deref(), words.as_deref())
}

#[tauri::command(async)]
pub fn sound_map(lib: Lib) -> Value {
    lib.sound_map()
}

/// The names written on the Map at zoom `level` (see mapnames.rs).
#[tauri::command(async)]
pub fn map_names(lib: Lib, level: u32) -> Value {
    mapnames::names(&lib.sound_map(), level)
}

#[tauri::command(async)]
pub fn tags(lib: Lib) -> Value {
    lib.all_tags()
}

#[tauri::command(async)]
pub fn catalog(lib: Lib, req: Value) -> Res<Value> {
    let p = s(&req, "path");
    lib.known_path(p)?;
    lib.catalog_set(p, &req)
}

#[tauri::command(async)]
pub fn trash(lib: Lib, req: Value) -> Res<Value> {
    lib.trash(s(&req, "path")).map(|_| done())
}

#[tauri::command(async)]
pub fn rename(lib: Lib, req: Value) -> Res<Value> {
    lib.rename(s(&req, "path"), s(&req, "name")).map(|p| json!({"path": p}))
}

/// Runs macOS's `open`, which shows things in Finder.
fn finder(args: &[&str]) {
    let _ = std::process::Command::new("/usr/bin/open").args(args).spawn();
}

/// Shows a library file in Finder, selected in its folder.
#[tauri::command(async)]
pub fn reveal(lib: Lib, req: Value) -> Res<Value> {
    let p = s(&req, "path");
    lib.known_path(p)?;
    finder(&["-R", p]);
    Ok(done())
}

/// Shows a folder of the library in Finder.
#[tauri::command(async)]
pub fn reveal_folder(lib: Lib, req: Value) -> Res<Value> {
    let p = std::fs::canonicalize(s(&req, "path")).map_err(|e| e.to_string())?;
    let ps = p.to_string_lossy().into_owned();
    let inside = lib.folders().iter().any(|f| {
        let r = std::fs::canonicalize(f).map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
        ps == r || library::is_inside(&ps, &r)
    });
    if !p.is_dir() || !inside {
        return Err("That folder is not part of your library".into());
    }
    // Handed to Finder by name: a folder that is a bundle (an app, a project package) is then
    // shown, where `open` alone would launch it.
    finder(&["-a", "Finder", &ps]);
    Ok(done())
}

#[tauri::command(async)]
pub fn reveal_data(lib: Lib) -> Value {
    finder(&[&lib.data_dir.to_string_lossy()]);
    done()
}

#[tauri::command(async)]
pub fn backup(lib: Lib, req: Value) -> Res<Value> {
    lib.export_backup(s(&req, "path")).map(|p| json!({"path": p}))
}

#[tauri::command(async)]
pub fn restore(lib: Lib, req: Value) -> Res<Value> {
    lib.restore_backup(s(&req, "path"))
}

#[tauri::command]
pub async fn pick_folder(app: tauri::AppHandle, title: Option<String>, start: Option<String>) -> Option<String> {
    let mut d = app.dialog().file();
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        d = d.set_title(t);
    }
    if let Some(s) = start.filter(|s| !s.is_empty()) {
        d = d.set_directory(s);
    }
    d.blocking_pick_folder().and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn pick_file(app: tauri::AppHandle) -> Option<String> {
    app.dialog().file().blocking_pick_file().and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned())
}
