//! Recording from a microphone or audio interface through CoreAudio (via cpal).
//!
//! A take has a thread of its own that keeps the stream open. The stream's callback only hands
//! frames over; the thread adds them to the take, and the page is told how far it has got.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{bounded, Receiver};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Frames per bar of the scrolling picture the page draws while recording.
const BAR: usize = 512;
/// Seconds of sound that can wait between the callback and the take's thread.
const WAITING_SECONDS: usize = 2;
/// How often the take's thread collects what has arrived.
const COLLECT: Duration = Duration::from_millis(20);

#[derive(Default)]
struct Take {
    channels: Vec<Vec<f32>>,
    sr: u32,
    error: Option<String>,
    drawn: usize,
}

struct Running {
    stop: mpsc::Sender<()>,
    thread: JoinHandle<()>,
}

#[derive(Default)]
pub struct Input {
    /// The take being recorded, or the last one until it is handed over.
    take: Mutex<Arc<Mutex<Take>>>,
    running: Mutex<Option<Running>>,
}

fn name_of(d: &cpal::Device) -> Option<String> {
    d.description().ok().map(|d| d.name().to_string())
}

pub fn devices() -> Vec<String> {
    cpal::default_host().input_devices().map(|ds| ds.filter_map(|d| name_of(&d)).collect()).unwrap_or_default()
}

/// Opens `device` (the default input when empty). Returns the running stream, the frames it
/// delivers (the first two channels of each), the sample rate and how many channels are kept.
fn open(device: &str, take: &Arc<Mutex<Take>>) -> Result<(cpal::Stream, Receiver<[f32; 2]>, u32, usize), String> {
    let host = cpal::default_host();
    let dev = if device.is_empty() { host.default_input_device() } else { host.input_devices().map_err(|e| e.to_string())?.find(|d| name_of(d).as_deref() == Some(device)) }.ok_or("That input is not connected")?;
    let config = dev.default_input_config().map_err(|e| e.to_string())?;
    let (sr, all) = (config.sample_rate(), (config.channels() as usize).max(1));
    let (tx, rx) = bounded(sr as usize * WAITING_SECONDS);
    let err_take = take.clone();
    let stream = dev
        .build_input_stream::<f32, _, _>(
            config.into(),
            move |data: &[f32], _| {
                for frame in data.chunks_exact(all) {
                    // Frames are only lost when the take's thread has not run for WAITING_SECONDS.
                    let _ = tx.try_send([frame[0], frame[all.min(2) - 1]]);
                }
            },
            move |e| err_take.lock().unwrap().error = Some(e.to_string()),
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, rx, sr, all.min(2)))
}

impl Input {
    /// Opens `device` (the default input when empty) and starts collecting. Returns the sample
    /// rate and channel count; more than two channels are kept as the first two.
    pub fn start(&self, device: &str) -> Result<(u32, usize), String> {
        self.stop();
        let take = Arc::new(Mutex::new(Take::default()));
        *self.take.lock().unwrap() = take.clone();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, usize), String>>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let device = device.to_string();
        let thread = std::thread::spawn(move || {
            let (stream, frames, sr, keep) = match open(&device, &take) {
                Ok(o) => o,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            {
                let mut t = take.lock().unwrap();
                t.channels = vec![Vec::new(); keep];
                t.sr = sr;
            }
            let _ = ready_tx.send(Ok((sr, keep)));
            let mut stream = Some(stream);
            while stream.is_some() {
                // Asked to stop, or the recorder is gone: the stream closes, and what it
                // delivered up to then is still collected below.
                if !matches!(stop_rx.recv_timeout(COLLECT), Err(mpsc::RecvTimeoutError::Timeout)) {
                    stream = None;
                }
                let mut t = take.lock().unwrap();
                for frame in frames.try_iter() {
                    for (ch, v) in t.channels.iter_mut().zip(frame) {
                        ch.push(v);
                    }
                }
            }
        });
        let r = ready_rx.recv().map_err(|_| "The input closed".to_string())?;
        if r.is_ok() {
            *self.running.lock().unwrap() = Some(Running { stop: stop_tx, thread });
        }
        r
    }

    /// How far the take has got: its length in frames, the loudest sample of each new bar of
    /// BAR frames since the last call, and the loudest of those.
    pub fn progress(&self) -> Result<serde_json::Value, String> {
        let take = self.take.lock().unwrap().clone();
        let mut t = take.lock().unwrap();
        if let Some(e) = t.error.take() {
            return Err(e);
        }
        let n = t.channels.first().map_or(0, Vec::len);
        let mut peaks = vec![];
        while t.drawn + BAR <= n {
            let a = t.drawn;
            peaks.push(t.channels.iter().flat_map(|c| c[a..a + BAR].iter()).fold(0.0f32, |m, x| m.max(x.abs())));
            t.drawn += BAR;
        }
        let level = peaks.iter().copied().fold(0.0f32, f32::max);
        Ok(serde_json::json!({"frames": n, "sr": t.sr, "peaks": peaks, "level": level}))
    }

    /// Stops and hands over the take: its channels and sample rate.
    pub fn finish(&self) -> (Vec<Vec<f32>>, u32) {
        self.stop();
        let take = self.take.lock().unwrap().clone();
        let mut t = take.lock().unwrap();
        (std::mem::take(&mut t.channels), t.sr)
    }

    pub fn recording(&self) -> bool {
        self.running.lock().unwrap().is_some()
    }

    /// Ends the take; returns once everything the input delivered is in it.
    pub fn stop(&self) {
        let running = self.running.lock().unwrap().take();
        if let Some(r) = running {
            let _ = r.stop.send(());
            let _ = r.thread.join();
        }
    }
}
