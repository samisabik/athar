//! The engine as the page uses it directly: the preview player, the output level and volume.

use super::{Arr, Audio, Ed, Files, Lib, Rec};
use crate::events::Watch;
use crate::library::Res;
use crate::sounds::Sounds;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use tauri::State;

#[tauri::command(async)]
pub fn audio_info(audio: Audio) -> Value {
    json!({"rate": audio.rate(), "now": audio.now(), "error": audio.error()})
}

/// The clock and the output level, for the scripts in app/drive.
#[tauri::command(async)]
pub fn audio_poll(audio: Audio) -> Value {
    json!({"rate": audio.rate(), "now": audio.now(), "level": audio.level()})
}

/// A file made ready for the preview player: its key in the engine, the length held, and
/// whether that is only the start of the file.
#[tauri::command(async)]
pub fn audio_preview(lib: Lib, files: Files, path: String) -> Res<Value> {
    lib.known_path(&path)?;
    let (key, sound, whole) = files.preview(&path)?;
    let length = sound.seconds();
    Ok(json!({"key": key, "length": length, "capped": whole > length + 0.01}))
}

#[derive(serde::Deserialize)]
pub struct StartReq {
    id: u64,
    /// The key of a sound held in memory (see sounds.rs).
    sound: String,
    offset: f64,
    duration: Option<f64>,
    looped: Option<(f64, f64)>,
}

/// Plays a sound now, straight to the output.
#[tauri::command(async)]
pub fn audio_start(lib: Lib, audio: Audio, files: Files, req: StartReq) -> Res<()> {
    if let Some(p) = Sounds::file_of(&req.sound) {
        lib.known_path(p)?;
    }
    let s = files.get(&req.sound)?;
    audio.play(req.id, &s, audio.now(), req.offset, req.duration, req.looped, vec![])
}

#[tauri::command(async)]
pub fn audio_stop(audio: Audio, id: u64) {
    audio.stop(id, 0.0);
}

#[tauri::command(async)]
pub fn audio_volume(audio: Audio, v: f32) {
    audio.set_volume(v);
}

/// Whether the page wants the spectrum of what plays (the EQ panel is open).
#[tauri::command(async)]
pub fn audio_watch_spectrum(watch: State<Watch>, on: bool) {
    watch.spectrum.store(on, Ordering::Relaxed);
}

/// The page has just loaded, after a start or a reload: whatever a page before it left playing
/// or recording stops, since nothing could stop it any more.
#[tauri::command(async)]
pub fn audio_reset(audio: Audio, ed: Ed, arr: Arr, rec: Rec) {
    ed.stop();
    arr.stop();
    rec.stop();
    audio.stop_all();
}
