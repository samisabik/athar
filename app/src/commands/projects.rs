//! Projects: their sounds, order, folders and pads, exports, and the stored arrangement.

use super::{done, s, Lib};
use crate::library::{Library, Res};
use crate::sp404;
use serde_json::{json, Value};

fn known(lib: &Library, id: i64) -> Res<()> {
    if lib.project_exists(id) { Ok(()) } else { Err("Project not found".into()) }
}

/// A text field of a request, or `default` when it is empty or missing.
fn or<'a>(req: &'a Value, k: &str, default: &'a str) -> &'a str {
    Some(s(req, k)).filter(|v| !v.is_empty()).unwrap_or(default)
}

#[tauri::command(async)]
pub fn projects(lib: Lib) -> Value {
    lib.drop_missing_projects();
    lib.adopt_project_folders();
    lib.projects()
}

#[tauri::command(async)]
pub fn create_project(lib: Lib, req: Value) -> Res<Value> {
    lib.create_project(s(&req, "name"))
}

#[tauri::command(async)]
pub fn rename_project(lib: Lib, id: i64, req: Value) -> Res<Value> {
    lib.rename_project(id, s(&req, "name")).map(|_| done())
}

#[tauri::command(async)]
pub fn delete_project(lib: Lib, id: i64) -> Res<Value> {
    lib.delete_project(id)?;
    Ok(done())
}

/// Adds a library sound to a project, or with `remove` takes it out.
#[tauri::command(async)]
pub fn project_item(lib: Lib, id: i64, req: Value) -> Res<Value> {
    let p = s(&req, "path");
    if req["remove"].as_bool().unwrap_or(false) {
        lib.project_remove(id, p)?;
    } else {
        lib.known_path(p)?;
        lib.project_add(id, p).map_err(|e| format!("Could not copy the sound into the project folder: {e}"))?;
    }
    Ok(done())
}

#[tauri::command(async)]
pub fn project_folder(lib: Lib, id: i64, req: Value) -> Res<Value> {
    known(&lib, id)?;
    lib.set_project_folder(id, s(&req, "path")).map(|f| json!({"folder": f}))
}

#[tauri::command(async)]
pub fn set_project_order(lib: Lib, id: i64, req: Value) -> Res<Value> {
    let paths: Vec<String> = serde_json::from_value(req["paths"].clone()).unwrap_or_default();
    lib.set_project_order(id, &paths)?;
    Ok(done())
}

#[tauri::command(async)]
pub fn project_pads(lib: Lib, id: i64) -> Value {
    lib.project_pads(id)
}

#[tauri::command(async)]
pub fn set_project_pads(lib: Lib, id: i64, req: Value) -> Res<Value> {
    known(&lib, id)?;
    lib.set_project_pads(id, &req["pads"]).map(|_| done())
}

#[tauri::command(async)]
pub fn export_project(lib: Lib, id: i64, req: Value) -> Res<Value> {
    known(&lib, id)?;
    lib.export_project(id, s(&req, "dest"), or(&req, "pattern", "{project}_{name}"), or(&req, "format", "copy"))
}

/// The SP-404MKII's SD card, when one is mounted, and its IMPORT folder.
#[tauri::command(async)]
pub fn sp404_card() -> Value {
    match sp404::card() {
        Some(c) => json!({"card": c, "import": format!("{c}/IMPORT")}),
        None => json!({"card": null}),
    }
}

#[tauri::command(async)]
pub fn arrangement(lib: Lib) -> Value {
    lib.arrangement_get()
}

pub fn store_arrangement(lib: &Library, a: &Value) -> Res<Value> {
    lib.arrangement_set(a).map_err(|e| format!("Bad arrangement: {e}"))
}
