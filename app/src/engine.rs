//! The audio engine: plays sounds through CoreAudio (via cpal) on a clock of its own.
//!
//! The page describes what to play as a small graph, the way WebAudio does: a sound starts at a
//! time on the engine clock and passes through a chain of nodes (gains with automation, EQ
//! filters, a spectrum tap) to the output. The audio thread only ever reads commands from a
//! channel; sounds arrive already resampled to the device rate.

use athar_core::decode::resample;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{unbounded, Receiver, Sender};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const TAP: usize = 4096;

/// A sound held in memory, one vector per channel, with copies resampled to the device rate.
pub struct Sound {
    pub channels: Vec<Vec<f32>>,
    pub sr: u32,
    at_rate: Mutex<Option<(u32, Arc<Vec<Vec<f32>>>)>>,
}

impl Sound {
    pub fn new(channels: Vec<Vec<f32>>, sr: u32) -> Arc<Self> {
        Arc::new(Self { channels, sr, at_rate: Mutex::new(None) })
    }

    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    fn at(&self, rate: u32) -> Arc<Vec<Vec<f32>>> {
        let mut c = self.at_rate.lock().unwrap();
        if let Some((r, d)) = c.as_ref() {
            if *r == rate {
                return d.clone();
            }
        }
        let d: Vec<Vec<f32>> = if self.sr == rate {
            self.channels.clone()
        } else {
            self.channels.iter().map(|ch| resample(ch, self.sr, rate).unwrap_or_default()).collect()
        };
        let d = Arc::new(d);
        *c = Some((rate, d.clone()));
        d
    }
}

/// One step of a gain node's automation, in seconds of the engine clock.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(tag = "k")]
pub enum Ev {
    #[serde(rename = "set")]
    Set { t: f64, v: f32 },
    #[serde(rename = "ramp")]
    Ramp { t: f64, v: f32 },
    #[serde(rename = "target")]
    Target { t: f64, v: f32, tau: f64 },
    #[serde(rename = "curve")]
    Curve { t: f64, dur: f64, values: Vec<f32> },
}

impl Ev {
    fn time(&self) -> f64 {
        match self {
            Ev::Set { t, .. } | Ev::Ramp { t, .. } | Ev::Target { t, .. } | Ev::Curve { t, .. } => *t,
        }
    }
}

#[derive(Clone, Default)]
struct Gain {
    value: f32,
    events: Vec<Ev>,
}

impl Gain {
    /// The value at time t, as WebAudio's AudioParam computes it for these event kinds.
    fn at(&self, t: f64) -> f32 {
        let mut v = self.value;
        let mut vt = 0.0f64;
        let mut i = 0;
        while i < self.events.len() {
            let e = &self.events[i];
            if e.time() > t {
                if let Ev::Ramp { t: te, v: ve } = e {
                    let span = (te - vt).max(1e-9);
                    return v + (ve - v) * ((t - vt) / span).clamp(0.0, 1.0) as f32;
                }
                return v;
            }
            match e {
                Ev::Set { t, v: x } | Ev::Ramp { t, v: x } => {
                    v = *x;
                    vt = *t;
                }
                Ev::Target { t: ts, v: target, tau } => {
                    let end = self.events.get(i + 1).map_or(t, |n| n.time().min(t));
                    v = target + (v - target) * (-(end - ts) / tau.max(1e-6)).exp() as f32;
                    vt = end;
                }
                Ev::Curve { t: ts, dur, values } => {
                    if values.is_empty() {
                    } else if t >= ts + dur {
                        v = *values.last().unwrap();
                        vt = ts + dur;
                    } else {
                        let x = ((t - ts) / dur.max(1e-9)) * (values.len() - 1) as f64;
                        let k = (x.floor() as usize).min(values.len() - 1);
                        let k1 = (k + 1).min(values.len() - 1);
                        return values[k] + (values[k1] - values[k]) * (x - k as f64) as f32;
                    }
                }
            }
            i += 1;
        }
        v
    }
}

#[derive(Clone, Copy, Default)]
struct Coefs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

/// WebAudio BiquadFilterNode coefficients (Audio EQ Cookbook, with WebAudio's reading of Q:
/// in dB for low- and high-pass, linear for peaking and notch, unused by shelves).
pub fn biquad(kind: &str, f: f64, q: f64, gain_db: f64, rate: f64) -> Option<[f32; 5]> {
    let w0 = 2.0 * std::f64::consts::PI * (f / rate).clamp(0.0, 0.5);
    let (sn, cs) = (w0.sin(), w0.cos());
    let a = 10f64.powf(gain_db / 40.0);
    let (b0, b1, b2, a0, a1, a2) = match kind {
        "lowpass" | "highpass" => {
            let alpha = sn / (2.0 * 10f64.powf(q / 20.0));
            let (b0, b1) = if kind == "lowpass" { ((1.0 - cs) / 2.0, 1.0 - cs) } else { ((1.0 + cs) / 2.0, -(1.0 + cs)) };
            (b0, b1, b0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha)
        }
        "peaking" => {
            let alpha = sn / (2.0 * q.max(1e-4));
            (1.0 + alpha * a, -2.0 * cs, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cs, 1.0 - alpha / a)
        }
        "notch" => {
            let alpha = sn / (2.0 * q.max(1e-4));
            (1.0, -2.0 * cs, 1.0, 1.0 + alpha, -2.0 * cs, 1.0 - alpha)
        }
        "lowshelf" | "highshelf" => {
            let alpha = sn / 2.0 * 2f64.sqrt();
            let k = 2.0 * a.sqrt() * alpha;
            if kind == "lowshelf" {
                (
                    a * ((a + 1.0) - (a - 1.0) * cs + k),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cs),
                    a * ((a + 1.0) - (a - 1.0) * cs - k),
                    (a + 1.0) + (a - 1.0) * cs + k,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cs),
                    (a + 1.0) + (a - 1.0) * cs - k,
                )
            } else {
                (
                    a * ((a + 1.0) + (a - 1.0) * cs + k),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cs),
                    a * ((a + 1.0) + (a - 1.0) * cs - k),
                    (a + 1.0) - (a - 1.0) * cs + k,
                    2.0 * ((a - 1.0) - (a + 1.0) * cs),
                    (a + 1.0) - (a - 1.0) * cs - k,
                )
            }
        }
        _ => return None,
    };
    Some([(b0 / a0) as f32, (b1 / a0) as f32, (b2 / a0) as f32, (a1 / a0) as f32, (a2 / a0) as f32])
}

#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(tag = "k", content = "id")]
pub enum Link {
    #[serde(rename = "gain")]
    Gain(u64),
    #[serde(rename = "filter")]
    Filter(u64),
    #[serde(rename = "tap")]
    Tap,
}

struct Voice {
    id: u64,
    data: Arc<Vec<Vec<f32>>>,
    start: u64,
    pos: usize,
    end: usize,
    looped: Option<(usize, usize)>,
    stop_at: Option<u64>,
    chain: Vec<Link>,
    filt: Vec<[f32; 4]>,
}

enum Cmd {
    Start(Box<Voice>),
    Stop(u64, u64),
    GainSet(u64, Gain),
    Filter(u64, Option<[f32; 5]>),
    Drop(Vec<u64>),
}

struct Graph {
    voices: Vec<Voice>,
    gains: HashMap<u64, Gain>,
    filters: HashMap<u64, Coefs>,
    tap: Vec<f32>,
    tap_at: usize,
}

pub struct Engine {
    tx: Sender<Cmd>,
    rate: AtomicU32,
    clock: Arc<AtomicU64>,
    level: Arc<AtomicU32>,
    volume: Arc<AtomicU32>,
    graph: Arc<Mutex<Graph>>,
    ended: Arc<Mutex<Vec<u64>>>,
    next: AtomicU64,
    error: Mutex<Option<String>>,
    sounds: Mutex<HashMap<String, Arc<Sound>>>,
    order: Mutex<Vec<String>>,
    uploads: Mutex<Vec<(String, Arc<Sound>)>>,
    held: Mutex<HashMap<String, Arc<Sound>>>,
}

impl Engine {
    pub fn start() -> Arc<Self> {
        let (tx, rx) = unbounded();
        let e = Arc::new(Self {
            tx,
            rate: AtomicU32::new(48_000),
            clock: Arc::new(AtomicU64::new(0)),
            level: Arc::new(AtomicU32::new(0)),
            volume: Arc::new(AtomicU32::new(0.8f32.to_bits())),
            graph: Arc::new(Mutex::new(Graph { voices: vec![], gains: HashMap::new(), filters: HashMap::new(), tap: vec![0.0; TAP], tap_at: 0 })),
            ended: Arc::new(Mutex::new(Vec::new())),
            next: AtomicU64::new(1),
            error: Mutex::new(None),
            sounds: Mutex::new(HashMap::new()),
            order: Mutex::new(Vec::new()),
            uploads: Mutex::new(Vec::new()),
            held: Mutex::new(HashMap::new()),
        });
        let me = e.clone();
        std::thread::spawn(move || me.run(rx));
        e
    }

    /// Keeps an output stream open on the current default device, opening a new one when the
    /// device changes or the stream fails. The graph carries over.
    fn run(self: Arc<Self>, rx: Receiver<Cmd>) {
        let rx = Arc::new(rx);
        loop {
            let (fail_tx, fail_rx) = std::sync::mpsc::channel::<()>();
            let stream = self.open(rx.clone(), fail_tx);
            match &stream {
                Ok(s) => {
                    let _ = s.play();
                    *self.error.lock().unwrap() = None;
                }
                Err(e) => *self.error.lock().unwrap() = Some(e.clone()),
            }
            let device_name = || cpal::default_host().default_output_device().and_then(|d| d.description().ok()).map(|d| d.name().to_string());
            let initial = device_name();
            // Watch for a failed stream or a change of default output device.
            loop {
                if fail_rx.recv_timeout(std::time::Duration::from_millis(1000)).is_ok() {
                    break;
                }
                if stream.is_err() || device_name() != initial {
                    break;
                }
            }
            drop(stream);
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    fn open(&self, rx: Arc<Receiver<Cmd>>, fail: std::sync::mpsc::Sender<()>) -> Result<cpal::Stream, String> {
        let device = cpal::default_host().default_output_device().ok_or("No audio output device")?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let rate = config.sample_rate();
        let channels = config.channels() as usize;
        let old = self.rate.swap(rate, Ordering::SeqCst);
        if old != rate {
            // Sounds already playing were resampled for the old rate; let them go.
            self.graph.lock().unwrap().voices.clear();
        }
        let graph = self.graph.clone();
        let clock = self.clock.clone();
        let level = self.level.clone();
        let volume = self.volume.clone();
        let ended = self.ended.clone();
        let sr = rate as f64;
        let cfg: cpal::StreamConfig = config.into();
        device
            .build_output_stream::<f32, _, _>(
                cfg,
                move |out: &mut [f32], _| {
                    let mut g = graph.lock().unwrap();
                    let frames = out.len() / channels;
                    let t0 = clock.load(Ordering::Relaxed);
                    while let Ok(c) = rx.try_recv() {
                        g.apply(c, t0);
                    }
                    g.render(out, channels, t0, sr, &ended);
                    let vol = f32::from_bits(volume.load(Ordering::Relaxed));
                    out.iter_mut().for_each(|s| *s *= vol);
                    let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                    level.store(peak.to_bits(), Ordering::Relaxed);
                    clock.store(t0 + frames as u64, Ordering::Relaxed);
                },
                move |_| {
                    let _ = fail.send(());
                },
                None,
            )
            .map_err(|e| e.to_string())
    }

    pub fn rate(&self) -> u32 {
        self.rate.load(Ordering::SeqCst)
    }

    pub fn now(&self) -> f64 {
        self.clock.load(Ordering::Relaxed) as f64 / self.rate() as f64
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    /// The loudest sample of the last block sent to the device.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn take_ended(&self) -> Vec<u64> {
        std::mem::take(&mut *self.ended.lock().unwrap())
    }

    pub fn id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::SeqCst)
    }

    /// A sound decoded from a file, kept for the next plays of the same file.
    pub fn sound_of_path(&self, path: &str) -> Result<Arc<Sound>, String> {
        self.cached(path, || {
            let d = athar_core::decode::decode(std::path::Path::new(path)).map_err(|e| e.to_string())?;
            Ok(Sound::new(d.channels, d.sr))
        })
    }

    /// The first `max_s` seconds of a file, for previews, with the file's whole length.
    pub fn preview_of_path(&self, path: &str, max_s: usize) -> Result<(String, f64), String> {
        let key = format!("cap:{path}");
        let mut full = 0.0;
        let s = self.cached(&key, || {
            let mut d = athar_core::decode::decode(std::path::Path::new(path)).map_err(|e| e.to_string())?;
            full = d.duration();
            let n = max_s * d.sr as usize;
            d.channels.iter_mut().for_each(|c| c.truncate(n));
            Ok(Sound::new(d.channels, d.sr))
        })?;
        if full == 0.0 {
            full = s.frames() as f64 / s.sr as f64;
        }
        Ok((key, full))
    }

    fn cached(&self, key: &str, make: impl FnOnce() -> Result<Arc<Sound>, String>) -> Result<Arc<Sound>, String> {
        if let Some(s) = self.sounds.lock().unwrap().get(key) {
            return Ok(s.clone());
        }
        let s = make()?;
        let mut cache = self.sounds.lock().unwrap();
        let mut order = self.order.lock().unwrap();
        if cache.insert(key.to_string(), s.clone()).is_none() {
            order.push(key.to_string());
        }
        if order.len() > 24 {
            let old = order.remove(0);
            cache.remove(&old);
        }
        Ok(s)
    }

    /// Sounds decoded for a preview (found sounds); the last few are kept for replays.
    pub fn upload(&self, sound: Arc<Sound>) -> String {
        let key = format!("buf:{}", self.id());
        let mut u = self.uploads.lock().unwrap();
        u.push((key.clone(), sound));
        if u.len() > 6 {
            u.remove(0);
        }
        key
    }

    /// Keeps a sound under a key of its own until `release`: recorded takes.
    pub fn hold(&self, sound: Arc<Sound>) -> String {
        let key = format!("held:{}", self.id());
        self.held.lock().unwrap().insert(key.clone(), sound);
        key
    }

    pub fn release(&self, key: &str) {
        self.held.lock().unwrap().remove(key);
    }

    pub fn sound(&self, key: &str) -> Result<Arc<Sound>, String> {
        if let Some(s) = self.held.lock().unwrap().get(key) {
            return Ok(s.clone());
        }
        if let Some(p) = key.strip_prefix("path:") {
            return self.sound_of_path(p);
        }
        if key.starts_with("cap:") {
            return self.sounds.lock().unwrap().get(key).cloned().ok_or_else(|| "That sound is no longer held".to_string());
        }
        self.uploads.lock().unwrap().iter().find(|(k, _)| k == key).map(|(_, s)| s.clone()).ok_or_else(|| "That sound is no longer held".to_string())
    }

    pub fn set_gain(&self, node: u64, value: f32, events: Vec<Ev>) {
        let mut events = events;
        events.sort_by(|a, b| a.time().total_cmp(&b.time()));
        let _ = self.tx.send(Cmd::GainSet(node, Gain { value, events }));
    }

    pub fn set_filter_coefs(&self, node: u64, c: Option<[f32; 5]>) {
        let _ = self.tx.send(Cmd::Filter(node, c));
    }

    /// A gain node's value now, as last set.
    pub fn gain_now(&self, node: u64) -> Option<f32> {
        let t = self.now();
        self.graph.lock().unwrap().gains.get(&node).map(|g| g.at(t))
    }

    /// The level of everything played, 0 to 1.
    pub fn set_volume(&self, v: f32) {
        self.volume.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn drop_nodes(&self, ids: Vec<u64>) {
        let _ = self.tx.send(Cmd::Drop(ids));
    }

    /// Plays `sound` from `offset` seconds, at `when` on the engine clock (now if in the past),
    /// for `duration` seconds or to the end, looping between `looped` (seconds) when given.
    pub fn play(&self, id: u64, sound: &Sound, when: f64, offset: f64, duration: Option<f64>, looped: Option<(f64, f64)>, chain: Vec<Link>) {
        let rate = self.rate();
        let data = sound.at(rate);
        let n = data.first().map_or(0, Vec::len);
        let r = rate as f64;
        let pos = ((offset.max(0.0) * r) as usize).min(n);
        let end = duration.map_or(n, |d| (pos + (d.max(0.0) * r) as usize).min(n));
        let looped = looped.map(|(a, b)| ((a.max(0.0) * r) as usize, ((b * r) as usize).min(n))).filter(|(a, b)| b > a);
        let start = ((when * r).max(0.0)) as u64;
        let _ = self.tx.send(Cmd::Start(Box::new(Voice {
            id,
            data,
            start,
            pos,
            end: if looped.is_some() { n } else { end },
            looped,
            stop_at: None,
            filt: vec![[0.0; 4]; chain.iter().filter(|l| matches!(l, Link::Filter(_))).count() * 2],
            chain,
        })));
    }

    pub fn stop(&self, id: u64, when: f64) {
        let _ = self.tx.send(Cmd::Stop(id, ((when * self.rate() as f64).max(0.0)) as u64));
    }

    /// The spectrum at the tap, in dB per bin, the way an AnalyserNode with fftSize 4096
    /// reports it (Blackman window).
    pub fn spectrum(&self) -> Vec<f32> {
        let (tap, at) = {
            let g = self.graph.lock().unwrap();
            (g.tap.clone(), g.tap_at)
        };
        let n = TAP;
        let mut buf: Vec<f64> = (0..n)
            .map(|i| {
                let x = tap[(at + i) % n] as f64;
                let w = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos() + 0.08 * (4.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
                x * w
            })
            .collect();
        let fft = realfft::RealFftPlanner::<f64>::new().plan_fft_forward(n);
        let mut spec = fft.make_output_vec();
        let _ = fft.process(&mut buf, &mut spec);
        spec.iter().take(n / 2).map(|c| (20.0 * (c.norm() / n as f64).max(1e-12).log10()) as f32).collect()
    }
}

impl Graph {
    fn apply(&mut self, c: Cmd, now: u64) {
        match c {
            Cmd::Start(mut v) => {
                // A sound that should have started already starts where it would be by now, so
                // the page's playhead and the sound agree.
                if v.start < now {
                    let mut late = (now - v.start) as usize;
                    v.start = now;
                    match v.looped {
                        Some((a, b)) if v.pos + late >= b => {
                            late -= b - v.pos;
                            v.pos = a + late % (b - a).max(1);
                        }
                        _ => v.pos += late,
                    }
                }
                self.voices.retain(|x| x.id != v.id);
                self.voices.push(*v);
            }
            Cmd::Stop(id, at) => {
                for v in self.voices.iter_mut().filter(|v| v.id == id) {
                    v.stop_at = Some(at);
                }
            }
            Cmd::GainSet(id, g) => {
                self.gains.insert(id, g);
            }
            Cmd::Filter(id, c) => {
                match c {
                    Some([b0, b1, b2, a1, a2]) => self.filters.insert(id, Coefs { b0, b1, b2, a1, a2 }),
                    None => self.filters.remove(&id),
                };
            }
            Cmd::Drop(ids) => {
                for id in ids {
                    self.gains.remove(&id);
                    self.filters.remove(&id);
                }
            }
        }
    }

    fn render(&mut self, out: &mut [f32], channels: usize, t0: u64, sr: f64, ended: &Mutex<Vec<u64>>) {
        out.iter_mut().for_each(|s| *s = 0.0);
        let frames = out.len() / channels;
        let (ta, tb) = (t0 as f64 / sr, (t0 + frames as u64) as f64 / sr);
        let mut done = Vec::new();
        let mut tap_block = vec![0.0f32; frames];
        let mut tapped = false;
        for v in self.voices.iter_mut() {
            let chans = v.data.len();
            if chans == 0 {
                done.push(v.id);
                continue;
            }
            // Gains at the two ends of the block, interpolated across it.
            let gains: Vec<Option<(f32, f32)>> = v.chain.iter().map(|l| if let Link::Gain(id) = l { self.gains.get(id).map(|g| (g.at(ta), g.at(tb))) } else { None }).collect();
            let mut finished = false;
            for i in 0..frames {
                let now = t0 + i as u64;
                if now < v.start {
                    continue;
                }
                if v.stop_at.is_some_and(|s| now >= s) || v.pos >= v.end {
                    finished = true;
                    break;
                }
                let mut l = v.data[0][v.pos];
                let mut r = if chans > 1 { v.data[1][v.pos] } else { l };
                let frac = i as f32 / frames as f32;
                let mut fi = 0;
                for (k, link) in v.chain.iter().enumerate() {
                    match link {
                        Link::Gain(_) => {
                            if let Some((a, b)) = gains[k] {
                                let g = a + (b - a) * frac;
                                l *= g;
                                r *= g;
                            }
                        }
                        Link::Filter(id) => {
                            if let Some(c) = self.filters.get(id) {
                                for (ch, x) in [&mut l, &mut r].into_iter().enumerate() {
                                    let st = &mut v.filt[fi * 2 + ch];
                                    let y = c.b0 * *x + c.b1 * st[0] + c.b2 * st[1] - c.a1 * st[2] - c.a2 * st[3];
                                    st[1] = st[0];
                                    st[0] = *x;
                                    st[3] = st[2];
                                    st[2] = y;
                                    *x = y;
                                }
                            }
                            fi += 1;
                        }
                        Link::Tap => {
                            tap_block[i] += 0.5 * (l + r);
                            tapped = true;
                        }
                    }
                }
                if channels == 1 {
                    out[i] += 0.5 * (l + r);
                } else {
                    out[i * channels] += l;
                    out[i * channels + 1] += r;
                }
                v.pos += 1;
                if let Some((a, b)) = v.looped {
                    if v.pos >= b {
                        v.pos = a;
                    }
                }
            }
            if finished {
                done.push(v.id);
            }
        }
        if tapped {
            for x in tap_block {
                self.tap[self.tap_at] = x;
                self.tap_at = (self.tap_at + 1) % TAP;
            }
        }
        if !done.is_empty() {
            self.voices.retain(|v| !done.contains(&v.id));
            if let Ok(mut e) = ended.try_lock() {
                e.extend(done);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_follows_webaudio() {
        let g = Gain { value: 1.0, events: vec![Ev::Set { t: 1.0, v: 0.0 }, Ev::Ramp { t: 2.0, v: 1.0 }] };
        assert_eq!(g.at(0.5), 1.0);
        assert_eq!(g.at(1.0), 0.0);
        assert!((g.at(1.5) - 0.5).abs() < 1e-6);
        assert_eq!(g.at(3.0), 1.0);
        let c = Gain { value: 0.0, events: vec![Ev::Curve { t: 0.0, dur: 1.0, values: vec![0.0, 1.0] }] };
        assert!((c.at(0.25) - 0.25).abs() < 1e-6);
        let t = Gain { value: 1.0, events: vec![Ev::Target { t: 0.0, v: 0.0, tau: 1.0 }] };
        assert!((t.at(1.0) - (-1.0f64).exp() as f32).abs() < 1e-5);
    }

    #[test]
    fn peaking_filter_has_the_set_gain_at_its_centre() {
        let [b0, b1, b2, a1, a2] = biquad("peaking", 1000.0, 1.0, 6.0, 48_000.0).unwrap();
        let w = 2.0 * std::f64::consts::PI * 1000.0 / 48_000.0;
        let z1 = num_complex(w);
        let h = |b: [f64; 3], a: [f64; 3]| {
            let num = (b[0] + b[1] * z1.0 + b[2] * z1.1, b[1] * z1.2 + b[2] * z1.3);
            let den = (1.0 + a[1] * z1.0 + a[2] * z1.1, a[1] * z1.2 + a[2] * z1.3);
            ((num.0 * num.0 + num.1 * num.1) / (den.0 * den.0 + den.1 * den.1)).sqrt()
        };
        let mag = h([b0 as f64, b1 as f64, b2 as f64], [1.0, a1 as f64, a2 as f64]);
        assert!((20.0 * mag.log10() - 6.0).abs() < 0.01, "{}", 20.0 * mag.log10());
    }

    /// cos and sin of -w and -2w, for evaluating a biquad on the unit circle.
    fn num_complex(w: f64) -> (f64, f64, f64, f64) {
        ((-w).cos(), (-2.0 * w).cos(), (-w).sin(), (-2.0 * w).sin())
    }
}
