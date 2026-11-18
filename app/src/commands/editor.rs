//! The editor: its document (see doc.rs), what its effect panels draw, and saving its sound.

use super::save::save_wav;
use super::{Audio, Ed, Files, Lib};
use crate::doc::{self, Ui};
use crate::library::Res;
use athar_core::dsp;
use serde_json::{json, Value};
use tauri::ipc::Response;
use tauri_plugin_dialog::DialogExt;

/// Opens a library file, or a sound held in memory (`key`, a recorded take), in the editor.
#[tauri::command(async)]
pub fn ed_open(lib: Lib, files: Files, ed: Ed, path: Option<String>, key: Option<String>) -> Res<Value> {
    let (sound, from) = match (path, key) {
        (Some(p), _) => {
            lib.known_path(&p)?;
            (files.file(&p)?, Some(p))
        }
        (None, Some(k)) => (files.get(&k)?, None),
        _ => return Err("Nothing to open".into()),
    };
    Ok(ed.open(sound, from))
}

/// The editor's sound was saved as `path` at `version`.
#[tauri::command(async)]
pub fn ed_saved(ed: Ed, path: String, version: u64) -> Value {
    ed.saved(path, version)
}

#[tauri::command(async)]
pub fn ed_regions(ed: Ed, p: Value, ui: Ui) -> Res<Value> {
    ed.regions(&p, ui)
}

#[tauri::command(async)]
pub fn ed_edit(ed: Ed, p: Value, ui: Option<Ui>) -> Res<Value> {
    let mut p = p;
    if let Some(ui) = ui {
        p["ui"] = json!(ui);
    }
    ed.edit(&p)
}

#[tauri::command(async)]
pub fn ed_undo(ed: Ed, redo: bool, ui: Ui) -> Res<Value> {
    ed.undo(redo, ui)
}

#[tauri::command(async)]
pub fn ed_view(ed: Ed, key: String, p: Value, head_s: Option<f64>) -> Res<Value> {
    ed.view(&key, &p, head_s)
}

/// Learns the noise print from [a, b) of the sound; without a range, forgets it.
#[tauri::command(async)]
pub fn ed_noise(ed: Ed, range: Option<(usize, usize)>) -> Res<()> {
    match range {
        Some((a, b)) => ed.learn_noise(a, b),
        None => {
            ed.forget_noise();
            Ok(())
        }
    }
}

#[tauri::command(async)]
pub fn ed_analyse(ed: Ed, p: Value) -> Res<Value> {
    ed.analyse(&p)
}

#[tauri::command(async)]
pub fn ed_peaks(ed: Ed, src: Option<String>, start: f64, spp: f64, width: usize) -> Res<Response> {
    ed.peaks(src.as_deref(), start, spp, width).map(Response::new)
}

#[tauri::command(async)]
pub fn ed_spec(ed: Ed, src: Option<String>, start: f64, spp: f64, pw: usize, rows: usize) -> Res<Response> {
    ed.spectrogram(src.as_deref(), start, spp, pw, rows).map(Response::new)
}

#[tauri::command(async)]
pub fn ed_play(ed: Ed, p: Value) -> Res<Value> {
    ed.play(&p)
}

#[tauri::command(async)]
pub fn ed_stop(ed: Ed, fade: Option<f64>, id: Option<u64>) {
    ed.stop_fading(fade.unwrap_or(0.0), id);
}

#[tauri::command(async)]
pub fn ed_eq(ed: Ed, bands: Vec<Value>) {
    ed.set_eq(&bands);
}

/// The EQ curve: dB per band at each frequency, drawn by the page.
#[tauri::command(async)]
pub fn eq_response(audio: Audio, bands: Vec<Value>, freqs: Vec<f64>) -> Vec<Vec<f32>> {
    doc::eq_response(&bands, audio.rate() as f64, &freqs)
}

/// An effect's transfer curve, drawn by its panel: the output level in dB for `n` input levels
/// from `from` to `to` dB, from the same formulas the effect runs.
#[tauri::command(async)]
pub fn fx_curve(tab: String, p: Value, from: f64, to: f64, n: usize) -> Vec<f64> {
    let n = n.clamp(2, 4096);
    let x = |k: usize| from + (to - from) * k as f64 / (n - 1) as f64;
    match tab.as_str() {
        "comp" => {
            let c = dsp::CompParams::from_json(&p);
            let up = if p["autoMakeup"].as_bool().unwrap_or(true) { dsp::auto_makeup(&c) } else { p["makeup"].as_f64().unwrap_or(0.0) };
            (0..n).map(|k| dsp::comp_curve(x(k), &c) + up).collect()
        }
        "sat" => {
            let c = dsp::SatParams::from_json(&p);
            (0..n).map(|k| dsp::sat_curve(x(k), c.drive, c.ceiling)).collect()
        }
        "max" => {
            let (gain, ceiling) = (p["gain"].as_f64().unwrap_or(0.0), p["ceiling"].as_f64().unwrap_or(-1.0));
            (0..n).map(|k| (x(k) + gain).min(ceiling)).collect()
        }
        _ => (0..n).map(x).collect(),
    }
}

/// [a, b) of the editor's sound saved as a new file; `p` gives rate, bits and mono.
#[tauri::command(async)]
pub fn ed_save(lib: Lib, ed: Ed, p: Value, meta: Value) -> Res<Value> {
    let wav = ed.render(&p)?;
    Ok(json!({"path": save_wav(&lib, &wav, &meta)?}))
}

/// The editor's sound written wherever the user picks in a save dialog.
#[tauri::command]
pub async fn ed_download(app: tauri::AppHandle, ed: Ed<'_>, p: Value, name: String) -> Res<Option<String>> {
    let wav = ed.render(&p)?;
    let Some(path) = app.dialog().file().set_file_name(&name).add_filter("WAV", &["wav"]).blocking_save_file().and_then(|p| p.into_path().ok()) else { return Ok(None) };
    std::fs::write(&path, wav).map_err(|e| format!("Save failed: {e}"))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}
