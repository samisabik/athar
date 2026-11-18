//! The editor's sound, held here rather than in the page: its samples, undo history, clipboard,
//! previews of effects and tools, analysis, waveform peaks, spectrogram tiles and playback.
//!
//! The page keeps what it draws and the regions; every change to the audio is a request here.
//! Sounds are immutable `Arc<Sound>`s: an edit makes a new one and the old one becomes an undo
//! step, and the engine plays them without a copy when they are at the device's rate.
//!
//! Commands run side by side. Anything slow (an edit, a preview, finding regions) is worked out
//! without the state lock, from the sound as it was, and goes in only if that is still the
//! editor's sound.

use crate::engine::{biquad, Coefs, Engine, Ev, Link};
use crate::library::Library;
use crate::sounds::Sound;
use athar_core::dsp;
use serde_json::{json, Value};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, OnceLock, Weak};

type Chs = Vec<Vec<f32>>;
type R<T> = Result<T, String>;

/// History lets go of its oldest steps once the sounds it holds take more than this.
const UNDO_BYTES: usize = 3 << 29;
const VIEWS_KEPT: usize = 6;
const PYRAMIDS_KEPT: usize = 4;
/// Filters of the live EQ; bands beyond these are not heard until the EQ is applied.
const EQ_NODES: usize = 24;
/// Samples under each entry of a sound's outline, which zoomed-out drawing reads instead of
/// the samples.
const PYRAMID: usize = 256;
/// Playing starts this long after it is asked for, so the sound is in the engine by then.
const LEAD: f64 = 0.03;

#[derive(Default)]
struct Play {
    /// The voice playing and the gain node that fades it.
    voice: Option<(u64, u64)>,
    /// Gain nodes of voices that are fading out, each with the engine time its voice stops.
    fading: Vec<(u64, f64)>,
    /// The live EQ's filter nodes, made on first use.
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
    /// Previews made by `view`: the key, the sound and what the page was told of it.
    views: Vec<(String, Arc<Sound>, Value)>,
    clip: Option<Arc<Sound>>,
    /// The learned noise print, as `dsp::denoise` takes it.
    noise: Option<Vec<f32>>,
    // Kept per sound. A Weak, not an address: a sound freed by an edit can leave its address to
    // the next one, which must not inherit its drawing.
    mono: Option<(Weak<Sound>, Arc<Vec<f32>>, f64)>,
    pyramids: Vec<(Weak<Sound>, Arc<Vec<(Vec<f32>, Vec<f32>)>>)>,
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

/// Whether `w` still points at the sound `s`.
fn same(w: &Weak<Sound>, s: &Arc<Sound>) -> bool {
    w.upgrade().is_some_and(|x| Arc::ptr_eq(&x, s))
}

/// An error unless `from`, which a change was worked out from, is still the editor's sound.
fn unchanged(st: &State, from: &Arc<Sound>) -> R<()> {
    match &st.cur {
        Some(c) if Arc::ptr_eq(c, from) => Ok(()),
        _ => Err("The sound changed while this ran. Try again".into()),
    }
}

fn num(p: &Value, k: &str) -> f64 {
    p[k].as_f64().unwrap_or(0.0)
}

fn idx(p: &Value, k: &str) -> usize {
    p[k].as_f64().unwrap_or(0.0).max(0.0) as usize
}

/// The span [a, b) a request names, within `len` samples. Without `b` it runs to the end, and
/// a `b` before `a` gives an empty span.
fn span(p: &Value, len: usize) -> (usize, usize) {
    let a = idx(p, "a").min(len);
    let b = if p["b"].is_null() { len } else { idx(p, "b").min(len) };
    (a, b.max(a))
}

fn frames(c: &Chs) -> usize {
    c.first().map_or(0, Vec::len)
}

fn slice(c: &Chs, a: usize, b: usize) -> Chs {
    c.iter().map(|d| d[a.min(d.len())..b.min(d.len())].to_vec()).collect()
}

fn curve(i: usize, m: usize) -> f32 {
    ((i as f64 + 0.5) / m as f64 * std::f64::consts::FRAC_PI_2).sin().powi(2) as f32
}

/// Replaces [a, b) with `insert` (or removes it). With `smooth` samples of fade, a removal
/// crossfades the audio leading up to the cut into the audio leading up to its far side, and an
/// insertion dips briefly to silence at each end.
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

/// The stretches of a sound louder than `th` (RMS over 10 ms), joined across gaps of up to
/// `gap_ms`, widened a little and cut at zero crossings: [a, b) each, in order.
fn sounding_spans(chs: &Chs, sr: u32, th: f64, gap_ms: f64) -> Vec<[usize; 2]> {
    let m = dsp::mix_mono(chs);
    let hop = ((sr as f64 * 0.01).round() as usize).max(1);
    let min_gap = gap_ms / 1000.0 * sr as f64;
    let (min_len, pre, post) = (sr as f64 * 0.05, sr as f64 * 0.01, sr as f64 * 0.06);
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut cur: Option<(usize, usize)> = None;
    for i in (0..m.len()).step_by(hop) {
        let e = (i + hop).min(m.len());
        let rms = (m[i..e].iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / (e - i) as f64).sqrt();
        if rms >= th {
            match &mut cur {
                Some(c) if (i - c.1) as f64 <= min_gap => c.1 = e,
                _ => {
                    out.extend(cur.take());
                    cur = Some((i, e));
                }
            }
        }
    }
    out.extend(cur);
    let mut res: Vec<(usize, usize)> = out
        .into_iter()
        .filter(|r| (r.1 - r.0) as f64 >= min_len)
        .map(|r| (zero_cross(chs, sr, (r.0 as f64 - pre).round().max(0.0) as usize), zero_cross(chs, sr, (r.1 as f64 + post).round().min(m.len() as f64) as usize)))
        .collect();
    res.sort_by_key(|r| r.0);
    for i in 0..res.len().saturating_sub(1) {
        res[i].1 = res[i].1.min(res[i + 1].0);
    }
    res.into_iter().filter(|r| r.1 > r.0).map(|r| [r.0, r.1]).collect()
}

/// The EQ applied: each band that is on as one biquad filter, one after the other.
pub fn render_eq(chs: &Chs, sr: u32, bands: &[Value]) -> Chs {
    let coefs: Vec<Coefs> = bands.iter().filter_map(|b| band_coefs(b, sr as f64)).collect();
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

/// The EQ's band types and the filter each one is.
const BAND_FILTERS: [(&str, &str); 6] = [("bell", "peaking"), ("lowshelf", "lowshelf"), ("highshelf", "highshelf"), ("lowcut", "highpass"), ("highcut", "lowpass"), ("notch", "notch")];

/// A band's filter; none for a band that is off.
fn band_coefs(b: &Value, rate: f64) -> Option<Coefs> {
    if !b["on"].as_bool().unwrap_or(false) {
        return None;
    }
    let kind = BAND_FILTERS.iter().find(|(k, _)| Some(*k) == b["type"].as_str()).map_or("peaking", |n| n.1);
    biquad(kind, num(b, "f"), num(b, "q"), num(b, "g"), rate)
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
        self.warm(&sound);
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
        // Old sounds are what history costs (each with the copy made to play it, when its rate
        // is not the device's); the oldest steps go once it holds too much. A sound that
        // several steps share counts once, and the one being edited not at all.
        let mut held: Vec<&Arc<Sound>> = vec![];
        for s in st.undo.iter().map(|s| &s.sound) {
            if !held.iter().any(|h| Arc::ptr_eq(h, s)) && !st.cur.as_ref().is_some_and(|c| Arc::ptr_eq(c, s)) {
                held.push(s);
            }
        }
        let mut total: usize = held.iter().map(|s| s.bytes()).sum();
        while total > UNDO_BYTES && !st.undo.is_empty() {
            let gone = st.undo.remove(0).sound;
            let shared = st.undo.iter().any(|s| Arc::ptr_eq(&s.sound, &gone)) || st.cur.as_ref().is_some_and(|c| Arc::ptr_eq(c, &gone));
            if !shared {
                total -= gone.bytes();
            }
        }
    }

    fn set_sound(&self, st: &mut State, s: Arc<Sound>) {
        if !st.cur.as_ref().is_some_and(|c| Arc::ptr_eq(c, &s)) {
            st.version += 1;
            st.views.clear();
            self.warm(&s);
        }
        st.cur = Some(s);
    }

    /// Readies a sound at another rate than the device's in the background, so its first play
    /// does not wait for the resampling. One that is replaced and dropped at once is skipped.
    fn warm(&self, s: &Arc<Sound>) {
        if s.sr == self.engine.rate() {
            return;
        }
        let (engine, sound) = (self.engine.clone(), Arc::downgrade(s));
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if let Some(s) = sound.upgrade() {
                let _ = engine.prepare(&s);
            }
        });
    }

    pub fn undo(&self, redo: bool, ui: Ui) -> R<Value> {
        self.stop();
        let mut st = self.st.lock().unwrap();
        let from = if redo { st.redo.pop() } else { st.undo.pop() };
        let Some(step) = from else { return Err("Nothing to undo".into()) };
        let here = Step { sound: st.cur.clone().unwrap_or_else(|| step.sound.clone()), regions: std::mem::take(&mut st.regions), ui };
        if redo { st.undo.push(here) } else { st.redo.push(here) }
        self.set_sound(&mut st, step.sound);
        st.regions = step.regions;
        self.persist(&st);
        Ok(Self::info(&st, &step.ui))
    }

    /// A change to the regions alone, as one step of history.
    pub fn regions(&self, p: &Value, ui: Ui) -> R<Value> {
        let cur = self.cur()?;
        let before = self.st.lock().unwrap().regions.clone();
        let mut after = ui.clone();
        let regions = Self::changed_regions(&cur, before.clone(), p, &mut after)?;
        let mut st = self.st.lock().unwrap();
        unchanged(&st, &cur)?;
        if st.regions != before {
            return Err("The regions changed while this ran. Try again".into());
        }
        Self::record(&mut st, &ui);
        st.regions = regions;
        self.persist(&st);
        Ok(Self::info(&st, &after))
    }

    /// `regions` of the sound `cur` after the change `p`. A region that goes takes the
    /// selection in `ui` with it when that was the region.
    fn changed_regions(cur: &Sound, mut regions: Vec<Region>, p: &Value, ui: &mut Ui) -> R<Vec<Region>> {
        let (chs, len, sr) = (&*cur.channels, cur.frames(), cur.sr);
        let (a, b) = span(p, len);
        let mut drop_sel = |r: &Region| {
            if ui.sel == Some([r.a, r.b]) {
                ui.sel = None;
            }
        };
        match p["op"].as_str().unwrap_or("") {
            "add" => {
                if b <= a {
                    return Err("Select a region first".into());
                }
                regions.push(Region { a, b, name: String::new() });
                regions.sort_by_key(|r| r.a);
            }
            "remove" => {
                let i = idx(p, "i");
                if i < regions.len() {
                    drop_sel(&regions.remove(i));
                }
            }
            "rename" => {
                if let Some(r) = regions.get_mut(idx(p, "i")) {
                    r.name = p["name"].as_str().unwrap_or("").trim().chars().take(80).collect();
                }
            }
            "clear" => {
                regions.iter().for_each(&mut drop_sel);
                regions.clear();
            }
            "grid" => {
                let step = num(p, "step");
                if !(step > 0.0) {
                    return Err("Enter the tempo first".into());
                }
                if (b - a) as f64 / step > 512.0 {
                    return Err("That would make more than 512 slices".into());
                }
                regions.clear();
                let mut x = a as f64;
                while x + step * 0.5 <= b as f64 {
                    regions.push(Region { a: x.round() as usize, b: ((x + step).round() as usize).min(b), name: String::new() });
                    x += step;
                }
                if regions.is_empty() {
                    return Err("The sound is shorter than one slice".into());
                }
            }
            "auto" => {
                regions = sounding_spans(chs, sr, num(p, "th"), num(p, "gap_ms")).into_iter().map(|[a, b]| Region { a, b, name: String::new() }).collect();
                if regions.is_empty() {
                    return Err("No sounds above the threshold".into());
                }
            }
            "hits" => {
                let found = dsp::onsets(&dsp::mix_mono(&slice(chs, a, b)), sr, num(p, "sensitivity"));
                let mut pts: Vec<usize> = found.iter().map(|&x| zero_cross(chs, sr, a + x)).collect();
                pts.sort();
                pts.dedup();
                // A hit that is already sounding at the very start has no rise to detect; it gets the first slice.
                let head = b.min(a + (sr as f64 * 0.01).round() as usize);
                let th = num(p, "th") as f32;
                if pts.first().copied().unwrap_or(b).saturating_sub(a) as f64 > sr as f64 * 0.03 && chs.iter().any(|d| d[a..head].iter().any(|v| v.abs() > th)) {
                    pts.insert(0, a);
                }
                regions = pts.iter().enumerate().map(|(i, &x)| Region { a: x, b: pts.get(i + 1).copied().unwrap_or(b), name: String::new() }).filter(|r| r.b.saturating_sub(r.a) as f64 >= sr as f64 * 0.01).collect();
                if regions.is_empty() {
                    return Err("No hits found. Raise the sensitivity.".into());
                }
            }
            o => return Err(format!("Unknown change to regions {o}")),
        }
        Ok(regions)
    }

    /// The edit `p` applied to `chs`. Returns the new samples and what the page needs to know:
    /// for edits that change the length, `n` is the new length of the edited span. `noise` is
    /// the print noise reduction works from; without one it is estimated from the span.
    fn edit_chs(chs: &Chs, sr: u32, p: &Value, clip: Option<&Arc<Sound>>, noise: Option<&[f32]>) -> R<(Chs, Value)> {
        let len = frames(chs);
        let (a, b) = span(p, len);
        let smooth = ((sr as f64) * num(p, "smooth_ms") / 1000.0).round() as usize;
        let kind = p["kind"].as_str().unwrap_or("");
        // New audio in place of [a, b); the page is told its length.
        let replaced = |seg: Chs, smooth: usize| (splice(chs, a, b, Some(&seg), smooth), json!({"n": frames(&seg)}));
        Ok(match kind {
            "trim" => (splice(&splice(chs, b, len, None, 0), 0, a, None, 0), json!({})),
            "delete" => (splice(chs, a, b, None, smooth), json!({})),
            "paste" => {
                let clip = clip.ok_or("Nothing copied")?;
                let mut ins: Chs = if clip.sr == sr { clip.channels.to_vec() } else { dsp::resample_sinc(&clip.channels, clip.sr, sr) };
                if ins.len() > chs.len() {
                    ins = vec![dsp::mix_mono(&ins)];
                }
                replaced(ins, smooth)
            }
            "mono" => (vec![dsp::mix_mono(chs)], json!({})),
            "respeed" => replaced(dsp::resample_sinc(&slice(chs, a, b), (num(p, "ratio") * 100000.0).round() as u32, 100000), 0),
            "stretch" => replaced(dsp::stretch_pitch(&slice(chs, a, b), sr, num(p, "length"), num(p, "semitones")), 0),
            "loopxfade" => replaced(dsp::loop_crossfade(&slice(chs, a, b), idx(p, "n")), 0),
            "trimsilence" => {
                let (x, y) = dsp::sound_bounds(chs, sr, num(p, "th")).ok_or("Nothing above the threshold")?;
                if x == 0 && y == len {
                    return Err("Nothing is cut at this level. Raise \"Silence below\" until the ends are marked.".into());
                }
                (splice(&splice(chs, y, len, None, 0), 0, x, None, 0), json!({"cut": [x, y]}))
            }
            "prep" => {
                let settings = dsp::PrepParams { silence: num(p, "th"), target_db: num(p, "db"), fade_in_ms: num(p, "fade_in_ms"), fade_out_ms: num(p, "fade_out_ms") };
                let (prepared, kept) = dsp::prep(chs, sr, &settings);
                (prepared, kept.map_or(json!({}), |(x, y)| json!({"cut": [x, y]})))
            }
            // The rest change samples where they are, in a copy of the sound.
            _ => {
                let mut out = chs.clone();
                let mut extra = json!({});
                let put = |out: &mut Chs, part: &Chs| {
                    for (d, s) in out.iter_mut().zip(part) {
                        let n = s.len().min(b - a);
                        d[a..a + n].copy_from_slice(&s[..n]);
                    }
                };
                match kind {
                    "silence" => out.iter_mut().for_each(|d| d[a..b].fill(0.0)),
                    "fade" => {
                        for s in p["spans"].as_array().into_iter().flatten() {
                            dsp::fade(&mut out, idx(s, "a"), idx(s, "b"), s["fadeIn"].as_bool().unwrap_or(true));
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
                    "loudnorm" => {
                        let m = dsp::measure(&slice(chs, a, b), sr);
                        if !m.lufs.is_finite() {
                            return Err("Selection is silent".into());
                        }
                        let gain = num(p, "target") - m.lufs;
                        scale(&mut out, a, b, 10f64.powf(gain / 20.0));
                        extra = json!({"over": m.true_peak + gain});
                    }
                    "declick" => {
                        let (repaired, clicks) = dsp::declick(&slice(chs, a, b), sr, num(p, "sensitivity"));
                        put(&mut out, &repaired);
                        extra = json!({"info": format!("{clicks} click{} repaired", if clicks == 1 { "" } else { "s" })});
                    }
                    "fx" => {
                        let tab = p["tab"].as_str().unwrap_or("");
                        let part = slice(chs, a, b);
                        let settings = &p["p"];
                        let done = |r: dsp::Processed| (r.channels, json!(r.info));
                        let (processed, info) = match tab {
                            "eq" => (render_eq(&part, sr, p["bands"].as_array().map_or(&[], Vec::as_slice)), Value::Null),
                            "nr" => {
                                let estimated;
                                let print = match noise {
                                    Some(n) => n,
                                    None => {
                                        estimated = dsp::estimate_profile(&part);
                                        &estimated
                                    }
                                };
                                done(dsp::denoise(&part, sr, print, &dsp::NrParams::from_json(settings))?)
                            }
                            "comp" => done(dsp::compress(&part, sr, &dsp::CompParams::from_json(settings))),
                            "max" => done(dsp::maximize(&part, sr, &dsp::MaxParams::from_json(settings))),
                            "sat" => done(dsp::saturate(&part, &dsp::SatParams::from_json(settings))),
                            "shape" => done(dsp::shape(&part, sr, &dsp::ShapeParams::from_json(settings))),
                            "lofi" => done(dsp::lofi(&part, sr, &dsp::LofiParams::from_json(settings))),
                            _ => return Err(format!("Unknown effect {tab}")),
                        };
                        put(&mut out, &processed);
                        extra = json!({"info": info});
                    }
                    _ => return Err(format!("Unknown edit {kind}")),
                }
                (out, extra)
            }
        })
    }

    /// The clipboard, and the learned noise print when `p` asks for it (`noise: "print"`).
    fn tools(&self, p: &Value) -> (Option<Arc<Sound>>, Option<Vec<f32>>) {
        let st = self.st.lock().unwrap();
        (st.clip.clone(), st.noise.clone().filter(|_| p["noise"].as_str() == Some("print")))
    }

    pub fn edit(&self, p: &Value) -> R<Value> {
        self.stop();
        let cur = self.cur()?;
        let ui: Ui = serde_json::from_value(p["ui"].clone()).unwrap_or_default();
        let len = cur.frames();
        let (a, b) = span(p, len);
        let mut p = p.clone();
        if matches!(p["kind"].as_str(), Some("copy" | "cut")) {
            let mut st = self.st.lock().unwrap();
            st.clip = Some(Sound::new(slice(&cur.channels, a, b), cur.sr));
            if p["kind"] == "copy" {
                return Ok(Self::info(&st, &ui));
            }
            p["kind"] = json!("delete");
        }
        let kind = p["kind"].as_str().unwrap_or("");
        let (clip, noise) = self.tools(&p);
        let (next, extra) = Self::edit_chs(&cur.channels, cur.sr, &p, clip.as_ref(), noise.as_deref())?;
        let mut st = self.st.lock().unwrap();
        unchanged(&st, &cur)?;
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
                let was = (b - a).max(1) as f64;
                shift_regions(&st.regions, a, b, n, &|x| a + ((x - a) as f64 * n as f64 / was).round() as usize)
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
        self.set_sound(&mut st, Sound::new(next, cur.sr));
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
            return Ok(v.2.clone());
        }
        let cur = self.cur()?;
        let (clip, noise) = self.tools(p);
        let (a, b) = span(p, cur.frames());
        let (sound, from, extra) = match head_s {
            None => {
                let (c, e) = Self::edit_chs(&cur.channels, cur.sr, p, clip.as_ref(), noise.as_deref())?;
                (Sound::new(c, cur.sr), None, e)
            }
            Some(h) => {
                let end = b.min(a + (h * cur.sr as f64).round() as usize);
                let mut q = p.clone();
                let kind = p["kind"].as_str().unwrap_or("");
                let mut print = noise;
                // What a tool reads from the whole span is read from all of it, then applied to
                // the excerpt: a level tool's gain, and the noise that noise reduction estimates.
                if kind == "normalize" {
                    let pk = peak(&cur.channels, a, b);
                    if pk == 0.0 {
                        return Err("Selection is silent".into());
                    }
                    q = json!({"kind": "gain", "db": num(p, "db") - 20.0 * (pk as f64).log10()});
                } else if kind == "loudnorm" {
                    let lufs = dsp::measure(&slice(&cur.channels, a, b), cur.sr).lufs;
                    if !lufs.is_finite() {
                        return Err("Selection is silent".into());
                    }
                    q = json!({"kind": "gain", "db": num(p, "target") - lufs});
                } else if kind == "fx" && p["tab"] == "nr" && print.is_none() {
                    print = Some(dsp::estimate_profile(&slice(&cur.channels, a, b)));
                }
                q["a"] = json!(0);
                q["b"] = json!(end - a);
                let (c, e) = Self::edit_chs(&slice(&cur.channels, a, end), cur.sr, &q, clip.as_ref(), print.as_deref())?;
                (Sound::new(c, cur.sr), Some(a), e)
            }
        };
        let mut out = json!({"length": sound.frames(), "from": from, "full": from.is_none()});
        if let (Some(o), Some(e)) = (out.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        let mut st = self.st.lock().unwrap();
        // A preview of a sound that has changed since is of no use to keep.
        if unchanged(&st, &cur).is_ok() {
            st.views.retain(|v| v.0 != key);
            st.views.push((key.to_string(), sound, out.clone()));
            if st.views.len() > VIEWS_KEPT {
                st.views.remove(0);
            }
        }
        Ok(out)
    }

    pub fn learn_noise(&self, a: usize, b: usize) -> R<()> {
        let cur = self.cur()?;
        let (a, b) = (a.min(b), a.max(b));
        self.st.lock().unwrap().noise = Some(dsp::noise_profile(&cur.channels, a, b));
        Ok(())
    }

    pub fn forget_noise(&self) {
        self.st.lock().unwrap().noise = None;
    }

    /// Readings that change nothing: zero crossings, sound bounds, slices, onsets, pitch, levels.
    pub fn analyse(&self, p: &Value) -> R<Value> {
        let s = self.src(p["src"].as_str())?;
        let (chs, sr) = (&*s.channels, s.sr);
        let (a, b) = span(p, s.frames());
        Ok(match p["kind"].as_str().unwrap_or("") {
            "zerocross" => json!(p["at"].as_array().into_iter().flatten().map(|x| zero_cross(chs, sr, x.as_f64().unwrap_or(0.0) as usize)).collect::<Vec<_>>()),
            "bounds" => json!(dsp::sound_bounds(chs, sr, num(p, "th")).map(|(x, y)| [x, y])),
            "peak" => json!(peak(chs, a, b)),
            "stats" => {
                let n = (chs.len() * (b - a)).max(1) as f64;
                let sum: f64 = chs.iter().flat_map(|d| d[a..b].iter()).map(|x| (*x as f64).powi(2)).sum();
                json!({"peak": dsp::db(peak(chs, a, b) as f64), "rms": dsp::db((sum / n).sqrt())})
            }
            "profile" => {
                let learned = self.st.lock().unwrap().noise.clone();
                json!(learned.unwrap_or_else(|| dsp::estimate_profile(&slice(chs, a, b))))
            }
            "measure" => {
                // A silent selection has no loudness: `lufs` is then null.
                let m = dsp::measure(&slice(chs, a, b), sr);
                json!({"lufs": m.lufs, "peak": m.peak, "truePeak": m.true_peak})
            }
            "pitch" => json!({"hz": dsp::detect_pitch(&dsp::mix_mono(&slice(chs, a, b)), sr)}),
            "onsets" => json!({"positions": dsp::onsets(&dsp::mix_mono(&slice(chs, a, b)), sr, num(p, "sensitivity"))}),
            "loud" => {
                // Whether anything in [a, b) is above th, on any channel.
                let th = num(p, "th") as f32;
                json!(chs.iter().any(|d| d[a..b].iter().any(|v| v.abs() > th)))
            }
            "regions" => json!(sounding_spans(chs, sr, num(p, "th"), num(p, "gap_ms"))),
            k => return Err(format!("Unknown reading {k}")),
        })
    }

    fn pyramid(&self, s: &Arc<Sound>) -> Arc<Vec<(Vec<f32>, Vec<f32>)>> {
        if let Some(p) = self.st.lock().unwrap().pyramids.iter().find(|p| same(&p.0, s)) {
            return p.1.clone();
        }
        let p: Arc<Vec<(Vec<f32>, Vec<f32>)>> = Arc::new(
            s.channels
                .iter()
                .map(|d| d.chunks_exact(PYRAMID).map(|c| c.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)))).unzip())
                .collect(),
        );
        let mut st = self.st.lock().unwrap();
        st.pyramids.retain(|p| p.0.strong_count() > 0);
        st.pyramids.push((Arc::downgrade(s), p.clone()));
        if st.pyramids.len() > PYRAMIDS_KEPT {
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
            for d in s.channels.iter() {
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
        let cached = self.st.lock().unwrap().mono.clone().filter(|m| same(&m.0, &s));
        let (_, data, pk) = match cached {
            Some(m) => m,
            None => {
                let m = Arc::new(dsp::mix_mono(&s.channels));
                let pk = dsp::spectrogram_peak(&m, s.sr);
                let e = (Arc::downgrade(&s), m, pk);
                self.st.lock().unwrap().mono = Some(e.clone());
                e
            }
        };
        Ok(dsp::spectrogram(&data, s.sr, start, spp, pw, rows, pk))
    }

    /// A WAV of [a, b) of the sound or a preview, at the given rate and depth.
    pub fn render(&self, p: &Value) -> R<Vec<u8>> {
        let s = self.src(p["src"].as_str())?;
        let (a, b) = span(p, s.frames());
        dsp::render_wav(&slice(&s.channels, a, b), s.sr, &dsp::RenderArgs::from_json(p, s.sr))
    }

    // ---------- playback

    /// Plays [a, b) of the sound or a preview from `start`, looping between a and b when asked,
    /// through the live EQ when `eq` is on. `fade_in`/`fade_out` (seconds) shape auditions.
    /// Returns the engine time the sound starts at.
    pub fn play(&self, p: &Value) -> R<Value> {
        self.stop();
        let s = self.src(p["src"].as_str())?;
        let sr = s.sr as f64;
        let (a, b) = span(p, s.frames());
        let start = idx(p, "start").clamp(a, b);
        if b <= a {
            return Err("Nothing to play".into());
        }
        let e = &self.engine;
        // A long sound at another rate than the device's takes a while to resample. That is
        // done before the start time is set, and without the lock, so the start is not cut.
        e.prepare(&s)?;
        let looped = p["loop"].as_bool().unwrap_or(false);
        let mut st = self.st.lock().unwrap();
        let now = e.now();
        let (over, fading) = std::mem::take(&mut st.play.fading).into_iter().partition(|f| f.1 <= now);
        st.play.fading = fading;
        e.drop_nodes(over.into_iter().map(|f| f.0).collect());
        let at = now + LEAD;
        let mut events = vec![];
        let fi = num(p, "fade_in");
        if fi > 0.0 {
            events.push(Ev::Set { t: at, v: 0.0 });
            events.push(Ev::Ramp { t: at + fi, v: 1.0 });
        }
        let fo = num(p, "fade_out");
        let dur = (b - start) as f64 / sr;
        if fo > 0.0 && !looped && dur > 4.0 * fo {
            events.push(Ev::Set { t: at + dur - fo, v: 1.0 });
            events.push(Ev::Ramp { t: at + dur, v: 0.0 });
        }
        // Each voice has a gain of its own, so one that is still fading out is not brought back
        // up by the next one starting.
        let fade = e.id();
        e.set_gain(fade, 1.0, events);
        let mut chain = vec![Link::Gain(fade)];
        if p["eq"].as_bool().unwrap_or(false) {
            chain.extend(self.eq_nodes(&mut st.play).iter().map(|id| Link::Filter(*id)));
        }
        chain.push(Link::Tap);
        let id = e.id();
        e.play(id, &s, at, start as f64 / sr, if looped { None } else { Some(dur) }, if looped { Some((a as f64 / sr, b as f64 / sr)) } else { None }, chain)?;
        st.play.voice = Some((id, fade));
        Ok(json!({"id": id, "at": at}))
    }

    fn eq_nodes<'a>(&self, play: &'a mut Play) -> &'a [u64] {
        if play.eq.is_empty() {
            play.eq = (0..EQ_NODES).map(|_| self.engine.id()).collect();
        }
        &play.eq
    }

    /// Fades the playing sound out over `fade` seconds (or stops it at once). With `only`, only
    /// when that is still the sound playing.
    pub fn stop_fading(&self, fade: f64, only: Option<u64>) {
        let mut st = self.st.lock().unwrap();
        if only.is_some() && st.play.voice.map(|v| v.0) != only {
            return;
        }
        let Some((id, node)) = st.play.voice.take() else { return };
        let e = &self.engine;
        if fade > 0.0 {
            let now = e.now();
            e.set_gain(node, 1.0, vec![Ev::Set { t: now, v: e.gain_now(node).unwrap_or(1.0) }, Ev::Ramp { t: now + fade, v: 0.0 }]);
            e.stop(id, now + fade);
            st.play.fading.push((node, now + fade));
        } else {
            e.stop(id, 0.0);
            e.drop_nodes(vec![node]);
        }
    }

    pub fn stop(&self) {
        self.stop_fading(0.0, None);
    }

    /// The live EQ on what plays: one filter per band that is on, in order.
    pub fn set_eq(&self, bands: &[Value]) {
        let mut st = self.st.lock().unwrap();
        let rate = self.engine.rate() as f64;
        for (i, id) in self.eq_nodes(&mut st.play).iter().enumerate() {
            self.engine.set_filter_coefs(*id, bands.get(i).and_then(|b| band_coefs(b, rate)));
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
        let d = Doc::new(Engine::silent());
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
    fn the_zoomed_out_outline_follows_the_sound_after_undo_and_another_edit() {
        // Normalize, undo, then turn it down: the normalized sound is freed on the way, and its
        // outline must not be drawn for the sound that replaces it.
        let d = Doc::new(Engine::silent());
        let n = 200_000;
        d.open(Sound::new(vec![(0..n).map(|i| 0.25 * (i as f32 * 0.01).sin()).collect()], 1000), None);
        let top = |d: &Doc| {
            let b = d.peaks(None, 0.0, 4000.0, 50).unwrap();
            b.chunks_exact(4).skip(50).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).fold(0.0f32, f32::max)
        };
        assert!((top(&d) - 0.25).abs() < 0.01);
        // Whether a freed address is handed out again at once is up to the allocator, so the
        // round is repeated until it would have been.
        for round in 0..40 {
            d.edit(&json!({"kind": "normalize", "db": 0, "ui": ui(None, 0)})).unwrap();
            assert!((top(&d) - 1.0).abs() < 0.01, "round {round}: normalized outline of {}", top(&d));
            d.undo(false, ui(None, 0)).unwrap();
            d.edit(&json!({"kind": "gain", "db": -20, "ui": ui(None, 0)})).unwrap();
            assert!((top(&d) - 0.025).abs() < 0.002, "round {round}: outline of {} for a sound peaking at 0.025", top(&d));
            d.undo(false, ui(None, 0)).unwrap();
        }
    }

    #[test]
    fn a_span_given_backwards_is_empty_and_harms_nothing() {
        let d = doc(1000);
        let before = d.peaks(None, 0.0, 1.0, 1000).unwrap();
        for kind in ["reverse", "silence", "gain", "delete", "respeed"] {
            let _ = d.edit(&json!({"kind": kind, "a": 700, "b": 200, "db": -6, "ratio": 2.0, "ui": ui(None, 0)}));
        }
        assert_eq!(d.peaks(None, 0.0, 1.0, 1000).unwrap(), before);
        for kind in ["stats", "loud", "peak", "measure", "onsets"] {
            d.analyse(&json!({"kind": kind, "a": 700, "b": 200})).unwrap();
        }
        for op in ["add", "grid", "hits"] {
            assert!(d.regions(&json!({"op": op, "a": 700, "b": 200, "step": 50.0}), ui(None, 0)).is_err());
        }
        assert!(d.play(&json!({"a": 700, "b": 200, "start": 0})).is_err());
        d.regions(&json!({"op": "add", "a": 200, "b": 700}), ui(None, 0)).unwrap();
    }

    #[test]
    fn work_from_a_sound_that_was_replaced_meanwhile_is_refused() {
        let d = doc(1000);
        let was = d.cur().unwrap();
        assert!(unchanged(&d.st.lock().unwrap(), &was).is_ok());
        d.open(Sound::new(vec![vec![0.0; 10]], 1000), None);
        assert!(unchanged(&d.st.lock().unwrap(), &was).is_err());
    }

    #[test]
    fn a_voice_fading_out_keeps_its_own_gain_when_the_next_one_starts() {
        let d = doc(1000);
        let first = d.play(&json!({"a": 0, "b": 1000, "start": 0})).unwrap()["id"].as_u64().unwrap();
        let (_, fading) = d.st.lock().unwrap().play.voice.unwrap();
        d.stop_fading(0.02, Some(first));
        d.play(&json!({"a": 0, "b": 1000, "start": 0})).unwrap();
        let st = d.st.lock().unwrap();
        assert_eq!(st.play.fading, vec![(fading, 0.02)]);
        assert_ne!(st.play.voice.unwrap().1, fading);
    }

    #[test]
    fn a_band_that_is_off_is_no_filter() {
        let band = |on: bool| json!({"on": on, "type": "bell", "f": 1000.0, "q": 1.0, "g": 6.0});
        assert!(band_coefs(&band(false), 48_000.0).is_none());
        let curve = eq_response(&[band(false), band(true)], 48_000.0, &[1000.0]);
        assert_eq!(curve[0][0], 0.0);
        assert!((curve[1][0] - 6.0).abs() < 0.01);
        let chs = vec![vec![0.5f32; 64]];
        assert_eq!(render_eq(&chs, 48_000, &[band(false)]), chs);
    }

    #[test]
    fn noise_reduction_runs_from_a_learned_print_and_from_an_estimate() {
        let d = doc(8192);
        let fx = |noise: Option<&str>| json!({"kind": "fx", "tab": "nr", "p": {}, "noise": noise, "ui": ui(None, 0)});
        d.edit(&fx(None)).unwrap();
        d.learn_noise(4096, 0).unwrap();
        assert_eq!(d.analyse(&json!({"kind": "profile"})).unwrap().as_array().unwrap().len(), d.st.lock().unwrap().noise.as_ref().unwrap().len());
        d.edit(&fx(Some("print"))).unwrap();
        d.view("head", &fx(Some("print")), Some(0.5)).unwrap();
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
