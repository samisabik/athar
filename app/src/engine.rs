//! The audio engine: plays sounds through CoreAudio (via cpal) on a clock of its own.
//!
//! A sound starts at a time on the engine clock and passes through a chain of nodes (gains with
//! automation, EQ filters, a spectrum tap) to the output. The editor and the Arranger build the
//! chains; the page only asks for a sound by its key.
//!
//! The output callback owns the graph. Other threads reach it through a channel of commands and
//! read what it publishes without a lock: the clock, the output level, the tap and the ids of
//! sounds that ended. Sounds arrive already at the device rate, and what the callback is done
//! with (voices, replaced automation) goes to another thread to be freed.

use crate::sounds::{Channels, Sound};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, TryLockError};
use std::time::Duration;

/// Samples of the tap the spectrum is taken from.
const TAP: usize = 4096;
/// Room made ahead of time, so the callback seldom has to ask for memory.
const VOICES: usize = 256;
const NODES: usize = 1024;
const BLOCK: usize = 8192;
/// Ended sounds and freed memory waiting for their readers.
const ENDED: usize = 4096;
const GARBAGE: usize = 4096;

/// One step of a gain node's automation, in seconds of the engine clock.
#[derive(Clone, Debug)]
pub enum Ev {
    /// Jumps to `v` at `t`.
    Set { t: f64, v: f32 },
    /// Arrives at `v` at `t` in a straight line from the event before it.
    Ramp { t: f64, v: f32 },
    /// From `t`, approaches `v` exponentially with time constant `tau`.
    Target { t: f64, v: f32, tau: f64 },
    /// From `t`, runs through `values` over `dur` seconds.
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

/// A biquad filter's coefficients b0, b1, b2, a1, a2, with a0 divided out.
pub type Coefs = [f32; 5];

/// Audio EQ Cookbook coefficients, reading Q as WebAudio's BiquadFilterNode does (in dB for
/// low- and high-pass, linear for peaking and notch, unused by shelves), which the EQ's Q
/// values were chosen for.
pub fn biquad(kind: &str, f: f64, q: f64, gain_db: f64, rate: f64) -> Option<Coefs> {
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


/// One node of a voice's chain.
#[derive(Clone, Copy, Debug)]
pub enum Link {
    Gain(u64),
    Filter(u64),
    Tap,
}

/// A link as one block runs it, looked up in the graph once before the block.
#[derive(Clone, Copy)]
enum Step {
    Skip,
    /// The gain at the two ends of the block, interpolated across it.
    Gain(f32, f32),
    /// The coefficients, and which of the voice's filter states they run on.
    Filter(Coefs, usize),
    Tap,
}

struct Voice {
    id: u64,
    /// The device rate `data` was made for.
    rate: u32,
    data: Channels,
    start: u64,
    pos: usize,
    end: usize,
    looped: Option<(usize, usize)>,
    stop_at: Option<u64>,
    chain: Vec<Link>,
    steps: Vec<Step>,
    /// Per filter link and channel: the last two inputs and outputs.
    filt: Vec<[f32; 4]>,
}

enum Cmd {
    Start(Box<Voice>),
    /// A voice and the engine time it stops at.
    Stop(u64, f64),
    Gain(u64, Gain),
    Filter(u64, Option<Coefs>),
    Drop(Vec<u64>),
    StopAll,
}

/// What the callback is done with, freed on another thread.
enum Garbage {
    Voice(Box<Voice>),
    Gain(Gain),
    Ids(Vec<u64>),
}

/// What the callback publishes, read without a lock.
struct Shared {
    /// Frames played since the engine started, at the current rate.
    clock: AtomicU64,
    level: AtomicU32,
    volume: AtomicU32,
    /// The last TAP samples that passed a tap, as f32 bits, and where the next one goes.
    tap: Vec<AtomicU32>,
    tap_at: AtomicUsize,
}

/// Where the callback hands things over to other threads. Neither ever makes it wait.
struct Outlets {
    ended: Sender<u64>,
    garbage: Sender<Garbage>,
}

impl Outlets {
    fn free(&self, g: Garbage) {
        // With no room left it is freed here after all.
        let _ = self.garbage.try_send(g);
    }
}

struct Graph {
    rate: u32,
    voices: Vec<Box<Voice>>,
    gains: HashMap<u64, Gain>,
    filters: HashMap<u64, Coefs>,
    tap_block: Vec<f32>,
    /// Ended voices not reported yet, for want of room.
    unreported: Vec<u64>,
}

pub struct Engine {
    tx: Sender<Cmd>,
    rate: AtomicU32,
    shared: Arc<Shared>,
    graph: Arc<Mutex<Graph>>,
    outlets: Arc<Outlets>,
    ended: Receiver<u64>,
    /// Each gain node as last set, for `gain_now`.
    gains: Mutex<HashMap<u64, Gain>>,
    next: AtomicU64,
    error: Mutex<Option<String>>,
}

impl Engine {
    fn new() -> (Arc<Self>, Receiver<Cmd>, Receiver<Garbage>) {
        let (tx, rx) = unbounded();
        let (ended_tx, ended) = bounded(ENDED);
        let (garbage_tx, garbage) = bounded(GARBAGE);
        let rate = 48_000;
        let e = Arc::new(Self {
            tx,
            rate: AtomicU32::new(rate),
            shared: Arc::new(Shared {
                clock: AtomicU64::new(0),
                level: AtomicU32::new(0),
                volume: AtomicU32::new(0.8f32.to_bits()),
                tap: (0..TAP).map(|_| AtomicU32::new(0)).collect(),
                tap_at: AtomicUsize::new(0),
            }),
            graph: Arc::new(Mutex::new(Graph {
                rate,
                voices: Vec::with_capacity(VOICES),
                gains: HashMap::with_capacity(NODES),
                filters: HashMap::with_capacity(NODES),
                tap_block: Vec::with_capacity(BLOCK),
                unreported: Vec::with_capacity(VOICES),
            })),
            outlets: Arc::new(Outlets { ended: ended_tx, garbage: garbage_tx }),
            ended,
            gains: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            error: Mutex::new(None),
        });
        (e, rx, garbage)
    }

    pub fn start() -> Arc<Self> {
        let (e, rx, garbage) = Self::new();
        let me = e.clone();
        std::thread::spawn(move || me.run(rx));
        std::thread::spawn(move || {
            for g in garbage {
                match g {
                    Garbage::Voice(v) => drop(v),
                    Garbage::Gain(g) => drop(g),
                    Garbage::Ids(ids) => drop(ids),
                }
            }
        });
        e
    }

    /// An engine with no output device: its clock stands still and nothing plays.
    #[cfg(test)]
    pub fn silent() -> Arc<Self> {
        Self::new().0
    }

    /// Keeps an output stream open on the current default device, opening a new one when the
    /// device changes or the stream fails. The graph carries over.
    fn run(self: Arc<Self>, rx: Receiver<Cmd>) {
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
                if fail_rx.recv_timeout(Duration::from_millis(1000)).is_ok() {
                    break;
                }
                if stream.is_err() || device_name() != initial {
                    break;
                }
            }
            drop(stream);
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn open(&self, rx: Receiver<Cmd>, fail: std::sync::mpsc::Sender<()>) -> Result<cpal::Stream, String> {
        let device = cpal::default_host().default_output_device().ok_or("No audio output device")?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let channels = config.channels() as usize;
        // No stream is running here, so the graph is free.
        self.set_rate(&mut self.graph.lock().unwrap_or_else(|e| e.into_inner()), config.sample_rate());
        let (graph, shared, outlets) = (self.graph.clone(), self.shared.clone(), self.outlets.clone());
        device
            .build_output_stream::<f32, _, _>(
                config.into(),
                move |out: &mut [f32], _| {
                    // The graph is held elsewhere only while a stream is being replaced; a block
                    // that falls in that moment is silent.
                    let mut g = match graph.try_lock() {
                        Ok(g) => g,
                        Err(TryLockError::Poisoned(e)) => e.into_inner(),
                        Err(TryLockError::WouldBlock) => return out.fill(0.0),
                    };
                    g.block(&rx, &shared, &outlets, out, channels);
                },
                move |_| {
                    let _ = fail.send(());
                },
                None,
            )
            .map_err(|e| e.to_string())
    }

    /// A new device rate: sounds playing were resampled for the old one, so they end, and the
    /// clock is counted in the new rate's frames from the same moment.
    fn set_rate(&self, g: &mut Graph, rate: u32) {
        if g.rate == rate {
            return;
        }
        for v in g.voices.drain(..) {
            if self.outlets.ended.try_send(v.id).is_err() {
                g.unreported.push(v.id);
            }
        }
        let frames = self.shared.clock.load(Ordering::Relaxed);
        self.shared.clock.store((frames as u128 * rate as u128 / g.rate as u128) as u64, Ordering::Relaxed);
        g.rate = rate;
        self.rate.store(rate, Ordering::SeqCst);
    }

    pub fn rate(&self) -> u32 {
        self.rate.load(Ordering::SeqCst)
    }

    pub fn now(&self) -> f64 {
        self.shared.clock.load(Ordering::Relaxed) as f64 / self.rate() as f64
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    /// The loudest sample of the last block sent to the device.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }

    /// The sounds that ended since the last call. Each id is handed out once.
    pub fn take_ended(&self) -> Vec<u64> {
        self.ended.try_iter().collect()
    }

    pub fn id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::SeqCst)
    }

    pub fn set_gain(&self, node: u64, value: f32, events: Vec<Ev>) {
        let mut events = events;
        events.sort_by(|a, b| a.time().total_cmp(&b.time()));
        let gain = Gain { value, events };
        self.gains.lock().unwrap().insert(node, gain.clone());
        let _ = self.tx.send(Cmd::Gain(node, gain));
    }

    pub fn set_filter_coefs(&self, node: u64, c: Option<Coefs>) {
        let _ = self.tx.send(Cmd::Filter(node, c));
    }

    /// A gain node's value now, as last set.
    pub fn gain_now(&self, node: u64) -> Option<f32> {
        let t = self.now();
        self.gains.lock().unwrap().get(&node).map(|g| g.at(t))
    }

    /// The level of everything played, 0 to 1.
    pub fn set_volume(&self, v: f32) {
        self.shared.volume.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn drop_nodes(&self, ids: Vec<u64>) {
        self.gains.lock().unwrap().retain(|id, _| !ids.contains(id));
        let _ = self.tx.send(Cmd::Drop(ids));
    }

    /// Makes `sound` ready to play at the device rate, which takes a while for a long sound at
    /// another rate. `play` does it too; doing it first keeps a start time from slipping by.
    pub fn prepare(&self, sound: &Sound) -> Result<(), String> {
        sound.at(self.rate()).map(|_| ())
    }

    /// Plays `sound` from `offset` seconds, at `when` on the engine clock (now if in the past),
    /// for `duration` seconds or to the end, looping between `looped` (seconds) when given.
    pub fn play(&self, id: u64, sound: &Sound, when: f64, offset: f64, duration: Option<f64>, looped: Option<(f64, f64)>, chain: Vec<Link>) -> Result<(), String> {
        let rate = self.rate();
        let data = sound.at(rate)?;
        let n = data.iter().map(Vec::len).min().unwrap_or(0);
        let r = rate as f64;
        let pos = ((offset.max(0.0) * r) as usize).min(n);
        let end = duration.map_or(n, |d| (pos + (d.max(0.0) * r) as usize).min(n));
        let looped = looped.map(|(a, b)| ((a.max(0.0) * r) as usize, ((b * r) as usize).min(n))).filter(|(a, b)| b > a);
        let _ = self.tx.send(Cmd::Start(Box::new(Voice {
            id,
            rate,
            data,
            start: ((when * r).max(0.0)) as u64,
            pos,
            end: if looped.is_some() { n } else { end },
            looped,
            stop_at: None,
            steps: Vec::with_capacity(chain.len()),
            filt: vec![[0.0; 4]; chain.iter().filter(|l| matches!(l, Link::Filter(_))).count() * 2],
            chain,
        })));
        Ok(())
    }

    /// Silences everything at once and forgets every gain and filter.
    pub fn stop_all(&self) {
        self.gains.lock().unwrap().clear();
        let _ = self.tx.send(Cmd::StopAll);
    }

    /// Stops a voice at `when` on the engine clock (now if in the past).
    pub fn stop(&self, id: u64, when: f64) {
        let _ = self.tx.send(Cmd::Stop(id, when));
    }

    /// The spectrum at the tap, in dB per bin: TAP samples under a Blackman window.
    pub fn spectrum(&self) -> Vec<f32> {
        static PLAN: OnceLock<(Arc<dyn realfft::RealToComplex<f64>>, Vec<f64>)> = OnceLock::new();
        let (fft, window) = PLAN.get_or_init(|| {
            let turn = |i: usize, k: f64| (k * std::f64::consts::PI * i as f64 / TAP as f64).cos();
            (realfft::RealFftPlanner::<f64>::new().plan_fft_forward(TAP), (0..TAP).map(|i| 0.42 - 0.5 * turn(i, 2.0) + 0.08 * turn(i, 4.0)).collect())
        });
        let at = self.shared.tap_at.load(Ordering::Relaxed);
        let mut buf: Vec<f64> = window.iter().enumerate().map(|(i, w)| f32::from_bits(self.shared.tap[(at + i) % TAP].load(Ordering::Relaxed)) as f64 * w).collect();
        let mut spec = fft.make_output_vec();
        let _ = fft.process(&mut buf, &mut spec);
        spec.iter().take(TAP / 2).map(|c| (20.0 * (c.norm() / TAP as f64).max(1e-12).log10()) as f32).collect()
    }
}

impl Graph {
    /// One block of output: takes the commands that arrived, renders, and publishes the clock,
    /// the level and the tap.
    fn block(&mut self, rx: &Receiver<Cmd>, shared: &Shared, outlets: &Outlets, out: &mut [f32], channels: usize) {
        let frames = out.len() / channels;
        let t0 = shared.clock.load(Ordering::Relaxed);
        while let Ok(c) = rx.try_recv() {
            self.apply(c, t0, outlets);
        }
        self.render(out, channels, t0, outlets);
        let mut at = shared.tap_at.load(Ordering::Relaxed);
        for x in &self.tap_block {
            shared.tap[at].store(x.to_bits(), Ordering::Relaxed);
            at = (at + 1) % TAP;
        }
        shared.tap_at.store(at, Ordering::Relaxed);
        let vol = f32::from_bits(shared.volume.load(Ordering::Relaxed));
        out.iter_mut().for_each(|s| *s *= vol);
        let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        shared.level.store(peak.to_bits(), Ordering::Relaxed);
        shared.clock.store(t0 + frames as u64, Ordering::Relaxed);
    }

    fn end(&mut self, v: Box<Voice>, outlets: &Outlets) {
        if outlets.ended.try_send(v.id).is_err() {
            self.unreported.push(v.id);
        }
        outlets.free(Garbage::Voice(v));
    }

    fn apply(&mut self, c: Cmd, now: u64, outlets: &Outlets) {
        match c {
            Cmd::Start(mut v) => {
                if let Some(i) = self.voices.iter().position(|x| x.id == v.id) {
                    outlets.free(Garbage::Voice(self.voices.swap_remove(i)));
                }
                // Made for a device rate that has changed since: it cannot play.
                if v.rate != self.rate {
                    return self.end(v, outlets);
                }
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
                self.voices.push(v);
            }
            Cmd::Stop(id, when) => {
                let at = (when * self.rate as f64).max(0.0) as u64;
                for v in self.voices.iter_mut().filter(|v| v.id == id) {
                    v.stop_at = Some(at);
                }
            }
            Cmd::Gain(id, g) => {
                if let Some(old) = self.gains.insert(id, g) {
                    outlets.free(Garbage::Gain(old));
                }
            }
            Cmd::Filter(id, c) => {
                match c {
                    Some(c) => self.filters.insert(id, c),
                    None => self.filters.remove(&id),
                };
            }
            Cmd::Drop(ids) => {
                for id in &ids {
                    if let Some(old) = self.gains.remove(id) {
                        outlets.free(Garbage::Gain(old));
                    }
                    self.filters.remove(id);
                }
                outlets.free(Garbage::Ids(ids));
            }
            Cmd::StopAll => {
                // Stopped by request, so not reported as ended.
                while let Some(v) = self.voices.pop() {
                    outlets.free(Garbage::Voice(v));
                }
                for (_, g) in self.gains.drain() {
                    outlets.free(Garbage::Gain(g));
                }
                self.filters.clear();
            }
        }
    }

    fn render(&mut self, out: &mut [f32], channels: usize, t0: u64, outlets: &Outlets) {
        out.fill(0.0);
        let frames = out.len() / channels;
        let sr = self.rate as f64;
        let span = (t0 as f64 / sr, (t0 + frames as u64) as f64 / sr);
        self.tap_block.clear();
        self.tap_block.resize(frames, 0.0);
        self.unreported.retain(|id| outlets.ended.try_send(*id).is_err());
        let mut i = 0;
        while i < self.voices.len() {
            let Graph { voices, gains, filters, tap_block, .. } = self;
            if voices[i].render(out, channels, t0, span, gains, filters, tap_block) {
                i += 1;
            } else {
                let v = self.voices.swap_remove(i);
                self.end(v, outlets);
            }
        }
    }
}

impl Voice {
    /// Adds this voice's part of the block that starts at frame `t0` and spans the engine times
    /// `span` to `out` and, past a tap, to `tap`. False once the voice has ended.
    fn render(&mut self, out: &mut [f32], channels: usize, t0: u64, span: (f64, f64), gains: &HashMap<u64, Gain>, filters: &HashMap<u64, Coefs>, tap: &mut [f32]) -> bool {
        if self.data.is_empty() {
            return false;
        }
        self.steps.clear();
        let mut states = 0;
        for link in &self.chain {
            self.steps.push(match link {
                Link::Gain(id) => gains.get(id).map_or(Step::Skip, |g| Step::Gain(g.at(span.0), g.at(span.1))),
                Link::Filter(id) => {
                    states += 1;
                    filters.get(id).map_or(Step::Skip, |c| Step::Filter(*c, states - 1))
                }
                Link::Tap => Step::Tap,
            });
        }
        let frames = tap.len();
        let stereo = self.data.len() > 1;
        for i in 0..frames {
            let now = t0 + i as u64;
            if now < self.start {
                continue;
            }
            if self.stop_at.is_some_and(|s| now >= s) || self.pos >= self.end {
                return false;
            }
            let mut l = self.data[0][self.pos];
            let mut r = if stereo { self.data[1][self.pos] } else { l };
            let frac = i as f32 / frames as f32;
            for step in &self.steps {
                match *step {
                    Step::Skip => {}
                    Step::Gain(a, b) => {
                        let g = a + (b - a) * frac;
                        l *= g;
                        r *= g;
                    }
                    Step::Filter([b0, b1, b2, a1, a2], k) => {
                        for (ch, x) in [&mut l, &mut r].into_iter().enumerate() {
                            let st = &mut self.filt[k * 2 + ch];
                            let y = b0 * *x + b1 * st[0] + b2 * st[1] - a1 * st[2] - a2 * st[3];
                            st[1] = st[0];
                            st[0] = *x;
                            st[3] = st[2];
                            st[2] = y;
                            *x = y;
                        }
                    }
                    Step::Tap => tap[i] += 0.5 * (l + r),
                }
            }
            if channels == 1 {
                out[i] += 0.5 * (l + r);
            } else {
                out[i * channels] += l;
                out[i * channels + 1] += r;
            }
            self.pos += 1;
            if let Some((a, b)) = self.looped {
                if self.pos >= b {
                    self.pos = a;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the output callback's work for one mono block.
    fn pump(e: &Engine, rx: &Receiver<Cmd>, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames];
        e.graph.lock().unwrap().block(rx, &e.shared, &e.outlets, &mut out, 1);
        out
    }

    #[test]
    fn automation_jumps_ramps_follows_curves_and_approaches_targets() {
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
        let [b0, b1, b2, a1, a2] = biquad("peaking", 1000.0, 1.0, 6.0, 48_000.0).unwrap().map(f64::from);
        // The response at z = e^(jw): real and imaginary parts of numerator and denominator.
        let w = 2.0 * std::f64::consts::PI * 1000.0 / 48_000.0;
        let (c1, s1, c2, s2) = (w.cos(), -w.sin(), (2.0 * w).cos(), -(2.0 * w).sin());
        let num = (b0 + b1 * c1 + b2 * c2).hypot(b1 * s1 + b2 * s2);
        let den = (1.0 + a1 * c1 + a2 * c2).hypot(a1 * s1 + a2 * s2);
        let db = 20.0 * (num / den).log10();
        assert!((db - 6.0).abs() < 0.01, "{db}");
    }

    #[test]
    fn a_sound_plays_through_its_chain_and_is_reported_ended_once() {
        let (e, rx, _garbage) = Engine::new();
        let half = e.id();
        e.set_gain(half, 0.5, vec![]);
        assert_eq!(e.gain_now(half), Some(0.5));
        e.play(7, &Sound::new(vec![vec![1.0; 100]], 48_000), 0.0, 0.0, None, None, vec![Link::Gain(half), Link::Tap]).unwrap();
        let out = pump(&e, &rx, 256);
        assert!(out[..100].iter().all(|x| (*x - 0.5 * 0.8).abs() < 1e-6), "half gain, then the output volume");
        assert!(out[100..].iter().all(|x| *x == 0.0));
        assert!((e.level() - 0.4).abs() < 1e-6);
        assert_eq!(e.take_ended(), vec![7]);
        assert!(e.take_ended().is_empty());
        e.play(8, &Sound::new(vec![vec![1.0; TAP]], 48_000), 0.0, 0.0, None, None, vec![Link::Tap]).unwrap();
        pump(&e, &rx, TAP);
        assert!(e.spectrum()[0] > -20.0, "the tap heard it");
        pump(&e, &rx, TAP);
        assert!(e.spectrum()[0] < -200.0, "and hears silence once nothing plays");
    }

    #[test]
    fn a_late_start_begins_where_the_sound_would_be_by_now() {
        let (e, rx, _garbage) = Engine::new();
        pump(&e, &rx, 480);
        let ramp: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        e.play(1, &Sound::new(vec![ramp], 48_000), 0.0, 0.0, None, None, vec![]).unwrap();
        assert_eq!(pump(&e, &rx, 4)[0], 480.0 * 0.8);
    }

    #[test]
    fn ended_sounds_wait_for_room_to_be_reported() {
        let (ended, taken) = bounded(1);
        let (garbage, _freed) = bounded(8);
        let outlets = Outlets { ended, garbage };
        let (e, _rx, _garbage) = Engine::new();
        let mut g = e.graph.lock().unwrap();
        for id in [1, 2] {
            let v = Voice { id, rate: g.rate, data: Arc::new(vec![vec![0.0; 1]]), start: 0, pos: 0, end: 1, looped: None, stop_at: None, chain: vec![], steps: vec![], filt: vec![] };
            g.apply(Cmd::Start(Box::new(v)), 0, &outlets);
        }
        let mut out = [0.0; 8];
        g.render(&mut out, 1, 0, &outlets);
        assert_eq!(taken.try_iter().collect::<Vec<u64>>(), vec![1]);
        g.render(&mut out, 1, 8, &outlets);
        assert_eq!(taken.try_iter().collect::<Vec<u64>>(), vec![2]);
    }

    #[test]
    fn a_new_device_rate_keeps_the_clock_and_ends_what_plays() {
        let (e, rx, _garbage) = Engine::new();
        e.play(3, &Sound::new(vec![vec![0.0; 96_000]], 48_000), 0.0, 0.0, None, None, vec![]).unwrap();
        pump(&e, &rx, 48_000);
        assert_eq!(e.now(), 1.0);
        e.set_rate(&mut e.graph.lock().unwrap(), 44_100);
        assert_eq!((e.rate(), e.now()), (44_100, 1.0));
        assert_eq!(e.take_ended(), vec![3]);
        // A sound made ready for the old rate that arrives after the change ends at once.
        let old = Sound::new(vec![vec![0.0; 10]], 44_100);
        e.play(4, &old, 0.0, 0.0, None, None, vec![]).unwrap();
        e.set_rate(&mut e.graph.lock().unwrap(), 48_000);
        pump(&e, &rx, 16);
        assert_eq!(e.take_ended(), vec![4]);
    }
}
