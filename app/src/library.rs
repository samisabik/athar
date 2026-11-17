//! The sound library: the SQLite index, the in-memory search index, background indexing,
//! the catalog (tags, notes, ratings), projects and their pads.

use athar_core::analysis::{analyze, peaks};
use athar_core::decode::{clips, decode, load_audio};
use athar_core::embed::Clap;
use rayon::prelude::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf, MAIN_SEPARATOR};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use unicode_normalization::UnicodeNormalization;

pub type Res<T> = Result<T, String>;

pub const AUDIO_EXTS: [&str; 9] = ["wav", "aif", "aiff", "flac", "mp3", "ogg", "m4a", "aac", "wv"];
const EMB_DIM: usize = 512;
const BATCH: usize = 32;
const HASH_SPAN: u64 = 1 << 20;
const BACKUPS_KEPT: usize = 10;
pub const SP404_PADS: i64 = 160;
const MAP_FINE_LIMIT: usize = 3000;
const ARRANGE_MAX_LANES: usize = 16;
const ARRANGE_MAX_HITS: usize = 64;
const ARRANGE_MAX_SECONDS: f64 = 600.0;

pub const CATEGORIES: [(&str, &str); 41] = [
    ("Kick", "a kick drum hit"), ("Snare", "a snare drum hit"), ("Clap", "a hand clap drum sample"),
    ("Hi-hat", "a hi-hat cymbal"), ("Cymbal", "a crash or ride cymbal"), ("Tom", "a tom drum hit"),
    ("Percussion", "a percussion hit such as shaker, conga or rim"), ("Drum loop", "a drum loop with a beat"),
    ("Bass", "a bass synth or bass guitar"), ("808", "an 808 sub bass"), ("Synth lead", "a synth lead melody"),
    ("Synth pad", "an ambient synth pad"), ("Pluck", "a plucked synth note"), ("Chord", "a chord stab"),
    ("Keys", "piano or electric piano"), ("Guitar", "an electric or acoustic guitar"), ("Strings", "orchestral strings"),
    ("Brass", "brass instruments"), ("Vocal", "a human voice singing or speaking"), ("FX", "a sound effect"),
    ("Riser", "a riser or sweep building up"), ("Impact", "a cinematic impact or boom"),
    ("Texture", "an atmospheric drone or texture"), ("Foley", "a foley sound of everyday objects"),
    ("Birds", "birds singing or calling"), ("Insects", "insects buzzing or crickets chirping"),
    ("Animals", "animal sounds such as dogs, cows or frogs"), ("Water", "running water, a stream or waves"),
    ("Rain", "rain falling"), ("Wind", "wind blowing"), ("Thunder", "thunder and storm"), ("Fire", "a crackling fire"),
    ("Footsteps", "footsteps walking"), ("Voices", "people talking in a crowd"), ("Traffic", "cars and city traffic"),
    ("Machinery", "machines, engines or motors"), ("Bells", "bells ringing"), ("Doors", "a door opening, closing or creaking"),
    ("Metal", "metal objects hitting or scraping"), ("Wood", "wooden objects knocking or creaking"),
    ("Paper", "paper rustling or crumpling"),
];
/// Text with no meaning, whose scores show which sounds match any text at all.
const NONSENSE: [&str; 5] = ["xq vlorp", "mmbrrt kazoodle", "fhqwhgads", "plinth snorf wub", "qwertyuiop"];
/// Above nonsense: the best a query must score to count as finding anything, and what each sound keeps.
const QUERY_FOUND: f32 = 0.25;
const QUERY_KEEP: f32 = 0.15;
const ROOM_TONE: (&str, &str) = ("Room tone", "quiet room tone or background hum");

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS folders (path TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS files (
    path TEXT PRIMARY KEY, folder TEXT, mtime REAL, size INTEGER, sha1 TEXT,
    duration REAL, sr INTEGER, channels INTEGER,
    peak_db REAL, rms_db REAL, crest REAL, brightness REAL,
    pitch_hz REAL, tempo REAL, onsets INTEGER, is_loop INTEGER,
    emb BLOB, peaks BLOB, error TEXT, recorded TEXT
);
CREATE INDEX IF NOT EXISTS files_folder ON files(folder);
CREATE INDEX IF NOT EXISTS files_sha1 ON files(sha1);
CREATE TABLE IF NOT EXISTS gone (path TEXT PRIMARY KEY, sha1 TEXT, at REAL);
CREATE INDEX IF NOT EXISTS gone_sha1 ON gone(sha1);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS catalog (
    path TEXT PRIMARY KEY, tags TEXT, memo TEXT, rating INTEGER, done INTEGER DEFAULT 0,
    recorded TEXT, updated REAL, origin TEXT
);
CREATE TABLE IF NOT EXISTS regions (path TEXT PRIMARY KEY, data TEXT);
CREATE TABLE IF NOT EXISTS projects (id INTEGER PRIMARY KEY, name TEXT UNIQUE, folder TEXT);
CREATE TABLE IF NOT EXISTS project_items (
    project_id INTEGER, path TEXT, added REAL, pos REAL, pad INTEGER, PRIMARY KEY (project_id, path)
);
";

pub fn default_settings() -> Value {
    json!({
        "text_size": 13, "show_cat": true, "cat_skin": "sukey", "accent": "#7a3dff", "autoplay": false,
        "preview_volume": 80, "results_limit": 250, "default_sort": "name", "hide_shorter_ms": 0,
        "rescan_on_open": true, "export_rate": 0, "export_bits": 24, "export_mono": false, "edit_suffix": "_edit",
        "export_format": "wav", "smooth_edits": true, "edit_fade_ms": 3, "loudness_target": -16.0,
        "prep_target_db": -1.0, "prep_fade_in_ms": 2, "prep_fade_out_ms": 10, "silence_db": -45,
        "slice_gap_ms": 250, "sp404_mono": false, "name_pattern": "{project}_{name}", "projects_dir": "",
        "theme": "system", "freesound_key": ""
    })
}

// ---------- small helpers

pub fn nfc(s: &str) -> String {
    s.nfc().collect()
}

/// True when path lies strictly below folder, comparing NFC forms.
pub fn is_inside(path: &str, folder: &str) -> bool {
    let (p, f) = (nfc(path), nfc(folder));
    let f = format!("{}{}", f.trim_end_matches(MAIN_SEPARATOR), MAIN_SEPARATOR);
    p != f && p.starts_with(&f)
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// mtime the way Python reports st_mtime, so files indexed by either agree.
fn mtime_of(md: &std::fs::Metadata) -> f64 {
    md.mtime() as f64 + md.mtime_nsec() as f64 * 1e-9
}

fn basename(p: &str) -> String {
    Path::new(p).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn safe_name(s: &str) -> String {
    let mut out = String::new();
    let mut in_bad = false;
    for c in s.chars() {
        if c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ' ') {
            out.push(c);
            in_bad = false;
        } else if !in_bad {
            out.push('_');
            in_bad = true;
        }
    }
    let t = out.trim().trim_start_matches(['.', ' ']).to_string();
    if t.is_empty() { "untitled".into() } else { t }
}

pub fn ascii_name(s: &str) -> String {
    let a: String = s.nfkd().filter(|c| c.is_ascii()).collect();
    let mut out = String::new();
    let mut in_bad = false;
    for c in a.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
            in_bad = false;
        } else if !in_bad {
            out.push('_');
            in_bad = true;
        }
    }
    let t = out.trim_matches('_').to_string();
    if t.is_empty() { "sample".into() } else { t }
}

pub fn unique_path(p: &Path) -> Res<PathBuf> {
    if !p.exists() {
        return Ok(p.to_path_buf());
    }
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for i in 2..10000 {
        let c = p.with_file_name(format!("{stem} ({i}){ext}"));
        if !c.exists() {
            return Ok(c);
        }
    }
    Err(format!("{} already exists", p.display()))
}

/// Identity of a file's content: its size plus its first and last megabyte.
fn file_hash(path: &str, size: u64) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut h = sha1_smol::Sha1::new();
    h.update(size.to_string().as_bytes());
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; HASH_SPAN as usize];
    let n = f.read(&mut buf)?;
    h.update(&buf[..n]);
    if size > 2 * HASH_SPAN {
        f.seek(SeekFrom::Start(size - HASH_SPAN))?;
        let mut tail = Vec::new();
        f.take(HASH_SPAN).read_to_end(&mut tail)?;
        h.update(&tail);
    }
    Ok(h.digest().to_string())
}

/// Recording time from a WAV's BWF 'bext' chunk, otherwise the file's creation time.
fn recorded_date(path: &str) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let lower = path.to_lowercase();
    if lower.ends_with(".wav") || lower.ends_with(".wave") || lower.ends_with(".bwf") {
        if let Ok(mut f) = std::fs::File::open(path) {
            let mut head = [0u8; 12];
            if f.read_exact(&mut head).is_ok() && (&head[..4] == b"RIFF" || &head[..4] == b"RF64") && &head[8..12] == b"WAVE" {
                for _ in 0..64 {
                    let mut hdr = [0u8; 8];
                    if f.read_exact(&mut hdr).is_err() {
                        break;
                    }
                    let size = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as u64;
                    if &hdr[..4] == b"bext" {
                        let mut data = vec![0u8; size.min(340) as usize];
                        let _ = f.read_exact(&mut data);
                        if data.len() >= 338 {
                            let d: String = String::from_utf8_lossy(&data[320..330]).trim_matches(['\0', ' ']).to_string();
                            let t: String = String::from_utf8_lossy(&data[330..338]).trim_matches(['\0', ' ']).to_string();
                            if d.len() == 10 && d.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                                let t5: String = t.chars().take(5).collect::<String>().replace(['-', '.'], ":");
                                return Some(format!("{} {}", d.replace([':', '/'], "-"), t5).trim().to_string());
                            }
                        }
                        break;
                    }
                    if f.seek(SeekFrom::Current((size + (size & 1)) as i64)).is_err() {
                        break;
                    }
                }
            }
        }
    }
    let md = std::fs::metadata(path).ok()?;
    let t = md.created().or_else(|_| md.modified()).ok()?;
    let dt: chrono::DateTime<chrono::Local> = t.into();
    Some(dt.format("%Y-%m-%d %H:%M").to_string())
}

fn note_name(hz: Option<f64>) -> Option<String> {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let hz = hz.filter(|h| *h > 0.0)?;
    let midi = (12.0 * (hz / 440.0).log2() + 69.0).round_ties_even() as i64;
    Some(format!("{}{}", NAMES[midi.rem_euclid(12) as usize], midi.div_euclid(12) - 1))
}

// ---------- the in-memory index

#[derive(Clone, Serialize, Default)]
pub struct Meta {
    pub path: String,
    pub folder: String,
    pub duration: Option<f64>,
    pub sr: Option<i64>,
    pub channels: Option<i64>,
    pub peak_db: Option<f64>,
    pub rms_db: Option<f64>,
    pub crest: Option<f64>,
    pub brightness: Option<f64>,
    pub pitch_hz: Option<f64>,
    pub tempo: Option<f64>,
    pub is_loop: bool,
    #[serde(skip)]
    pub peaks: Vec<u8>,
    pub recorded: Option<String>,
}

struct Item {
    meta: Meta,
    emb: Vec<f32>,
    cat: Option<(usize, f32)>,
}

#[derive(Default)]
struct Store {
    items: Vec<Item>,
    pos: HashMap<String, usize>,
    version: u64,
}

impl Store {
    fn put(&mut self, meta: Meta, emb: Vec<f32>, cat: Option<(usize, f32)>) {
        self.version += 1;
        match self.pos.get(&meta.path) {
            Some(&i) => self.items[i] = Item { meta, emb, cat },
            None => {
                self.pos.insert(meta.path.clone(), self.items.len());
                self.items.push(Item { meta, emb, cat });
            }
        }
    }

    fn drop_path(&mut self, path: &str) {
        if let Some(i) = self.pos.remove(path) {
            self.version += 1;
            let last = self.items.len() - 1;
            self.items.swap_remove(i);
            if i != last {
                let moved = self.items[i].meta.path.clone();
                self.pos.insert(moved, i);
            }
        }
    }

    fn rename(&mut self, old: &str, new: &str, folder: &str) {
        if old == new {
            return;
        }
        self.drop_path(new);
        if let Some(i) = self.pos.remove(old) {
            self.version += 1;
            self.items[i].meta.path = new.to_string();
            self.items[i].meta.folder = folder.to_string();
            self.pos.insert(new.to_string(), i);
        }
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

// ---------- the library

enum Job {
    Folder(String),
    Files(String, Vec<String>),
}

#[derive(Serialize, Clone, Default)]
pub struct Status {
    pub model: String,
    pub error: Option<String>,
    pub queued: usize,
    pub done: usize,
    pub current: Option<String>,
    pub folder: Option<String>,
}

pub struct Library {
    db: Mutex<Connection>,
    store: RwLock<Store>,
    engine: OnceLock<Clap>,
    categories: OnceLock<(Vec<String>, Vec<Vec<f32>>)>,
    nonsense: OnceLock<Vec<f32>>,
    status: Mutex<Status>,
    jobs: Mutex<Sender<Job>>,
    pending: AtomicUsize,
    map: Mutex<Option<(u64, Value)>>,
    map_busy: std::sync::atomic::AtomicBool,
    model: PathBuf,
    watcher: Mutex<Option<Sender<(bool, String)>>>,
    changed: Arc<Mutex<HashSet<String>>>,
    pub data_dir: PathBuf,
}

fn row_meta(r: &rusqlite::Row) -> rusqlite::Result<(Meta, Vec<u8>)> {
    Ok((
        Meta {
            path: r.get("path")?,
            folder: r.get::<_, Option<String>>("folder")?.unwrap_or_default(),
            duration: r.get("duration")?,
            sr: r.get("sr")?,
            channels: r.get("channels")?,
            peak_db: r.get("peak_db")?,
            rms_db: r.get("rms_db")?,
            crest: r.get("crest")?,
            brightness: r.get("brightness")?,
            pitch_hz: r.get("pitch_hz")?,
            tempo: r.get("tempo")?,
            is_loop: r.get::<_, Option<i64>>("is_loop")?.unwrap_or(0) != 0,
            peaks: r.get::<_, Option<Vec<u8>>>("peaks")?.unwrap_or_default(),
            recorded: r.get("recorded")?,
        },
        r.get::<_, Vec<u8>>("emb")?,
    ))
}

fn emb_of(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

fn emb_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Where this app keeps its index: its own folder, so trying it never touches the index the
/// Python app uses. On first launch it starts from a copy of that index, when there is one.
pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("ATHAR_HOME") {
        return PathBuf::from(d);
    }
    let base = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Library/Application Support");
    let d = base.join("Athar Next");
    if !d.join("index.db").exists() {
        let _ = std::fs::create_dir_all(&d);
        let old = base.join("Athar/index.db");
        if old.exists() {
            let _ = std::fs::copy(&old, d.join("index.db"));
        }
    }
    d
}

/// The model shipped in the app bundle, otherwise the copies made by packaging/export_model.py
/// and packaging/quantize_audio.py. ATHAR_MODEL points at another folder.
pub fn model_dir(resources: Option<&Path>) -> PathBuf {
    if let Ok(m) = std::env::var("ATHAR_MODEL") {
        return PathBuf::from(m);
    }
    if let Some(r) = resources.map(|r| r.join("model")).filter(|m| m.join("audio.onnx").exists()) {
        return r;
    }
    let build = Path::new(env!("CARGO_MANIFEST_DIR")).join("../build");
    if build.join("model-next/audio.onnx").exists() {
        return build.join("model-next");
    }
    build.join("model")
}

impl Library {
    pub fn open(model: PathBuf) -> Res<Arc<Self>> {
        let data_dir = data_dir();
        std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let db_path = data_dir.join("index.db");
        let db = Connection::open(&db_path).map_err(|e| e.to_string())?;
        db.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        // Older indexes may lack columns added later; adding them twice fails harmlessly.
        for sql in [
            "ALTER TABLE files ADD COLUMN recorded TEXT",
            "ALTER TABLE projects ADD COLUMN folder TEXT",
            "ALTER TABLE project_items ADD COLUMN pos REAL",
            "ALTER TABLE catalog ADD COLUMN origin TEXT",
            "ALTER TABLE project_items ADD COLUMN pad INTEGER",
        ] {
            let _ = db.execute(sql, []);
        }
        let mut store = Store::default();
        {
            let mut st = db.prepare("SELECT * FROM files WHERE emb IS NOT NULL").map_err(|e| e.to_string())?;
            let rows = st.query_map([], row_meta).map_err(|e| e.to_string())?;
            for (m, blob) in rows.flatten() {
                store.put(m, emb_of(&blob), None);
            }
        }
        let (tx, rx) = channel();
        let lib = Arc::new(Self {
            db: Mutex::new(db),
            store: RwLock::new(store),
            engine: OnceLock::new(),
            categories: OnceLock::new(),
            nonsense: OnceLock::new(),
            status: Mutex::new(Status { model: "loading".into(), ..Default::default() }),
            jobs: Mutex::new(tx),
            pending: AtomicUsize::new(0),
            map: Mutex::new(None),
            map_busy: std::sync::atomic::AtomicBool::new(false),
            model,
            watcher: Mutex::new(None),
            changed: Arc::new(Mutex::new(HashSet::new())),
            data_dir,
        });
        lib.daily_backup();
        let l = lib.clone();
        std::thread::spawn(move || l.boot(rx));
        Ok(lib)
    }

    fn boot(self: Arc<Self>, rx: Receiver<Job>) {
        match Clap::load(&self.model) {
            Ok(clap) => {
                let mut texts: Vec<&str> = CATEGORIES.iter().map(|c| c.1).collect();
                texts.push(ROOM_TONE.1);
                let names: Vec<String> = CATEGORIES.iter().map(|c| c.0.to_string()).chain([ROOM_TONE.0.to_string()]).collect();
                match clap.embed_texts(&texts) {
                    Ok(m) => {
                        let _ = self.categories.set((names, m));
                    }
                    Err(e) => self.status.lock().unwrap().error = Some(e.to_string()),
                }
                if let Ok(m) = clap.embed_texts(&NONSENSE) {
                    let mean = (0..m[0].len()).map(|d| m.iter().map(|v| v[d]).sum::<f32>() / m.len() as f32).collect();
                    let _ = self.nonsense.set(mean);
                }
                let _ = self.engine.set(clap);
                self.assign_categories();
                self.status.lock().unwrap().model = "ready".into();
            }
            Err(e) => {
                let mut s = self.status.lock().unwrap();
                s.model = "error".into();
                s.error = Some(format!("{e} (model folder {})", self.model.display()));
                return;
            }
        }
        self.start_watching();
        self.ensure_project_folders();
        if self.settings()["rescan_on_open"].as_bool().unwrap_or(true) {
            self.rescan();
        }
        while let Ok(job) = rx.recv() {
            match job {
                Job::Folder(f) => self.scan(&f),
                Job::Files(f, paths) => self.index(&f, paths),
            }
            self.pending.fetch_sub(1, Ordering::SeqCst);
            let mut s = self.status.lock().unwrap();
            s.current = None;
            s.folder = None;
        }
    }

    fn category_of(&self, emb: &[f32]) -> Option<(usize, f32)> {
        let (_, m) = self.categories.get()?;
        m.iter().enumerate().map(|(i, c)| (i, dot(emb, c))).max_by(|a, b| a.1.total_cmp(&b.1))
    }

    fn assign_categories(&self) {
        let mut s = self.store.write().unwrap();
        let cats: Vec<_> = s.items.par_iter().map(|it| self.category_of(&it.emb)).collect();
        for (it, c) in s.items.iter_mut().zip(cats) {
            it.cat = c;
        }
        s.version += 1;
    }

    fn category_names(&self) -> &[String] {
        self.categories.get().map(|c| c.0.as_slice()).unwrap_or(&[])
    }

    fn enqueue(&self, job: Job) {
        self.pending.fetch_add(1, Ordering::SeqCst);
        let _ = self.jobs.lock().unwrap().send(job);
    }

    /// Tags, notes, ratings, projects and settings as dated JSON beside the index: the first
    /// launch of each day writes one, and the last BACKUPS_KEPT are kept.
    fn daily_backup(&self) {
        let rows = |sql: &str, f: &dyn Fn(&rusqlite::Row) -> rusqlite::Result<Value>| -> Vec<Value> {
            let db = self.db();
            let mut st = match db.prepare(sql) {
                Ok(st) => st,
                Err(_) => return vec![],
            };
            st.query_map([], |r| f(r)).map(|it| it.flatten().collect()).unwrap_or_default()
        };
        let catalog = rows("SELECT path, tags, memo, rating, updated FROM catalog", &|r| {
            Ok(json!({"path": r.get::<_, String>(0)?, "tags": r.get::<_, Option<String>>(1)?, "memo": r.get::<_, Option<String>>(2)?, "rating": r.get::<_, Option<i64>>(3)?, "updated": r.get::<_, Option<f64>>(4)?}))
        });
        let projects = rows("SELECT id, name FROM projects", &|r| Ok(json!({"id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?})));
        if catalog.is_empty() && projects.is_empty() {
            return;
        }
        let items = rows("SELECT project_id, path, added, pad FROM project_items", &|r| {
            Ok(json!({"project_id": r.get::<_, i64>(0)?, "path": r.get::<_, String>(1)?, "added": r.get::<_, Option<f64>>(2)?, "pad": r.get::<_, Option<i64>>(3)?}))
        });
        let dir = self.data_dir.join("backups");
        let _ = std::fs::create_dir_all(&dir);
        let today = dir.join(format!("catalog-{}.json", chrono::Local::now().format("%Y-%m-%d")));
        if !today.exists() {
            let data = json!({"catalog": catalog, "projects": projects, "project_items": items, "folders": self.folders(), "settings": self.settings()});
            let _ = std::fs::write(&today, data.to_string());
        }
        let mut old: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("catalog-"))).collect())
            .unwrap_or_default();
        old.sort();
        if old.len() > BACKUPS_KEPT {
            for p in &old[..old.len() - BACKUPS_KEPT] {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    // ---------- db helpers

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap()
    }

    fn meta_get(&self, key: &str) -> Option<String> {
        self.db().query_row("SELECT value FROM meta WHERE key=?", [key], |r| r.get(0)).optional().ok().flatten()
    }

    fn meta_set(&self, key: &str, value: &str) -> Res<()> {
        self.db().execute("INSERT OR REPLACE INTO meta VALUES (?, ?)", params![key, value]).map(|_| ()).map_err(|e| e.to_string())
    }

    // ---------- status and settings

    pub fn status(&self) -> Value {
        let s = self.status.lock().unwrap().clone();
        let mut v = serde_json::to_value(s).unwrap();
        v["indexed"] = json!(self.store.read().unwrap().items.len());
        v["pending_jobs"] = json!(self.pending.load(Ordering::SeqCst));
        v["device"] = json!(if self.engine.get().is_some() { Some("cpu") } else { None });
        v
    }

    pub fn settings(&self) -> Value {
        let mut st = default_settings();
        if let Some(saved) = self.meta_get("settings").and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
            if let (Some(o), Some(saved)) = (st.as_object_mut(), saved.as_object()) {
                for (k, v) in saved {
                    if o.contains_key(k) {
                        o.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        if st["projects_dir"].as_str().unwrap_or("").is_empty() {
            st["projects_dir"] = json!(PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("Music/Athar Projects"));
        }
        st
    }

    pub fn update_settings(&self, changes: &Value) -> Res<Value> {
        let defaults = default_settings();
        let mut merged = self.settings();
        if let Some(ch) = changes.as_object() {
            for (k, v) in ch {
                let Some(d) = defaults.get(k) else { continue };
                let coerced = match d {
                    Value::Bool(_) => json!(v.as_bool().unwrap_or_else(|| v.as_f64().map(|n| n != 0.0).unwrap_or(false))),
                    Value::Number(n) if n.is_i64() => json!(v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).ok_or(format!("{k} must be a number"))?.round() as i64),
                    Value::Number(_) => json!(v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).ok_or(format!("{k} must be a number"))?),
                    _ => json!(v.as_str().map(String::from).unwrap_or_else(|| v.to_string())),
                };
                merged[k] = coerced;
            }
        }
        self.meta_set("settings", &merged.to_string())?;
        Ok(merged)
    }

    // ---------- folders

    pub fn folders(&self) -> Vec<String> {
        let db = self.db();
        let mut st = db.prepare("SELECT path FROM folders ORDER BY path").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().flatten().collect()
    }

    fn locate(&self, path: &str, folders: &[String]) -> Option<(String, String)> {
        let mut fs: Vec<&String> = folders.iter().collect();
        fs.sort_by_key(|f| std::cmp::Reverse(f.len()));
        fs.into_iter().find(|f| is_inside(path, f)).map(|f| {
            let rel = &nfc(path)[nfc(f).trim_end_matches(MAIN_SEPARATOR).len() + 1..];
            (f.clone(), format!("{}{}{}", f.trim_end_matches(MAIN_SEPARATOR), MAIN_SEPARATOR, rel))
        })
    }

    pub fn folder_counts(&self) -> Value {
        let folders = self.folders();
        let db = self.db();
        let mut counts: HashMap<String, (i64, i64, i64)> = HashMap::new();
        let mut st = db.prepare("SELECT folder, COUNT(*), SUM(emb IS NOT NULL), COALESCE(SUM(size), 0) FROM files GROUP BY folder").unwrap();
        for r in st.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().flatten() {
            counts.insert(r.0.unwrap_or_default(), (r.1, r.2, r.3));
        }
        Value::Array(
            folders
                .iter()
                .map(|f| {
                    let c = counts.get(f).copied().unwrap_or((0, 0, 0));
                    json!({"path": f, "files": c.0, "indexed": c.1, "bytes": c.2, "offline": !Path::new(f).is_dir()})
                })
                .collect(),
        )
    }

    pub fn add_folder(&self, path: &str) -> Res<String> {
        let p = std::fs::canonicalize(expand(path)).map_err(|_| format!("Not a folder: {path}"))?;
        if !p.is_dir() {
            return Err(format!("Not a folder: {}", p.display()));
        }
        let p = p.to_string_lossy().into_owned();
        for f in self.folders() {
            if is_inside(&p, &f) {
                return Err(format!("Already in the library as part of {f}"));
            }
            if is_inside(&f, &p) {
                return Err(format!("Contains the library folder {f}; remove that one first"));
            }
        }
        self.db().execute("INSERT OR IGNORE INTO folders(path) VALUES (?)", [&p]).map_err(|e| e.to_string())?;
        self.watch(&p);
        self.enqueue(Job::Folder(p.clone()));
        Ok(p)
    }

    pub fn remove_folder(&self, path: &str) {
        if let Some(w) = self.watcher.lock().unwrap().as_ref() {
            let _ = w.send((false, path.to_string()));
        }
        let gone: Vec<String> = {
            let db = self.db();
            let _ = db.execute("DELETE FROM folders WHERE path=?", [path]);
            let mut st = db.prepare("SELECT path FROM files WHERE folder=?").unwrap();
            let gone = st.query_map([path], |r| r.get(0)).unwrap().flatten().collect();
            let _ = db.execute("DELETE FROM files WHERE folder=?", [path]);
            gone
        };
        let mut s = self.store.write().unwrap();
        for p in gone {
            s.drop_path(&p);
        }
    }

    pub fn rescan(&self) {
        for f in self.folders() {
            self.enqueue(Job::Folder(f));
        }
    }

    pub fn index_file(&self, path: &str) {
        if let Some((folder, p)) = self.locate(path, &self.folders()) {
            self.enqueue(Job::Files(folder, vec![p]));
        }
    }

    pub fn subfolders(&self, path: &str) -> Value {
        let folders = self.folders();
        let (folder, path) = match folders.iter().find(|f| nfc(f) == nfc(path)) {
            Some(f) => (f.clone(), f.clone()),
            None => match self.locate(path, &folders) {
                Some(x) => x,
                None => return json!([]),
            },
        };
        let base = format!("{}{}", nfc(&path).trim_end_matches(MAIN_SEPARATOR), MAIN_SEPARATOR);
        let mut counts: HashMap<String, i64> = HashMap::new();
        let mut deeper: HashSet<String> = HashSet::new();
        {
            let db = self.db();
            let mut st = db.prepare("SELECT path FROM files WHERE folder=?").unwrap();
            for p in st.query_map([&folder], |r| r.get::<_, String>(0)).unwrap().flatten() {
                let p = nfc(&p);
                let Some(rest) = p.strip_prefix(&base) else { continue };
                let parts: Vec<&str> = rest.split(MAIN_SEPARATOR).collect();
                if parts.len() > 1 {
                    *counts.entry(parts[0].to_string()).or_default() += 1;
                    if parts.len() > 2 {
                        deeper.insert(parts[0].to_string());
                    }
                }
            }
        }
        if let Ok(rd) = std::fs::read_dir(&path) {
            for e in rd.flatten() {
                let name = nfc(&e.file_name().to_string_lossy());
                if e.path().is_dir() && !name.starts_with('.') {
                    counts.entry(name).or_insert(0);
                }
            }
        }
        let mut names: Vec<_> = counts.into_iter().collect();
        names.sort_by_key(|(n, _)| n.to_lowercase());
        Value::Array(names.into_iter().map(|(n, c)| json!({"path": format!("{base}{n}"), "name": n, "files": c, "nested": deeper.contains(&n)})).collect())
    }

    // ---------- live watching: new, changed, moved and deleted files show up without a rescan

    fn start_watching(self: &Arc<Self>) {
        let changed = self.changed.clone();
        // The watcher lives on its own thread and takes requests from a channel: FSEvents
        // must not be started or stopped from the main thread, which would deadlock.
        let (tx, rx) = channel::<(bool, String)>();
        std::thread::spawn(move || {
            use notify::Watcher;
            let w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(ev) = res {
                    let mut c = changed.lock().unwrap();
                    for p in ev.paths {
                        c.insert(p.to_string_lossy().into_owned());
                    }
                }
            });
            let mut w = match w {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("folder watching unavailable: {e}");
                    return;
                }
            };
            while let Ok((on, folder)) = rx.recv() {
                let _ = if on { w.watch(Path::new(&folder), notify::RecursiveMode::Recursive) } else { w.unwatch(Path::new(&folder)) };
            }
        });
        *self.watcher.lock().unwrap() = Some(tx);
        for f in self.folders() {
            self.watch(&f);
        }
        let me = self.clone();
        std::thread::spawn(move || loop {
            // Changes are gathered for a moment, so a file still being written is indexed once.
            std::thread::sleep(std::time::Duration::from_secs(2));
            me.flush_changes();
        });
    }

    fn watch(&self, folder: &str) {
        if let Some(w) = self.watcher.lock().unwrap().as_ref() {
            let _ = w.send((true, folder.to_string()));
        }
    }

    fn flush_changes(&self) {
        let batch: Vec<String> = std::mem::take(&mut *self.changed.lock().unwrap()).into_iter().collect();
        if batch.is_empty() {
            return;
        }
        let folders = self.folders();
        let mut by_folder: HashMap<String, Vec<String>> = HashMap::new();
        let mut rescan: HashSet<String> = HashSet::new();
        for p in batch {
            if folders.contains(&p) {
                continue;
            }
            let Some((folder, p)) = self.locate(&p, &folders) else { continue };
            let rel = &p[folder.len()..];
            if rel.split(MAIN_SEPARATOR).any(|c| c.starts_with('.')) {
                continue;
            }
            let path = Path::new(&p);
            if path.is_dir() {
                rescan.insert(folder);
                continue;
            }
            if !path.exists() {
                if self.store.read().unwrap().pos.contains_key(&p) {
                    self.forget(&p, false);
                } else {
                    let prefix = format!("{p}{MAIN_SEPARATOR}");
                    let under: Vec<String> = self.store.read().unwrap().items.iter().map(|it| it.meta.path.clone()).filter(|x| x.starts_with(&prefix)).collect();
                    for x in under {
                        self.forget(&x, false);
                    }
                }
                continue;
            }
            let ext = path.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !AUDIO_EXTS.contains(&ext.as_str()) {
                continue;
            }
            let Ok(md) = std::fs::metadata(path) else { continue };
            let known: Option<(f64, i64)> = self.db().query_row("SELECT mtime, size FROM files WHERE path=?", [&p], |r| Ok((r.get(0)?, r.get(1)?))).optional().ok().flatten();
            if known != Some((mtime_of(&md), md.size() as i64)) {
                by_folder.entry(folder).or_default().push(p);
            }
        }
        for f in rescan {
            self.enqueue(Job::Folder(f));
        }
        for (f, paths) in by_folder {
            self.enqueue(Job::Files(f, paths));
        }
    }

    // ---------- indexing

    fn scan(&self, folder: &str) {
        if !Path::new(folder).is_dir() {
            return;
        }
        let others: HashSet<String> = self.folders().into_iter().filter(|f| f != folder).collect();
        let known: HashMap<String, (f64, i64)> = {
            let db = self.db();
            let mut st = db.prepare("SELECT path, mtime, size FROM files WHERE folder=?").unwrap();
            st.query_map([folder], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, Option<f64>>(1)?.unwrap_or(0.0), r.get::<_, Option<i64>>(2)?.unwrap_or(0)))))
                .unwrap()
                .flatten()
                .collect()
        };
        let mut seen = HashSet::new();
        let mut todo = Vec::new();
        let mut stack = vec![PathBuf::from(folder)];
        let mut unreadable = Vec::new();
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                unreadable.push(dir.to_string_lossy().into_owned());
                continue;
            };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let p = e.path();
                let ps = p.to_string_lossy().into_owned();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if !others.contains(&ps) {
                        stack.push(p);
                    }
                    continue;
                }
                let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
                if !AUDIO_EXTS.contains(&ext.as_str()) {
                    continue;
                }
                let Ok(md) = std::fs::metadata(&p) else { continue };
                seen.insert(ps.clone());
                if known.get(&ps) != Some(&(mtime_of(&md), md.size() as i64)) {
                    todo.push(ps);
                }
            }
        }
        for gone in known.keys().filter(|k| !seen.contains(*k)) {
            if !unreadable.iter().any(|u| gone == u || is_inside(gone, u)) {
                self.forget(gone, false);
            }
        }
        // Files indexed before recording dates were kept get theirs now.
        let undated: Vec<String> = {
            let db = self.db();
            let mut st = db.prepare("SELECT path FROM files WHERE folder=? AND recorded IS NULL AND emb IS NOT NULL").unwrap();
            st.query_map([folder], |r| r.get(0)).unwrap().flatten().filter(|p: &String| seen.contains(p) && !todo.contains(p)).collect()
        };
        for p in undated {
            let rec = recorded_date(&p);
            let _ = self.db().execute("UPDATE files SET recorded=? WHERE path=?", params![rec, p]);
            let mut s = self.store.write().unwrap();
            if let Some(&i) = s.pos.get(&p) {
                s.items[i].meta.recorded = rec;
            }
        }
        todo.sort();
        self.index(folder, todo);
    }

    fn index(&self, folder: &str, paths: Vec<String>) {
        let Some(clap) = self.engine.get() else { return };
        {
            let mut s = self.status.lock().unwrap();
            s.folder = Some(folder.to_string());
            s.queued = paths.len();
            s.done = 0;
        }
        for (b, batch) in paths.chunks(BATCH).enumerate() {
            if !self.folders().iter().any(|f| f == folder) {
                return;
            }
            self.status.lock().unwrap().current = batch.first().map(|p| basename(p));
            // Decoding, analysis and embedding run side by side; the model sessions are shared out.
            let done: Vec<(Prepared, Option<Vec<f32>>)> = batch
                .par_iter()
                .map(|p| {
                    let prep = prepare(folder, p);
                    let emb = match &prep {
                        Prepared::Ok(_, cl) => clap.embed_sound(cl).ok(),
                        _ => None,
                    };
                    (prep, emb)
                })
                .collect();
            for (p, emb) in done {
                match p {
                    Prepared::Skip => {}
                    Prepared::Failed(row) => {
                        let _ = self.save_row(&row, None);
                        self.store.write().unwrap().drop_path(&row.meta.path);
                    }
                    Prepared::Ok(row, _) => {
                        let Some(emb) = emb else { continue };
                        if !Path::new(&row.meta.path).exists() {
                            continue;
                        }
                        if self.save_row(&row, Some(&emb)).is_ok() {
                            self.reattach(&row.meta.path, row.sha1.as_deref());
                            let cat = self.category_of(&emb);
                            self.store.write().unwrap().put(row.meta.clone(), emb, cat);
                        }
                    }
                }
            }
            self.status.lock().unwrap().done = ((b + 1) * BATCH).min(paths.len());
        }
    }

    fn save_row(&self, row: &FileRow, emb: Option<&[f32]>) -> Res<()> {
        let m = &row.meta;
        self.db()
            .execute(
                "INSERT OR REPLACE INTO files (path, folder, mtime, size, sha1, duration, sr, channels, peak_db, rms_db, crest,
                 brightness, pitch_hz, tempo, onsets, is_loop, emb, peaks, error, recorded)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    m.path, m.folder, row.mtime, row.size, row.sha1, m.duration, m.sr, m.channels, m.peak_db, m.rms_db, m.crest,
                    m.brightness, m.pitch_hz, m.tempo, row.onsets, m.is_loop as i64, emb.map(emb_blob),
                    if row.error.is_some() { None } else { Some(m.peaks.clone()) }, row.error, m.recorded
                ],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Remove a file from the index. Its tags, note, rating and project places are kept, and its
    /// content hash remembered, so they come back if the file reappears here or elsewhere.
    pub fn forget(&self, path: &str, purge: bool) {
        {
            let db = self.db();
            let sha: Option<String> = db.query_row("SELECT sha1 FROM files WHERE path=?", [path], |r| r.get(0)).optional().ok().flatten().flatten();
            let _ = db.execute("DELETE FROM files WHERE path=?", [path]);
            if purge {
                for t in ["catalog", "regions", "project_items", "gone"] {
                    let _ = db.execute(&format!("DELETE FROM {t} WHERE path=?"), [path]);
                }
            } else if let Some(s) = sha {
                let _ = db.execute("INSERT OR REPLACE INTO gone VALUES (?, ?, ?)", params![path, s, now()]);
            }
        }
        self.store.write().unwrap().drop_path(path);
    }

    fn reattach(&self, path: &str, sha1: Option<&str>) {
        let db = self.db();
        let _ = db.execute("DELETE FROM gone WHERE path = ?", [path]);
        let Some(sha1) = sha1 else { return };
        let olds: Vec<String> = {
            let mut st = db.prepare("SELECT path FROM gone WHERE sha1 = ? ORDER BY at DESC").unwrap();
            st.query_map([sha1], |r| r.get(0)).unwrap().flatten().collect()
        };
        for old in olds {
            let _ = db.execute("DELETE FROM gone WHERE path = ?", [&old]);
            if Path::new(&old).exists() {
                continue;
            }
            for t in ["catalog", "regions"] {
                let has: bool = db.query_row(&format!("SELECT 1 FROM {t} WHERE path = ?"), [path], |_| Ok(true)).optional().ok().flatten().unwrap_or(false);
                if !has {
                    let _ = db.execute(&format!("UPDATE {t} SET path = ? WHERE path = ?"), [path, &old]);
                }
            }
            let _ = db.execute("UPDATE OR IGNORE project_items SET path = ? WHERE path = ?", [path, &old]);
            let _ = db.execute("DELETE FROM project_items WHERE path = ?", [&old]);
            return;
        }
    }

    // ---------- files

    pub fn is_known(&self, path: &str) -> bool {
        if self.store.read().unwrap().pos.contains_key(path) {
            return true;
        }
        let real = std::fs::canonicalize(path).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| path.to_string());
        self.folders().iter().any(|f| {
            let rf = std::fs::canonicalize(f).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| f.clone());
            is_inside(&real, &rf)
        })
    }

    pub fn known_path(&self, path: &str) -> Res<()> {
        if !self.is_known(path) {
            return Err("Path is outside your library folders".into());
        }
        if !Path::new(path).is_file() {
            return Err("File not found".into());
        }
        Ok(())
    }

    pub fn trash(&self, path: &str) -> Res<()> {
        self.known_path(path)?;
        trash::delete(path).map_err(|e| format!("Could not move to Trash: {e}"))?;
        self.forget(path, true);
        Ok(())
    }

    pub fn rename(&self, path: &str, new_stem: &str) -> Res<String> {
        self.known_path(path)?;
        let src = Path::new(path);
        let stem = safe_name(new_stem).trim_matches(['.', ' ']).to_string();
        if stem.is_empty() {
            return Err("Name is empty".into());
        }
        let ext = src.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let target = src.with_file_name(format!("{stem}{ext}"));
        if target == src {
            return Ok(path.into());
        }
        if target.exists() {
            return Err(format!("{} already exists in that folder", basename(&target.to_string_lossy())));
        }
        std::fs::rename(src, &target).map_err(|e| e.to_string())?;
        let new = target.to_string_lossy().into_owned();
        let md = std::fs::metadata(&target).map_err(|e| e.to_string())?;
        let folder: String = {
            let db = self.db();
            let folder: String = db.query_row("SELECT folder FROM files WHERE path=?", [path], |r| r.get(0)).unwrap_or_default();
            for t in ["files", "catalog", "regions"] {
                let _ = db.execute(&format!("DELETE FROM {t} WHERE path=?"), [&new]);
                let _ = db.execute(&format!("UPDATE {t} SET path=? WHERE path=?"), [&new, path]);
            }
            let _ = db.execute("UPDATE OR IGNORE project_items SET path=? WHERE path=?", [&new, path]);
            let _ = db.execute("DELETE FROM project_items WHERE path=?", [path]);
            let _ = db.execute("UPDATE files SET mtime=?, size=? WHERE path=?", params![mtime_of(&md), md.size() as i64, new]);
            folder
        };
        self.move_in_arrangement(path, &new);
        self.store.write().unwrap().rename(path, &new, &folder);
        Ok(new)
    }

    // ---------- catalog

    fn catalog_all(&self) -> HashMap<String, (Vec<String>, String, i64, Option<String>)> {
        let db = self.db();
        let mut st = db.prepare("SELECT path, tags, memo, rating, origin FROM catalog").unwrap();
        st.query_map([], |r| {
            let tags: Option<String> = r.get(1)?;
            Ok((
                r.get::<_, String>(0)?,
                (
                    tags.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default(),
                    r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    r.get::<_, Option<String>>(4)?,
                ),
            ))
        })
        .unwrap()
        .flatten()
        .collect()
    }

    pub fn catalog_get(&self, path: &str) -> Value {
        let row = self
            .db()
            .query_row("SELECT tags, memo, rating FROM catalog WHERE path=?", [path], |r| {
                Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<i64>>(2)?))
            })
            .optional()
            .ok()
            .flatten();
        let (tags, memo, rating) = row.unwrap_or((None, Some(String::new()), Some(0)));
        let tags: Vec<String> = tags.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        json!({"path": path, "tags": tags, "memo": memo.unwrap_or_default(), "rating": rating.unwrap_or(0)})
    }

    pub fn catalog_set(&self, path: &str, changes: &Value) -> Res<Value> {
        let t = now();
        {
            let db = self.db();
            db.execute("INSERT OR IGNORE INTO catalog(path, done, updated) VALUES (?, 0, ?)", params![path, t]).map_err(|e| e.to_string())?;
            if let Some(r) = changes.get("rating").filter(|v| !v.is_null()) {
                let r = r.as_f64().unwrap_or(0.0).clamp(0.0, 5.0) as i64;
                let _ = db.execute("UPDATE catalog SET rating=?, updated=? WHERE path=?", params![r, t, path]);
            }
            if let Some(m) = changes.get("memo").and_then(|v| v.as_str()) {
                let _ = db.execute("UPDATE catalog SET memo=?, updated=? WHERE path=?", params![m, t, path]);
            }
            if let Some(tags) = changes.get("tags").and_then(|v| v.as_array()) {
                let mut set: Vec<String> = tags.iter().filter_map(|t| t.as_str()).map(|t| nfc(&t.trim().to_lowercase())).filter(|t| !t.is_empty()).collect();
                set.sort();
                set.dedup();
                let _ = db.execute("UPDATE catalog SET tags=?, updated=? WHERE path=?", params![serde_json::to_string(&set).unwrap(), t, path]);
            }
        }
        Ok(self.catalog_get(path))
    }

    fn catalog_copy(&self, src: &str, dst: &str) {
        let c = self.catalog_get(src);
        let mine = self.catalog_get(dst);
        let has = |v: &Value| v["tags"].as_array().is_some_and(|a| !a.is_empty()) || v["memo"].as_str().is_some_and(|m| !m.is_empty()) || v["rating"].as_i64().unwrap_or(0) > 0;
        if !has(&c) || has(&mine) {
            return;
        }
        let _ = self.catalog_set(dst, &c);
    }

    fn set_origin(&self, path: &str, origin: &str) {
        if origin.is_empty() || nfc(origin) == nfc(path) {
            return;
        }
        let db = self.db();
        let _ = db.execute("INSERT OR IGNORE INTO catalog(path, done, updated) VALUES (?, 0, ?)", params![path, now()]);
        let _ = db.execute("UPDATE catalog SET origin=? WHERE path=? AND origin IS NULL", [origin, path]);
    }

    pub fn all_tags(&self) -> Value {
        let mut counts: HashMap<String, i64> = HashMap::new();
        {
            let db = self.db();
            let mut st = db.prepare("SELECT c.tags FROM catalog c JOIN files f ON f.path = c.path WHERE c.tags IS NOT NULL").unwrap();
            for t in st.query_map([], |r| r.get::<_, String>(0)).unwrap().flatten() {
                for tag in serde_json::from_str::<Vec<String>>(&t).unwrap_or_default() {
                    *counts.entry(tag).or_default() += 1;
                }
            }
        }
        let mut v: Vec<_> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Value::Array(v.into_iter().map(|(t, n)| json!({"tag": t, "count": n})).collect())
    }

    // ---------- regions and the arrangement

    pub fn regions_get(&self, path: &str) -> Value {
        self.db()
            .query_row("SELECT data FROM regions WHERE path=?", [path], |r| r.get::<_, String>(0))
            .optional()
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(json!([]))
    }

    pub fn regions_set(&self, path: &str, regions: &Value) -> Res<Value> {
        let mut clean = Vec::new();
        for r in regions.as_array().ok_or("regions must be a list")? {
            let (a, b) = (r["start"].as_f64().ok_or("start")?, r["end"].as_f64().ok_or("end")?);
            if b > a && a >= 0.0 {
                let name: String = r["name"].as_str().unwrap_or("").chars().take(80).collect();
                clean.push(json!({"start": a, "end": b, "name": name}));
            }
        }
        let db = self.db();
        if clean.is_empty() {
            let _ = db.execute("DELETE FROM regions WHERE path=?", [path]);
        } else {
            let _ = db.execute("INSERT OR REPLACE INTO regions VALUES (?, ?)", params![path, Value::Array(clean.clone()).to_string()]);
        }
        Ok(Value::Array(clean))
    }

    pub fn arrangement_get(&self) -> Value {
        self.meta_get("arrangement").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({"loop": true, "lanes": []}))
    }

    pub fn arrangement_set(&self, data: &Value) -> Res<Value> {
        let mut lanes = Vec::new();
        for (n, lane) in data["lanes"].as_array().cloned().unwrap_or_default().iter().take(ARRANGE_MAX_LANES).enumerate() {
            let mut clips = Vec::new();
            for clip in lane["clips"].as_array().cloned().unwrap_or_default().iter().take(ARRANGE_MAX_HITS) {
                let path = clip["path"].as_str().ok_or("clip without a path")?.to_string();
                let duration = clip["duration"].as_f64().unwrap_or(0.0).max(0.0);
                let fade_in = clip["in"].as_f64().unwrap_or(0.0).max(0.0).min(duration);
                let fade_out = clip["out"].as_f64().unwrap_or(0.0).max(0.0).min(duration - fade_in);
                clips.push(json!({
                    "path": path, "name": basename(&path), "duration": duration, "sr": clip["sr"].as_i64().unwrap_or(0),
                    "at": clip["at"].as_f64().unwrap_or(0.0).clamp(0.0, ARRANGE_MAX_SECONDS), "in": fade_in, "out": fade_out,
                }));
            }
            let name: String = lane["name"].as_str().unwrap_or("").trim().chars().take(60).collect();
            lanes.push(json!({
                "name": if name.is_empty() { format!("Lane {}", n + 1) } else { name },
                "gain": lane["gain"].as_f64().unwrap_or(0.0).clamp(-24.0, 6.0),
                "mute": lane["mute"].as_bool().unwrap_or(false), "solo": lane["solo"].as_bool().unwrap_or(false), "clips": clips,
            }));
        }
        let clean = json!({"loop": data["loop"].as_bool().unwrap_or(true), "lanes": lanes});
        self.meta_set("arrangement", &clean.to_string())?;
        Ok(clean)
    }

    fn move_in_arrangement(&self, src: &str, dst: &str) {
        let mut a = self.arrangement_get();
        let mut changed = false;
        if let Some(lanes) = a["lanes"].as_array_mut() {
            for lane in lanes {
                if let Some(clips) = lane["clips"].as_array_mut() {
                    for c in clips {
                        if c["path"] == src {
                            c["path"] = json!(dst);
                            c["name"] = json!(basename(dst));
                            changed = true;
                        }
                    }
                }
            }
        }
        if changed {
            let _ = self.meta_set("arrangement", &a.to_string());
        }
    }

    // ---------- search

    pub fn embedding_of(&self, path: &str) -> Option<Vec<f32>> {
        let s = self.store.read().unwrap();
        s.pos.get(path).map(|&i| s.items[i].emb.clone())
    }

    pub fn embed_text(&self, words: &str) -> Res<Vec<f32>> {
        let clap = self.engine.get().ok_or("Model is still loading")?;
        clap.embed_texts(&[words]).map(|mut v| v.remove(0)).map_err(|e| e.to_string())
    }

    fn sub(path: &str, folder: &str) -> String {
        if folder.is_empty() {
            return String::new();
        }
        let dir = Path::new(&nfc(path)).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let f = nfc(folder);
        let f = f.trim_end_matches(MAIN_SEPARATOR);
        dir.get(f.len() + 1..).unwrap_or("").to_string()
    }

    pub fn search(&self, q: &SearchQuery, vec: Option<Vec<f32>>, exclude: Option<&str>, words: Option<&str>) -> Res<Value> {
        let cat_rows = self.catalog_all();
        let names = self.category_names().to_vec();
        let members: Option<HashSet<String>> = q.project.map(|pid| {
            let db = self.db();
            let mut st = db.prepare("SELECT path FROM project_items WHERE project_id=?").unwrap();
            st.query_map([pid], |r| r.get(0)).unwrap().flatten().collect()
        });
        let store = self.store.read().unwrap();
        let n = store.items.len();
        if n == 0 {
            return Ok(json!([]));
        }
        let mut scores: Vec<f32> = match &vec {
            Some(v) => store.items.par_iter().map(|it| dot(&it.emb, v)).collect(),
            None => vec![0.0; n],
        };
        let mut boost = vec![0.0f32; n];
        let mut relevant: Option<Vec<bool>> = None;
        if let Some(words) = words.map(str::trim).filter(|w| !w.is_empty()) {
            let phrase = nfc(&words.to_lowercase());
            let terms: Vec<&str> = phrase.split(|c: char| c.is_whitespace() || matches!(c, '_' | '-' | '.')).filter(|w| w.chars().count() > 1).collect();
            for (i, it) in store.items.iter().enumerate() {
                let c = cat_rows.get(&it.meta.path);
                let hay = nfc(
                    &[basename(&it.meta.path), Self::sub(&it.meta.path, &it.meta.folder), c.map(|c| c.0.join(" ")).unwrap_or_default(), c.map(|c| c.1.clone()).unwrap_or_default()]
                        .join(" ")
                        .to_lowercase(),
                );
                if hay.contains(&phrase) {
                    boost[i] = 1.0;
                } else if !terms.is_empty() {
                    boost[i] = 0.5 * terms.iter().filter(|t| hay.contains(*t)).count() as f32 / terms.len() as f32;
                }
            }
            // CLAP ranks every sound, even for nonsense queries, and some sounds score high for
            // any text. Each sound's score for nonsense text is taken off: the query must find
            // sounds clearly, or only matches by name, tag or note are kept.
            let mut rel: Vec<bool> = boost.iter().map(|b| *b > 0.0).collect();
            if let (Some(v), Some(junk)) = (&vec, self.nonsense.get()) {
                let q: Vec<f32> = v.iter().zip(junk).map(|(a, b)| a - b).collect();
                let adj: Vec<f32> = store.items.par_iter().map(|it| dot(&it.emb, &q)).collect();
                if adj.iter().copied().fold(f32::NEG_INFINITY, f32::max) >= QUERY_FOUND {
                    for (r, a) in rel.iter_mut().zip(&adj) {
                        *r |= *a >= QUERY_KEEP;
                    }
                }
            }
            relevant = Some(rel);
            for (s, b) in scores.iter_mut().zip(&boost) {
                *s += b;
            }
        }
        let cat_idx = q.category.as_ref().and_then(|c| names.iter().position(|n| n == c));
        let mut idx: Vec<usize> = (0..n)
            .filter(|&i| {
                let it = &store.items[i];
                let m = &it.meta;
                let c = cat_rows.get(&m.path);
                q.min_dur.is_none_or(|d| m.duration.unwrap_or(0.0) >= d)
                    && (q.category.is_none() || cat_idx.is_none() || it.cat.map(|c| c.0) == cat_idx)
                    && q.folder.as_ref().is_none_or(|f| f.is_empty() || &m.folder == f)
                    && q.under.as_ref().is_none_or(|u| u.is_empty() || is_inside(&m.path, u))
                    && members.as_ref().is_none_or(|ms| ms.contains(&m.path))
                    && q.tag.as_ref().is_none_or(|t| t.is_empty() || c.is_some_and(|c| c.0.contains(t)))
                    && q.min_rating.is_none_or(|r| r == 0 || c.map(|c| c.2).unwrap_or(0) >= r)
                    && exclude.is_none_or(|e| m.path != e)
                    && relevant.as_ref().is_none_or(|r| r[i])
            })
            .collect();
        let limit = q.limit.unwrap_or(200).max(0) as usize;
        let name_of = |i: usize| basename(&store.items[i].meta.path).to_lowercase();
        let sort = if vec.is_some() || q.sort.as_deref() != Some("score") { q.sort.clone().unwrap_or("score".into()) } else { "name".into() };
        match sort.as_str() {
            "score" if vec.is_some() => idx.sort_by(|&a, &b| scores[b].total_cmp(&scores[a])),
            "rating" => idx.sort_by_key(|&i| std::cmp::Reverse(cat_rows.get(&store.items[i].meta.path).map(|c| c.2).unwrap_or(0))),
            "recorded" => idx.sort_by_key(|&i| {
                let d: String = store.items[i].meta.recorded.clone().unwrap_or_default().chars().filter(|c| c.is_ascii_digit()).collect();
                std::cmp::Reverse(d.parse::<i64>().unwrap_or(0))
            }),
            "duration" => idx.sort_by(|&a, &b| store.items[a].meta.duration.unwrap_or(0.0).total_cmp(&store.items[b].meta.duration.unwrap_or(0.0))),
            "category" => idx.sort_by_key(|&i| (store.items[i].cat.map(|c| names[c.0].to_lowercase()).unwrap_or_default(), name_of(i))),
            "brightness" => idx.sort_by(|&a, &b| store.items[a].meta.brightness.unwrap_or(0.0).total_cmp(&store.items[b].meta.brightness.unwrap_or(0.0))),
            "loudness" => idx.sort_by(|&a, &b| store.items[b].meta.rms_db.unwrap_or(-200.0).total_cmp(&store.items[a].meta.rms_db.unwrap_or(-200.0))),
            "pitch" => idx.sort_by(|&a, &b| store.items[a].meta.pitch_hz.unwrap_or(1e9).total_cmp(&store.items[b].meta.pitch_hz.unwrap_or(1e9))),
            "tempo" => idx.sort_by(|&a, &b| store.items[a].meta.tempo.unwrap_or(1e9).total_cmp(&store.items[b].meta.tempo.unwrap_or(1e9))),
            _ => idx.sort_by_key(|&i| name_of(i)),
        }
        idx.truncate(limit);
        let missing: Vec<String> = idx.iter().map(|&i| store.items[i].meta.path.clone()).filter(|p| !Path::new(p).exists()).collect();
        let out: Vec<Value> = idx
            .iter()
            .filter(|&&i| !missing.contains(&store.items[i].meta.path))
            .map(|&i| {
                let it = &store.items[i];
                let m = &it.meta;
                let c = cat_rows.get(&m.path);
                json!({
                    "tags": c.map(|c| c.0.clone()).unwrap_or_default(), "memo": c.map(|c| c.1.clone()).unwrap_or_default(),
                    "rating": c.map(|c| c.2).unwrap_or(0), "origin": c.and_then(|c| c.3.clone()), "recorded": m.recorded,
                    "path": m.path, "name": basename(&m.path), "folder": m.folder, "sub": Self::sub(&m.path, &m.folder),
                    "score": scores[i], "duration": m.duration, "sr": m.sr, "channels": m.channels, "rms_db": m.rms_db,
                    "peak_db": m.peak_db, "crest": m.crest, "brightness": m.brightness, "pitch_hz": m.pitch_hz,
                    "note": note_name(m.pitch_hz), "tempo": m.tempo, "is_loop": m.is_loop,
                    "category": it.cat.map(|c| names[c.0].clone()), "category_score": it.cat.map(|c| c.1).unwrap_or(0.0),
                    "peaks": m.peaks, "text_match": boost[i],
                })
            })
            .collect();
        drop(store);
        // Files gone from a folder that is still there are forgotten; a whole folder offline
        // (an unplugged drive) stays indexed and hidden.
        for p in missing {
            let folder = self.store.read().unwrap().pos.get(&p).map(|&i| self.store.read().unwrap().items[i].meta.folder.clone());
            if folder.is_some_and(|f| Path::new(&f).is_dir()) {
                self.forget(&p, false);
            }
        }
        Ok(Value::Array(out))
    }

    pub fn category_counts(&self) -> Value {
        let names = self.category_names();
        let store = self.store.read().unwrap();
        let med = |mut v: Vec<f64>| -> Option<f64> {
            if v.is_empty() {
                return None;
            }
            v.sort_by(f64::total_cmp);
            let k = v.len();
            Some(if k % 2 == 1 { v[k / 2] } else { (v[k / 2 - 1] + v[k / 2]) / 2.0 })
        };
        Value::Array(
            names
                .iter()
                .enumerate()
                .map(|(ci, n)| {
                    let its: Vec<&Item> = store.items.iter().filter(|it| it.cat.map(|c| c.0) == Some(ci)).collect();
                    json!({
                        "name": n, "count": its.len(),
                        "brightness": med(its.iter().filter_map(|it| it.meta.brightness.filter(|b| *b != 0.0)).collect()),
                        "duration": med(its.iter().filter_map(|it| it.meta.duration.filter(|d| *d != 0.0)).collect()),
                    })
                })
                .collect(),
        )
    }

    /// Every sound on a flat map, close to the sounds it resembles. Kept until the library
    /// changes; then the last map, minus sounds that are gone, is handed back while the new
    /// one is worked out in the background.
    pub fn sound_map(self: &Arc<Self>) -> Value {
        let version = self.store.read().unwrap().version;
        let cached = self.map.lock().unwrap().clone();
        match cached {
            Some((v, m)) if v == version => m,
            Some((_, m)) => {
                if !self.map_busy.swap(true, Ordering::SeqCst) {
                    let me = self.clone();
                    std::thread::spawn(move || {
                        me.build_map();
                        me.map_busy.store(false, Ordering::SeqCst);
                    });
                }
                let s = self.store.read().unwrap();
                Value::Array(m.as_array().unwrap().iter().filter(|p| p["path"].as_str().is_some_and(|x| s.pos.contains_key(x))).cloned().collect())
            }
            None => self.build_map(),
        }
    }

    fn build_map(&self) -> Value {
        let store = self.store.read().unwrap();
        let n = store.items.len();
        let names = self.category_names();
        let mut xy = vec![[0.5f64, 0.5]; n];
        if n >= 3 {
            let mut mean = vec![0.0f64; EMB_DIM];
            for it in &store.items {
                for (m, e) in mean.iter_mut().zip(&it.emb) {
                    *m += *e as f64 / n as f64;
                }
            }
            let x: Vec<Vec<f64>> = store.items.iter().map(|it| it.emb.iter().zip(&mean).map(|(e, m)| *e as f64 - m).collect()).collect();
            let mut comps: Vec<Vec<f64>> = Vec::new();
            for c in 0..2 {
                let mut v: Vec<f64> = (0..EMB_DIM).map(|k| ((k * 7 + c * 13) % 11) as f64 - 5.0).collect();
                for _ in 0..60 {
                    let proj: Vec<f64> = x.iter().map(|r| r.iter().zip(&v).map(|(a, b)| a * b).sum()).collect();
                    let mut w = vec![0.0f64; EMB_DIM];
                    for (r, p) in x.iter().zip(&proj) {
                        for (wk, rk) in w.iter_mut().zip(r) {
                            *wk += rk * p;
                        }
                    }
                    for prev in &comps {
                        let d: f64 = w.iter().zip(prev).map(|(a, b)| a * b).sum();
                        for (wk, pk) in w.iter_mut().zip(prev) {
                            *wk -= d * pk;
                        }
                    }
                    let norm = w.iter().map(|a| a * a).sum::<f64>().sqrt().max(1e-12);
                    v = w.iter().map(|a| a / norm).collect();
                }
                comps.push(v);
            }
            for (i, r) in x.iter().enumerate() {
                xy[i] = [r.iter().zip(&comps[0]).map(|(a, b)| a * b).sum(), r.iter().zip(&comps[1]).map(|(a, b)| a * b).sum()];
            }
            // The finer layout costs time that grows with the square of the library, so past a
            // few thousand sounds the rough map is used as it is.
            if (12..=MAP_FINE_LIMIT).contains(&n) {
                let sd = (xy.iter().map(|p| p[0] * p[0]).sum::<f64>() / n as f64).sqrt().max(1e-12);
                let init: Vec<[f64; 2]> = xy.iter().map(|p| [p[0] / sd * 1e-4, p[1] / sd * 1e-4]).collect();
                let embs: Vec<Vec<f32>> = store.items.iter().map(|it| it.emb.clone()).collect();
                xy = crate::tsne::tsne(&embs, &init, 30f64.min((n - 1) as f64 / 3.0), 1000);
            }
            for d in 0..2 {
                let lo = xy.iter().map(|p| p[d]).fold(f64::INFINITY, f64::min);
                let hi = xy.iter().map(|p| p[d]).fold(f64::NEG_INFINITY, f64::max);
                let span = if hi - lo > 0.0 { hi - lo } else { 1.0 };
                for p in xy.iter_mut() {
                    p[d] = (p[d] - lo) / span;
                }
            }
        }
        let out = Value::Array(
            store
                .items
                .iter()
                .enumerate()
                .map(|(i, it)| {
                    json!({"path": it.meta.path, "name": basename(&it.meta.path), "x": (xy[i][0] * 1e4).round() / 1e4, "y": (xy[i][1] * 1e4).round() / 1e4,
                           "category": it.cat.map(|c| names[c.0].clone()), "duration": it.meta.duration, "sr": it.meta.sr})
                })
                .collect(),
        );
        let v = store.version;
        drop(store);
        *self.map.lock().unwrap() = Some((v, out.clone()));
        out
    }

    // ---------- projects

    pub fn projects(&self) -> Value {
        let db = self.db();
        let mut st = db
            .prepare(
                "SELECT p.id, p.name, p.folder, COUNT(f.path) n, COALESCE(SUM(f.size), 0) bytes FROM projects p
                 LEFT JOIN project_items i ON i.project_id=p.id LEFT JOIN files f ON f.path=i.path GROUP BY p.id ORDER BY p.name",
            )
            .unwrap();
        Value::Array(
            st.query_map([], |r| {
                Ok(json!({"id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "folder": r.get::<_, Option<String>>(2)?,
                          "n": r.get::<_, i64>(3)?, "bytes": r.get::<_, i64>(4)?}))
            })
            .unwrap()
            .flatten()
            .collect(),
        )
    }

    pub fn project_exists(&self, pid: i64) -> bool {
        self.db().query_row("SELECT 1 FROM projects WHERE id=?", [pid], |_| Ok(())).is_ok()
    }

    fn project_name(&self, pid: i64) -> Option<String> {
        self.db().query_row("SELECT name FROM projects WHERE id=?", [pid], |r| r.get(0)).ok()
    }

    pub fn project_folder(&self, pid: i64) -> Option<String> {
        self.db().query_row("SELECT folder FROM projects WHERE id=?", [pid], |r| r.get::<_, Option<String>>(0)).ok().flatten().filter(|f| !f.is_empty())
    }

    pub fn create_project(&self, name: &str) -> Res<Value> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Name required".into());
        }
        let fresh = !self.projects().as_array().unwrap().iter().any(|p| p["name"] == name);
        let (id, folder): (i64, Option<String>) = {
            let db = self.db();
            db.execute("INSERT OR IGNORE INTO projects(name) VALUES (?)", [name]).map_err(|e| e.to_string())?;
            db.query_row("SELECT id, folder FROM projects WHERE name=?", [name], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?
        };
        let mut p = json!({"id": id, "name": name, "folder": folder});
        if fresh {
            match self.give_project_folder(id, name) {
                Ok(f) => p["folder"] = json!(f),
                Err(e) => p["folder_error"] = json!(e),
            }
        }
        Ok(p)
    }

    /// The project's own folder inside the projects folder from Settings; every sound added
    /// to the project is copied there.
    fn give_project_folder(&self, pid: i64, name: &str) -> Res<String> {
        let base = expand(self.settings()["projects_dir"].as_str().unwrap_or(""));
        let folder = unique_path(&base.join(safe_name(name)))?;
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let base = std::fs::canonicalize(&base).map_err(|e| e.to_string())?.to_string_lossy().into_owned();
        if !self.folders().iter().any(|f| nfc(f) == nfc(&base) || is_inside(&base, f) || is_inside(f, &base)) {
            self.add_folder(&base)?;
        }
        self.set_project_folder(pid, &folder.to_string_lossy())
    }

    fn ensure_project_folders(&self) {
        let missing: Vec<(i64, String)> = {
            let db = self.db();
            let mut st = db.prepare("SELECT id, name FROM projects WHERE folder IS NULL OR folder = ''").unwrap();
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().flatten().collect()
        };
        for (id, name) in missing {
            let _ = self.give_project_folder(id, &name);
        }
    }

    fn dismissed_folders(&self) -> HashSet<String> {
        self.meta_get("projects_dismissed").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    /// A project whose folder was deleted outside the app leaves the list. A folder on a disk
    /// that is not mounted (its parent is missing too) keeps its project.
    pub fn drop_missing_projects(&self) {
        let gone: Vec<i64> = {
            let db = self.db();
            let mut st = db.prepare("SELECT id, folder FROM projects WHERE folder IS NOT NULL AND folder != ''").unwrap();
            st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .unwrap()
                .flatten()
                .filter(|(_, f)| {
                    let p = Path::new(f);
                    !p.exists() && p.parent().is_some_and(Path::is_dir)
                })
                .map(|(id, _)| id)
                .collect()
        };
        let db = self.db();
        for id in gone {
            let _ = db.execute("DELETE FROM project_items WHERE project_id=?", [id]);
            let _ = db.execute("DELETE FROM projects WHERE id=?", [id]);
        }
    }

    /// A folder made by hand inside the projects folder becomes a project, with the sounds it holds.
    pub fn adopt_project_folders(&self) {
        let base = expand(self.settings()["projects_dir"].as_str().unwrap_or(""));
        let Ok(base) = std::fs::canonicalize(&base) else { return };
        let projects = self.projects();
        let list = projects.as_array().unwrap();
        let mut owned: HashSet<String> = list.iter().filter_map(|p| p["folder"].as_str()).map(nfc).collect();
        owned.extend(self.dismissed_folders().iter().map(|f| nfc(f)));
        let mut names: HashSet<String> = list.iter().filter_map(|p| p["name"].as_str().map(String::from)).collect();
        let Ok(rd) = std::fs::read_dir(&base) else { return };
        let mut found: Vec<String> = rd.flatten().filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.')).map(|e| e.path().to_string_lossy().into_owned()).collect();
        found.sort();
        for folder in found {
            if owned.contains(&nfc(&folder)) {
                continue;
            }
            let stem = basename(&folder);
            let (mut name, mut n) = (stem.clone(), 2);
            while names.contains(&name) {
                name = format!("{stem} ({n})");
                n += 1;
            }
            names.insert(name.clone());
            let id: i64 = {
                let db = self.db();
                let _ = db.execute("INSERT OR IGNORE INTO projects(name) VALUES (?)", [&name]);
                match db.query_row("SELECT id FROM projects WHERE name=?", [&name], |r| r.get(0)) {
                    Ok(id) => id,
                    Err(_) => continue,
                }
            };
            if self.set_project_folder(id, &folder).is_err() {
                continue;
            }
            let prefix = format!("{folder}{MAIN_SEPARATOR}");
            let inside: Vec<String> = self.store.read().unwrap().items.iter().map(|it| it.meta.path.clone()).filter(|p| p.starts_with(&prefix)).collect();
            let db = self.db();
            for p in inside {
                let _ = db.execute("INSERT OR IGNORE INTO project_items(project_id, path, added) VALUES (?, ?, ?)", params![id, p, now()]);
            }
        }
    }

    pub fn rename_project(&self, pid: i64, name: &str) -> Res<()> {
        if name.trim().is_empty() {
            return Err("Name required".into());
        }
        self.db().execute("UPDATE projects SET name=? WHERE id=?", params![name.trim(), pid]).map(|_| ()).map_err(|_| "A project with that name already exists".into())
    }

    pub fn delete_project(&self, pid: i64) {
        if let Some(f) = self.project_folder(pid) {
            let mut gone: Vec<String> = self.dismissed_folders().into_iter().collect();
            gone.push(f);
            gone.sort();
            gone.dedup();
            let _ = self.meta_set("projects_dismissed", &serde_json::to_string(&gone).unwrap());
        }
        let db = self.db();
        let _ = db.execute("DELETE FROM project_items WHERE project_id=?", [pid]);
        let _ = db.execute("DELETE FROM projects WHERE id=?", [pid]);
    }

    /// The project's folder, checked to exist and to be inside the library.
    pub fn project_folder_ready(&self, pid: i64) -> Res<Option<String>> {
        let Some(folder) = self.project_folder(pid) else { return Ok(None) };
        if !Path::new(&folder).is_dir() {
            return Err(format!("The project folder was not found: {folder}"));
        }
        if !self.folders().iter().any(|f| nfc(f) == nfc(&folder) || is_inside(&folder, f)) {
            self.add_folder(&folder)?;
        }
        Ok(Some(folder))
    }

    fn into_project_folder(&self, folder: &str, path: &str) -> Res<String> {
        if is_inside(path, folder) {
            return Ok(path.into());
        }
        let mut target = Path::new(folder).join(basename(path));
        let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        let same = target.is_file()
            && std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0) == size
            && file_hash(&target.to_string_lossy(), size).ok() == file_hash(path, size).ok();
        if !same {
            target = unique_path(&target)?;
            std::fs::copy(path, &target).map_err(|e| e.to_string())?;
        }
        let t = target.to_string_lossy().into_owned();
        self.catalog_copy(path, &t);
        self.set_origin(&t, path);
        self.index_file(&t);
        Ok(t)
    }

    pub fn set_project_folder(&self, pid: i64, path: &str) -> Res<String> {
        let p = std::fs::canonicalize(expand(path)).map_err(|_| format!("Not a folder: {path}"))?;
        if !p.is_dir() {
            return Err(format!("Not a folder: {}", p.display()));
        }
        let mut path = p.to_string_lossy().into_owned();
        if !self.folders().iter().any(|f| nfc(f) == nfc(&path) || is_inside(&path, f)) {
            path = self.add_folder(&path)?;
        }
        self.db().execute("UPDATE projects SET folder=? WHERE id=?", params![path, pid]).map_err(|e| e.to_string())?;
        let mut olds: Vec<String> = self.project_paths(pid).into_iter().collect();
        olds.sort();
        for old in olds {
            if is_inside(&old, &path) || !Path::new(&old).exists() {
                continue;
            }
            let new = self.into_project_folder(&path, &old)?;
            let db = self.db();
            let _ = db.execute(
                "INSERT OR IGNORE INTO project_items(project_id, path, added, pos, pad) SELECT project_id, ?, added, pos, pad FROM project_items WHERE project_id=? AND path=?",
                params![new, pid, old],
            );
            let _ = db.execute("DELETE FROM project_items WHERE project_id=? AND path=?", params![pid, old]);
        }
        Ok(path)
    }

    pub fn project_add(&self, pid: i64, path: &str) -> Res<String> {
        if self.project_folder(pid).is_none() {
            let name = self.project_name(pid).ok_or("Project not found")?;
            self.give_project_folder(pid, &name)?;
        }
        let mut p = path.to_string();
        if let Some(folder) = self.project_folder_ready(pid)? {
            p = self.into_project_folder(&folder, path)?;
        }
        self.db().execute("INSERT OR IGNORE INTO project_items(project_id, path, added) VALUES (?, ?, ?)", params![pid, p, now()]).map_err(|e| e.to_string())?;
        Ok(p)
    }

    pub fn project_remove(&self, pid: i64, path: &str) {
        let _ = self.db().execute("DELETE FROM project_items WHERE project_id=? AND path=?", params![pid, path]);
    }

    fn project_paths(&self, pid: i64) -> HashSet<String> {
        let db = self.db();
        let mut st = db.prepare("SELECT path FROM project_items WHERE project_id=?").unwrap();
        st.query_map([pid], |r| r.get(0)).unwrap().flatten().collect()
    }

    pub fn project_order(&self, pid: i64) -> Value {
        let db = self.db();
        let mut st = db.prepare("SELECT path FROM project_items WHERE project_id=? ORDER BY pos IS NULL, pos, path").unwrap();
        Value::Array(st.query_map([pid], |r| r.get::<_, String>(0)).unwrap().flatten().map(Value::String).collect())
    }

    pub fn set_project_order(&self, pid: i64, paths: &[String]) {
        let db = self.db();
        for (n, p) in paths.iter().enumerate() {
            let _ = db.execute("UPDATE project_items SET pos=? WHERE project_id=? AND path=?", params![n as f64, pid, p]);
        }
    }

    pub fn project_pads(&self, pid: i64) -> Value {
        let db = self.db();
        let mut st = db.prepare("SELECT path, pad FROM project_items WHERE project_id=? AND pad IS NOT NULL").unwrap();
        let m: serde_json::Map<String, Value> = st.query_map([pid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).unwrap().flatten().map(|(p, n)| (p, json!(n))).collect();
        Value::Object(m)
    }

    pub fn set_project_pads(&self, pid: i64, pads: &Value) -> Res<()> {
        let mut clean: Vec<(String, i64)> = Vec::new();
        for (k, v) in pads.as_object().ok_or("pads must be an object")? {
            if let Some(n) = v.as_i64() {
                if !(0..SP404_PADS).contains(&n) {
                    return Err(format!("Pads are numbered 0 to {}", SP404_PADS - 1));
                }
                clean.push((k.clone(), n));
            }
        }
        let used: HashSet<i64> = clean.iter().map(|c| c.1).collect();
        if used.len() != clean.len() {
            return Err("Two sounds are on the same pad".into());
        }
        let members = self.project_paths(pid);
        if let Some((p, _)) = clean.iter().find(|(p, _)| !members.contains(p)) {
            return Err(format!("Not in this project: {p}"));
        }
        let db = self.db();
        let _ = db.execute("UPDATE project_items SET pad=NULL WHERE project_id=?", [pid]);
        for (p, n) in clean {
            let _ = db.execute("UPDATE project_items SET pad=? WHERE project_id=? AND path=?", params![n, pid, p]);
        }
        Ok(())
    }

    /// Where a file saved from the editor lands, for a project or a chosen folder.
    pub fn save_target_dir(&self, project: i64, dest_dir: &str) -> Res<PathBuf> {
        if project != 0 {
            let name = self.project_name(project).ok_or("Project not found")?;
            let f = match self.project_folder_ready(project).map_err(|e| format!("The project folder cannot be used: {e}"))? {
                Some(f) => f,
                None => self.give_project_folder(project, &name)?,
            };
            return Ok(PathBuf::from(f));
        }
        let d = expand(dest_dir);
        if !d.is_dir() {
            return Err(format!("Not a folder: {}", d.display()));
        }
        Ok(d)
    }

    pub fn after_save(&self, target: &str, source: &str, project: i64) -> Res<()> {
        if !source.is_empty() && self.is_known(source) {
            self.catalog_copy(source, target);
            self.set_origin(target, source);
        }
        self.index_file(target);
        if project != 0 {
            self.project_add(project, target)?;
        }
        Ok(())
    }
}

/// A file-naming pattern as Python's str.format would read it, with the keys project, category,
/// n and name; n takes a width such as {n:03d}.
pub fn fill_pattern(pattern: &str, project: &str, category: &str, n: usize, name: &str) -> Res<String> {
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let j = rest[i..].find('}').ok_or("Bad naming pattern: an opening brace is not closed")? + i;
        let (key, spec) = rest[i + 1..j].split_once(':').unwrap_or((&rest[i + 1..j], ""));
        let v = match key {
            "project" => project.to_string(),
            "category" => category.to_string(),
            "name" => name.to_string(),
            "n" => {
                let digits = spec.trim_end_matches('d');
                let width: usize = digits.parse().unwrap_or(0);
                if digits.starts_with('0') { format!("{n:0width$}") } else { format!("{n:width$}") }
            }
            k => return Err(format!("Bad naming pattern: unknown field {k}")),
        };
        out.push_str(&v);
        rest = &rest[j + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

impl Library {
    pub fn export_project(&self, pid: i64, dest: &str, pattern: &str, fmt: &str) -> Res<Value> {
        fill_pattern(pattern, "p", "c", 1, "n")?;
        let name = self.project_name(pid).ok_or("Project not found")?;
        if fmt == "sp404" {
            return self.export_sp404_project(pid, &name, dest);
        }
        let dir = expand(dest).join(safe_name(&name));
        std::fs::create_dir_all(&dir).map_err(|e| format!("Export failed: {e}"))?;
        let cats = self.categories_of_project(pid);
        let mut written = Vec::new();
        for (n, path) in self.project_order(pid).as_array().unwrap().iter().filter_map(|v| v.as_str()).enumerate() {
            let src = Path::new(path);
            if !src.exists() {
                continue;
            }
            let stem = src.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
            let ext = src.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
            let cat = cats.get(path).cloned().unwrap_or_else(|| "Sound".into());
            let base = fill_pattern(pattern, &safe_name(&name), &cat, n + 1, &stem)?;
            let target = unique_path(&dir.join(format!("{}{ext}", safe_name(&base))))?;
            std::fs::copy(src, &target).map_err(|e| format!("Export failed: {e}"))?;
            written.push(target.to_string_lossy().into_owned());
        }
        Ok(json!({"folder": dir, "files": written}))
    }

    fn categories_of_project(&self, pid: i64) -> HashMap<String, String> {
        let names = self.category_names().to_vec();
        let members = self.project_paths(pid);
        let s = self.store.read().unwrap();
        s.items.iter().filter(|it| members.contains(&it.meta.path)).filter_map(|it| it.cat.map(|c| (it.meta.path.clone(), names[c.0].clone()))).collect()
    }

    /// Pad number to path: the pads as laid out, then the sounds without a pad on the free pads
    /// in project order.
    fn sp404_layout(&self, pid: i64) -> Res<std::collections::BTreeMap<usize, String>> {
        let order: Vec<String> = self.project_order(pid).as_array().unwrap().iter().filter_map(|v| v.as_str().map(String::from)).filter(|p| Path::new(p).exists()).collect();
        let mut pads: std::collections::BTreeMap<usize, String> = std::collections::BTreeMap::new();
        for (p, n) in self.project_pads(pid).as_object().unwrap() {
            if order.contains(p) {
                pads.insert(n.as_i64().unwrap_or(0) as usize, p.clone());
            }
        }
        let placed: HashSet<String> = pads.values().cloned().collect();
        let rest: Vec<&String> = order.iter().filter(|p| !placed.contains(*p)).collect();
        let free: Vec<usize> = (0..SP404_PADS as usize).filter(|n| !pads.contains_key(n)).collect();
        if rest.len() > free.len() {
            return Err(format!("{} sounds do not fit on the {SP404_PADS} pads of the SP-404MKII", order.len()));
        }
        for (n, p) in free.into_iter().zip(rest) {
            pads.insert(n, p.clone());
        }
        Ok(pads)
    }

    /// The project as an SP-404MKII project folder in IMPORT on the SD card: IMPORT PROJECT on
    /// the device lists it there to copy into one of its 16 project slots.
    fn export_sp404_project(&self, pid: i64, name: &str, dest: &str) -> Res<Value> {
        use crate::sp404;
        let layout = self.sp404_layout(pid)?;
        if layout.is_empty() {
            return Err("The project has no sounds to export".into());
        }
        let folder = crate::audio::sp404_dir(&expand(dest)).join(ascii_name(name));
        if folder.exists() {
            let theirs = std::fs::read(folder.join("PADCONF.BIN")).ok().and_then(|b| sp404::padconf_name(&b));
            if theirs.as_deref() != Some(sp404::ascii_text(name, sp404::PROJECT_NAME_LEN).trim()) {
                return Err(format!("{} on the card holds another project", basename(&folder.to_string_lossy())));
            }
            std::fs::remove_dir_all(&folder).map_err(|e| e.to_string())?;
        }
        for sub in ["SMPL", "PTN", "PICTURE"] {
            std::fs::create_dir_all(folder.join(sub)).map_err(|e| e.to_string())?;
        }
        for pic in sp404::PICTURES {
            std::fs::write(folder.join("PICTURE").join(format!("{pic}.bmp")), sp404::blank_picture()).map_err(|e| e.to_string())?;
        }
        let mono = self.settings()["sp404_mono"].as_bool().unwrap_or(false);
        let loops: HashMap<String, Option<f64>> = {
            let s = self.store.read().unwrap();
            layout.values().map(|p| (p.clone(), s.pos.get(p).and_then(|&i| if s.items[i].meta.is_loop { s.items[i].meta.tempo } else { None }))).collect()
        };
        let mut pads = std::collections::BTreeMap::new();
        let mut files = Vec::new();
        for (n, path) in &layout {
            let data = sp404::smp_bytes(&crate::audio::sp404_channels(Path::new(path), mono)?);
            let f = folder.join("SMPL").join(sp404::bank_file(*n));
            std::fs::write(&f, &data).map_err(|e| e.to_string())?;
            files.push(f.to_string_lossy().into_owned());
            let stem = Path::new(path).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
            pads.insert(*n, (data.len(), stem, loops.get(path).copied().flatten()));
        }
        std::fs::write(folder.join("PADCONF.BIN"), sp404::padconf_bytes(name, &pads, 90.0)).map_err(|e| e.to_string())?;
        Ok(json!({"folder": folder, "files": files}))
    }

    /// Everything written about the sounds, as one file in dest.
    pub fn export_backup(&self, dest: &str) -> Res<String> {
        let d = expand(dest);
        if !d.is_dir() {
            return Err(format!("Not a folder: {}", d.display()));
        }
        let catalog: Vec<Value> = {
            let db = self.db();
            let mut st = db.prepare("SELECT path, tags, memo, rating FROM catalog").unwrap();
            st.query_map([], |r| Ok(json!({"path": r.get::<_, String>(0)?, "tags": r.get::<_, Option<String>>(1)?, "memo": r.get::<_, Option<String>>(2)?, "rating": r.get::<_, Option<i64>>(3)?}))).unwrap().flatten().collect()
        };
        let regions: Vec<Value> = {
            let db = self.db();
            let mut st = db.prepare("SELECT path, data FROM regions").unwrap();
            st.query_map([], |r| Ok(json!({"path": r.get::<_, String>(0)?, "data": r.get::<_, String>(1)?}))).unwrap().flatten().collect()
        };
        let projects: Vec<Value> = self
            .projects()
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let id = p["id"].as_i64().unwrap_or(0);
                json!({"name": p["name"], "folder": p["folder"], "paths": self.project_order(id), "pads": self.project_pads(id)})
            })
            .collect();
        let now = chrono::Local::now();
        let data = json!({"athar_backup": 1, "made": now.format("%Y-%m-%dT%H:%M:%S").to_string(), "catalog": catalog, "projects": projects, "regions": regions});
        let target = unique_path(&d.join(format!("Athar backup {}.json", now.format("%Y-%m-%d"))))?;
        std::fs::write(&target, data.to_string()).map_err(|e| format!("Could not write the backup: {e}"))?;
        Ok(target.to_string_lossy().into_owned())
    }

    /// Tags, notes, ratings, regions and projects from a backup file. What the file holds wins
    /// for the sounds it names; everything else is kept. The file is checked in full first.
    pub fn restore_backup(&self, file: &str) -> Res<Value> {
        let text = std::fs::read_to_string(expand(file)).map_err(|e| format!("That file could not be read as a backup: {e}"))?;
        let data: Value = serde_json::from_str(&text).map_err(|e| format!("That file could not be read as a backup: {e}"))?;
        let cat = data["catalog"].as_array().ok_or("That file is not an Athar backup")?;
        let mut sounds = Vec::new();
        for r in cat {
            let tags: Vec<String> = match &r["tags"] {
                Value::String(t) => serde_json::from_str(t).unwrap_or_default(),
                Value::Array(a) => a.iter().filter_map(|t| t.as_str().map(String::from)).collect(),
                _ => vec![],
            };
            let mut tags: Vec<String> = tags.iter().map(|t| nfc(&t.trim().to_lowercase())).filter(|t| !t.is_empty()).collect();
            tags.sort();
            tags.dedup();
            let memo = r["memo"].as_str().unwrap_or("").to_string();
            let rating = r["rating"].as_i64().unwrap_or(0).clamp(0, 5);
            if let Some(p) = r["path"].as_str() {
                if !tags.is_empty() || !memo.is_empty() || rating > 0 {
                    sounds.push((p.to_string(), serde_json::to_string(&tags).unwrap(), memo, rating));
                }
            }
        }
        let mut members: HashMap<i64, Vec<(f64, String)>> = HashMap::new();
        let mut dated_pads: HashMap<i64, HashMap<String, i64>> = HashMap::new();
        for it in data["project_items"].as_array().cloned().unwrap_or_default() {
            let pid = it["project_id"].as_i64().unwrap_or(-1);
            let path = it["path"].as_str().unwrap_or("").to_string();
            members.entry(pid).or_default().push((it["added"].as_f64().unwrap_or(0.0), path.clone()));
            if let Some(pad) = it["pad"].as_i64() {
                dated_pads.entry(pid).or_default().insert(path, pad);
            }
        }
        let mut projects = Vec::new();
        for p in data["projects"].as_array().cloned().unwrap_or_default() {
            let name = p["name"].as_str().unwrap_or("").trim().to_string();
            if name.is_empty() {
                continue;
            }
            let paths: Vec<String> = if let Some(a) = p["paths"].as_array() {
                a.iter().filter_map(|x| x.as_str().map(String::from)).collect()
            } else {
                let mut m = members.get(&p["id"].as_i64().unwrap_or(-2)).cloned().unwrap_or_default();
                m.sort_by(|a, b| a.0.total_cmp(&b.0));
                m.into_iter().map(|x| x.1).collect()
            };
            let pads: HashMap<String, i64> = match p["pads"].as_object() {
                Some(o) => o.iter().filter_map(|(k, v)| v.as_i64().map(|n| (k.clone(), n))).collect(),
                None => dated_pads.get(&p["id"].as_i64().unwrap_or(-2)).cloned().unwrap_or_default(),
            };
            let pads: HashMap<String, i64> = pads.into_iter().filter(|(_, n)| (0..SP404_PADS).contains(n)).collect();
            projects.push((name, p["folder"].as_str().map(String::from), paths, pads));
        }
        let regions: Vec<(String, String)> = data["regions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|r| Some((r["path"].as_str()?.to_string(), if r["data"].is_string() { r["data"].as_str()?.to_string() } else { r["data"].to_string() })))
            .collect();
        let t = now();
        let mut ids = Vec::new();
        {
            let mut db = self.db();
            let tx = db.transaction().map_err(|e| e.to_string())?;
            for (path, tags, memo, rating) in &sounds {
                tx.execute("INSERT OR IGNORE INTO catalog(path, done, updated) VALUES (?, 0, ?)", params![path, t]).map_err(|e| e.to_string())?;
                tx.execute("UPDATE catalog SET tags=?, memo=?, rating=?, updated=? WHERE path=?", params![tags, memo, rating, t, path]).map_err(|e| e.to_string())?;
            }
            for (path, blob) in &regions {
                tx.execute("INSERT OR REPLACE INTO regions VALUES (?, ?)", params![path, blob]).map_err(|e| e.to_string())?;
            }
            for (name, folder, paths, pads) in &projects {
                tx.execute("INSERT OR IGNORE INTO projects(name) VALUES (?)", [name]).map_err(|e| e.to_string())?;
                let pid: i64 = tx.query_row("SELECT id FROM projects WHERE name=?", [name], |r| r.get(0)).map_err(|e| e.to_string())?;
                for (n, path) in paths.iter().enumerate() {
                    tx.execute("INSERT OR IGNORE INTO project_items(project_id, path, added) VALUES (?, ?, ?)", params![pid, path, t]).map_err(|e| e.to_string())?;
                    tx.execute("UPDATE project_items SET pos=? WHERE project_id=? AND path=?", params![n as f64, pid, path]).map_err(|e| e.to_string())?;
                }
                if !pads.is_empty() {
                    tx.execute("UPDATE project_items SET pad=NULL WHERE project_id=?", [pid]).map_err(|e| e.to_string())?;
                    for (path, pad) in pads {
                        tx.execute("UPDATE project_items SET pad=? WHERE project_id=? AND path=?", params![pad, pid, path]).map_err(|e| e.to_string())?;
                    }
                }
                ids.push((pid, folder.clone()));
            }
            tx.commit().map_err(|e| e.to_string())?;
        }
        for (pid, folder) in ids {
            if let Some(f) = folder.filter(|f| Path::new(f).is_dir()) {
                if self.project_folder(pid).is_none() {
                    let _ = self.set_project_folder(pid, &f);
                }
            }
        }
        Ok(json!({"sounds": sounds.len(), "projects": projects.len(), "regions": regions.len()}))
    }
}

pub fn expand(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        return PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest);
    }
    PathBuf::from(p)
}

#[derive(serde::Deserialize, Default)]
pub struct SearchQuery {
    pub query: Option<String>,
    pub similar_to: Option<String>,
    pub category: Option<String>,
    pub min_dur: Option<f64>,
    pub folder: Option<String>,
    pub under: Option<String>,
    pub project: Option<i64>,
    pub tag: Option<String>,
    pub min_rating: Option<i64>,
    pub sort: Option<String>,
    pub limit: Option<i64>,
}

struct FileRow {
    meta: Meta,
    mtime: f64,
    size: i64,
    sha1: Option<String>,
    onsets: i64,
    error: Option<String>,
}

enum Prepared {
    Ok(FileRow, Vec<Vec<f32>>),
    Failed(FileRow),
    Skip,
}

/// Decode and analyse one file; runs on the rayon pool.
fn prepare(folder: &str, p: &str) -> Prepared {
    let Ok(md) = std::fs::metadata(p) else { return Prepared::Skip };
    let base = FileRow { meta: Meta { path: p.into(), folder: folder.into(), ..Default::default() }, mtime: mtime_of(&md), size: md.size() as i64, sha1: None, onsets: 0, error: None };
    let run = || -> Result<(FileRow, Vec<Vec<f32>>), String> {
        let d = decode(Path::new(p)).map_err(|e| e.to_string())?;
        let y = load_audio(&d).map_err(|e| e.to_string())?;
        let a = analyze(&y, d.duration(), &basename(p)).map_err(|e| e.to_string())?;
        let cl = clips(&d, &y).map_err(|e| e.to_string())?;
        let meta = Meta {
            path: p.into(),
            folder: folder.into(),
            duration: Some(d.duration()),
            sr: Some(d.sr as i64),
            channels: Some(d.channels.len() as i64),
            peak_db: Some(a.peak_db),
            rms_db: Some(a.rms_db),
            crest: Some(a.crest),
            brightness: Some(a.brightness),
            pitch_hz: a.pitch_hz,
            tempo: a.tempo,
            is_loop: a.is_loop,
            peaks: peaks(&d, &y),
            recorded: recorded_date(p),
        };
        Ok((FileRow { meta, mtime: base.mtime, size: base.size, sha1: file_hash(p, base.size as u64).ok(), onsets: a.onsets as i64, error: None }, cl))
    };
    match run() {
        Ok((row, cl)) => Prepared::Ok(row, cl),
        Err(e) => Prepared::Failed(FileRow { error: Some(e), ..base }),
    }
}
