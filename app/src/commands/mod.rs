//! The commands the page calls, by subject. Each module's functions are registered in main.rs.

pub mod arranger;
pub mod audio;
pub mod discover;
pub mod editor;
pub mod library;
pub mod projects;
pub mod recorder;
pub mod save;

use serde_json::{json, Value};
use std::sync::Arc;
use tauri::State;

type Lib<'a> = State<'a, Arc<crate::library::Library>>;
type Audio<'a> = State<'a, Arc<crate::engine::Engine>>;
type Files<'a> = State<'a, Arc<crate::sounds::Sounds>>;
type Ed<'a> = State<'a, crate::doc::Doc>;
type Arr<'a> = State<'a, crate::arr::Arr>;
type Rec<'a> = State<'a, crate::input::Input>;

/// A text field of a request; empty when it is not there.
fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// What a command answers when it has nothing to tell.
fn done() -> Value {
    json!({"ok": true})
}

/// Tells the page how far a download from `url` has got, as `discover-progress` events.
fn progress<'a>(app: &'a tauri::AppHandle, url: &'a str) -> impl Fn(u64, u64) + 'a {
    use tauri::Emitter;
    move |got, total| {
        let _ = app.emit("discover-progress", json!({"url": url, "got": got, "total": total}));
    }
}
