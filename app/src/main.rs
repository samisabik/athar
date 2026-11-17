//! Athar as one native process: the library and its indexing in Rust, the interface in a
//! webview. The page is served by the `athar://` protocol, so files, decoded audio and the
//! interface all come from here without a network server. Everything else is a command.

mod audio;
mod arr;
mod discover;
mod doc;
mod engine;
mod events;
#[cfg(target_os = "macos")]
mod finder_drop;
mod input;
mod library;
mod mapnames;
mod sp404;
mod tsne;

use library::{Library, Res, SearchQuery};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::http::{Request, Response, StatusCode};
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;

type Lib<'a> = State<'a, Arc<Library>>;

/// Unsaved edits in the page hold a quit until the page has asked about them.
#[derive(Default)]
struct Quit {
    dirty: std::sync::atomic::AtomicBool,
    quitting: std::sync::atomic::AtomicBool,
}

#[tauri::command(async)]
fn set_dirty(q: State<Quit>, dirty: bool) {
    q.dirty.store(dirty, std::sync::atomic::Ordering::SeqCst);
}

#[tauri::command(async)]
fn quit_now(app: tauri::AppHandle, q: State<Quit>) {
    q.quitting.store(true, std::sync::atomic::Ordering::SeqCst);
    app.exit(0);
}

/// Where the interface files are: in the app bundle's resources, or in the repository when run
/// from source.
struct Ui(PathBuf);

fn ui_dir(resources: Option<&Path>) -> PathBuf {
    // Built into the app, the interface is in its resources. Run from the build folder, it is the
    // repository's own files, not the copy the build leaves next to the binary, which is only
    // refreshed when the app is bundled.
    let in_app = std::env::current_exe().is_ok_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"));
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    if !in_app && repo.join("index.html").exists() {
        return repo;
    }
    resources.map(|r| r.join("ui")).filter(|u| u.join("index.html").exists()).unwrap_or(repo)
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

// ---------- commands

#[tauri::command(async)]
fn status(lib: Lib) -> Value {
    lib.status()
}

#[tauri::command(async)]
fn get_settings(lib: Lib) -> Value {
    let mut st = lib.settings();
    st["_data_dir"] = json!(lib.data_dir);
    st["_version"] = json!(env!("CARGO_PKG_VERSION"));
    st["_preview_max_s"] = json!(audio::PREVIEW_MAX_SECONDS);
    st
}

#[tauri::command(async)]
fn set_settings(lib: Lib, req: Value) -> Res<Value> {
    lib.update_settings(&req)
}

#[tauri::command(async)]
fn folders(lib: Lib) -> Value {
    lib.folder_counts()
}

#[tauri::command(async)]
fn add_folder(lib: Lib, req: Value) -> Res<Value> {
    lib.add_folder(s(&req, "path")).map(|p| json!({"path": p}))
}

#[tauri::command(async)]
fn remove_folder(lib: Lib, req: Value) -> Value {
    lib.remove_folder(s(&req, "path"));
    json!({"ok": true})
}

#[tauri::command(async)]
fn subfolders(lib: Lib, req: Value) -> Value {
    lib.subfolders(s(&req, "path"))
}

#[tauri::command(async)]
fn rescan(lib: Lib) -> Value {
    lib.rescan();
    json!({"ok": true})
}

#[tauri::command(async)]
fn categories(lib: Lib) -> Value {
    lib.category_counts()
}

#[tauri::command]
async fn search(lib: Lib<'_>, req: SearchQuery) -> Res<Value> {
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
fn sound_map(lib: Lib) -> Value {
    lib.inner().sound_map()
}

#[tauri::command(async)]
fn tags(lib: Lib) -> Value {
    lib.all_tags()
}

#[tauri::command(async)]
fn catalog(lib: Lib, req: Value) -> Res<Value> {
    let p = s(&req, "path");
    lib.known_path(p)?;
    lib.catalog_set(p, &req)
}

#[tauri::command(async)]
fn trash(lib: Lib, req: Value) -> Res<Value> {
    lib.trash(s(&req, "path")).map(|_| json!({"ok": true}))
}

#[tauri::command(async)]
fn rename(lib: Lib, req: Value) -> Res<Value> {
    lib.rename(s(&req, "path"), s(&req, "name")).map(|p| json!({"path": p}))
}

fn open(args: &[&str]) {
    let _ = std::process::Command::new("open").args(args).spawn();
}

#[tauri::command(async)]
fn reveal(lib: Lib, req: Value) -> Res<Value> {
    let p = s(&req, "path");
    lib.known_path(p)?;
    open(&["-R", p]);
    Ok(json!({"ok": true}))
}

#[tauri::command(async)]
fn reveal_folder(lib: Lib, req: Value) -> Res<Value> {
    let p = std::fs::canonicalize(s(&req, "path")).map_err(|e| e.to_string())?;
    let ps = p.to_string_lossy().into_owned();
    let inside = lib.folders().iter().any(|f| {
        let r = std::fs::canonicalize(f).map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
        ps == r || library::is_inside(&ps, &r)
    });
    if !p.is_dir() || !inside {
        return Err("That folder is not part of your library".into());
    }
    open(&[&ps]);
    Ok(json!({"ok": true}))
}

#[tauri::command(async)]
fn reveal_data(lib: Lib) -> Value {
    open(&[&lib.data_dir.to_string_lossy()]);
    json!({"ok": true})
}

#[tauri::command(async)]
fn sp404_card() -> Value {
    match audio::sp404_card() {
        Some(c) => json!({"card": c, "import": format!("{c}/IMPORT")}),
        None => json!({"card": null}),
    }
}

#[tauri::command(async)]
fn projects(lib: Lib) -> Value {
    lib.drop_missing_projects();
    lib.adopt_project_folders();
    lib.projects()
}

#[tauri::command(async)]
fn create_project(lib: Lib, req: Value) -> Res<Value> {
    lib.create_project(s(&req, "name"))
}

#[tauri::command(async)]
fn rename_project(lib: Lib, id: i64, req: Value) -> Res<Value> {
    lib.rename_project(id, s(&req, "name")).map(|_| json!({"ok": true}))
}

#[tauri::command(async)]
fn delete_project(lib: Lib, id: i64) -> Value {
    lib.delete_project(id);
    json!({"ok": true})
}

#[tauri::command(async)]
fn project_item(lib: Lib, id: i64, req: Value) -> Res<Value> {
    let p = s(&req, "path");
    if req.get("remove").and_then(|r| r.as_bool()).unwrap_or(false) {
        lib.project_remove(id, p);
    } else {
        lib.known_path(p)?;
        lib.project_add(id, p).map_err(|e| format!("Could not copy the sound into the project folder: {e}"))?;
    }
    Ok(json!({"ok": true}))
}

#[tauri::command(async)]
fn project_folder(lib: Lib, id: i64, req: Value) -> Res<Value> {
    if !lib.project_exists(id) {
        return Err("Project not found".into());
    }
    lib.set_project_folder(id, s(&req, "path")).map(|f| json!({"folder": f}))
}

#[tauri::command(async)]
fn project_order(lib: Lib, id: i64) -> Value {
    lib.project_order(id)
}

#[tauri::command(async)]
fn set_project_order(lib: Lib, id: i64, req: Value) -> Value {
    let paths: Vec<String> = serde_json::from_value(req["paths"].clone()).unwrap_or_default();
    lib.set_project_order(id, &paths);
    json!({"ok": true})
}

#[tauri::command(async)]
fn project_pads(lib: Lib, id: i64) -> Value {
    lib.project_pads(id)
}

#[tauri::command(async)]
fn set_project_pads(lib: Lib, id: i64, req: Value) -> Res<Value> {
    if !lib.project_exists(id) {
        return Err("Project not found".into());
    }
    lib.set_project_pads(id, &req["pads"]).map(|_| json!({"ok": true}))
}

#[tauri::command(async)]
fn arrangement(lib: Lib) -> Value {
    lib.arrangement_get()
}

#[tauri::command(async)]
fn set_arrangement(lib: Lib, req: Value) -> Res<Value> {
    lib.arrangement_set(&req).map_err(|e| format!("Bad arrangement: {e}"))
}

// ---------- the audio engine

type Audio<'a> = State<'a, Arc<engine::Engine>>;

#[tauri::command(async)]
fn audio_info(audio: Audio) -> Value {
    json!({"rate": audio.rate(), "now": audio.now(), "error": audio.error()})
}

#[tauri::command(async)]
fn audio_poll(audio: Audio) -> Value {
    json!({"rate": audio.rate(), "now": audio.now(), "ended": audio.take_ended(), "level": audio.level()})
}

#[derive(serde::Deserialize)]
struct StartReq {
    id: u64,
    sound: String,
    when: f64,
    offset: f64,
    duration: Option<f64>,
    looped: Option<(f64, f64)>,
    chain: Vec<engine::Link>,
}

/// A file made ready for the preview player: its key in the engine and its whole length.
#[tauri::command(async)]
fn audio_preview(lib: Lib, audio: Audio, path: String) -> Res<Value> {
    lib.known_path(&path)?;
    let (key, duration) = audio.preview_of_path(&path, audio::PREVIEW_MAX_SECONDS)?;
    let s = audio.sound(&key)?;
    let length = s.frames() as f64 / s.sr as f64;
    Ok(json!({"key": key, "length": length, "capped": duration > length + 0.01}))
}

#[tauri::command(async)]
fn audio_start(lib: Lib, audio: Audio, req: StartReq) -> Res<()> {
    if let Some(p) = req.sound.strip_prefix("path:") {
        lib.known_path(p)?;
    }
    let s = audio.sound(&req.sound)?;
    // A start time of 0 means now; a later one is a time on the engine clock.
    let when = if req.when <= 0.0 { audio.now() } else { req.when };
    audio.play(req.id, &s, when, req.offset, req.duration, req.looped, req.chain);
    Ok(())
}

#[tauri::command(async)]
fn audio_stop(audio: Audio, id: u64, when: f64) {
    audio.stop(id, when);
}

type Ed<'a> = State<'a, doc::Doc>;

/// Opens a library file, or a sound the engine holds (`key`, a recorded take), in the editor.
#[tauri::command(async)]
fn ed_open(lib: Lib, audio: Audio, ed: Ed, path: Option<String>, key: Option<String>) -> Res<Value> {
    let (sound, from) = match (path, key) {
        (Some(p), _) => {
            lib.known_path(&p)?;
            (audio.sound_of_path(&p)?, Some(p))
        }
        (None, Some(k)) => (audio.sound(&k)?, None),
        _ => return Err("Nothing to open".into()),
    };
    Ok(ed.open(sound, from))
}

/// The editor's sound was saved as `path` at `version`.
#[tauri::command(async)]
fn ed_saved(ed: Ed, path: String, version: u64) -> Value {
    ed.saved(path, version)
}

#[tauri::command(async)]
fn ed_regions(ed: Ed, p: Value, ui: doc::Ui) -> Res<Value> {
    ed.regions(&p, ui)
}

#[tauri::command(async)]
fn ed_edit(ed: Ed, p: Value, ui: Option<doc::Ui>) -> Res<Value> {
    let mut p = p;
    if let Some(ui) = ui {
        p["ui"] = json!(ui);
    }
    ed.edit(&p)
}

#[tauri::command(async)]
fn ed_undo(ed: Ed, redo: bool, ui: doc::Ui) -> Res<Value> {
    ed.undo(redo, ui)
}

#[tauri::command(async)]
fn ed_view(ed: Ed, key: String, p: Value, head_s: Option<f64>) -> Res<Value> {
    ed.view(&key, &p, head_s)
}

#[tauri::command(async)]
fn ed_noise(ed: Ed, range: Option<(usize, usize)>) -> Res<()> {
    match range {
        Some((a, b)) => ed.learn_noise(a, b),
        None => {
            ed.forget_noise();
            Ok(())
        }
    }
}

#[tauri::command(async)]
fn ed_analyse(ed: Ed, p: Value) -> Res<Value> {
    ed.analyse(&p)
}

#[tauri::command(async)]
fn ed_peaks(ed: Ed, src: Option<String>, start: f64, spp: f64, width: usize) -> Res<tauri::ipc::Response> {
    ed.peaks(src.as_deref(), start, spp, width).map(tauri::ipc::Response::new)
}

#[tauri::command(async)]
fn ed_spec(ed: Ed, src: Option<String>, start: f64, spp: f64, pw: usize, rows: usize) -> Res<tauri::ipc::Response> {
    ed.spectrogram(src.as_deref(), start, spp, pw, rows).map(tauri::ipc::Response::new)
}

#[tauri::command(async)]
fn ed_play(ed: Ed, p: Value) -> Res<Value> {
    ed.play(&p)
}

#[tauri::command(async)]
fn ed_stop(ed: Ed, fade: Option<f64>, id: Option<u64>) {
    ed.stop_fading(fade.unwrap_or(0.0), id);
}

#[tauri::command(async)]
fn ed_eq(ed: Ed, bands: Vec<Value>) {
    ed.set_eq(&bands);
}

/// The names written on the Map at zoom `level` (see mapnames.rs).
#[tauri::command(async)]
fn map_names(lib: Lib, level: u32) -> Value {
    mapnames::names(&lib.inner().sound_map(), level)
}

/// An effect's transfer curve, drawn by its panel: the output level in dB for `n` input levels
/// from `from` to `to` dB, from the same formulas the effect runs.
#[tauri::command(async)]
fn fx_curve(tab: String, p: Value, from: f64, to: f64, n: usize) -> Vec<f64> {
    use athar_core::dsp;
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

/// The EQ curve: dB per band at each frequency, drawn by the page.
#[tauri::command(async)]
fn eq_response(audio: Audio, bands: Vec<Value>, freqs: Vec<f64>) -> Vec<Vec<f32>> {
    doc::eq_response(&bands, audio.rate() as f64, &freqs)
}

type ArrS<'a> = State<'a, arr::Arr>;

#[tauri::command(async)]
fn arr_load(lib: Lib, arr: ArrS, path: String) -> Res<Value> {
    lib.known_path(&path)?;
    arr.load(&path)
}

#[tauri::command(async)]
fn arr_keep(arr: ArrS, paths: Vec<String>) {
    arr.keep_only(&paths);
}

#[tauri::command(async)]
fn arr_play(arr: ArrS, from: f64) -> f64 {
    arr.play(from)
}

/// The arrangement changed: what plays follows, and with `store` it is kept for next time.
#[tauri::command(async)]
fn arr_set(lib: Lib, arr: ArrS, a: Value, store: bool) -> Res<Value> {
    let shown = arr.set(a.clone());
    if store {
        lib.arrangement_set(&a).map_err(|e| format!("Bad arrangement: {e}"))?;
    }
    Ok(shown)
}

/// The lanes that are heard mixed and saved as a new file. Returns its path and how many dB
/// over full scale the mix was before it was turned down to fit (0 when it fit).
#[tauri::command(async)]
fn arr_save(lib: Lib, arr: ArrS, p: Value, meta: Value) -> Res<Value> {
    let heard = arr.heard();
    if heard.is_empty() {
        return Err("No lane is sounding, so there is nothing to save.".into());
    }
    let sr = heard.iter().map(|h| h.1.sr).max().unwrap_or(48000);
    let seconds = heard.iter().map(|h| h.0.at + h.1.frames() as f64 / h.1.sr as f64).fold(0.0, f64::max);
    let lanes: Vec<_> = heard
        .iter()
        .map(|(c, s, g)| athar_core::arrange::MixLane {
            chs: s.channels.iter().take(2).cloned().collect(),
            sr: s.sr,
            gain: *g as f64,
            hits: vec![c.at],
            fades: vec![Some(athar_core::arrange::Fade { fade_in: c.fade_in, fade_out: c.fade_out })],
        })
        .collect();
    let args = athar_core::arrange::MixArgs {
        sr,
        seconds,
        rate: p["rate"].as_u64().filter(|r| *r > 0).map_or(sr, |r| r as u32),
        bits: p["bits"].as_u64().unwrap_or(24) as u32,
        mono: p["mono"].as_bool().unwrap_or(false),
        fade_end: false,
        dither_seed: None,
    };
    let mix = athar_core::arrange::render_mix(&lanes, &args)?;
    Ok(json!({"path": save_wav(&lib, &mix.wav, &meta)?, "over": mix.over}))
}

#[tauri::command(async)]
fn arr_stop(arr: ArrS) {
    arr.stop();
}

/// Whether the page wants the spectrum of what plays (the EQ panel is open).
#[tauri::command(async)]
fn audio_watch_spectrum(watch: State<events::Watch>, on: bool) {
    watch.spectrum.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The page has just loaded, after a start or a reload: whatever a page before it left playing
/// or recording stops, since nothing could stop it any more.
#[tauri::command(async)]
fn audio_reset(audio: Audio, ed: Ed, arr: ArrS, rec: State<input::Input>) {
    ed.stop();
    arr.stop();
    rec.stop();
    audio.stop_all();
}

#[tauri::command(async)]
fn audio_volume(audio: Audio, v: f32) {
    audio.set_volume(v);
}

#[tauri::command(async)]
fn rec_devices() -> Vec<String> {
    input::devices()
}

#[tauri::command(async)]
fn rec_start(rec: State<input::Input>, device: String) -> Res<Value> {
    let (sr, channels) = rec.start(&device)?;
    Ok(json!({"sr": sr, "channels": channels}))
}

/// Ends the take and keeps it in the engine; returns its key and length.
#[tauri::command(async)]
fn rec_stop(rec: State<input::Input>, audio: Audio) -> Value {
    let (channels, sr) = rec.finish();
    let frames = channels.first().map_or(0, Vec::len);
    if frames == 0 {
        return json!({"key": null, "frames": 0, "sr": sr});
    }
    let n = channels.len();
    json!({"key": audio.hold(engine::Sound::new(channels, sr)), "frames": frames, "sr": sr, "channels": n})
}

#[tauri::command(async)]
fn rec_discard(audio: Audio, key: String) {
    audio.release(&key);
}

/// Lets a test script running in the page report back (see ATHAR_DRIVE).
#[tauri::command(async)]
fn debug_log(msg: String) {
    eprintln!("[page] {msg}");
}

#[tauri::command(async)]
fn export_project(lib: Lib, id: i64, req: Value) -> Res<Value> {
    if !lib.project_exists(id) {
        return Err("Project not found".into());
    }
    let pattern = if s(&req, "pattern").is_empty() { "{project}_{name}" } else { s(&req, "pattern") };
    let format = if s(&req, "format").is_empty() { "copy" } else { s(&req, "format") };
    lib.export_project(id, s(&req, "dest"), pattern, format)
}

#[tauri::command(async)]
fn backup(lib: Lib, req: Value) -> Res<Value> {
    lib.export_backup(s(&req, "path")).map(|p| json!({"path": p}))
}

#[tauri::command(async)]
fn restore(lib: Lib, req: Value) -> Res<Value> {
    lib.restore_backup(s(&req, "path"))
}

#[tauri::command(async)]
fn discover_sources(lib: Lib) -> Value {
    discover::sources(lib.settings()["freesound_key"].as_str().unwrap_or(""))
}

#[tauri::command(async)]
fn discover_search(lib: Lib, req: Value) -> Res<Value> {
    let key = lib.settings()["freesound_key"].as_str().unwrap_or("").to_string();
    discover::search(s(&req, "source"), s(&req, "query"), req["page"].as_u64().unwrap_or(1), &key)
}

#[tauri::command(async)]
fn discover_waveform(req: Value) -> Value {
    discover::waveform(s(&req, "id"), 48)
}

#[tauri::command(async)]
fn discover_files(req: Value) -> Res<Value> {
    discover::files(s(&req, "source"), s(&req, "id"))
}

/// How much of a found sound a preview fetches: about PREVIEW_FETCH_SECONDS of it when its
/// length is known, and at most PREVIEW_BYTES. Adding it to a project fetches all of it.
const PREVIEW_BYTES: u64 = 3 * 1024 * 1024;
const PREVIEW_FETCH_SECONDS: f64 = 20.0;
/// A preview plays at most this many seconds, which also caps what it holds in memory.
const PREVIEW_SECONDS: usize = 120;

/// Where previews are fetched to. Each file is deleted as soon as it is decoded, so nothing a
/// preview fetches stays on disk.
fn preview_dir() -> PathBuf {
    std::env::temp_dir().join(format!("athar-preview-{}", std::process::id()))
}

/// Folders left by earlier runs that quit before they cleaned up.
fn clear_old_previews() {
    let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) else { return };
    let mine = preview_dir();
    for e in rd.flatten() {
        if e.file_name().to_string_lossy().starts_with("athar-preview-") && e.path() != mine {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// A found sound fetched from its archive and decoded for the engine, to preview it: the start of
/// a long file, or all of a short one. Progress goes out as `discover-progress` events.
#[tauri::command(async)]
fn discover_preview(app: tauri::AppHandle, audio: Audio, url: String, duration: Option<f64>) -> Res<Value> {
    let tell = |got: u64, total: u64| {
        let _ = app.emit("discover-progress", json!({"url": url, "got": got, "total": total}));
    };
    let dir = preview_dir();
    let fetch = |limit: Option<discover::Limit>| -> Res<(athar_core::decode::Decoded, bool)> {
        let (path, partial) = discover::download_part(&url, &dir, "preview", limit, &tell)?;
        let d = athar_core::decode::decode(&path);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
        Ok((d.map_err(|_| "That sound could not be played from the archive".to_string())?, partial))
    };
    // Some formats cannot be read from their start alone (an MP4 can keep its index at the end);
    // those are fetched whole.
    let part = |total: u64| match duration.filter(|d| *d > 0.0) {
        Some(d) if total > 0 => ((total as f64 * PREVIEW_FETCH_SECONDS / d) as u64).clamp(256 * 1024, PREVIEW_BYTES),
        _ => PREVIEW_BYTES,
    };
    let (mut d, partial) = match fetch(Some(&part)) {
        Ok((d, partial)) if d.frames() > 0 => (d, partial),
        Ok((_, true)) | Err(_) => fetch(None)?,
        Ok(r) => r,
    };
    let cap = PREVIEW_SECONDS * d.sr as usize;
    let partial = partial || d.frames() > cap;
    d.channels.iter_mut().for_each(|c| c.truncate(cap));
    let duration = d.duration();
    Ok(json!({"key": audio.upload(engine::Sound::new(d.channels, d.sr)), "duration": duration, "partial": partial}))
}

/// Download a found sound into a project's folder and add it to the project, with where it
/// came from and its licence written into its note.
#[tauri::command(async)]
fn discover_save(app: tauri::AppHandle, lib: Lib, req: Value) -> Res<Value> {
    let pid = req["project"].as_i64().unwrap_or(0);
    let dest = lib.project_folder_ready(pid)?.ok_or("That project has no folder yet. Choose one on the Projects page")?;
    let url = s(&req, "url").to_string();
    let tell = |got: u64, total: u64| {
        let _ = app.emit("discover-progress", json!({"url": url, "got": got, "total": total}));
    };
    let path = discover::download(&url, Path::new(&dest), s(&req, "title"), &tell)?;
    let p = path.to_string_lossy().into_owned();
    let note: String = s(&req, "note").chars().take(2000).collect();
    let mut tags = vec!["discovered".to_string()];
    if !s(&req, "source").is_empty() {
        tags.push(s(&req, "source").to_string());
    }
    lib.catalog_set(&p, &json!({"memo": note, "tags": tags}))?;
    lib.index_file(&p);
    lib.project_add(pid, &p)?;
    Ok(json!({"path": p}))
}

#[tauri::command(async)]
fn not_yet(what: String) -> Res<Value> {
    Err(format!("{what} is not in this build yet"))
}

/// Writes a rendered WAV as a new file. `meta` says where and how: { dest_dir, name, format,
/// source, project, sp404 }. Returns the new file's path.
fn save_wav(lib: &Library, data: &[u8], meta: &Value) -> Res<String> {
    let project = meta["project"].as_i64().unwrap_or(0);
    let sp404 = meta["sp404"].as_bool().unwrap_or(false);
    let format = if s(meta, "format").is_empty() { "wav" } else { s(meta, "format") };
    if !["wav", "flac", "mp3"].contains(&format) || (sp404 && format != "wav") {
        return Err(format!("Cannot save as {format}"));
    }
    let mut dir = lib.save_target_dir(project, s(meta, "dest_dir"))?;
    if sp404 {
        dir = audio::sp404_dir(&dir);
    }
    let name = s(meta, "name");
    let stem = Path::new(name).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = if sp404 { library::ascii_name(&stem) } else { library::safe_name(&stem) };
    let target = library::unique_path(&dir.join(format!("{stem}.{format}")))?;
    if sp404 {
        let mono = lib.settings()["sp404_mono"].as_bool().unwrap_or(false);
        audio::write_sp404(data, &target, mono)?;
    } else if format != "wav" {
        std::fs::write(&target, audio::transcode(data, format)?).map_err(|e| format!("Save failed: {e}"))?;
    } else {
        std::fs::write(&target, data).map_err(|e| format!("Save failed: {e}"))?;
    }
    let t = target.to_string_lossy().into_owned();
    lib.after_save(&t, s(meta, "source"), project)?;
    Ok(t)
}

/// [a, b) of the editor's sound saved as a new file; `p` gives rate, bits and mono.
#[tauri::command(async)]
fn ed_save(lib: Lib, ed: Ed, p: Value, meta: Value) -> Res<Value> {
    let wav = ed.render(&p)?;
    Ok(json!({"path": save_wav(&lib, &wav, &meta)?}))
}

/// The editor's sound written wherever the user picks in a save dialog.
#[tauri::command]
async fn ed_download(app: tauri::AppHandle, ed: Ed<'_>, p: Value, name: String) -> Res<Option<String>> {
    let wav = ed.render(&p)?;
    let Some(path) = app.dialog().file().set_file_name(&name).add_filter("WAV", &["wav"]).blocking_save_file().and_then(|p| p.into_path().ok()) else { return Ok(None) };
    std::fs::write(&path, wav).map_err(|e| format!("Save failed: {e}"))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

/// A recorded take saved as a new file.
#[tauri::command(async)]
fn rec_save(lib: Lib, audio: Audio, key: String, meta: Value) -> Res<Value> {
    let s = audio.sound(&key)?;
    let out = athar_core::dsp::run("render", s.channels.clone(), s.sr, &json!({"rate": s.sr, "bits": 24}))?;
    Ok(json!({"path": save_wav(&lib, &out.bytes.ok_or("Nothing was rendered")?, &meta)?}))
}

/// A library file rendered (optionally prepared for a sampler) and saved as a new file, for a
/// batch export.
#[tauri::command(async)]
fn batch_save(lib: Lib, path: String, p: Value, meta: Value) -> Res<Value> {
    lib.known_path(&path)?;
    let d = athar_core::decode::decode(Path::new(&path)).map_err(|e| e.to_string())?;
    let mut q = p;
    if q["rate"].as_u64().unwrap_or(0) == 0 {
        q["rate"] = json!(d.sr);
    }
    let out = athar_core::dsp::run("render", d.channels, d.sr, &q)?;
    Ok(json!({"path": save_wav(&lib, &out.bytes.ok_or("Nothing was rendered")?, &meta)?}))
}

#[tauri::command]
async fn pick_folder(app: tauri::AppHandle, start: Option<String>) -> Option<String> {
    let mut d = app.dialog().file();
    if let Some(s) = start.filter(|s| !s.is_empty()) {
        d = d.set_directory(s);
    }
    d.blocking_pick_folder().and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
async fn pick_file(app: tauri::AppHandle) -> Option<String> {
    app.dialog().file().blocking_pick_file().and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned())
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------- the athar:// protocol: the interface, sound files and decoded audio

fn respond(status: StatusCode, mime: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Cache-Control", "no-store")
        .header("Access-Control-Allow-Origin", "*")
        .body(body)
        .unwrap()
}

fn mime_of(path: &str) -> &'static str {
    match Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("woff2") => "font/woff2",
        Some("json") => "application/json",
        Some("wav") => "audio/wav",
        Some("aif") | Some("aiff") => "audio/aiff",
        Some("flac") => "audio/flac",
        Some("mp3") => "audio/mpeg",
        Some("ogg") => "audio/ogg",
        Some("m4a") | Some("aac") => "audio/mp4",
        _ => "application/octet-stream",
    }
}

fn serve(ui: &Path, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let path = req.uri().path().to_string();
    let path = percent_decode(&path);
    if path == "/" || path == "/index.html" {
        return match std::fs::read(ui.join("index.html")) {
            Ok(b) => respond(StatusCode::OK, "text/html; charset=utf-8", b),
            Err(e) => respond(StatusCode::INTERNAL_SERVER_ERROR, "text/plain", e.to_string().into_bytes()),
        };
    }
    if let Some(rel) = path.strip_prefix("/static/") {
        if rel.split('/').any(|c| c == ".." || c.is_empty()) {
            return respond(StatusCode::FORBIDDEN, "text/plain", b"Forbidden".to_vec());
        }
        let p = ui.join(rel);
        return match std::fs::read(&p) {
            Ok(b) => respond(StatusCode::OK, mime_of(rel), b),
            Err(_) => respond(StatusCode::NOT_FOUND, "text/plain", b"Not found".to_vec()),
        };
    }
    respond(StatusCode::NOT_FOUND, "text/plain", b"Not found".to_vec())
}

fn main() {
    let engine = engine::Engine::start();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Quit::default())
        .manage(engine.clone())
        .manage(doc::Doc::new(engine.clone()))
        .manage(arr::Arr::new(engine))
        .manage(input::Input::default())
        .manage(events::Watch::default())
        .register_asynchronous_uri_scheme_protocol("athar", |ctx, req, responder| {
            let app = ctx.app_handle();
            let ui = app.state::<Ui>().0.clone();
            std::thread::spawn(move || responder.respond(serve(&ui, &req)));
        })
        .invoke_handler(tauri::generate_handler![
            status, get_settings, set_settings, folders, add_folder, remove_folder, subfolders, rescan, categories, search,
            sound_map, tags, catalog, trash, rename, reveal, reveal_folder, reveal_data, sp404_card, projects, create_project,
            rename_project, delete_project, project_item, project_folder, project_order, set_project_order, project_pads,
            set_project_pads, arrangement, set_arrangement, discover_sources, not_yet,
            pick_folder, pick_file, debug_log, set_dirty, quit_now, export_project, backup, restore, discover_search,
            discover_waveform, discover_files, discover_save, discover_preview, audio_info, audio_poll, audio_preview, audio_start, rec_devices, rec_start, rec_stop, rec_discard, ed_open, ed_saved, ed_regions, ed_edit, ed_undo, ed_view, ed_noise, ed_analyse, ed_peaks, ed_spec, ed_play, ed_stop, ed_eq, eq_response, fx_curve, map_names, audio_volume, audio_reset, audio_watch_spectrum, ed_save, ed_download, rec_save, batch_save, arr_load, arr_keep, arr_play, arr_set, arr_stop, arr_save,
            audio_stop
        ])
        .setup(|app| {
            let resources = app.path().resource_dir().ok();
            app.manage(Ui(ui_dir(resources.as_deref())));
            let lib = Library::open(library::model_dir(resources.as_deref()))?;
            app.state::<doc::Doc>().attach(lib.clone());
            app.manage(lib);
            events::start(app.handle().clone());
            std::thread::spawn(clear_old_previews);
            let url = WebviewUrl::CustomProtocol("athar://localhost/".parse().unwrap());
            let mut builder = WebviewWindowBuilder::new(app, "main", url);
            if std::env::var("ATHAR_DRIVE").is_ok() {
                // A test window keeps the page's own start-up errors for the test script to read.
                builder = builder.initialization_script("window.__startErrors = []; addEventListener('error', (e) => __startErrors.push(String(e.message))); addEventListener('unhandledrejection', (e) => __startErrors.push(String(e.reason && e.reason.message || e.reason)));");
            }
            builder
                // A window driven by a test script says so, so it is not mistaken for the one in use.
                .title(if std::env::var("ATHAR_DRIVE").is_ok() { "Athar DEBUG".to_string() } else { format!("Athar {}", env!("CARGO_PKG_VERSION")) })
                .inner_size(1280.0, 820.0)
                .min_inner_size(900.0, 600.0)
                // The page's own drag and drop (pads, the Arranger) only works when the window does
                // not take file drops for itself.
                .disable_drag_drop_handler()
                .build()?;
            let win = app.get_webview_window("main").unwrap();
            #[cfg(target_os = "macos")]
            {
                let handle = app.handle().clone();
                let _ = win.with_webview(move |wv| {
                    finder_drop::install(wv.inner(), move |paths| {
                        // Folders join the library; anything else is said to be left out.
                        let lib = handle.state::<Arc<Library>>().inner().clone();
                        let (mut added, mut problems) = (vec![], vec![]);
                        for p in paths {
                            let name = Path::new(&p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone());
                            if !Path::new(&p).is_dir() {
                                problems.push(format!("{name} is a file: drop folders to add them to the library."));
                            } else {
                                match lib.add_folder(&p) {
                                    Ok(_) => added.push(name),
                                    Err(e) => problems.push(format!("{name}: {e}")),
                                }
                            }
                        }
                        let _ = handle.emit("dropped", json!({"added": added, "problems": problems}));
                    });
                });
            }
            let handle = app.handle().clone();
            win.clone().on_window_event(move |event| match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    let q = handle.state::<Quit>();
                    use std::sync::atomic::Ordering::SeqCst;
                    if q.dirty.load(SeqCst) && !q.quitting.load(SeqCst) {
                        api.prevent_close();
                        let _ = win.eval("window.atharConfirmQuit()");
                    }
                }
                _ => {}
            });
            // ATHAR_DRIVE names a script run in the page once it has loaded, to check the app
            // through its real interface.
            if let Ok(script) = std::env::var("ATHAR_DRIVE") {
                let code = std::fs::read_to_string(&script).expect("ATHAR_DRIVE script");
                let win = app.get_webview_window("main").unwrap();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(4));
                    let _ = win.eval(&format!("(async () => {{ const log = (m) => window.__TAURI_INTERNALS__.invoke('debug_log', {{ msg: typeof m === 'string' ? m : JSON.stringify(m) }}); const wait = (ms) => new Promise(r => setTimeout(r, ms)); try {{ {code} }} catch (e) {{ log('ERROR ' + e + ' ' + e.stack); }} }})()"));
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Athar");
}
