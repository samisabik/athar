//! The app's one window: its menu, what closing and quitting do while the page holds unsaved
//! work, folders dropped from Finder, and the test script a driven window runs.

#[cfg(target_os = "macos")]
use crate::finder_drop;
use crate::library::Library;
use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::Arc;
use std::time::Duration;
use tauri::menu::{Menu, MenuEvent, MenuItem};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, WindowEvent, Wry};

const QUIT: &str = "quit";
/// How long after the window opens a test script starts, so the page has loaded its library.
const DRIVE_DELAY: Duration = Duration::from_secs(4);

/// Unsaved edits in the page hold a quit until the page has asked about them.
#[derive(Default)]
pub struct Quit {
    dirty: AtomicBool,
    quitting: AtomicBool,
}

#[tauri::command(async)]
pub fn set_dirty(q: State<Quit>, dirty: bool) {
    q.dirty.store(dirty, SeqCst);
}

#[tauri::command(async)]
pub fn quit_now(app: AppHandle, q: State<Quit>) {
    q.quitting.store(true, SeqCst);
    app.exit(0);
}

/// Whether a close or a quit has to wait: the page holds unsaved work and has not agreed yet.
/// The page is then asked, and calls `quit_now` to go on.
fn asks_first(app: &AppHandle) -> bool {
    let q = app.state::<Quit>();
    let Some(win) = app.get_webview_window("main").filter(|_| q.dirty.load(SeqCst) && !q.quitting.load(SeqCst)) else { return false };
    let _ = win.eval("window.atharConfirmQuit()");
    true
}

/// The standard menu, with a Quit of our own. The standard Quit ends the process at once,
/// without the window hearing of it, so unsaved work would go unasked.
pub fn menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::default(app)?;
    if let Some(app_menu) = menu.items()?.first().and_then(|m| m.as_submenu()) {
        if let Some(standard_quit) = app_menu.items()?.last() {
            app_menu.remove(standard_quit)?;
        }
        app_menu.append(&MenuItem::with_id(app, QUIT, format!("Quit {}", app.package_info().name), true, Some("CmdOrCtrl+Q"))?)?;
    }
    Ok(menu)
}

pub fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    if event.id() == QUIT && !asks_first(app) {
        app.exit(0);
    }
}

/// Folders dropped from Finder join the library; anything else is said to be left out.
fn dropped(app: &AppHandle, paths: Vec<String>) {
    let lib = app.state::<Arc<Library>>();
    let (mut added, mut problems) = (vec![], vec![]);
    for p in paths {
        let name = Path::new(&p).file_name().map_or_else(|| p.clone(), |n| n.to_string_lossy().into_owned());
        if !Path::new(&p).is_dir() {
            problems.push(format!("{name} is a file: drop folders to add them to the library."));
        } else {
            match lib.add_folder(&p) {
                Ok(_) => added.push(name),
                Err(e) => problems.push(format!("{name}: {e}")),
            }
        }
    }
    let _ = app.emit("dropped", json!({"added": added, "problems": problems}));
}

/// Opens the window on the interface. With ATHAR_DRIVE naming a script, the window is a test
/// window: it says so in its title, so it is not mistaken for the one in use, and runs the
/// script in the page to check the app through its real interface.
pub fn open(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let drive = std::env::var("ATHAR_DRIVE").ok();
    let url = WebviewUrl::CustomProtocol("athar://localhost/".parse()?);
    let mut builder = WebviewWindowBuilder::new(app, "main", url)
        .title(if drive.is_some() { "Athar DEBUG".to_string() } else { format!("Athar {}", env!("CARGO_PKG_VERSION")) })
        .inner_size(1280.0, 820.0)
        .min_inner_size(900.0, 600.0)
        // The page's own drag and drop (pads, the Arranger) only works when the window does
        // not take file drops for itself.
        .disable_drag_drop_handler();
    if drive.is_some() {
        // A test window keeps the page's own start-up errors for the test script to read.
        builder = builder.initialization_script("window.__startErrors = []; addEventListener('error', (e) => __startErrors.push(String(e.message))); addEventListener('unhandledrejection', (e) => __startErrors.push(String(e.reason && e.reason.message || e.reason)));");
    }
    let win = builder.build()?;
    #[cfg(target_os = "macos")]
    {
        let handle = app.clone();
        let _ = win.with_webview(move |wv| finder_drop::install(wv.inner(), move |paths| dropped(&handle, paths)));
    }
    let handle = app.clone();
    win.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            if asks_first(&handle) {
                api.prevent_close();
            }
        }
    });
    if let Some(script) = drive {
        let code = std::fs::read_to_string(&script).map_err(|e| format!("ATHAR_DRIVE names {script}, which cannot be read: {e}"))?;
        std::thread::spawn(move || {
            std::thread::sleep(DRIVE_DELAY);
            let _ = win.eval(format!("(async () => {{ const log = (m) => window.__TAURI_INTERNALS__.invoke('debug_log', {{ msg: typeof m === 'string' ? m : JSON.stringify(m) }}); const wait = (ms) => new Promise(r => setTimeout(r, ms)); try {{ {code} }} catch (e) {{ log('ERROR ' + e + ' ' + e.stack); }} }})()"));
        });
    }
    Ok(())
}

/// Lets a test script running in the page report back (see `open`).
#[tauri::command(async)]
pub fn debug_log(msg: String) {
    eprintln!("[page] {msg}");
}
