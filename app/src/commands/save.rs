//! Writing a rendered sound as a new file of the library.

use super::{s, Lib};
use crate::library::{self, Library, Res};
use crate::{export, sp404};
use athar_core::dsp;
use serde_json::{json, Value};
use std::path::Path;

/// Writes a rendered WAV as a new file. `meta` says where and how: { dest_dir, name, format,
/// source, project, sp404 }. Returns the new file's path.
pub fn save_wav(lib: &Library, data: &[u8], meta: &Value) -> Res<String> {
    let project = meta["project"].as_i64().unwrap_or(0);
    let sp404 = meta["sp404"].as_bool().unwrap_or(false);
    let format = Some(s(meta, "format")).filter(|f| !f.is_empty()).unwrap_or("wav");
    if !["wav", "flac", "mp3"].contains(&format) || (sp404 && format != "wav") {
        return Err(format!("Cannot save as {format}"));
    }
    let mut dir = lib.save_target_dir(project, s(meta, "dest_dir"))?;
    if sp404 {
        dir = sp404::import_dir(&dir);
    }
    let stem = Path::new(s(meta, "name")).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = if sp404 { library::ascii_name(&stem) } else { library::safe_name(&stem) };
    let target = library::unique_path(&dir.join(format!("{stem}.{format}")))?;
    let write = |bytes: &[u8]| std::fs::write(&target, bytes).map_err(|e| format!("Save failed: {e}"));
    if sp404 {
        sp404::write_wav(data, &target, lib.settings()["sp404_mono"].as_bool().unwrap_or(false))?;
    } else if format == "wav" {
        write(data)?;
    } else {
        write(&export::transcode(data, format)?)?;
    }
    let t = target.to_string_lossy().into_owned();
    lib.after_save(&t, s(meta, "source"), project)?;
    Ok(t)
}

/// A library file rendered (optionally prepared for a sampler) and saved as a new file, for a
/// batch export.
#[tauri::command(async)]
pub fn batch_save(lib: Lib, path: String, p: Value, meta: Value) -> Res<Value> {
    lib.known_path(&path)?;
    let d = athar_core::decode::decode(Path::new(&path)).map_err(|e| e.to_string())?;
    let wav = dsp::render_wav(&d.channels, d.sr, &dsp::RenderArgs::from_json(&p, d.sr))?;
    Ok(json!({"path": save_wav(&lib, &wav, &meta)?}))
}
