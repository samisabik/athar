//! Athar as one native process: the library and its indexing, the audio engine and the editor
//! in Rust, the interface in a webview. The page is loaded through the `athar://` protocol
//! (protocol.rs) and asks for everything else by command (commands/).

mod arr;
mod commands;
mod discover;
mod doc;
mod engine;
mod events;
mod export;
#[cfg(target_os = "macos")]
mod finder_drop;
mod input;
mod library;
mod mapnames;
mod protocol;
mod sounds;
mod sp404;
#[cfg(test)]
mod testkit;
mod tsne;
mod window;

use commands::{arranger, audio, discover as found, editor, library as lib, projects, recorder, save};
use std::sync::Arc;
use tauri::{Manager, RunEvent};

fn main() {
    let engine = engine::Engine::start();
    let sounds = Arc::new(sounds::Sounds::default());
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(window::Quit::default())
        .manage(engine.clone())
        .manage(sounds.clone())
        .manage(doc::Doc::new(engine.clone()))
        .manage(arr::Arr::new(engine, sounds))
        .manage(input::Input::default())
        .manage(events::Watch::default())
        .register_asynchronous_uri_scheme_protocol("athar", |ctx, req, responder| {
            let ui = ctx.app_handle().state::<protocol::Ui>().0.clone();
            std::thread::spawn(move || responder.respond(protocol::serve(&ui, &req)));
        })
        .menu(window::menu)
        .on_menu_event(window::on_menu_event)
        .invoke_handler(tauri::generate_handler![
            lib::status, lib::get_settings, lib::set_settings, lib::folders, lib::add_folder, lib::remove_folder, lib::subfolders, lib::rescan,
            lib::categories, lib::search, lib::sound_map, lib::map_names, lib::tags, lib::catalog, lib::trash, lib::rename,
            lib::reveal, lib::reveal_folder, lib::reveal_data, lib::backup, lib::restore, lib::pick_folder, lib::pick_file,
            projects::projects, projects::create_project, projects::rename_project, projects::delete_project, projects::project_item,
            projects::project_folder, projects::set_project_order, projects::project_pads, projects::set_project_pads,
            projects::export_project, projects::sp404_card, projects::arrangement,
            audio::audio_info, audio::audio_poll, audio::audio_preview, audio::audio_start, audio::audio_stop, audio::audio_volume,
            audio::audio_watch_spectrum, audio::audio_reset,
            editor::ed_open, editor::ed_saved, editor::ed_regions, editor::ed_edit, editor::ed_undo, editor::ed_view, editor::ed_noise,
            editor::ed_analyse, editor::ed_peaks, editor::ed_spec, editor::ed_play, editor::ed_stop, editor::ed_eq, editor::eq_response,
            editor::fx_curve, editor::ed_save, editor::ed_download,
            arranger::arr_load, arranger::arr_keep, arranger::arr_play, arranger::arr_stop, arranger::arr_set, arranger::arr_save,
            recorder::rec_devices, recorder::rec_start, recorder::rec_stop, recorder::rec_discard, recorder::rec_save,
            found::discover_sources, found::discover_search, found::discover_waveform, found::discover_files, found::discover_preview,
            found::discover_save,
            save::batch_save,
            window::set_dirty, window::quit_now, window::debug_log,
        ])
        .setup(|app| {
            let resources = app.path().resource_dir().ok();
            app.manage(protocol::Ui(protocol::ui_dir(resources.as_deref())));
            let lib = library::Library::open(library::model_dir(resources.as_deref()))?;
            app.state::<doc::Doc>().attach(lib.clone());
            app.manage(lib);
            events::start(app.handle().clone());
            std::thread::spawn(found::clear_old_previews);
            window::open(app.handle())
        })
        .build(tauri::generate_context!())
        .expect("error while running Athar")
        .run(|_, event| {
            if let RunEvent::Exit = event {
                found::remove_preview_dir();
            }
        });
}
