//! The Arranger's playback: lanes of clips played on the engine clock, one pass after another
//! when it loops. The page sends the arrangement whenever it changes; passes not yet begun use
//! the new one, and lane levels change at once.

use crate::engine::{Engine, Ev, Link, Sound};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const LOOKAHEAD: f64 = 0.25;
const WAVE_BINS: usize = 400;
const FADE_POINTS: usize = 33;

#[derive(Deserialize, Clone)]
pub struct Clip {
    pub path: String,
    pub at: f64,
    #[serde(default, rename = "in")]
    pub fade_in: f64,
    #[serde(default, rename = "out")]
    pub fade_out: f64,
}

#[derive(Deserialize, Clone)]
pub struct Lane {
    /// Linear, with mute and solo already applied.
    pub gain: f32,
    pub clips: Vec<Clip>,
}

#[derive(Deserialize, Clone)]
pub struct Arrangement {
    pub lanes: Vec<Lane>,
    pub length: f64,
    #[serde(rename = "loop")]
    pub looped: bool,
}

struct Pass {
    t: f64,
    from: f64,
    voices: Vec<u64>,
    nodes: Vec<u64>,
}

struct Playing {
    a: Arrangement,
    lane_nodes: Vec<u64>,
    passes: Vec<Pass>,
    next: f64,
    end: f64,
    changed: bool,
}

#[derive(Default)]
struct State {
    sounds: HashMap<String, Arc<Sound>>,
    /// The arrangement as the page keeps and stores it: lanes with a name, gain in dB, mute,
    /// solo and clips ({ path, at, in, out }), and whether it loops.
    plain: Value,
    playing: Option<Playing>,
    run: u64,
    changes: u64,
}

pub struct Arr {
    st: Arc<Mutex<State>>,
    engine: Arc<Engine>,
}

fn fade_curve(out: bool) -> Vec<f32> {
    let mut v: Vec<f32> = (0..FADE_POINTS).map(|i| (i as f64 / (FADE_POINTS - 1) as f64 * std::f64::consts::FRAC_PI_2).sin() as f32).collect();
    if out {
        v.reverse();
    }
    v
}

impl Arr {
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { st: Arc::new(Mutex::new(State::default())), engine }
    }

    /// Decodes a clip's file once and keeps it while the Arranger uses it. Returns its length and
    /// the loudest sample of each of WAVE_BINS stretches, for drawing.
    pub fn load(&self, path: &str) -> Result<Value, String> {
        let held = self.st.lock().unwrap().sounds.get(path).cloned();
        let s = match held {
            Some(s) => s,
            None => {
                let d = athar_core::decode::decode(std::path::Path::new(path)).map_err(|e| e.to_string())?;
                let s = Sound::new(d.channels, d.sr);
                self.st.lock().unwrap().sounds.insert(path.to_string(), s.clone());
                s
            }
        };
        let n = s.frames();
        let bins = WAVE_BINS.min(n).max(1);
        let mut peaks = vec![0.0f32; bins];
        for d in s.channels.iter().take(2) {
            for (k, p) in peaks.iter_mut().enumerate() {
                for x in &d[k * n / bins..(k + 1) * n / bins] {
                    *p = p.max(x.abs());
                }
            }
        }
        Ok(json!({"duration": n as f64 / s.sr as f64, "sr": s.sr, "channels": s.channels.len(), "peaks": peaks}))
    }

    /// The arrangement as stored; while playing, lane levels follow at once and the rest from
    /// the next pass.
    /// Returns what the page shows of it: whether each lane is heard, how many heard clips have
    /// a sound to mix, and the name of the first lane heard.
    pub fn set(&self, plain: Value) -> Value {
        self.st.lock().unwrap().plain = plain;
        let a = self.plan();
        let st = self.st.lock().unwrap();
        let lanes = st.plain["lanes"].as_array().cloned().unwrap_or_default();
        let heard: Vec<bool> = lanes.iter().map(|l| Self::audible(l, &lanes)).collect();
        let clips: usize = a.lanes.iter().filter(|l| l.gain > 0.0).map(|l| l.clips.len()).sum();
        let first = lanes.iter().zip(&a.lanes).find(|(_, l)| l.gain > 0.0 && !l.clips.is_empty()).and_then(|(l, _)| l["name"].as_str().map(String::from));
        drop(st);
        self.update(a);
        json!({"heard": heard, "clips": clips, "first": first})
    }

    /// Whether a lane sounds: not muted, and soloed when any lane is.
    fn audible(lane: &Value, lanes: &[Value]) -> bool {
        let solo = lanes.iter().any(|l| l["solo"].as_bool().unwrap_or(false));
        !lane["mute"].as_bool().unwrap_or(false) && (!solo || lane["solo"].as_bool().unwrap_or(false))
    }

    /// What plays: each lane's level, its clips whose sounds are loaded, and one pass's length
    /// (where the last clip ends).
    fn plan(&self) -> Arrangement {
        let st = self.st.lock().unwrap();
        let lanes = st.plain["lanes"].as_array().cloned().unwrap_or_default();
        let mut length: f64 = 0.0;
        let lanes = lanes
            .iter()
            .map(|lane| {
                let gain = if Self::audible(lane, &lanes) { 10f64.powf(lane["gain"].as_f64().unwrap_or(0.0) / 20.0) as f32 } else { 0.0 };
                let clips = lane["clips"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| {
                        let path = c["path"].as_str()?.to_string();
                        let s = st.sounds.get(&path)?;
                        let at = c["at"].as_f64().unwrap_or(0.0);
                        length = length.max(at + s.frames() as f64 / s.sr as f64);
                        Some(Clip { path, at, fade_in: c["in"].as_f64().unwrap_or(0.0), fade_out: c["out"].as_f64().unwrap_or(0.0) })
                    })
                    .collect();
                Lane { gain, clips }
            })
            .collect();
        Arrangement { lanes, length, looped: st.plain["loop"].as_bool().unwrap_or(true) }
    }

    /// The clips that are heard, each with its sound and its lane's level, for a mixdown.
    pub fn heard(&self) -> Vec<(Clip, Arc<Sound>, f32)> {
        let a = self.plan();
        let st = self.st.lock().unwrap();
        a.lanes.into_iter().filter(|l| l.gain > 0.0).flat_map(|l| l.clips.into_iter().map(move |c| (c, l.gain))).filter_map(|(c, g)| Some((c.clone(), st.sounds.get(&c.path)?.clone(), g))).collect()
    }

    /// Lets go of every file no clip uses any more.
    pub fn keep_only(&self, paths: &[String]) {
        self.st.lock().unwrap().sounds.retain(|p, _| paths.contains(p));
    }

    fn schedule(engine: &Engine, sounds: &HashMap<String, Arc<Sound>>, p: &mut Playing, t: f64, from: f64) {
        while p.lane_nodes.len() < p.a.lanes.len() {
            let id = engine.id();
            engine.set_gain(id, p.a.lanes[p.lane_nodes.len()].gain, vec![]);
            p.lane_nodes.push(id);
        }
        let mut pass = Pass { t, from, voices: vec![], nodes: vec![] };
        for (li, lane) in p.a.lanes.iter().enumerate() {
            for c in &lane.clips {
                let Some(s) = sounds.get(&c.path) else { continue };
                let dur = s.frames() as f64 / s.sr as f64;
                if c.at + dur <= from {
                    continue;
                }
                let mut chain = vec![];
                let t0 = t + c.at - from;
                if c.fade_in > 0.0 || c.fade_out > 0.0 {
                    let node = engine.id();
                    let mut ev = vec![];
                    if c.fade_in > 0.0 && t0 >= t {
                        ev.push(Ev::Curve { t: t0, dur: c.fade_in, values: fade_curve(false) });
                    }
                    let t_out = t0 + dur - c.fade_out;
                    if c.fade_out > 0.0 && t_out >= t {
                        ev.push(Ev::Curve { t: t_out, dur: c.fade_out, values: fade_curve(true) });
                    }
                    engine.set_gain(node, 1.0, ev);
                    chain.push(Link::Gain(node));
                    pass.nodes.push(node);
                }
                chain.push(Link::Gain(p.lane_nodes[li]));
                let id = engine.id();
                let (when, offset) = if c.at >= from { (t0, 0.0) } else { (t, from - c.at) };
                engine.play(id, s, when, offset, None, None, chain);
                pass.voices.push(id);
            }
        }
        p.passes.push(pass);
        p.changed = true;
        if p.passes.len() > 4 {
            let old = p.passes.remove(0);
            engine.drop_nodes(old.nodes);
        }
        p.next = t + p.a.length - from;
        p.end = p.next;
    }

    /// Starts playing the stored arrangement from `from` seconds in; returns the engine time it
    /// starts at.
    pub fn play(&self, from: f64) -> f64 {
        self.stop();
        let a = self.plan();
        let e = &self.engine;
        let t = e.now() + 0.05;
        let run = {
            let mut st = self.st.lock().unwrap();
            let mut p = Playing { a, lane_nodes: vec![], passes: vec![], next: 0.0, end: 0.0, changed: false };
            let sounds = st.sounds.clone();
            Self::schedule(e, &sounds, &mut p, t, from.max(0.0));
            st.playing = Some(p);
            st.run += 1;
            st.run
        };
        let (st, engine) = (self.st.clone(), self.engine.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(40));
            let mut g = st.lock().unwrap();
            if g.run != run {
                return;
            }
            let now = engine.now();
            let sounds = g.sounds.clone();
            let Some(p) = g.playing.as_mut() else { return };
            if p.a.looped && p.a.length > 0.0 {
                // A pass that should already have begun, after a stall, begins now instead.
                if now > p.next - LOOKAHEAD {
                    let at = p.next.max(now + 0.05);
                    Self::schedule(&engine, &sounds, p, at, 0.0);
                }
            } else if now > p.end {
                let p = g.playing.take().unwrap();
                g.changes += 1;
                Self::release(&engine, p);
                return;
            }
        });
        t
    }

    fn release(engine: &Engine, p: Playing) {
        for pass in p.passes {
            for v in pass.voices {
                engine.stop(v, 0.0);
            }
            engine.drop_nodes(pass.nodes);
        }
        engine.drop_nodes(p.lane_nodes);
    }

    fn update(&self, a: Arrangement) {
        let mut st = self.st.lock().unwrap();
        let Some(p) = st.playing.as_mut() else { return };
        let now = self.engine.now();
        for (i, id) in p.lane_nodes.iter().enumerate() {
            let g = a.lanes.get(i).map_or(0.0, |l| l.gain);
            self.engine.set_gain(*id, self.engine.gain_now(*id).unwrap_or(g), vec![Ev::Target { t: now, v: g, tau: 0.01 }]);
        }
        let first = p.passes.last().map_or(0.0, |x| x.t - x.from);
        if !a.looped {
            p.end = first + a.length;
        }
        p.next = first + a.length;
        p.a = a;
    }

    pub fn stop(&self) {
        let mut st = self.st.lock().unwrap();
        st.run += 1;
        if let Some(p) = st.playing.take() {
            st.changes += 1;
            Self::release(&self.engine, p);
        }
    }

    /// The state, when it changed since `seen`: a pass began, or playing stopped.
    pub fn changed(&self, seen: &mut u64) -> Option<Value> {
        let mut st = self.st.lock().unwrap();
        if let Some(p) = st.playing.as_mut() {
            if std::mem::take(&mut p.changed) {
                st.changes += 1;
            }
        }
        if st.changes == *seen {
            return None;
        }
        *seen = st.changes;
        drop(st);
        Some(self.state())
    }

    /// Whether it plays, and when each recent pass began (engine time) and from where.
    pub fn state(&self) -> Value {
        let st = self.st.lock().unwrap();
        match &st.playing {
            Some(p) => json!({"playing": true, "passes": p.passes.iter().map(|x| [x.t, x.from]).collect::<Vec<_>>()}),
            None => json!({"playing": false, "passes": []}),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(dir: &std::path::Path, name: &str, seconds: f64) -> String {
        let path = dir.join(name);
        let spec = hound::WavSpec { channels: 1, sample_rate: 8000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..(8000.0 * seconds) as usize {
            w.write_sample(((i as f64 * 0.1).sin() * 8000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        path.to_string_lossy().into_owned()
    }

    fn arr() -> (Arr, String, String, tempdir_lite::Dir) {
        let dir = tempdir_lite::Dir::new();
        let a = tone(dir.path(), "a.wav", 1.0);
        let b = tone(dir.path(), "b.wav", 0.5);
        let arr = Arr::new(Engine::start());
        arr.load(&a).unwrap();
        arr.load(&b).unwrap();
        (arr, a, b, dir)
    }

    #[test]
    fn a_pass_ends_where_the_last_clip_does() {
        let (arr, a, b, _d) = arr();
        arr.set(json!({"loop": true, "lanes": [{"gain": 0, "clips": [{"path": a, "at": 0}]}, {"gain": -6, "clips": [{"path": b, "at": 2.0}]}]}));
        let plan = arr.plan();
        assert!((plan.length - 2.5).abs() < 1e-9);
        assert!((plan.lanes[1].gain - 0.501).abs() < 0.001);
    }

    #[test]
    fn mute_and_solo_decide_what_is_heard() {
        let (arr, a, b, _d) = arr();
        let lanes = |mute: bool, solo: bool| json!({"loop": false, "lanes": [{"gain": 0, "mute": mute, "clips": [{"path": a, "at": 0}]}, {"gain": 0, "solo": solo, "clips": [{"path": b, "at": 0}]}]});
        arr.set(lanes(false, false));
        assert_eq!(arr.heard().len(), 2);
        assert_eq!(arr.set(lanes(true, false))["heard"], json!([false, true]));
        assert_eq!(arr.heard().iter().map(|h| h.0.path.clone()).collect::<Vec<_>>(), vec![b.clone()]);
        arr.set(lanes(false, true));
        assert_eq!(arr.heard().iter().map(|h| h.0.path.clone()).collect::<Vec<_>>(), vec![b.clone()]);
    }

    #[test]
    fn clips_without_a_loaded_sound_are_left_out() {
        let (arr, a, _b, _d) = arr();
        arr.set(json!({"loop": true, "lanes": [{"gain": 0, "clips": [{"path": a, "at": 0}, {"path": "/nowhere.wav", "at": 5.0}]}]}));
        let plan = arr.plan();
        assert_eq!(plan.lanes[0].clips.len(), 1);
        assert!((plan.length - 1.0).abs() < 1e-9);
    }

    #[test]
    fn playing_reports_its_first_pass_and_stopping_ends_it() {
        let (arr, a, _b, _d) = arr();
        arr.set(json!({"loop": true, "lanes": [{"gain": 0, "clips": [{"path": a, "at": 0}]}]}));
        let t = arr.play(0.25);
        let mut seen = 0;
        let st = arr.changed(&mut seen).unwrap();
        assert_eq!(st["playing"], true);
        assert_eq!(st["passes"][0], json!([t, 0.25]));
        arr.stop();
        assert_eq!(arr.changed(&mut seen).unwrap()["playing"], false);
    }
}

#[cfg(test)]
mod tempdir_lite {
    /// A folder for one test's files, removed afterwards.
    pub struct Dir(std::path::PathBuf);
    impl Dir {
        pub fn new() -> Self {
            let p = std::env::temp_dir().join(format!("athar-test-{}-{:?}", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
