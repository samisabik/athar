//! The Arranger: its clips' sounds, playback (see arr.rs) and the mixdown.

use super::projects::store_arrangement;
use super::save::save_wav;
use super::{Arr, Lib};
use crate::library::Res;
use athar_core::arrange::{render_mix, Fade, MixArgs, MixLane};
use serde_json::{json, Value};

#[tauri::command(async)]
pub fn arr_load(lib: Lib, arr: Arr, path: String) -> Res<Value> {
    lib.known_path(&path)?;
    arr.load(&path)
}

#[tauri::command(async)]
pub fn arr_keep(arr: Arr, paths: Vec<String>) {
    arr.keep_only(&paths);
}

#[tauri::command(async)]
pub fn arr_play(arr: Arr, from: f64) -> f64 {
    arr.play(from)
}

#[tauri::command(async)]
pub fn arr_stop(arr: Arr) {
    arr.stop();
}

/// The arrangement changed: what plays follows, and with `store` it is kept for next time.
#[tauri::command(async)]
pub fn arr_set(lib: Lib, arr: Arr, a: Value, store: bool) -> Res<Value> {
    let shown = arr.set(a.clone());
    if store {
        store_arrangement(&lib, &a)?;
    }
    Ok(shown)
}

/// The lanes that are heard mixed and saved as a new file. Returns its path and how many dB
/// over full scale the mix was before it was turned down to fit (0 when it fit).
#[tauri::command(async)]
pub fn arr_save(lib: Lib, arr: Arr, p: Value, meta: Value) -> Res<Value> {
    let heard = arr.heard();
    if heard.is_empty() {
        return Err("No lane is sounding, so there is nothing to save.".into());
    }
    let sr = heard.iter().map(|h| h.1.sr).max().unwrap_or(48000);
    let seconds = heard.iter().map(|h| h.0.at + h.1.seconds()).fold(0.0, f64::max);
    let lanes: Vec<_> = heard
        .iter()
        .map(|(c, s, g)| MixLane { chs: s.channels.iter().take(2).cloned().collect(), sr: s.sr, gain: *g as f64, at: c.at, fade: Fade { fade_in: c.fade_in, fade_out: c.fade_out } })
        .collect();
    let args = MixArgs {
        sr,
        seconds,
        rate: p["rate"].as_u64().filter(|r| *r > 0).map_or(sr, |r| r as u32),
        bits: p["bits"].as_u64().unwrap_or(24) as u32,
        mono: p["mono"].as_bool().unwrap_or(false),
    };
    let mix = render_mix(&lanes, &args)?;
    Ok(json!({"path": save_wav(&lib, &mix.wav, &meta)?, "over": mix.over}))
}
