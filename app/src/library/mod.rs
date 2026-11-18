//! The sound library: the SQLite index, the in-memory search index, background indexing,
//! the catalog (tags, notes, ratings), projects and their pads.

mod arrangement;
mod backup;
mod catalog;
mod export;
mod files;
mod index;
mod map;
mod paths;
mod projects;
mod search;
mod settings;
mod store;
mod watch;
#[cfg(test)]
mod tests;

pub use paths::{ascii_name, is_inside, model_dir, safe_name, unique_path};
pub use search::SearchQuery;

use athar_core::embed::Clap;
use catalog::CatalogRow;
use paths::data_dir;
use rayon::prelude::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use store::{dot, emb_of, row_meta, Store, EMB_DIM};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub type Res<T> = Result<T, String>;

const CATEGORIES: [(&str, &str); 41] = [
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

const ROOM_TONE: (&str, &str) = ("Room tone", "quiet room tone or background hum");

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS folders (path TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS files (
    path TEXT PRIMARY KEY, folder TEXT, mtime REAL, size INTEGER, sha1 TEXT,
    duration REAL, sr INTEGER, channels INTEGER, rms_db REAL, brightness REAL,
    pitch_hz REAL, tempo REAL, is_loop INTEGER,
    emb BLOB, peaks BLOB, error TEXT, recorded TEXT
);
CREATE INDEX IF NOT EXISTS files_folder ON files(folder);
CREATE INDEX IF NOT EXISTS files_sha1 ON files(sha1);
CREATE TABLE IF NOT EXISTS gone (path TEXT PRIMARY KEY, sha1 TEXT, at REAL);
CREATE INDEX IF NOT EXISTS gone_sha1 ON gone(sha1);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS catalog (
    path TEXT PRIMARY KEY, tags TEXT, memo TEXT, rating INTEGER, updated REAL, origin TEXT
);
CREATE TABLE IF NOT EXISTS regions (path TEXT PRIMARY KEY, data TEXT);
CREATE TABLE IF NOT EXISTS projects (id INTEGER PRIMARY KEY, name TEXT UNIQUE, folder TEXT);
CREATE TABLE IF NOT EXISTS project_items (
    project_id INTEGER, path TEXT, added REAL, pos REAL, pad INTEGER, PRIMARY KEY (project_id, path)
);
";

/// Turns the clips of one sound into its embedding.
type Embed<'a> = &'a (dyn Fn(&[Vec<f32>]) -> Res<Vec<f32>> + Sync);

enum Job {
    /// Walk `dir`, which is the library folder `folder` or a folder inside it.
    Scan { folder: String, dir: String },
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
    jobs: Sender<Job>,
    pending: AtomicUsize,
    /// Folders with a scan waiting in the queue, so a second request for one is dropped.
    scans: Mutex<HashSet<String>>,
    /// The catalog as search reads it, loaded on first use and dropped by every write.
    catalog: Mutex<Option<Arc<HashMap<String, CatalogRow>>>>,
    map: Mutex<Option<(u64, Value)>>,
    map_busy: AtomicBool,
    model: PathBuf,
    watcher: Mutex<Option<Sender<(bool, String)>>>,
    changed: Arc<Mutex<HashSet<String>>>,
    /// Folders that were missing when their watch was asked for, such as one on a drive that is away.
    unwatched: Arc<Mutex<HashSet<String>>>,
    pub data_dir: PathBuf,
}

impl Library {
    pub fn open(model: PathBuf) -> Res<Arc<Self>> {
        let (lib, rx) = Self::create(data_dir(), model)?;
        let l = lib.clone();
        std::thread::spawn(move || l.boot(rx));
        Ok(lib)
    }

    /// The library on its index in data_dir, before the model loads and the queue starts.
    fn create(data_dir: PathBuf, model: PathBuf) -> Res<(Arc<Self>, Receiver<Job>)> {
        std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let db = Connection::open(data_dir.join("index.db")).map_err(|e| e.to_string())?;
        // Every library keeps its journal the same way, whichever version of the app made it.
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;").map_err(|e| e.to_string())?;
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
        // An embedding of another length was made by another model: those files are indexed again.
        db.execute("DELETE FROM files WHERE emb IS NOT NULL AND length(emb) != ?", [(EMB_DIM * 4) as i64]).map_err(|e| e.to_string())?;
        let mut store = Store::default();
        for (m, blob) in query_rows(&db, "SELECT * FROM files WHERE emb IS NOT NULL", [], row_meta) {
            store.put(m, emb_of(&blob), None);
        }
        let (tx, rx) = channel();
        let lib = Arc::new(Self {
            db: Mutex::new(db),
            store: RwLock::new(store),
            engine: OnceLock::new(),
            categories: OnceLock::new(),
            nonsense: OnceLock::new(),
            status: Mutex::new(Status { model: "loading".into(), ..Default::default() }),
            jobs: tx,
            pending: AtomicUsize::new(0),
            scans: Mutex::new(HashSet::new()),
            catalog: Mutex::new(None),
            map: Mutex::new(None),
            map_busy: AtomicBool::new(false),
            model,
            watcher: Mutex::new(None),
            changed: Arc::new(Mutex::new(HashSet::new())),
            unwatched: Arc::new(Mutex::new(HashSet::new())),
            data_dir,
        });
        lib.retire_old_name_pattern();
        lib.daily_backup();
        Ok((lib, rx))
    }

    /// Loads the model, then works through the queue for as long as the app runs.
    fn boot(self: Arc<Self>, rx: Receiver<Job>) {
        let clap = match Clap::load(&self.model) {
            Ok(clap) => clap,
            Err(e) => {
                let mut s = self.status.lock().unwrap();
                s.model = "error".into();
                s.error = Some(format!("{e} (model folder {})", self.model.display()));
                return;
            }
        };
        let mut texts: Vec<&str> = CATEGORIES.iter().map(|c| c.1).collect();
        texts.push(ROOM_TONE.1);
        let names: Vec<String> = CATEGORIES.iter().map(|c| c.0.to_string()).chain([ROOM_TONE.0.to_string()]).collect();
        let mut problems = Vec::new();
        match clap.embed_texts(&texts) {
            Ok(m) => {
                let _ = self.categories.set((names, m));
            }
            Err(e) => problems.push(format!("Sounds could not be sorted into categories: {e}")),
        }
        match clap.embed_texts(&NONSENSE) {
            Ok(m) => {
                let mean = (0..m[0].len()).map(|d| m.iter().map(|v| v[d]).sum::<f32>() / m.len() as f32).collect();
                let _ = self.nonsense.set(mean);
            }
            Err(e) => problems.push(format!("Searching by words keeps weak matches: {e}")),
        }
        let _ = self.engine.set(clap);
        self.assign_categories();
        {
            let mut s = self.status.lock().unwrap();
            s.model = "ready".into();
            if !problems.is_empty() {
                s.error = Some(problems.join(". "));
            }
        }
        self.start_watching();
        self.ensure_project_folders();
        if self.settings()["rescan_on_open"].as_bool().unwrap_or(true) {
            self.rescan();
        }
        let clap = self.engine.get().unwrap();
        let embed = |cl: &[Vec<f32>]| clap.embed_sound(cl).map_err(|e| e.to_string());
        while let Ok(job) = rx.recv() {
            self.run_job(job, &embed);
        }
    }

    fn run_job(&self, job: Job, embed: Embed) {
        match job {
            Job::Scan { folder, dir } => {
                self.scans.lock().unwrap().remove(&dir);
                self.scan(&folder, &dir, embed);
            }
            Job::Files(folder, paths) => self.index(&folder, paths, embed),
        }
        self.pending.fetch_sub(1, Ordering::SeqCst);
        let mut s = self.status.lock().unwrap();
        s.current = None;
        s.folder = None;
    }

    fn category_of(&self, emb: &[f32]) -> Option<usize> {
        let (_, m) = self.categories.get()?;
        m.iter().enumerate().map(|(i, c)| (i, dot(emb, c))).max_by(|a, b| a.1.total_cmp(&b.1)).map(|best| best.0)
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
        // Nothing takes the job when the model did not load, so it is not counted as waiting.
        if self.jobs.send(job).is_err() {
            self.pending.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Queues a walk of `dir`, the library folder `folder` or a folder inside it, unless one
    /// is already waiting.
    fn enqueue_scan(&self, folder: &str, dir: &str) {
        if self.scans.lock().unwrap().insert(dir.to_string()) {
            self.enqueue(Job::Scan { folder: folder.into(), dir: dir.into() });
        }
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        // A thread that panicked while it held the lock leaves the connection usable.
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn rows<T, P: rusqlite::Params>(&self, sql: &str, p: P, f: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>) -> Vec<T> {
        query_rows(&self.db(), sql, p, f)
    }

    fn one<T, P: rusqlite::Params>(&self, sql: &str, p: P, f: impl FnOnce(&rusqlite::Row) -> rusqlite::Result<T>) -> Option<T> {
        query_one(&self.db(), sql, p, f)
    }

    /// One statement that touches neither tags, notes, ratings nor origins: those go through `write`.
    fn exec<P: rusqlite::Params>(&self, sql: &str, p: P) -> Res<usize> {
        self.db().execute(sql, p).map_err(|e| e.to_string())
    }

    /// Several statements as one transaction: all of them are saved, or none.
    fn write<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Res<T> {
        let out = {
            let mut db = self.db();
            let tx = db.transaction().map_err(|e| e.to_string())?;
            let out = f(&tx).map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            out
        };
        *self.catalog.lock().unwrap() = None;
        Ok(out)
    }

    fn meta_get(&self, key: &str) -> Option<String> {
        self.one("SELECT value FROM meta WHERE key=?", [key], |r| r.get(0))
    }

    fn meta_set(&self, key: &str, value: &str) -> Res<()> {
        self.exec("INSERT OR REPLACE INTO meta VALUES (?, ?)", params![key, value]).map(|_| ())
    }

    pub fn status(&self) -> Value {
        let s = self.status.lock().unwrap().clone();
        let mut v = serde_json::to_value(s).unwrap();
        v["indexed"] = json!(self.store.read().unwrap().items.len());
        v["pending_jobs"] = json!(self.pending.load(Ordering::SeqCst));
        v
    }
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// Every row of a query. A query or a row that fails is reported on stderr and left out.
fn query_rows<T, P: rusqlite::Params>(db: &Connection, sql: &str, p: P, f: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>) -> Vec<T> {
    let run = || -> rusqlite::Result<Vec<T>> {
        let mut st = db.prepare(sql)?;
        let rows = st.query_map(p, f)?;
        Ok(rows.filter_map(|r| r.map_err(|e| eprintln!("library: a row was skipped: {e} ({sql})")).ok()).collect())
    };
    run().unwrap_or_else(|e| {
        eprintln!("library: a query failed: {e} ({sql})");
        vec![]
    })
}

/// The first row of a query, if there is one.
fn query_one<T, P: rusqlite::Params>(db: &Connection, sql: &str, p: P, f: impl FnOnce(&rusqlite::Row) -> rusqlite::Result<T>) -> Option<T> {
    db.query_row(sql, p, f).optional().unwrap_or_else(|e| {
        eprintln!("library: a query failed: {e} ({sql})");
        None
    })
}

fn has_row(db: &Connection, table: &str, path: &str) -> bool {
    query_one(db, &format!("SELECT 1 FROM {table} WHERE path=?"), [path], |_| Ok(())).is_some()
}

/// Reports a write that failed, for the callers that have nobody to return the error to.
fn report<T>(what: &str, r: Res<T>) -> Option<T> {
    r.map_err(|e| eprintln!("library: {what} failed: {e}")).ok()
}
