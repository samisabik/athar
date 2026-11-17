//! Recording from a microphone or audio interface through CoreAudio (via cpal).
//!
//! The stream lives on its own thread for as long as the take runs; the page collects what has
//! arrived since its last call.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

/// Frames per bar of the scrolling picture the page draws while recording.
const BAR: usize = 512;

#[derive(Default)]
struct Take {
    channels: Vec<Vec<f32>>,
    sr: u32,
    error: Option<String>,
    drawn: usize,
}

#[derive(Default)]
pub struct Input {
    take: Arc<Mutex<Take>>,
    stop: Mutex<Option<mpsc::Sender<()>>>,
}

fn name_of(d: &cpal::Device) -> Option<String> {
    d.description().ok().map(|d| d.name().to_string())
}

pub fn devices() -> Vec<String> {
    cpal::default_host().input_devices().map(|ds| ds.filter_map(|d| name_of(&d)).collect()).unwrap_or_default()
}

impl Input {
    /// Opens `device` (the default input when empty) and starts collecting. Returns the sample
    /// rate and channel count; more than two channels are kept as the first two.
    pub fn start(&self, device: &str) -> Result<(u32, usize), String> {
        self.stop();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, usize), String>>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let take = self.take.clone();
        let device = device.to_string();
        std::thread::spawn(move || {
            let open = || -> Result<(cpal::Stream, u32, usize), String> {
                let host = cpal::default_host();
                let dev = if device.is_empty() {
                    host.default_input_device()
                } else {
                    host.input_devices().map_err(|e| e.to_string())?.find(|d| name_of(d).as_deref() == Some(device.as_str()))
                }
                .ok_or("That input is not connected")?;
                let config = dev.default_input_config().map_err(|e| e.to_string())?;
                let (sr, all) = (config.sample_rate(), config.channels() as usize);
                let keep = all.clamp(1, 2);
                *take.lock().unwrap() = Take { channels: vec![Vec::new(); keep], sr, error: None, drawn: 0 };
                let (data_take, err_take) = (take.clone(), take.clone());
                let stream = dev
                    .build_input_stream::<f32, _, _>(
                        config.into(),
                        move |data: &[f32], _| {
                            let mut t = data_take.lock().unwrap();
                            for frame in data.chunks_exact(all) {
                                for (c, ch) in t.channels.iter_mut().enumerate() {
                                    ch.push(frame[c]);
                                }
                            }
                        },
                        move |e| err_take.lock().unwrap().error = Some(e.to_string()),
                        None,
                    )
                    .map_err(|e| e.to_string())?;
                stream.play().map_err(|e| e.to_string())?;
                Ok((stream, sr, keep))
            };
            match open() {
                Ok((stream, sr, chs)) => {
                    let _ = ready_tx.send(Ok((sr, chs)));
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        });
        let r = ready_rx.recv().map_err(|_| "The input closed".to_string())?;
        if r.is_ok() {
            *self.stop.lock().unwrap() = Some(stop_tx);
        }
        r
    }

    /// How far the take has got: its length in frames, the loudest sample of each new bar of
    /// BAR frames since the last call, and the loudest of those.
    pub fn progress(&self) -> Result<serde_json::Value, String> {
        let mut t = self.take.lock().unwrap();
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
        let mut t = self.take.lock().unwrap();
        (std::mem::take(&mut t.channels), t.sr)
    }

    pub fn recording(&self) -> bool {
        self.stop.lock().unwrap().is_some()
    }

    pub fn stop(&self) {
        if let Some(s) = self.stop.lock().unwrap().take() {
            let _ = s.send(());
        }
    }
}
