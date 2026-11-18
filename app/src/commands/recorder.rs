//! The recorder: takes from an input device, kept in memory until they are saved or discarded.

use super::save::save_wav;
use super::{Files, Lib, Rec};
use crate::input;
use crate::library::Res;
use crate::sounds::Sound;
use athar_core::dsp;
use serde_json::{json, Value};

#[tauri::command(async)]
pub fn rec_devices() -> Vec<String> {
    input::devices()
}

#[tauri::command(async)]
pub fn rec_start(rec: Rec, device: String) -> Res<Value> {
    let (sr, channels) = rec.start(&device)?;
    Ok(json!({"sr": sr, "channels": channels}))
}

/// Ends the take and keeps it in memory; returns its key and length.
#[tauri::command(async)]
pub fn rec_stop(rec: Rec, files: Files) -> Value {
    let (channels, sr) = rec.finish();
    let frames = channels.first().map_or(0, Vec::len);
    if frames == 0 {
        return json!({"key": null, "frames": 0, "sr": sr});
    }
    let n = channels.len();
    json!({"key": files.hold(Sound::new(channels, sr)), "frames": frames, "sr": sr, "channels": n})
}

#[tauri::command(async)]
pub fn rec_discard(files: Files, key: String) {
    files.release(&key);
}

/// A recorded take saved as a new file.
#[tauri::command(async)]
pub fn rec_save(lib: Lib, files: Files, key: String, meta: Value) -> Res<Value> {
    let s = files.get(&key)?;
    let wav = dsp::render_wav(&s.channels, s.sr, &dsp::RenderArgs { rate: s.sr, bits: 24, mono: false, prep: None })?;
    Ok(json!({"path": save_wav(&lib, &wav, &meta)?}))
}
