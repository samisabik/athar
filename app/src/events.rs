//! What the page is told without asking: the engine clock, sounds that ended, the Arranger's
//! passes, the spectrum while the EQ shows it, and a take's progress while recording.

use crate::{arr::Arr, engine::Engine, input::Input};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// The clock is sent this often; the page counts time in between.
const CLOCK: Duration = Duration::from_millis(250);
const SPECTRUM: Duration = Duration::from_millis(33);
const RECORDING: Duration = Duration::from_millis(40);

#[derive(Default)]
pub struct Watch {
    pub spectrum: AtomicBool,
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let engine = app.state::<Arc<Engine>>().inner().clone();
        let (mut clock, mut spec, mut rec) = (Instant::now() - CLOCK, Instant::now(), Instant::now());
        let mut arr_seen = 0;
        loop {
            std::thread::sleep(Duration::from_millis(5));
            let ended = engine.take_ended();
            if !ended.is_empty() || clock.elapsed() >= CLOCK {
                clock = Instant::now();
                let _ = app.emit("engine", json!({"now": engine.now(), "rate": engine.rate(), "ended": ended, "error": engine.error()}));
            }
            if let Some(st) = app.state::<Arr>().changed(&mut arr_seen) {
                let _ = app.emit("arranger", st);
            }
            if app.state::<Watch>().spectrum.load(Ordering::Relaxed) && spec.elapsed() >= SPECTRUM {
                spec = Instant::now();
                let _ = app.emit("spectrum", engine.spectrum());
            }
            let input = app.state::<Input>();
            if input.recording() && rec.elapsed() >= RECORDING {
                rec = Instant::now();
                let _ = app.emit("recording", input.progress().map_err(|e| json!({"error": e})).unwrap_or_else(|e| e));
            }
        }
    });
}
