//! The editor's sound, held here rather than in the page: its samples, undo history, clipboard,
//! previews of effects and tools, analysis, waveform peaks, spectrogram tiles and playback.
//!
//! The page keeps what it draws and the regions; every change to the audio is a request here.
//! Sounds are immutable `Arc<Sound>`s: an edit makes a new one and the old one becomes an undo
//! step, and the engine plays them without a copy.

use crate::engine::{biquad, Engine, Ev, Link, Sound};
use crate::library::Library;
use athar_core::dsp;
use serde_json::{json, Value};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, OnceLock};

type Chs = Vec<Vec<f32>>;
type R<T> = Result<T, String>;

const UNDO_BYTES: usize = 3 << 29;
const VIEWS_KEPT: usize = 6;
const EQ_NODES: usize = 24;
const PYRAMID: usize = 256;

#[derive(Default)]
struct Play {
    voice: Option<u64>,
    fade: u64,
    eq: Vec<u64>,
}

/// A named span of the sound, in samples.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub a: usize,
    pub b: usize,
    #[serde(default)]
    pub name: String,
}

/// What the page shows that edits move: the selection and the cursor. The page owns them while
/// the pointer moves them, and sends them with every change so they go into the history.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ui {
    pub sel: Option<[usize; 2]>,
    pub cursor: usize,
}

/// One step of history: everything an undo puts back.
#[derive(Clone)]
struct Step {
    sound: Arc<Sound>,
    regions: Vec<Region>,
    ui: Ui,
}

#[derive(Default)]
struct State {
    cur: Option<Arc<Sound>>,
    version: u64,
    saved: u64,
    path: Option<String>,
    regions: Vec<Region>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    views: Vec<(String, Arc<Sound>, usize, Value)>,
    clip: Option<Arc<Sound>>,
    noise: Option<Value>,
    mono: Option<(usize, Arc<Vec<f32>>, f64)>,
    pyramids: Vec<(usize, Arc<Vec<(Vec<f32>, Vec<f32>)>>)>,
    play: Play,
}

pub struct Doc {
    st: Mutex<State>,
    engine: Arc<Engine>,
    lib: OnceLock<Arc<Library>>,
}

/// Regions after [a, b) became n samples long: edges after it shift with the audio; edges inside
/// it go through `inner`, which by default collapses them onto a.
pub fn shift_regions(regions: &[Region], a: usize, b: usize, n: usize, inner: &dyn Fn(usize) -> usize) -> Vec<Region> {
    let d = n as i64 - (b as i64 - a as i64);
    let moved = |p: usize| (p as i64 + d).max(0) as usize;
    let start = |p: usize| if p < a { p } else if p >= b { moved(p) } else { inner(p) };
    let end = |p: usize| if p <= a { p } else if p >= b { moved(p) } else { inner(p) };
    regions.iter().map(|r| Region { a: start(r.a), b: end(r.b), name: r.name.clone() }).filter(|r| r.b > r.a).collect()
}

fn crop_regions(regions: &[Region], a: usize, b: usize, len: usize) -> Vec<Region> {
    let r = shift_regions(regions, b, len, 0, &|_| b);
    shift_regions(&r, 0, a, 0, &|_| 0)
}

fn ptr(s: &Arc<Sound>) -> usize {
    Arc::as_ptr(s) as usize
}

fn num(p: &Value, k: &str) -> f64 {
    p[k].as_f64().unwrap_or(0.0)
}

fn idx(p: &Value, k: &str) -> usize {
    p[k].as_f64().unwrap_or(0.0).max(0.0) as usize
}

fn frames(c: &Chs) -> usize {
    c.first().map_or(0, Vec::len)
}

fn bytes(s: &Sound) -> usize {
    4 * s.channels.len() * s.frames()
}

fn op(name: &str, chs: Chs, sr: u32, p: Value) -> R<dsp::Out> {
    dsp::run(name, chs, sr, &p)
}

fn slice(c: &Chs, a: usize, b: usize) -> Chs {
    c.iter().map(|d| d[a.min(d.len())..b.min(d.len())].to_vec()).collect()
}

fn curve(i: usize, m: usize) -> f32 {
    ((i as f64 + 0.5) / m as f64 * std::f64::consts::FRAC_PI_2).sin().powi(2) as f32
}

/// Replaces [a, b) with `insert` (or removes it). With `smooth` samples of fade, a removal
/// crossfades the audio leading up to the cut into the audio leading up to its far side, and an
/// insertion dips briefly to silence at each end. As the page's splice did.
fn splice(chs: &Chs, a: usize, b: usize, insert: Option<&Chs>, smooth: usize) -> Chs {
    let len = frames(chs);
    let (a, b) = (a.min(len), b.min(len).max(a.min(len)));
    let n = insert.map_or(0, frames);
    let k = if smooth > 0 && a > 0 && b < len { smooth } else { 0 };
    chs.iter()
        .enumerate()
        .map(|(c, ch)| {
            let mut out = Vec::with_capacity(len - (b - a) + n);
            out.extend_from_slice(&ch[..a]);
            if let Some(ins) = insert {
                out.extend_from_slice(&ins[c.min(ins.len() - 1)]);
            }
            out.extend_from_slice(&ch[b..]);
            if k > 0 && insert.is_none() {
                let m = k.min(a).min(b - a);
                for i in 0..m {
                    let w = curve(i, m);
                    out[a - m + i] = ch[a - m + i] * (1.0 - w) + ch[b - m + i] * w;
                }
            } else if k > 0 {
                let m = (k >> 1).max(1).min(a).min(n >> 1).min(len - b);
                for i in 0..m {
                    let w = curve(i, m);
                    out[a - 1 - i] *= w;
                    out[a + i] *= w;
                    out[a + n - 1 - i] *= w;
                    out[a + n + i] *= w;
                }
            }
            out
        })
        .collect()
}

fn fade(chs: &mut Chs, a: usize, b: usize, fade_in: bool) {
    let n = b.saturating_sub(a);
    for d in chs.iter_mut() {
        for i in 0..n.min(d.len().saturating_sub(a)) {
            let t = i as f64 / n as f64 * std::f64::consts::FRAC_PI_2;
            d[a + i] *= (if fade_in { t.sin() } else { t.cos() }).powi(2) as f32;
        }
    }
}

fn scale(chs: &mut Chs, a: usize, b: usize, k: f64) {
    for d in chs.iter_mut() {
        let e = b.min(d.len());
        for x in &mut d[a.min(e)..e] {
            *x = (*x as f64 * k) as f32;
        }
    }
}

fn peak(chs: &Chs, a: usize, b: usize) -> f32 {
    chs.iter().flat_map(|d| d[a.min(d.len())..b.min(d.len())].iter()).fold(0.0f32, |m, x| m.max(x.abs()))
}

fn mono(chs: &Chs) -> Vec<f32> {
    dsp::mix_mono(chs)
}

/// The nearest point within 10 ms where the sum of all channels crosses zero.
fn zero_cross(chs: &Chs, sr: u32, s: usize) -> usize {
    let len = frames(chs) as i64;
    let w = (sr as f64 * 0.01).round() as i64;
    let at = |i: i64| chs.iter().map(|d| d[i as usize] as f64).sum::<f64>();
    let s = s as i64;
    for k in 0..w {
        for i in [s + k, s - k] {
            if i > 0 && i < len && (at(i - 1) <= 0.0) != (at(i) <= 0.0) {
                return i as usize;
            }
        }
    }
    s.max(0) as usize
}

/// The span from the first to the last sample above `th`, padded by 5 ms.
fn sound_bounds(chs: &Chs, sr: u32, th: f32) -> Option<(usize, usize)> {
    let m = mono(chs);
    let (mut a, mut b) = (0, m.len());
    while a < b && m[a].abs() < th {
        a += 1;
    }
    while b > a && m[b - 1].abs() < th {
        b -= 1;
    }
    if a >= b {
        return None;
    }
    let pad = (sr as f64 * 0.005).round() as usize;
    Some((a.saturating_sub(pad), (b + pad).min(m.len())))
}

fn remove_dc(chs: &mut Chs) {
    for d in chs.iter_mut() {
        let m = d.iter().map(|x| *x as f64).sum::<f64>() / d.len().max(1) as f64;
        for x in d.iter_mut() {
            *x = (*x as f64 - m) as f32;
        }
    }
}

fn normalize_all(chs: &mut Chs, target_db: f64) {
    let p = peak(chs, 0, frames(chs));
    if p > 0.0 {
        let k = 10f64.powf(target_db / 20.0) / p as f64;
        let n = frames(chs);
        scale(chs, 0, n, k);
    }
}

/// The EQ bands applied in order, as WebAudio's offline renderer runs a chain of BiquadFilterNodes.
pub fn render_eq(chs: &Chs, sr: u32, bands: &[Value]) -> Chs {
    let coefs: Vec<[f32; 5]> = bands.iter().filter(|b| b["on"].as_bool().unwrap_or(false)).filter_map(|b| band_coefs(b, sr as f64)).collect();
    chs.iter()
        .map(|d| {
            let mut y = d.clone();
            for c in &coefs {
                let (mut x1, mut x2, mut y1, mut y2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
                for v in y.iter_mut() {
                    let x = *v;
                    let o = c[0] * x + c[1] * x1 + c[2] * x2 - c[3] * y1 - c[4] * y2;
                    x2 = x1;
                    x1 = x;
                    y2 = y1;
                    y1 = o;
                    *v = o;
                }
            }
            y
        })
        .collect()
}

const BAND_NODES: [(&str, &str); 6] = [("bell", "peaking"), ("lowshelf", "lowshelf"), ("highshelf", "highshelf"), ("lowcut", "highpass"), ("highcut", "lowpass"), ("notch", "notch")];

fn band_coefs(b: &Value, rate: f64) -> Option<[f32; 5]> {
    let on = b["on"].as_bool().unwrap_or(false);
    let kind = if on { BAND_NODES.iter().find(|(k, _)| Some(*k) == b["type"].as_str()).map_or("peaking", |n| n.1) } else { "peaking" };
    biquad(kind, num(b, "f"), num(b, "q"), if on { num(b, "g") } else { 0.0 }, rate)
}

/// The magnitude in dB of the EQ chain at each frequency, for drawing its curve.
pub fn eq_response(bands: &[Value], rate: f64, freqs: &[f64]) -> Vec<Vec<f32>> {
    bands
        .iter()
        .map(|b| {
            let c = band_coefs(b, rate).unwrap_or([1.0, 0.0, 0.0, 0.0, 0.0]);
            freqs
                .iter()
                .map(|f| {
                    let w = 2.0 * std::f64::consts::PI * f / rate;
                    let (c1, s1, c2, s2) = (w.cos(), -w.sin(), (2.0 * w).cos(), -(2.0 * w).sin());
                    let c: Vec<f64> = c.iter().map(|x| *x as f64).collect();
                    let (nr, ni) = (c[0] + c[1] * c1 + c[2] * c2, c[1] * s1 + c[2] * s2);
                    let (dr, di) = (1.0 + c[3] * c1 + c[4] * c2, c[3] * s1 + c[4] * s2);
                    (20.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).sqrt().max(1e-12).log10()) as f32
                })
                .collect()
        })
        .collect()
}

impl Doc {
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { st: Mutex::new(State::default()), engine, lib: OnceLock::new() }
    }

    /// Where regions are kept with their files.
    pub fn attach(&self, lib: Arc<Library>) {
        let _ = self.lib.set(lib);
    }

    fn cur(&self) -> R<Arc<Sound>> {
        self.st.lock().unwrap().cur.clone().ok_or_else(|| "No sound is open in the editor".into())
    }

    /// The sound to read from: the editor's own, or a preview made by `view`.
    fn src(&self, key: Option<&str>) -> R<Arc<Sound>> {
        let st = self.st.lock().unwrap();
        match key.filter(|k| !k.is_empty() && *k != "doc") {
            Some(k) => st.views.iter().find(|v| v.0 == k).map(|v| v.1.clone()).ok_or_else(|| "That preview is no longer held".into()),
            None => st.cur.clone().ok_or_else(|| "No sound is open in the editor".into()),
        }
    }

    /// The whole state the page shows. `ui` is the selection and cursor after the change.
    fn info(st: &State, ui: &Ui) -> Value {
        let c = st.cur.as_ref();
        json!({
            "sr": c.map_or(0, |s| s.sr),
            "channels": c.map_or(0, |s| s.channels.len()),
            "length": c.map_or(0, |s| s.frames()),
            "version": st.version,
            "dirty": st.version != st.saved,
            "path": st.path,
            "undo": st.undo.len(),
            "redo": st.redo.len(),
            "regions": st.regions,
            "sel": ui.sel,
            "cursor": ui.cursor,
        })
    }

    /// Opens a sound; a library file (`path`) brings back the regions kept with it.
    pub fn open(&self, sound: Arc<Sound>, path: Option<String>) -> Value {
        self.stop();
        let sr = sound.sr as f64;
        let len = sound.frames();
        let regions: Vec<Region> = match (&path, self.lib.get()) {
            (Some(p), Some(lib)) => lib
                .regions_get(p)
                .as_array()
                .into_iter()
                .flatten()
                .map(|r| Region { a: (num(r, "start") * sr).round() as usize, b: ((num(r, "end") * sr).round() as usize).min(len), name: r["name"].as_str().unwrap_or("").to_string() })
                .filter(|r| r.b > r.a)
                .collect(),
            _ => vec![],
        };
        let mut st = self.st.lock().unwrap();
        let version = st.version + 1;
        *st = State { play: std::mem::take(&mut st.play), clip: st.clip.take(), version, saved: version, path, regions, ..State::default() };
        st.cur = Some(sound);
        Self::info(&st, &Ui::default())
    }

    /// The sound was saved as `path`: it is now that file, unchanged since.
    pub fn saved(&self, path: String, version: u64) -> Value {
        let mut st = self.st.lock().unwrap();
        if st.version == version {
            st.saved = version;
        }
        st.path = Some(path);
        self.persist(&st);
        Self::info(&st, &Ui::default())
    }

    /// Regions are kept with the file whenever what is shown is what is on disk.
    fn persist(&self, st: &State) {
        let (Some(path), Some(lib), Some(cur)) = (&st.path, self.lib.get(), &st.cur) else { return };
        if st.version != st.saved {
            return;
        }
        let sr = cur.sr as f64;
        let list: Vec<Value> = st.regions.iter().map(|r| json!({"start": r.a as f64 / sr, "end": r.b as f64 / sr, "name": r.name})).collect();
        let _ = lib.regions_set(path, &Value::Array(list));
    }

    /// Makes a step of history from the state before a change.
    fn record(st: &mut State, ui: &Ui) {
        if let Some(cur) = st.cur.clone() {
            st.undo.push(Step { sound: cur, regions: st.regions.clone(), ui: ui.clone() });
        }
        st.redo.clear();
        // Old sounds are what history costs; the oldest steps go once it holds too much.
        loop {
            let mut seen = std::collections::HashSet::new();
            let cur = st.cur.as_ref().map(ptr);
            let total: usize = st.undo.iter().chain(st.redo.iter()).filter(|s| Some(ptr(&s.sound)) != cur && seen.insert(ptr(&s.sound))).map(|s| bytes(&s.sound)).sum();
            if total <= UNDO_BYTES || st.undo.is_empty() {
                break;
            }
            st.undo.remove(0);
        }
    }

    fn set_sound(st: &mut State, s: Arc<Sound>) {
        if st.cur.as_ref().map(ptr) != Some(ptr(&s)) {
            st.version += 1;
            st.views.clear();
        }
        st.cur = Some(s);
    }

    pub fn undo(&self, redo: bool, ui: Ui) -> R<Value> {
        self.stop();
        let mut st = self.st.lock().unwrap();
        let from = if redo { st.redo.pop() } else { st.undo.pop() };
        let Some(step) = from else { return Err("Nothing to undo".into()) };
        let here = Step { sound: st.cur.clone().unwrap_or_else(|| step.sound.clone()), regions: std::mem::take(&mut st.regions), ui };
        if redo { st.undo.push(here) } else { st.redo.push(here) }
        Self::set_sound(&mut st, step.sound);
        st.regions = step.regions;
        self.persist(&st);
        Ok(Self::info(&st, &step.ui))
    }

    /// A change to the regions alone, as one step of history.
    pub fn regions(&self, p: &Value, ui: Ui) -> R<Value> {
        let cur = self.cur()?;
        let (len, sr) = (cur.frames(), cur.sr);
        let mut st = self.st.lock().unwrap();
        let mut regions = st.regions.clone();
        let mut ui2 = ui.clone();
        let drop_sel = |ui: &mut Ui, r: &Region| {
            if ui.sel == Some([r.a, r.b]) {
                ui.sel = None;
            }
        };
        match p["op"].as_str().unwrap_or("") {
            "add" => {
                let (a, b) = (idx(p, "a").min(len), idx(p, "b").min(len));
                if b <= a {
                    return Err("Select a region first".into());
                }
                regions.push(Region { a, b, name: String::new() });
                regions.sort_by_key(|r| r.a);
            }
            "remove" => {
                let i = idx(p, "i");
                if i < regions.len() {
                    let gone = regions.remove(i);
                    drop_sel(&mut ui2, &gone);
                }
            }
            "rename" => {
                if let Some(r) = regions.get_mut(idx(p, "i")) {
                    r.name = p["name"].as_str().unwrap_or("").trim().chars().take(80).collect();
                }
            }
            "clear" => {
                for r in &regions {
                    drop_sel(&mut ui2, r);
                }
                regions.clear();
            }
            "grid" => {
                let (a, b, step) = (idx(p, "a").min(len), idx(p, "b").min(len), num(p, "step"));
                if !(step > 0.0) {
                    return Err("Enter the tempo first".into());
                }
                if (b - a) as f64 / step > 512.0 {
                    return Err("That would make more than 512 slices".into());
                }
                let mut found = vec![];
                let mut x = a as f64;
                while x + step * 0.5 <= b as f64 {
                    found.push(Region { a: x.round() as usize, b: ((x + step).round() as usize).min(b), name: String::new() });
                    x += step;
                }
                if found.is_empty() {
                    return Err("The sound is shorter than one slice".into());
                }
                regions = found;
            }
            "auto" => {
                drop(st);
                let found = self.analyse(&json!({"kind": "regions", "th": num(p, "th"), "gap_ms": num(p, "gap_ms")}))?;
                st = self.st.lock().unwrap();
                regions = found.as_array().into_iter().flatten().filter_map(|r| Some(Region { a: r[0].as_u64()? as usize, b: r[1].as_u64()? as usize, name: String::new() })).collect();
                if regions.is_empty() {
                    return Err("No sounds above the threshold".into());
                }
            }
            "hits" => {
                let (a, b) = (idx(p, "a").min(len), idx(p, "b").min(len));
                let chs = &cur.channels;
                let pos = op("onsets", vec![mono(&slice(chs, a, b))], sr, json!({"sensitivity": num(p, "sensitivity")}))?.info;
                let mut pts: Vec<usize> = pos["positions"].as_array().into_iter().flatten().filter_map(|x| x.as_u64()).map(|x| zero_cross(chs, sr, a + x as usize)).collect();
                pts.sort();
                pts.dedup();
                // A hit that is already sounding at the very start has no rise to detect; it gets the first slice.
                let head = b.min(a + (sr as f64 * 0.01).round() as usize);
                let th = num(p, "th") as f32;
                if (pts.first().copied().unwrap_or(b) - a) as f64 > sr as f64 * 0.03 && chs.iter().any(|d| d[a..head].iter().any(|v| v.abs() > th)) {
                    pts.insert(0, a);
                }
                let found: Vec<Region> = pts.iter().enumerate().map(|(i, &x)| Region { a: x, b: pts.get(i + 1).copied().unwrap_or(b), name: String::new() }).filter(|r| (r.b - r.a) as f64 >= sr as f64 * 0.01).collect();
                if found.is_empty() {
                    return Err("No hits found. Raise the sensitivity.".into());
                }
                regions = found;
            }
            o => return Err(format!("Unknown change to regions {o}")),
        }
        Self::record(&mut st, &ui);
        st.regions = regions;
        self.persist(&st);
        Ok(Self::info(&st, &ui2))
    }

    /// The edit `p` applied to `chs`. Returns the new samples and what the page needs to know:
    /// for edits that change the length, `n` is the new length of the edited span.
    fn edit_chs(&self, chs: &Chs, sr: u32, p: &Value, clip: Option<&Arc<Sound>>, noise: Option<&Value>) -> R<(Chs, Value)> {
        let len = frames(chs);
        let (a, b) = (idx(p, "a").min(len), if p["b"].is_null() { len } else { idx(p, "b").min(len) });
        let smooth = ((sr as f64) * num(p, "smooth_ms") / 1000.0).round() as usize;
        let kind = p["kind"].as_str().unwrap_or("");
        let mut out = chs.clone();
        let mut extra = json!({});
        match kind {
            "trim" => out = splice(&splice(chs, b, len, None, 0), 0, a, None, 0),
            "delete" => out = splice(chs, a, b, None, smooth),
            "paste" => {
                let clip = clip.ok_or("Nothing copied")?;
                let mut ins: Chs = if clip.sr == sr { clip.channels.clone() } else { op("resample", clip.channels.clone(), clip.sr, json!({"from": clip.sr, "to": sr}))?.channels };
                if ins.len() > chs.len() {
                    ins = vec![mono(&ins)];
                }
                extra = json!({"n": frames(&ins)});
                out = splice(chs, a, b, Some(&ins), smooth);
            }
            "silence" => out.iter_mut().for_each(|d| d[a..b].fill(0.0)),
            "fade" => {
                for s in p["spans"].as_array().into_iter().flatten() {
                    fade(&mut out, idx(s, "a"), idx(s, "b"), s["fadeIn"].as_bool().unwrap_or(true));
                }
            }
            "normalize" => {
                let pk = peak(chs, a, b);
                if pk == 0.0 {
                    return Err("Selection is silent".into());
                }
                scale(&mut out, a, b, 10f64.powf(num(p, "db") / 20.0) / pk as f64);
            }
            "gain" => scale(&mut out, a, b, 10f64.powf(num(p, "db") / 20.0)),
            "reverse" => out.iter_mut().for_each(|d| d[a..b].reverse()),
            "mono" => out = vec![mono(chs)],
            "respeed" => {
                let ratio = num(p, "ratio");
                let seg = op("resample", slice(chs, a, b), sr, json!({"from": (ratio * 100000.0).round(), "to": 100000}))?.channels;
                extra = json!({"n": frames(&seg)});
                out = splice(chs, a, b, Some(&seg), 0);
            }
            "loudnorm" => {
                let m = op("measure", slice(chs, a, b), sr, json!({}))?.info;
                let lufs = m["lufs"].as_f64().filter(|x| x.is_finite()).ok_or("Selection is silent")?;
                let gain = num(p, "target") - lufs;
                scale(&mut out, a, b, 10f64.powf(gain / 20.0));
                extra = json!({"over": m["truePeak"].as_f64().unwrap_or(-120.0) + gain});
            }
            "stretch" => {
                let seg = op("stretchPitch", slice(chs, a, b), sr, json!({"length": num(p, "length"), "semitones": num(p, "semitones")}))?.channels;
                extra = json!({"n": frames(&seg)});
                out = splice(chs, a, b, Some(&seg), 0);
            }
            "loopxfade" => {
                let seg = dsp::loop_crossfade(&slice(chs, a, b), idx(p, "n"));
                extra = json!({"n": frames(&seg)});
                out = splice(chs, a, b, Some(&seg), 0);
            }
            "declick" => {
                let r = op("declick", slice(chs, a, b), sr, json!({"sensitivity": num(p, "sensitivity")}))?;
                for (d, s) in out.iter_mut().zip(&r.channels) {
                    d[a..a + s.len().min(b - a)].copy_from_slice(&s[..s.len().min(b - a)]);
                }
                extra = r.info;
            }
            "trimsilence" => {
                let (x, y) = sound_bounds(chs, sr, num(p, "th") as f32).ok_or("Nothing above the threshold")?;
                if x == 0 && y == len {
                    return Err("Nothing is cut at this level. Raise \"Silence below\" until the ends are marked.".into());
                }
                out = splice(&splice(chs, y, len, None, 0), 0, x, None, 0);
                extra = json!({"cut": [x, y]});
            }
            "prep" => {
                remove_dc(&mut out);
                if let Some((x, y)) = sound_bounds(&out, sr, num(p, "th") as f32) {
                    let n = frames(&out);
                    out = splice(&splice(&out, y, n, None, 0), 0, x, None, 0);
                    extra = json!({"cut": [x, y]});
                }
                normalize_all(&mut out, num(p, "db"));
                let n = frames(&out);
                let fi = ((sr as f64) * num(p, "fade_in_ms") / 1000.0).round() as usize;
                let fo = ((sr as f64) * num(p, "fade_out_ms") / 1000.0).round() as usize;
                fade(&mut out, 0, fi.min(n), true);
                fade(&mut out, n.saturating_sub(fo), n, false);
            }
            "fx" => {
                let tab = p["tab"].as_str().unwrap_or("");
                let part = slice(chs, a, b);
                let r = match tab {
                    "eq" => dsp::Out { channels: render_eq(&part, sr, p["bands"].as_array().map(Vec::as_slice).unwrap_or(&[])), info: Value::Null, bytes: None },
                    "nr" => {
                        let profile = match noise {
                            Some(n) => n.clone(),
                            None => json!(dsp::estimate_profile(&part)),
                        };
                        op("denoise", part, sr, json!({"p": p["p"], "profile": profile}))?
                    }
                    "comp" => op("compress", part, sr, json!({"p": p["p"]}))?,
                    "max" => op("maximize", part, sr, json!({"p": p["p"]}))?,
                    "sat" => op("saturate", part, sr, json!({"p": p["p"]}))?,
                    "shape" => op("shape", part, sr, json!({"p": p["p"]}))?,
                    "lofi" => op("lofi", part, sr, json!({"p": p["p"]}))?,
                    _ => return Err(format!("Unknown effect {tab}")),
                };
                for (d, s) in out.iter_mut().zip(&r.channels) {
                    let n = s.len().min(b - a);
                    d[a..a + n].copy_from_slice(&s[..n]);
                }
                extra = json!({"info": r.info.get("info").cloned().unwrap_or(Value::Null)});
            }
            _ => return Err(format!("Unknown edit {kind}")),
        }
        Ok((out, extra))
    }

    pub fn edit(&self, p: &Value) -> R<Value> {
        self.stop();
        let cur = self.cur()?;
        let ui: Ui = serde_json::from_value(p["ui"].clone()).unwrap_or_default();
        let (clip, noise) = {
            let st = self.st.lock().unwrap();
            (st.clip.clone(), st.noise.clone())
        };
        let kind = p["kind"].as_str().unwrap_or("");
        let len = cur.frames();
        let (a, b) = (idx(p, "a").min(len), if p["b"].is_null() { len } else { idx(p, "b").min(len) });
        if kind == "copy" || kind == "cut" {
            let c = Sound::new(slice(&cur.channels, a, b), cur.sr);
            self.st.lock().unwrap().clip = Some(c);
            if kind == "copy" {
                let st = self.st.lock().unwrap();
                return Ok(Self::info(&st, &ui));
            }
        }
        let p = if kind == "cut" {
            let mut q = p.clone();
            q["kind"] = json!("delete");
            q
        } else {
            p.clone()
        };
        let kind = p["kind"].as_str().unwrap_or("");
        let (next, extra) = self.edit_chs(&cur.channels, cur.sr, &p, clip.as_ref(), if p["noise"].as_str() == Some("print") { noise.as_ref() } else { None })?;
        let mut st = self.st.lock().unwrap();
        // Where the selection, cursor and regions go with the audio.
        let had_sel = ui.sel.is_some_and(|s| s[1] > s[0]);
        let n = extra["n"].as_u64().map(|x| x as usize);
        let mut after = ui.clone();
        let regions = match kind {
            "trim" | "trimsilence" | "prep" => {
                let (x, y) = match kind {
                    "trim" => (a, b),
                    _ => extra["cut"].as_array().map_or((0, len), |c| (c[0].as_u64().unwrap_or(0) as usize, c[1].as_u64().unwrap_or(len as u64) as usize)),
                };
                after = Ui { sel: None, cursor: 0 };
                crop_regions(&st.regions, x, y, len)
            }
            "delete" => {
                after = Ui { sel: None, cursor: a };
                shift_regions(&st.regions, a, b, 0, &|_| a)
            }
            "paste" => {
                let n = n.unwrap_or(0);
                after.sel = Some([a, a + n]);
                shift_regions(&st.regions, a, b, n, &|_| a)
            }
            "respeed" | "stretch" => {
                let n = n.unwrap_or(b - a);
                if had_sel {
                    after.sel = Some([a, a + n]);
                }
                let span = (b - a).max(1) as f64;
                shift_regions(&st.regions, a, b, n, &|x| a + ((x - a) as f64 * n as f64 / span).round() as usize)
            }
            "loopxfade" => {
                let end = a + n.unwrap_or(b - a);
                after = Ui { sel: Some([a, end]), cursor: a };
                shift_regions(&st.regions, a, b, end - a, &|x| x.min(end))
            }
            _ => st.regions.clone(),
        };
        let new_len = frames(&next);
        after.cursor = after.cursor.min(new_len);
        after.sel = after.sel.map(|s| [s[0].min(new_len), s[1].min(new_len)]);
        let regions: Vec<Region> = regions.into_iter().map(|r| Region { a: r.a.min(new_len), b: r.b.min(new_len), name: r.name }).filter(|r| r.b > r.a).collect();
        Self::record(&mut st, &ui);
        Self::set_sound(&mut st, Sound::new(next, cur.sr));
        st.regions = regions;
        let mut info = Self::info(&st, &after);
        if let (Some(o), Some(e)) = (info.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                if k != "cut" {
                    o.insert(k.clone(), v.clone());
                }
            }
        }
        Ok(info)
    }

    /// A preview of edit `p` kept under `key`: the whole sound with the edit made, or with
    /// `head_s`, only the first seconds of the edited span (level tools still measure all of it).
    pub fn view(&self, key: &str, p: &Value, head_s: Option<f64>) -> R<Value> {
        if let Some(v) = self.st.lock().unwrap().views.iter().find(|v| v.0 == key) {
            return Ok(v.3.clone());
        }
        let cur = self.cur()?;
        let (clip, noise) = {
            let st = self.st.lock().unwrap();
            (st.clip.clone(), st.noise.clone())
        };
        let noise = if p["noise"].as_str() == Some("print") { noise } else { None };
        let len = cur.frames();
        let (a, b) = (idx(p, "a").min(len), if p["b"].is_null() { len } else { idx(p, "b").min(len) });
        let (sound, from, extra) = match head_s {
            None => {
                let (c, e) = self.edit_chs(&cur.channels, cur.sr, p, clip.as_ref(), noise.as_ref())?;
                (Sound::new(c, cur.sr), usize::MAX, e)
            }
            Some(h) => {
                let end = b.min(a + (h * cur.sr as f64).round() as usize);
                let mut q = p.clone();
                let kind = p["kind"].as_str().unwrap_or("");
                // Level tools set their gain from the whole span, then apply it to the excerpt.
                if kind == "normalize" {
                    let pk = peak(&cur.channels, a, b);
                    if pk == 0.0 {
                        return Err("Selection is silent".into());
                    }
                    q = json!({"kind": "gain", "db": num(p, "db") - 20.0 * (pk as f64).log10()});
                } else if kind == "loudnorm" {
                    let m = op("measure", slice(&cur.channels, a, b), cur.sr, json!({}))?.info;
                    let lufs = m["lufs"].as_f64().filter(|x| x.is_finite()).ok_or("Selection is silent")?;
                    q = json!({"kind": "gain", "db": num(p, "target") - lufs});
                } else if kind == "fx" && p["tab"] == "nr" && noise.is_none() {
                    q["noise"] = json!("print");
                }
                let part = slice(&cur.channels, a, end);
                q["a"] = json!(0);
                q["b"] = json!(end - a);
                let nz = if kind == "fx" && p["tab"] == "nr" { Some(noise.clone().unwrap_or_else(|| json!(dsp::estimate_profile(&slice(&cur.channels, a, b))))) } else { None };
                let (c, e) = self.edit_chs(&part, cur.sr, &q, clip.as_ref(), nz.as_ref())?;
                (Sound::new(c, cur.sr), a, e)
            }
        };
        let n = sound.frames();
        let mut out = json!({"length": n, "from": if from == usize::MAX { Value::Null } else { json!(from) }, "full": from == usize::MAX});
        if let (Some(o), Some(e)) = (out.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        let mut st = self.st.lock().unwrap();
        st.views.retain(|v| v.0 != key);
        st.views.push((key.to_string(), sound, from, out.clone()));
        if st.views.len() > VIEWS_KEPT {
            st.views.remove(0);
        }
        Ok(out)
    }

    pub fn learn_noise(&self, a: usize, b: usize) -> R<()> {
        let cur = self.cur()?;
        let profile = dsp::noise_profile(&cur.channels, a, b);
        self.st.lock().unwrap().noise = Some(json!(profile));
        Ok(())
    }

    pub fn forget_noise(&self) {
        self.st.lock().unwrap().noise = None;
    }

    /// Readings that change nothing: zero crossings, sound bounds, slices, onsets, pitch, levels.
    pub fn analyse(&self, p: &Value) -> R<Value> {
        let s = self.src(p["src"].as_str())?;
        let (chs, sr, len) = (&s.channels, s.sr, s.frames());
        let (a, b) = (idx(p, "a").min(len), if p["b"].is_null() { len } else { idx(p, "b").min(len) });
        Ok(match p["kind"].as_str().unwrap_or("") {
            "zerocross" => json!(p["at"].as_array().into_iter().flatten().map(|x| zero_cross(chs, sr, x.as_f64().unwrap_or(0.0) as usize)).collect::<Vec<_>>()),
            "bounds" => json!(sound_bounds(chs, sr, num(p, "th") as f32).map(|(x, y)| [x, y])),
            "peak" => json!(peak(chs, a, b)),
            "stats" => {
                let n = (chs.len() * (b - a)).max(1) as f64;
                let sum: f64 = chs.iter().flat_map(|d| d[a..b].iter()).map(|x| (*x as f64).powi(2)).sum();
                json!({"peak": dsp::db(peak(chs, a, b) as f64), "rms": dsp::db((sum / n).sqrt())})
            }
            "profile" => match { let n = self.st.lock().unwrap().noise.clone(); n } {
                Some(n) => n,
                None => json!(dsp::estimate_profile(&slice(chs, a, b))),
            },
            "measure" => op("measure", slice(chs, a, b), sr, json!({}))?.info,
            "pitch" => op("pitch", vec![mono(&slice(chs, a, b))], sr, json!({}))?.info,
            "onsets" => op("onsets", vec![mono(&slice(chs, a, b))], sr, json!({"sensitivity": num(p, "sensitivity")}))?.info,
            "loud" => {
                // Whether anything in [a, b) is above th, on any channel.
                let th = num(p, "th") as f32;
                json!(chs.iter().any(|d| d[a..b].iter().any(|v| v.abs() > th)))
            }
            "regions" => {
                let m = mono(chs);
                let hop = (sr as f64 * 0.01).round() as usize;
                let th = num(p, "th");
                let min_gap = num(p, "gap_ms") / 1000.0 * sr as f64;
                let (min_len, pre, post) = (sr as f64 * 0.05, sr as f64 * 0.01, sr as f64 * 0.06);
                let mut out: Vec<(usize, usize)> = Vec::new();
                let mut cur: Option<(usize, usize)> = None;
                let mut i = 0;
                while i < m.len() {
                    let e = (i + hop).min(m.len());
                    let rms = (m[i..e].iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / (e - i).max(1) as f64).sqrt();
                    if rms >= th {
                        match &mut cur {
                            Some(c) if (i - c.1) as f64 <= min_gap => c.1 = e,
                            _ => {
                                if let Some(c) = cur.take() {
                                    out.push(c);
                                }
                                cur = Some((i, e));
                            }
                        }
                    }
                    i += hop;
                }
                if let Some(c) = cur {
                    out.push(c);
                }
                let mut res: Vec<(usize, usize)> = out
                    .into_iter()
                    .filter(|r| (r.1 - r.0) as f64 >= min_len)
                    .map(|r| (zero_cross(chs, sr, (r.0 as f64 - pre).round().max(0.0) as usize), zero_cross(chs, sr, (r.1 as f64 + post).round().min(m.len() as f64) as usize)))
                    .collect();
                res.sort_by_key(|r| r.0);
                for i in 0..res.len().saturating_sub(1) {
                    res[i].1 = res[i].1.min(res[i + 1].0);
                }
                json!(res.into_iter().filter(|r| r.1 > r.0).map(|r| [r.0, r.1]).collect::<Vec<_>>())
            }
            k => return Err(format!("Unknown reading {k}")),
        })
    }

    fn pyramid(&self, s: &Arc<Sound>) -> Arc<Vec<(Vec<f32>, Vec<f32>)>> {
        let key = ptr(s);
        if let Some(p) = self.st.lock().unwrap().pyramids.iter().find(|p| p.0 == key) {
            return p.1.clone();
        }
        let p: Arc<Vec<(Vec<f32>, Vec<f32>)>> = Arc::new(
            s.channels
                .iter()
                .map(|d| d.chunks_exact(PYRAMID).map(|c| c.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)))).unzip())
                .collect(),
        );
        let mut st = self.st.lock().unwrap();
        st.pyramids.push((key, p.clone()));
        if st.pyramids.len() > 4 {
            st.pyramids.remove(0);
        }
        p
    }

    /// What to draw for `width` pixels from sample `start`, `spp` samples per pixel: per channel,
    /// the lowest and highest sample under each pixel; or zoomed in past one sample per pixel,
    /// the samples themselves. Little-endian f32, channel after channel, lows then highs.
    pub fn peaks(&self, key: Option<&str>, start: f64, spp: f64, width: usize) -> R<Vec<u8>> {
        let s = self.src(key)?;
        let n = s.frames();
        let mut out = Vec::new();
        let mut put = |x: f32| out.extend_from_slice(&x.to_le_bytes());
        if spp <= 1.0 {
            let a = start.max(0.0).floor() as usize;
            let b = ((start + width as f64 * spp).floor() as usize + 2).min(n);
            for d in &s.channels {
                for v in &d[a.min(b)..b] {
                    put(*v);
                }
            }
            return Ok(out);
        }
        let pyr = if spp >= (PYRAMID * 2) as f64 { Some(self.pyramid(&s)) } else { None };
        for (c, d) in s.channels.iter().enumerate() {
            let mut lo = vec![1.0f32; width];
            let mut hi = vec![-1.0f32; width];
            for x in 0..width {
                let a = (start + x as f64 * spp).floor().max(0.0) as usize;
                let b = ((start + (x + 1) as f64 * spp).floor().max(0.0) as usize).min(n);
                if a >= n {
                    break;
                }
                let (mut mn, mut mx) = (1.0f32, -1.0f32);
                let mut i = a;
                if let Some(p) = &pyr {
                    let (k0, k1) = (a.div_ceil(PYRAMID), b / PYRAMID);
                    while i < b.min(k0 * PYRAMID) {
                        mn = mn.min(d[i]);
                        mx = mx.max(d[i]);
                        i += 1;
                    }
                    for k in k0..k1.min(p[c].0.len()) {
                        mn = mn.min(p[c].0[k]);
                        mx = mx.max(p[c].1[k]);
                    }
                    i = i.max(k1 * PYRAMID);
                }
                while i < b {
                    mn = mn.min(d[i]);
                    mx = mx.max(d[i]);
                    i += 1;
                }
                lo[x] = mn;
                hi[x] = mx;
            }
            lo.iter().chain(hi.iter()).for_each(|v| put(*v));
        }
        Ok(out)
    }

    /// One spectrogram tile as RGBA pixels.
    pub fn spectrogram(&self, key: Option<&str>, start: f64, spp: f64, pw: usize, rows: usize) -> R<Vec<u8>> {
        let s = self.src(key)?;
        let id = ptr(&s);
        let cached = self.st.lock().unwrap().mono.clone().filter(|m| m.0 == id);
        let (_, data, pk) = match cached {
            Some(m) => m,
            None => {
                let m = Arc::new(mono(&s.channels));
                let pk = dsp::spectrogram_peak(&m, s.sr);
                let e = (id, m, pk);
                self.st.lock().unwrap().mono = Some(e.clone());
                e
            }
        };
        Ok(dsp::spectrogram(&data, s.sr, start, spp, pw, rows, pk))
    }

    /// A WAV of [a, b) of the sound or a preview, at the given rate and depth.
    pub fn render(&self, p: &Value) -> R<Vec<u8>> {
        let s = self.src(p["src"].as_str())?;
        let len = s.frames();
        let (a, b) = (idx(p, "a").min(len), if p["b"].is_null() { len } else { idx(p, "b").min(len) });
        let rate = p["rate"].as_u64().filter(|r| *r > 0).unwrap_or(s.sr as u64);
        op("render", slice(&s.channels, a, b), s.sr, json!({"rate": rate, "bits": p["bits"].as_u64().unwrap_or(24), "mono": p["mono"].as_bool().unwrap_or(false), "prep": p["prep"]}))?
            .bytes
            .ok_or_else(|| "Nothing was rendered".into())
    }

    // ---------- playback

    /// Plays [a, b) of the sound or a preview from `start`, looping between a and b when asked,
    /// through the live EQ when `eq` is on. `fade_in`/`fade_out` (seconds) shape auditions.
    /// Returns the engine time the sound starts at.
    pub fn play(&self, p: &Value) -> R<Value> {
        self.stop();
        let s = self.src(p["src"].as_str())?;
        let len = s.frames();
        let sr = s.sr as f64;
        let (a, b) = (idx(p, "a").min(len), if p["b"].is_null() { len } else { idx(p, "b").min(len) });
        let start = idx(p, "start").clamp(a, b);
        if b <= a {
            return Err("Nothing to play".into());
        }
        let e = &self.engine;
        let looped = p["loop"].as_bool().unwrap_or(false);
        let mut st = self.st.lock().unwrap();
        if st.play.eq.is_empty() {
            st.play.eq = (0..EQ_NODES).map(|_| e.id()).collect();
            st.play.fade = e.id();
            for id in &st.play.eq {
                e.set_filter_coefs(*id, None);
            }
        }
        let now = e.now() + 0.03;
        let mut events = vec![];
        let fi = num(p, "fade_in");
        if fi > 0.0 {
            events.push(Ev::Set { t: now, v: 0.0 });
            events.push(Ev::Ramp { t: now + fi, v: 1.0 });
        }
        let fo = num(p, "fade_out");
        let dur = (b - start) as f64 / sr;
        if fo > 0.0 && !looped && dur > 4.0 * fo {
            events.push(Ev::Set { t: now + dur - fo, v: 1.0 });
            events.push(Ev::Ramp { t: now + dur, v: 0.0 });
        }
        e.set_gain(st.play.fade, 1.0, events);
        let mut chain = vec![Link::Gain(st.play.fade)];
        if p["eq"].as_bool().unwrap_or(false) {
            chain.extend(st.play.eq.iter().map(|id| Link::Filter(*id)));
        }
        chain.push(Link::Tap);
        let id = e.id();
        e.play(id, &s, now, start as f64 / sr, if looped { None } else { Some(dur) }, if looped { Some((a as f64 / sr, b as f64 / sr)) } else { None }, chain);
        st.play.voice = Some(id);
        Ok(json!({"id": id, "at": now}))
    }

    /// Fades the playing sound out over `fade` seconds (or stops it at once). With `only`, only
    /// when that is still the sound playing.
    pub fn stop_fading(&self, fade: f64, only: Option<u64>) {
        let mut st = self.st.lock().unwrap();
        if only.is_some() && st.play.voice != only {
            return;
        }
        if let Some(id) = st.play.voice.take() {
            let e = &self.engine;
            let now = e.now();
            if fade > 0.0 {
                e.set_gain(st.play.fade, 1.0, vec![Ev::Set { t: now, v: e.gain_now(st.play.fade).unwrap_or(1.0) }, Ev::Ramp { t: now + fade, v: 0.0 }]);
                e.stop(id, now + fade);
            } else {
                e.stop(id, 0.0);
            }
        }
    }

    pub fn stop(&self) {
        self.stop_fading(0.0, None);
    }

    /// The live EQ on what plays: one filter per band, in order; the rest pass sound unchanged.
    pub fn set_eq(&self, bands: &[Value]) {
        let mut st = self.st.lock().unwrap();
        let e = &self.engine;
        if st.play.eq.is_empty() {
            st.play.eq = (0..EQ_NODES).map(|_| e.id()).collect();
            st.play.fade = e.id();
        }
        let rate = e.rate() as f64;
        for (i, id) in st.play.eq.iter().enumerate() {
            e.set_filter_coefs(*id, bands.get(i).and_then(|b| band_coefs(b, rate)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(a: usize, b: usize) -> Region {
        Region { a, b, name: String::new() }
    }

    fn doc(n: usize) -> Doc {
        let d = Doc::new(Engine::start());
        let ramp: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
        d.open(Sound::new(vec![ramp], 1000), None);
        d
    }

    fn ui(sel: Option<[usize; 2]>, cursor: usize) -> Ui {
        Ui { sel, cursor }
    }

    #[test]
    fn regions_after_a_cut_move_with_the_audio() {
        let regions = vec![r(0, 100), r(150, 250), r(400, 500)];
        assert_eq!(shift_regions(&regions, 200, 300, 0, &|_| 200), vec![r(0, 100), r(150, 200), r(300, 400)]);
        assert_eq!(crop_regions(&regions, 100, 450, 1000), vec![r(50, 150), r(300, 350)]);
    }

    #[test]
    fn a_removal_keeps_its_join_smooth_and_the_rest_of_the_sound() {
        let chs = vec![(0..100).map(|i| i as f32).collect::<Vec<f32>>()];
        let out = splice(&chs, 40, 60, None, 4);
        assert_eq!(out[0].len(), 80);
        assert_eq!(&out[0][..36], &chs[0][..36]);
        assert_eq!(&out[0][40..], &chs[0][60..]);
    }

    #[test]
    fn an_edit_moves_regions_and_selection_and_undo_puts_them_back() {
        let d = doc(1000);
        d.regions(&json!({"op": "add", "a": 600, "b": 700}), ui(None, 0)).unwrap();
        let after = d.edit(&json!({"kind": "delete", "a": 100, "b": 300, "ui": ui(Some([100, 300]), 100)})).unwrap();
        assert_eq!(after["length"], 800);
        assert_eq!(after["regions"][0]["a"], 400);
        assert_eq!(after["sel"], Value::Null);
        assert_eq!(after["cursor"], 100);
        let back = d.undo(false, ui(None, 100)).unwrap();
        assert_eq!(back["length"], 1000);
        assert_eq!(back["regions"][0]["a"], 600);
        assert_eq!(back["sel"], json!([100, 300]));
        let again = d.undo(true, ui(Some([100, 300]), 100)).unwrap();
        assert_eq!(again["length"], 800);
        assert_eq!(again["regions"][0]["a"], 400);
    }

    #[test]
    fn region_steps_undo_without_touching_the_sound() {
        let d = doc(1000);
        let v = d.edit(&json!({"kind": "gain", "a": 0, "b": 1000, "db": -6, "ui": ui(None, 0)})).unwrap()["version"].as_u64().unwrap();
        d.regions(&json!({"op": "grid", "a": 0, "b": 1000, "step": 250.0}), ui(None, 0)).unwrap();
        let back = d.undo(false, ui(None, 0)).unwrap();
        assert_eq!(back["regions"], json!([]));
        assert_eq!(back["version"].as_u64().unwrap(), v);
        assert_eq!(back["undo"], 1);
    }

    #[test]
    fn paste_selects_what_it_put_in() {
        let d = doc(1000);
        d.edit(&json!({"kind": "copy", "a": 0, "b": 100, "ui": ui(Some([0, 100]), 0)})).unwrap();
        let after = d.edit(&json!({"kind": "paste", "a": 500, "b": 500, "ui": ui(None, 500)})).unwrap();
        assert_eq!(after["length"], 1100);
        assert_eq!(after["sel"], json!([500, 600]));
    }
}
