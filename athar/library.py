import json
import os
import queue
import re
import shutil
import sqlite3
import struct
import unicodedata
from contextlib import contextmanager
from datetime import datetime
import hashlib
import threading
import time
from concurrent.futures import ThreadPoolExecutor
import traceback
from pathlib import Path

import numpy as np
from watchdog.events import FileSystemEventHandler
from watchdog.observers import Observer

from . import engine as eng

AUDIO_EXTS = {".wav", ".aif", ".aiff", ".flac", ".mp3", ".ogg", ".m4a", ".aac", ".wv"}
# Files analysed and embedded per step: a multiple of the embedding callers, so they stay busy.
BATCH = eng.EMBED_CALLERS * 4
DECODE_THREADS = 4
ANALYSIS_VERSION = 3
HASH_SPAN = 1 << 20
BACKUPS_KEPT = 10
EMB_DIM = 512
SP404_SR = 48000
SP404_MAX_SECONDS = 960

SCHEMA = """
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
    project_id INTEGER, path TEXT, added REAL, pos REAL, PRIMARY KEY (project_id, path)
);
"""

META_COLS = ["path", "folder", "duration", "sr", "channels", "peak_db", "rms_db", "crest",
             "brightness", "pitch_hz", "tempo", "onsets", "is_loop", "peaks", "recorded"]


DEFAULT_SETTINGS = {
    "text_size": 13,
    "show_cat": True,
    "cat_skin": "sukey",
    "accent": "#7a3dff",
    "autoplay": False,
    "preview_volume": 80,
    "results_limit": 250,
    "default_sort": "name",
    "hide_shorter_ms": 0,
    "rescan_on_open": True,
    "export_rate": 0,
    "export_bits": 24,
    "export_mono": False,
    "edit_suffix": "_edit",
    "export_format": "wav",
    "smooth_edits": True,
    "edit_fade_ms": 3,
    "loudness_target": -16.0,
    "prep_target_db": -1.0,
    "prep_fade_in_ms": 2,
    "prep_fade_out_ms": 10,
    "silence_db": -45,
    "slice_gap_ms": 250,
    "sp404_mono": False,
    "name_pattern": "{project}_{category}_{n:03d}_{name}",
    "projects_dir": "",
    "theme": "system",
    "freesound_key": "",
}


MAP_FINE_LIMIT = 6000
# Limits of the Arranger. athar/static/arrange.js holds the same ones for the page.
ARRANGE_MAX_LANES = 16
ARRANGE_MAX_HITS = 64
ARRANGE_MAX_SECONDS = 600


def default_projects_dir():
    """Where new projects keep their sounds until the user picks another place. A Athar
    started on its own data folder (tests, trials) keeps projects in there too."""
    if os.environ.get("ATHAR_HOME"):
        return str(data_dir() / "Projects")
    music = Path.home() / "Music"
    # Projects made while the app was called Lyra stay where they are, and new ones join them.
    if (music / "Lyra Projects").is_dir() and not (music / "Athar Projects").exists():
        return str(music / "Lyra Projects")
    return str(music / "Athar Projects")


def data_dir():
    env = os.environ.get("ATHAR_HOME")
    base = Path.home() / "Library" / "Application Support"
    d = Path(env) if env else base / "Athar"
    if not env and not d.exists():
        # The app was called Lyra, and Waves before that: carry the library over.
        for old in ("Lyra", "Waves"):
            if (base / old).is_dir():
                (base / old).rename(d)
                break
    d.mkdir(parents=True, exist_ok=True)
    return d


def nfc(s):
    return unicodedata.normalize("NFC", s)


def is_inside(path, folder):
    """True when path lies strictly below folder. Compares NFC forms, since macOS may hand
    back the same name precomposed or decomposed depending on the API."""
    path, folder = nfc(path), nfc(folder)
    return path != folder and path.startswith(folder.rstrip(os.sep) + os.sep)


def file_hash(path, size):
    """Identity of a file's content: its size plus its first and last megabyte."""
    h = hashlib.sha1(str(size).encode())
    with open(path, "rb") as f:
        h.update(f.read(HASH_SPAN))
        if size > 2 * HASH_SPAN:
            f.seek(size - HASH_SPAN)
            h.update(f.read(HASH_SPAN))
    return h.hexdigest()


def safe_name(s):
    return re.sub(r"[^\w\-. ]+", "_", s).strip().lstrip(". ") or "untitled"


def ascii_name(s):
    s = unicodedata.normalize("NFKD", s).encode("ascii", "ignore").decode()
    return re.sub(r"[^A-Za-z0-9_\-]+", "_", s).strip("_") or "sample"


def sp404_dir(dest):
    """The SP-404MKII imports from IMPORT at the SD card root; use it when present."""
    dest = Path(dest).expanduser()
    imp = dest / "IMPORT"
    return imp if imp.is_dir() else dest


def to_sp404(src, target, mono=False):
    """48 kHz, 16-bit PCM WAV: the format the SP-404MKII imports from SD card."""
    import librosa
    import soundfile as sf
    y, sr = eng.read_any(str(src))
    try:
        exact = sf.info(str(src)).subtype in ("PCM_16", "PCM_S8", "PCM_U8")
    except Exception:
        exact = False
    if y.shape[0] > 1 and mono:
        y, exact = y.mean(axis=0, keepdims=True), False
    else:
        y = y[:2]
    if sr != SP404_SR:
        y, exact = np.stack([librosa.resample(c, orig_sr=sr, target_sr=SP404_SR) for c in y]), False
    y = y[:, : SP404_SR * SP404_MAX_SECONDS]
    if not exact:
        # TPDF dither, one 16-bit step peak to peak, so the word-length cut adds noise instead of distortion.
        rng = np.random.default_rng()
        y = y + (rng.random(y.shape, np.float32) - rng.random(y.shape, np.float32)) / 32768
    y = np.clip(y, -1, 1)
    sf.write(str(target), y.T, SP404_SR, subtype="PCM_16", format="WAV")


SAVE_FORMATS = {"wav": None, "flac": "FLAC", "mp3": "MP3"}


def transcode(wav, target, fmt):
    """Rewrite a WAV as FLAC or MP3. FLAC keeps 16 or 24 bits; anything wider becomes 24."""
    import soundfile as sf
    y, sr = sf.read(str(wav), dtype="float32", always_2d=True)
    if fmt == "flac":
        sub = "PCM_16" if sf.info(str(wav)).subtype == "PCM_16" else "PCM_24"
        sf.write(str(target), y, sr, format="FLAC", subtype=sub)
    else:
        sf.write(str(target), np.clip(y, -1, 1), sr, format="MP3", subtype="MPEG_LAYER_III")


def check_pattern(pattern):
    try:
        pattern.format(project="p", category="c", n=1, name="n")
    except (KeyError, ValueError, IndexError, AttributeError) as e:
        raise ValueError(f"Bad naming pattern: {e}") from None


# Schema changes for existing libraries, applied in order on startup. Entry i upgrades
# PRAGMA user_version i to i + 1. SCHEMA always describes the latest version, so a new
# library starts at len(MIGRATIONS) and skips them.
MIGRATIONS = [
    "ALTER TABLE files ADD COLUMN recorded TEXT;",
    "ALTER TABLE projects ADD COLUMN folder TEXT;",
    "ALTER TABLE project_items ADD COLUMN pos REAL;",
    "ALTER TABLE catalog ADD COLUMN origin TEXT;",
]


class _Store:
    """Indexed sounds held for search: one metadata dict and one embedding row per path.
    Rows live in a single growing matrix, so a snapshot is one copy, not a restack."""

    def __init__(self):
        self.paths, self.metas, self.pos = [], [], {}
        self.M = np.zeros((1024, EMB_DIM), np.float32)

    def __len__(self):
        return len(self.paths)

    def get(self, path):
        i = self.pos.get(path)
        return None if i is None else (self.metas[i], self.M[i].copy())

    def put(self, path, meta, emb=None):
        i = self.pos.get(path)
        if i is None:
            i = len(self.paths)
            if i == len(self.M):
                self.M = np.concatenate([self.M, np.zeros_like(self.M)])
            self.paths.append(path)
            self.metas.append(meta)
            self.pos[path] = i
        else:
            self.metas[i] = meta
        if emb is not None:
            self.M[i] = emb

    def drop(self, path):
        i = self.pos.pop(path, None)
        if i is None:
            return False
        last = len(self.paths) - 1
        if i != last:
            self.paths[i], self.metas[i], self.M[i] = self.paths[last], self.metas[last], self.M[last]
            self.pos[self.paths[i]] = i
        self.paths.pop()
        self.metas.pop()
        return True

    def rename(self, old, new, **changes):
        if old not in self.pos:
            return False
        if new != old:
            self.drop(new)
        i = self.pos.pop(old)
        self.paths[i] = new
        self.metas[i] = {**self.metas[i], "path": new, **changes}
        self.pos[new] = i
        return True


class Library:
    def migrate(self):
        v = self.db.execute("PRAGMA user_version").fetchone()[0]
        for i in range(v, len(MIGRATIONS)):
            self.db.executescript(MIGRATIONS[i])
            self.db.execute(f"PRAGMA user_version = {i + 1}")
        self.db.commit()

    def __init__(self, db_path=None):
        self.dblock = threading.RLock()
        self._maplock, self._map = threading.Lock(), None
        self.db_path = str(db_path or data_dir() / "index.db")
        self.db = sqlite3.connect(self.db_path, check_same_thread=False)
        self.db.row_factory = sqlite3.Row
        fresh = not self.db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='files'").fetchone()
        self.db.executescript(SCHEMA)
        if fresh:
            self.db.execute(f"PRAGMA user_version = {len(MIGRATIONS)}")
        self.migrate()
        row = self.q("SELECT value FROM meta WHERE key='analysis_version'")
        if not row or int(row[0]["value"]) != ANALYSIS_VERSION:
            # Older analysis: mark every file stale so the next scan re-analyses it.
            self.x("UPDATE files SET mtime=0")
            self.x("INSERT OR REPLACE INTO meta VALUES ('analysis_version', ?)", (str(ANALYSIS_VERSION),))
        self.engine = None
        self.ready = threading.Event()
        self.status = {"model": "loading", "error": None, "queued": 0, "done": 0,
                       "current": None, "folder": None}
        self.jobs = queue.Queue()
        self._store = _Store()
        self._cat, self._hay = None, {}
        self._dirty = True
        self._cache = None
        self._memlock = threading.Lock()
        self._observer = None
        self._watches = {}
        self._watch_lock = threading.Lock()
        self._pending, self._pending_lock = set(), threading.Lock()
        self._pool = ThreadPoolExecutor(DECODE_THREADS, thread_name_prefix="athar-decode")
        self._load_memory()
        self._backup()
        threading.Thread(target=self._boot, daemon=True).start()
        threading.Thread(target=self._worker, daemon=True).start()

    # ---- db helpers
    def q(self, sql, args=()):
        with self.dblock:
            return self.db.execute(sql, args).fetchall()

    def x(self, sql, args=()):
        with self.dblock:
            self.db.execute(sql, args)
            self.db.commit()

    @contextmanager
    def tx(self):
        with self.dblock:
            try:
                yield self.db
                self.db.commit()
            except BaseException:
                self.db.rollback()
                raise

    # ---- startup
    def _boot(self):
        delay = 5
        while self.engine is None:
            try:
                self.engine = eng.Engine()
            except Exception as e:
                self.status["model"] = "error"
                self.status["error"] = f"{type(e).__name__}: {e}"
                traceback.print_exc()
                time.sleep(delay)
                delay = min(delay * 2, 300)
        self.status.update(model="ready", error=None)
        self.ready.set()
        try:
            self._start_watching()
        except Exception:
            traceback.print_exc()
        self.ensure_project_folders()
        if self.settings()["rescan_on_open"]:
            self.rescan()

    def _backup(self):
        """Tags, notes, ratings, projects and settings as dated JSON beside the index: the
        first launch of each day writes one, and the most recent BACKUPS_KEPT are kept."""
        try:
            data = {
                "catalog": [dict(r) for r in self.q("SELECT path, tags, memo, rating, updated FROM catalog")],
                "projects": [dict(r) for r in self.q("SELECT id, name FROM projects")],
                "project_items": [dict(r) for r in self.q("SELECT project_id, path, added FROM project_items")],
                "folders": self.folders(),
                "settings": self.settings(),
            }
            if not (data["catalog"] or data["projects"]):
                return
            d = Path(self.db_path).parent / "backups"
            d.mkdir(exist_ok=True)
            today = d / f"catalog-{datetime.now():%Y-%m-%d}.json"
            if not today.exists():
                today.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
            for old in sorted(d.glob("catalog-*.json"))[:-BACKUPS_KEPT]:
                old.unlink()
        except Exception:
            traceback.print_exc()

    def _load_memory(self):
        cols = ", ".join(META_COLS)
        for r in self.q(f"SELECT {cols}, emb FROM files WHERE emb IS NOT NULL"):
            self._put_mem(dict(r))

    def _put_mem(self, row):
        emb = np.frombuffer(row.pop("emb"), dtype=np.float32)
        with self._memlock:
            self._store.put(row["path"], row, emb)
            self._dirty = True

    def _drop_mem(self, path):
        with self._memlock:
            if self._store.drop(path):
                self._dirty = True

    def count(self):
        with self._memlock:
            return len(self._store)

    def snapshot(self):
        """Arrays for vectorised search, copied only when the index changed."""
        with self._memlock:
            if self._dirty or self._cache is None:
                paths, metas = list(self._store.paths), list(self._store.metas)
                M = self._store.M[:len(paths)].copy()
                cats = np.zeros(len(paths), dtype=int)
                cat_scores = np.zeros(len(paths), dtype=np.float32)
                if self.engine is not None and len(paths):
                    S = M @ self.engine.category_matrix.T
                    cats = S.argmax(1)
                    cat_scores = S.max(1)
                self._cache = (paths, metas, M, cats, cat_scores)
                self._dirty = self.engine is None
            return self._cache

    # ---- catalog: tags, notes, rating
    def catalog_all(self):
        """Every catalog row by path, read once and kept until a catalog write. Rows whose
        file is missing stay here on purpose, so their tags return when the file does."""
        with self.dblock:
            if self._cat is None:
                out = {}
                for r in self.db.execute("SELECT path, tags, memo, rating, origin FROM catalog"):
                    d = dict(r)
                    d["tags"] = json.loads(d["tags"]) if d["tags"] else []
                    out[d["path"]] = d
                self._cat, self._hay = out, {}
            return self._cat

    def _catalog_changed(self):
        self._cat = None

    def catalog_get(self, path):
        rows = self.q("SELECT path, tags, memo, rating FROM catalog WHERE path=?", (path,))
        d = dict(rows[0]) if rows else {"path": path, "tags": None, "memo": "", "rating": 0}
        d["tags"] = json.loads(d["tags"]) if d["tags"] else []
        return d

    def catalog_set(self, path, changes):
        fields = {k: v for k, v in changes.items() if k in ("tags", "memo", "rating")}
        if "rating" in fields:
            fields["rating"] = max(0, min(5, int(fields["rating"] or 0)))
        if "tags" in fields:
            fields["tags"] = json.dumps(sorted({nfc(t.strip().lower()) for t in fields["tags"] if t and t.strip()}))
        with self.tx() as db:
            db.execute("INSERT OR IGNORE INTO catalog(path, done, updated) VALUES (?, 0, ?)", (path, time.time()))
            for k, v in fields.items():
                db.execute(f"UPDATE catalog SET {k}=?, updated=? WHERE path=?", (v, time.time(), path))
            self._catalog_changed()
        return self.catalog_get(path)

    def catalog_copy(self, src, dst):
        """Give dst the tags, note and rating of src, for a file saved from an edit of src.
        Whatever dst already has is kept."""
        c = self.catalog_get(src)
        mine = self.catalog_get(dst)
        if not (c["tags"] or c["memo"] or c["rating"]) or mine["tags"] or mine["memo"] or mine["rating"]:
            return
        self.catalog_set(dst, {"tags": c["tags"], "memo": c["memo"] or "", "rating": c["rating"] or 0})

    def set_origin(self, path, origin):
        """Remember which file path was made from: the sound an edit was saved from, or the
        library sound a project copy was taken from. The first origin recorded is kept."""
        if not origin or nfc(origin) == nfc(path):
            return
        with self.tx() as db:
            db.execute("INSERT OR IGNORE INTO catalog(path, done, updated) VALUES (?, 0, ?)", (path, time.time()))
            db.execute("UPDATE catalog SET origin=? WHERE path=? AND origin IS NULL", (origin, path))
            self._catalog_changed()

    # ---- regions: named spans of a file, in seconds, kept between sessions
    def regions_get(self, path):
        rows = self.q("SELECT data FROM regions WHERE path=?", (path,))
        return json.loads(rows[0]["data"]) if rows else []

    def regions_set(self, path, regions):
        clean = []
        for r in regions:
            a, b = float(r["start"]), float(r["end"])
            if b > a >= 0:
                clean.append({"start": a, "end": b, "name": str(r.get("name") or "")[:80]})
        if clean:
            self.x("INSERT OR REPLACE INTO regions VALUES (?, ?)", (path, json.dumps(clean)))
        else:
            self.x("DELETE FROM regions WHERE path=?", (path,))
        return clean

    # ---- the arrangement: the Arranger's named lanes, each with a level and clips placed in time
    def arrangement_get(self):
        row = self.q("SELECT value FROM meta WHERE key='arrangement'")
        return self._lanes_of_clips(json.loads(row[0]["value"])) if row else {"loop": True, "lanes": []}

    @staticmethod
    def _lanes_of_clips(data):
        """An arrangement kept from when each lane was one sound placed several times, as it is
        now: a named lane holding clips. Each old lane becomes a lane named after its sound."""
        if all("clips" in lane for lane in data.get("lanes") or []):
            return data
        lanes = []
        for lane in data.get("lanes") or []:
            if "clips" in lane:
                lanes.append(lane)
                continue
            lanes.append({
                "name": os.path.splitext(lane.get("name") or "")[0], "gain": lane.get("gain") or 0,
                "mute": bool(lane.get("mute")), "solo": bool(lane.get("solo")),
                "clips": [{"path": lane["path"], "name": lane.get("name") or "", "duration": lane.get("duration") or 0,
                           "sr": lane.get("sr") or 0, "at": at, "in": 0.0, "out": 0.0} for at in lane.get("hits") or []],
            })
        return {"loop": bool(data.get("loop", True)), "lanes": lanes}

    def arrangement_set(self, data):
        lanes = []
        for n, lane in enumerate(list(self._lanes_of_clips(data).get("lanes") or [])[:ARRANGE_MAX_LANES]):
            clips = []
            for clip in list(lane.get("clips") or [])[:ARRANGE_MAX_HITS]:
                path = str(clip["path"])
                duration = max(0.0, float(clip.get("duration") or 0))
                # The two fades of a clip cannot be longer, together, than the clip.
                fade_in = min(duration, max(0.0, float(clip.get("in") or 0)))
                fade_out = min(duration - fade_in, max(0.0, float(clip.get("out") or 0)))
                clips.append({
                    "path": path, "name": os.path.basename(path), "duration": duration, "sr": int(clip.get("sr") or 0),
                    "at": min(ARRANGE_MAX_SECONDS, max(0.0, float(clip.get("at") or 0))), "in": fade_in, "out": fade_out,
                })
            lanes.append({
                "name": str(lane.get("name") or "").strip()[:60] or f"Lane {n + 1}",
                "gain": min(6.0, max(-24.0, float(lane.get("gain") or 0))),
                "mute": bool(lane.get("mute")), "solo": bool(lane.get("solo")), "clips": clips,
            })
        clean = {"loop": bool(data.get("loop", True)), "lanes": lanes}
        self.x("INSERT OR REPLACE INTO meta VALUES ('arrangement', ?)", (json.dumps(clean),))
        return clean

    def _move_arrangement(self, db, src, dst, is_dir):
        """Point the Arranger's clips at a file's new path. Call inside tx()."""
        row = db.execute("SELECT value FROM meta WHERE key='arrangement'").fetchone()
        if not row:
            return
        data, sp, changed = self._lanes_of_clips(json.loads(row[0])), src.rstrip(os.sep) + os.sep, False
        for lane in data["lanes"]:
            for clip in lane["clips"]:
                p = clip["path"]
                if is_dir and p.startswith(sp):
                    p = dst.rstrip(os.sep) + os.sep + p[len(sp):]
                elif not is_dir and p == src:
                    p = dst
                if p != clip["path"]:
                    clip["path"], clip["name"], changed = p, os.path.basename(p), True
        if changed:
            db.execute("INSERT OR REPLACE INTO meta VALUES ('arrangement', ?)", (json.dumps(data),))

    def all_tags(self):
        counts = {}
        for r in self.q("SELECT c.tags FROM catalog c JOIN files f ON f.path = c.path WHERE c.tags IS NOT NULL"):
            for t in json.loads(r["tags"] or "[]"):
                counts[t] = counts.get(t, 0) + 1
        return [{"tag": t, "count": n} for t, n in sorted(counts.items(), key=lambda x: (-x[1], x[0]))]

    # ---- moving and renaming
    def _move_rows(self, db, src, dst, is_dir, folder):
        """Re-key index, catalog and project rows from src to dst. Call inside tx()."""
        if is_dir:
            sp, dp = src.rstrip(os.sep) + os.sep, dst.rstrip(os.sep) + os.sep
            match, args = "substr(path, 1, ?) = ?", (len(sp), sp)
            taken, taken_args = "substr({t}.path, 1, ?) = ?", (len(dp), dp)
            new, new_args = "? || substr(path, ?)", (dp, len(sp) + 1)
            origin, origin_args = "? || substr({t}.path, ?)", (sp, len(dp) + 1)
        else:
            sp = dp = None
            match, args = "path = ?", (src,)
            taken, taken_args = "{t}.path = ?", (dst,)
            new, new_args = "?", (dst,)
            origin, origin_args = "?", (src,)
        # A row already at the destination gives way only to a row arriving from the source.
        # Saving by "write a temp file, rename it over the original" moves a path that was
        # never indexed onto one that was, and the original's tags and rating must survive.
        for t in ("files", "catalog", "regions"):
            db.execute(f"DELETE FROM {t} WHERE {taken.format(t=t)} AND EXISTS "
                       f"(SELECT 1 FROM {t} s WHERE s.path = {origin.format(t=t)})", (*taken_args, *origin_args))
        db.execute(f"UPDATE files SET path = {new}, folder = ? WHERE {match}", (*new_args, folder, *args))
        db.execute(f"UPDATE catalog SET path = {new} WHERE {match}", (*new_args, *args))
        db.execute(f"UPDATE regions SET path = {new} WHERE {match}", (*new_args, *args))
        db.execute(f"UPDATE OR IGNORE project_items SET path = {new} WHERE {match}", (*new_args, *args))
        db.execute(f"DELETE FROM project_items WHERE {match}", args)
        db.execute(f"DELETE FROM gone WHERE {match}", args)
        self._move_arrangement(db, src, dst, is_dir)
        self._catalog_changed()
        with self._memlock:
            moved = [p for p in self._store.paths if (p.startswith(sp) if is_dir else p == src)]
            for p in moved:
                self._store.rename(p, dp + p[len(sp):] if is_dir else dst, folder=folder)
            if moved:
                self._dirty = True

    def move(self, src, dst, is_dir=False):
        """Follow a move made outside Athar, keeping tags, ratings and project membership."""
        folder, dst = self._locate(dst)
        if folder is None:
            self.forget(src, is_dir)
            return
        with self.tx() as db:
            self._move_rows(db, src, dst, is_dir, folder)

    def rename(self, path, new_stem):
        src = Path(path)
        if not src.is_file():
            raise FileNotFoundError(path)
        stem = safe_name(new_stem).strip(". ")
        if not stem:
            raise ValueError("Name is empty")
        target = src.with_name(stem + src.suffix)
        if target == src:
            return str(src)
        if target.exists() and not os.path.samefile(target, src):
            raise FileExistsError(f"{target.name} already exists in that folder")
        new = str(target)
        # Holding the lock across the disk rename keeps the watcher's move event from
        # running between the rename and the row updates.
        with self.tx() as db:
            src.rename(target)
            folder = (db.execute("SELECT folder FROM files WHERE path=?", (path,)).fetchone() or [None])[0]
            self._move_rows(db, path, new, False, folder)
            st = target.stat()
            db.execute("UPDATE files SET mtime=?, size=? WHERE path=?", (st.st_mtime, st.st_size, new))
        return new

    # ---- settings
    def settings(self):
        row = self.q("SELECT value FROM meta WHERE key='settings'")
        saved = json.loads(row[0]["value"]) if row else {}
        st = {**DEFAULT_SETTINGS, **{k: v for k, v in saved.items() if k in DEFAULT_SETTINGS}}
        st["projects_dir"] = st["projects_dir"] or default_projects_dir()
        return st

    def update_settings(self, changes):
        with self.dblock:
            merged = self.settings()
            for k, v in changes.items():
                if k in DEFAULT_SETTINGS:
                    default = DEFAULT_SETTINGS[k]
                    merged[k] = type(default)(v) if not isinstance(default, bool) else bool(v)
            self.x("INSERT OR REPLACE INTO meta VALUES ('settings', ?)", (json.dumps(merged),))
        return merged

    # ---- folders
    def folders(self):
        return [r["path"] for r in self.q("SELECT path FROM folders ORDER BY path")]

    def _locate(self, path, folders=None):
        """The library folder holding path, and path rebuilt on that folder's spelling."""
        for f in sorted(folders if folders is not None else self.folders(), key=len, reverse=True):
            if is_inside(path, f):
                rel = Path(path).parts[len(Path(f).parts):]
                return f, os.path.join(f, *rel)
        return None, None

    def add_folder(self, path):
        path = str(Path(path).expanduser().resolve())
        if not os.path.isdir(path):
            raise ValueError(f"Not a folder: {path}")
        for f in self.folders():
            if is_inside(path, f):
                raise ValueError(f"Already in the library as part of {f}")
            if is_inside(f, path):
                raise ValueError(f"Contains the library folder {f}; remove that one first")
        self.x("INSERT OR IGNORE INTO folders(path) VALUES (?)", (path,))
        self._watch(path)
        self.jobs.put(("folder", path))
        return path

    def remove_folder(self, path):
        self._unwatch(path)
        with self.tx() as db:
            db.execute("DELETE FROM folders WHERE path=?", (path,))
            rows = db.execute("SELECT path FROM files WHERE folder=?", (path,)).fetchall()
            db.execute("DELETE FROM files WHERE folder=?", (path,))
            for r in rows:
                self._drop_mem(r["path"])

    # ---- live folder watching: deletes, moves and new files show up without a rescan
    def _start_watching(self):
        observer = Observer()
        observer.start()
        self._observer = observer
        for f in self.folders():
            self._watch(f)
        threading.Thread(target=self._flush_pending, daemon=True).start()

    def _watch(self, folder):
        with self._watch_lock:
            if self._observer is None or folder in self._watches or not os.path.isdir(folder):
                return
            try:
                self._watches[folder] = self._observer.schedule(_FolderEvents(self), folder, recursive=True)
            except OSError:
                traceback.print_exc()

    def _unwatch(self, folder):
        with self._watch_lock:
            w = self._watches.pop(folder, None)
            if w is not None:
                self._observer.unschedule(w)

    def _changed(self, path):
        if Path(path).suffix.lower() in AUDIO_EXTS and not os.path.basename(path).startswith("."):
            with self._pending_lock:
                self._pending.add(path)

    def _flush_pending(self):
        # Batch changes for a moment so a file still being written is indexed once, complete.
        while True:
            time.sleep(2)
            with self._pending_lock:
                batch, self._pending = self._pending, set()
            if not batch:
                continue
            try:
                folders = self.folders()
                by_folder = {}
                for p in batch:
                    folder, p = self._locate(p, folders)
                    if folder is None or any(c.startswith(".") for c in Path(p).parts[len(Path(folder).parts):]):
                        continue
                    try:
                        st = os.stat(p)
                    except OSError:
                        continue
                    row = self.q("SELECT mtime, size FROM files WHERE path=?", (p,))
                    if row and (row[0]["mtime"], row[0]["size"]) == (st.st_mtime, st.st_size):
                        continue
                    by_folder.setdefault(folder, []).append(p)
                for folder, paths in by_folder.items():
                    self.jobs.put(("files", folder, paths))
            except Exception:
                traceback.print_exc()

    def forget(self, path, is_dir=False, purge=False):
        """Remove a file (or everything under a folder) from the index. Its tags, note, rating
        and project membership are kept and its content hash is remembered, so they return if
        the file comes back at this path or turns up at another one. purge drops those too."""
        if is_dir:
            prefix = path.rstrip(os.sep) + os.sep
            match, args = "substr(path, 1, ?) = ?", (len(prefix), prefix)
        else:
            match, args = "path = ?", (path,)
        with self.tx() as db:
            gone = db.execute(f"SELECT path, sha1 FROM files WHERE {match}", args).fetchall()
            db.execute(f"DELETE FROM files WHERE {match}", args)
            if purge:
                for table in ("catalog", "regions", "project_items", "gone"):
                    db.execute(f"DELETE FROM {table} WHERE {match}", args)
                self._catalog_changed()
            else:
                db.executemany("INSERT OR REPLACE INTO gone VALUES (?, ?, ?)",
                               [(r["path"], r["sha1"], time.time()) for r in gone if r["sha1"]])
        for r in gone:
            self._drop_mem(r["path"])

    def _reattach(self, db, path, sha1):
        """Give a newly indexed file the tags and project places of a vanished file with the
        same content. Call inside the db lock."""
        db.execute("DELETE FROM gone WHERE path = ?", (path,))
        if not sha1:
            return
        for r in db.execute("SELECT path FROM gone WHERE sha1 = ? ORDER BY at DESC", (sha1,)).fetchall():
            old = r["path"]
            db.execute("DELETE FROM gone WHERE path = ?", (old,))
            if os.path.exists(old):
                continue
            for t in ("catalog", "regions"):
                if not db.execute(f"SELECT 1 FROM {t} WHERE path = ?", (path,)).fetchone():
                    db.execute(f"UPDATE {t} SET path = ? WHERE path = ?", (path, old))
            db.execute("UPDATE OR IGNORE project_items SET path = ? WHERE path = ?", (path, old))
            db.execute("DELETE FROM project_items WHERE path = ?", (old,))
            self._catalog_changed()
            return

    def trash(self, path):
        """Move a file to the macOS Trash (restorable from Finder) and forget it."""
        from Foundation import NSURL, NSFileManager
        ok, _, err = NSFileManager.defaultManager().trashItemAtURL_resultingItemURL_error_(
            NSURL.fileURLWithPath_(path), None, None)
        if not ok:
            raise OSError(str(err.localizedDescription()) if err else "Could not move to Trash")
        self.forget(path, purge=True)

    def rescan(self):
        for f in self.folders():
            self._watch(f)
            self.jobs.put(("folder", f))

    def index_file(self, path):
        folder, path = self._locate(path)
        if folder:
            self.jobs.put(("files", folder, [path]))

    def folder_counts(self):
        """A folder that cannot be found (deleted, renamed, or on a drive that is not plugged
        in) is reported as offline and kept, with its index, until the user removes it."""
        offline = {f for f in self.folders() if not os.path.isdir(f)}
        rows = self.q("SELECT folder, COUNT(*) n, SUM(emb IS NOT NULL) ok, SUM(size) bytes FROM files GROUP BY folder")
        counts = {r["folder"]: (r["n"], r["ok"], r["bytes"] or 0) for r in rows}
        return [{"path": f, "files": counts.get(f, (0, 0, 0))[0], "indexed": counts.get(f, (0, 0, 0))[1],
                 "bytes": counts.get(f, (0, 0, 0))[2], "offline": f in offline} for f in self.folders()]

    # ---- indexing
    def _worker(self):
        self.ready.wait()
        while True:
            job = self.jobs.get()
            try:
                if job[0] == "folder":
                    self._scan(job[1])
                else:
                    self._index(job[1], job[2])
            except Exception:
                traceback.print_exc()
            finally:
                self.status.update(current=None, folder=None)

    def _scan(self, folder):
        if not os.path.isdir(folder):
            return
        self._watch(folder)
        others = set(self.folders()) - {folder}
        known = {r["path"]: (r["mtime"], r["size"])
                 for r in self.q("SELECT path, mtime, size FROM files WHERE folder=?", (folder,))}
        seen, todo, failed = set(), [], []
        for root, dirs, names in os.walk(folder, onerror=lambda e: failed.append(e.filename)):
            dirs[:] = [d for d in dirs if not d.startswith(".") and os.path.join(root, d) not in others]
            for n in names:
                if n.startswith(".") or Path(n).suffix.lower() not in AUDIO_EXTS:
                    continue
                p = os.path.join(root, n)
                try:
                    st = os.stat(p)
                except OSError:
                    failed.append(p)
                    continue
                seen.add(p)
                if known.get(p) != (st.st_mtime, st.st_size):
                    todo.append(p)
        if not os.path.isdir(folder):
            return
        unreadable = [str(f) for f in failed if f]
        for gone in set(known) - seen:
            if not any(gone == f or is_inside(gone, f) for f in unreadable):
                self.forget(gone)
        self._backfill_recorded(folder, seen - set(todo))
        self._index(folder, todo)

    def _backfill_recorded(self, folder, paths):
        rows = self.q("SELECT path FROM files WHERE folder=? AND recorded IS NULL AND emb IS NOT NULL", (folder,))
        for p in (r["path"] for r in rows if r["path"] in paths):
            rec = recorded_date(p)
            self.x("UPDATE files SET recorded=? WHERE path=?", (rec, p))
            with self._memlock:
                i = self._store.pos.get(p)
                if i is not None:
                    self._store.metas[i] = {**self._store.metas[i], "recorded": rec}
                    self._dirty = True

    def _index(self, folder, paths):
        self.status.update(folder=folder, queued=len(paths), done=0)
        for i in range(0, len(paths), BATCH):
            if folder not in self.folders():
                return
            batch = paths[i:i + BATCH]
            self.status["current"] = os.path.basename(batch[0])
            loaded, failed = [], []
            for kind, row, clips in self._pool.map(lambda p: self._prepare(folder, p), batch):
                if kind == "ok":
                    loaded.append((row, clips))
                elif kind == "failed":
                    failed.append(row)
            if loaded:
                # A long recording is embedded as several clips spread across it, averaged.
                flat = self.engine.embed_audio([c for _, clips in loaded for c in clips])
                at = 0
                for row, clips in loaded:
                    emb = flat[at:at + len(clips)].mean(axis=0)
                    at += len(clips)
                    row["emb"] = (emb / max(float(np.linalg.norm(emb)), 1e-12)).astype(np.float32).tobytes()
            # The folder may have been removed, or a file deleted, while this batch ran.
            with self.dblock:
                if folder not in self.folders():
                    return
                for row in failed:
                    if os.path.exists(row["path"]):
                        self._save(row)
                        self._drop_mem(row["path"])
                for row, _ in loaded:
                    if os.path.exists(row["path"]):
                        self._save(row)
                        self._reattach(self.db, row["path"], row["sha1"])
                        self._put_mem({k: row.get(k) for k in META_COLS} | {"emb": row["emb"]})
                self.db.commit()
            self.status["done"] = min(i + BATCH, len(paths))

    @staticmethod
    def _prepare(folder, p):
        """Decode and analyse one file. Runs on the decode threads."""
        try:
            st = os.stat(p)
        except OSError:
            return "skip", None, None
        base = {"path": p, "folder": folder, "mtime": st.st_mtime, "size": st.st_size}
        try:
            duration, sr, ch = eng.file_info(p)
            y = eng.load_audio(p)
            row = {**base, "sha1": file_hash(p, st.st_size), "duration": duration, "sr": sr, "channels": ch,
                   "peaks": eng.file_peaks(p, y, duration), "recorded": recorded_date(p), "error": None}
            row.update(eng.analyze(y, eng.CLAP_SR, duration, os.path.basename(p)))
            return "ok", row, eng.clips(p, y, duration)
        except Exception as e:
            return "failed", {**base, "error": f"{type(e).__name__}: {e}"}, None

    def _save(self, row):
        cols = list(row)
        sql = (f"INSERT OR REPLACE INTO files ({', '.join(cols)}) "
               f"VALUES ({', '.join('?' * len(cols))})")
        self.x(sql, [row[c] for c in cols])

    # ---- search
    def embedding_of(self, path):
        with self._memlock:
            item = self._store.get(path)
        return None if item is None else item[1]

    def _haystack(self, p, cat_rows, folder=None):
        hay = self._hay.get(p)
        if hay is None:
            c = cat_rows.get(p) or {}
            hay = nfc(" ".join([os.path.basename(p), self._sub(p, folder), " ".join(c.get("tags") or []), c.get("memo") or ""]).lower())
            self._hay[p] = hay
        return hay

    def _sub(self, path, folder=None):
        """The subfolders between a file and its library folder, as "Drums/Snares"."""
        folder = folder or self._locate(path)[0]
        if not folder:
            return ""
        return os.path.dirname(nfc(path))[len(nfc(folder).rstrip(os.sep)) + 1:]

    def subfolders(self, path):
        """The folders directly inside path, with how many indexed sounds each holds at any
        depth. A folder on disk with no sounds yet is listed too, with a count of zero."""
        folder = next((f for f in self.folders() if nfc(f) == nfc(path)), None)
        if folder:
            path = folder
        else:
            folder, path = self._locate(path)
        if not folder:
            return []
        base = nfc(path).rstrip(os.sep) + os.sep
        counts, deeper = {}, set()
        for r in self.q("SELECT path FROM files WHERE folder=?", (folder,)):
            p = nfc(r["path"])
            if not p.startswith(base):
                continue
            parts = p[len(base):].split(os.sep)
            if len(parts) > 1:
                counts[parts[0]] = counts.get(parts[0], 0) + 1
                if len(parts) > 2:
                    deeper.add(parts[0])
        try:
            with os.scandir(path) as entries:
                for e in entries:
                    if e.is_dir() and not e.name.startswith("."):
                        counts.setdefault(nfc(e.name), 0)
        except OSError:
            pass
        return [{"path": base + n, "name": n, "files": c, "nested": n in deeper}
                for n, c in sorted(counts.items(), key=lambda x: x[0].lower())]

    def search(self, vec=None, *, category=None, min_dur=None, folder=None, under=None, project=None,
               sort="score", limit=200, exclude=None, tag=None, min_rating=None, words=None):
        paths, metas, M, cats, cat_scores = self.snapshot()
        cat_rows = self.catalog_all()
        n = len(paths)
        if n == 0:
            return []
        scores = M @ vec if vec is not None else np.zeros(n, np.float32)
        relevant = None
        boost = np.zeros(n, np.float32)
        if words and words.strip():
            phrase = nfc(words.strip().lower())
            terms = [w for w in re.split(r"[\s_\-.]+", phrase) if len(w) > 1]
            for i, p in enumerate(paths):
                hay = self._haystack(p, cat_rows, metas[i]["folder"])
                if phrase in hay:
                    boost[i] = 1.0
                elif terms:
                    boost[i] = 0.5 * sum(t in hay for t in terms) / len(terms)
            # CLAP ranks every sound, even for nonsense queries. Keep only sounds that stand
            # clearly above the rest of the library, plus anything matching by name, tag or note.
            raw = scores
            med = float(np.median(raw))
            if vec is not None and float(raw.max()) - med >= 0.15:
                relevant = (raw >= med + 0.12) | (boost > 0)
            else:
                relevant = boost > 0
            if vec is not None and float(raw.max()) >= 0.33:
                # A library of near-identical sounds (all 808s, say) has no spread; keep strong matches anyway.
                relevant |= raw >= max(0.33, float(raw.max()) - 0.08)
            scores = scores + boost
        mask = np.ones(n, bool)
        dur = np.array([m["duration"] or 0 for m in metas])
        if min_dur is not None:
            mask &= dur >= min_dur
        if category and self.engine is not None and category in self.engine.category_names:
            mask &= cats == self.engine.category_names.index(category)
        if folder:
            mask &= np.array([m["folder"] == folder for m in metas])
        if under:
            mask &= np.array([is_inside(p, under) for p in paths])
        if project:
            members = {r["path"] for r in self.q(
                "SELECT path FROM project_items WHERE project_id=?", (project,))}
            mask &= np.array([p in members for p in paths])
        if tag:
            mask &= np.array([tag in ((cat_rows.get(p) or {}).get("tags") or []) for p in paths])
        if min_rating:
            mask &= np.array([((cat_rows.get(p) or {}).get("rating") or 0) >= min_rating for p in paths])
        if exclude:
            mask &= np.array([p != exclude for p in paths])

        if relevant is not None:
            mask &= relevant
        idx = np.flatnonzero(mask)
        keys = {
            "score": lambda i: -scores[i],
            "rating": lambda i: -((cat_rows.get(paths[i]) or {}).get("rating") or 0),
            "recorded": lambda i: -int("".join(ch for ch in (metas[i]["recorded"] or "0") if ch.isdigit()) or 0),
            "name": lambda i: os.path.basename(paths[i]).lower(),
            "duration": lambda i: dur[i],
            "category": lambda i: (self.engine.category_names[cats[i]].lower() if self.engine else "",
                                   os.path.basename(paths[i]).lower()),
            "brightness": lambda i: metas[i]["brightness"] or 0,
            "loudness": lambda i: -(metas[i]["rms_db"] or -200),
            "pitch": lambda i: metas[i]["pitch_hz"] or 1e9,
            "tempo": lambda i: metas[i]["tempo"] or 1e9,
        }
        if sort == "score" and vec is not None:
            top = idx[np.argsort(-scores[idx])[:limit]] if idx.size else idx
        else:
            top = sorted(idx, key=keys.get(sort, keys["name"]))[:limit]
        missing = {i for i in top if not os.path.exists(paths[i])}
        down = {f for f in {metas[i]["folder"] for i in missing} if not os.path.isdir(f)}
        for i in missing:
            # Files whose whole folder is unreachable are hidden and stay indexed.
            if metas[i]["folder"] not in down:
                self.forget(paths[i])
        missing = {paths[i] for i in missing}
        out = []
        for i in top:
            if paths[i] in missing:
                continue
            d = self.describe(i, paths, metas, cats, cat_scores, scores, cat_rows)
            d["text_match"] = float(boost[i])
            out.append(d)
        return out

    def describe(self, i, paths, metas, cats, cat_scores, scores, cat_rows):
        m = metas[i]
        c = cat_rows.get(paths[i]) or {}
        return {
            "tags": c.get("tags") or [], "memo": c.get("memo") or "", "rating": c.get("rating") or 0,
            "origin": c.get("origin"), "recorded": m["recorded"],
            "path": paths[i], "name": os.path.basename(paths[i]), "folder": m["folder"],
            "sub": self._sub(paths[i], m["folder"]),
            "score": float(scores[i]), "duration": m["duration"], "sr": m["sr"],
            "channels": m["channels"], "rms_db": m["rms_db"], "peak_db": m["peak_db"],
            "crest": m["crest"], "brightness": m["brightness"], "pitch_hz": m["pitch_hz"],
            "note": eng.note_name(m["pitch_hz"]), "tempo": m["tempo"],
            "is_loop": bool(m["is_loop"]),
            "category": self.engine.category_names[cats[i]] if self.engine else None,
            "category_score": float(cat_scores[i]),
            "peaks": list(m["peaks"] or b""),
        }

    def category_counts(self):
        """Per category: count, median brightness (Hz) and median length (s)."""
        paths, metas, _, cats, _ = self.snapshot()
        if self.engine is None:
            return []
        out = []
        for i, n in enumerate(self.engine.category_names):
            idx = np.flatnonzero(cats == i) if len(paths) else []
            br = [metas[j]["brightness"] for j in idx if metas[j]["brightness"]]
            du = [metas[j]["duration"] for j in idx if metas[j]["duration"]]
            out.append({"name": n, "count": int(len(idx)),
                        "brightness": float(np.median(br)) if br else None,
                        "duration": float(np.median(du)) if du else None})
        return out

    # ---- projects
    def projects(self):
        return [dict(r) for r in self.q(
            "SELECT p.id, p.name, p.folder, COUNT(f.path) n FROM projects p "
            "LEFT JOIN project_items i ON i.project_id=p.id "
            "LEFT JOIN files f ON f.path=i.path GROUP BY p.id ORDER BY p.name")]

    def project_exists(self, pid):
        return bool(self.q("SELECT 1 FROM projects WHERE id=?", (pid,)))

    def create_project(self, name):
        self.x("INSERT OR IGNORE INTO projects(name) VALUES (?)", (name.strip(),))
        return dict(self.q("SELECT id, name, folder FROM projects WHERE name=?", (name.strip(),))[0])

    def give_project_folder(self, pid, name):
        """Make the project its own folder inside the projects folder from Settings, so every
        sound added to it is copied there. Returns the folder."""
        base = Path(self.settings()["projects_dir"]).expanduser()
        folder = unique_path(base / safe_name(name))
        folder.mkdir(parents=True)
        base = str(base.resolve())
        if not any(nfc(f) == nfc(base) or is_inside(base, f) or is_inside(f, base) for f in self.folders()):
            self.add_folder(base)
        return self.set_project_folder(pid, str(folder))

    def ensure_project_folders(self):
        """Every project owns a folder. Projects made before that was the rule get one now, and
        their sounds are copied into it; the originals stay where they are."""
        for p in self.q("SELECT id, name FROM projects WHERE folder IS NULL OR folder = ''"):
            try:
                self.give_project_folder(p["id"], p["name"])
            except (OSError, ValueError):
                traceback.print_exc()

    def _dismissed_folders(self):
        row = self.q("SELECT value FROM meta WHERE key='projects_dismissed'")
        return set(json.loads(row[0]["value"])) if row else set()

    def adopt_project_folders(self):
        """A folder made by hand inside the projects folder is a project too: it joins the list,
        with whatever sounds it already holds. A folder whose project was deleted in the app is
        left alone, so it does not come back."""
        base = Path(self.settings()["projects_dir"]).expanduser()
        if not base.is_dir():
            return
        base = base.resolve()
        owned = {nfc(p["folder"]) for p in self.projects() if p["folder"]} | {nfc(f) for f in self._dismissed_folders()}
        names = {p["name"] for p in self.projects()}
        try:
            found = sorted(e.path for e in os.scandir(base) if e.is_dir() and not e.name.startswith("."))
        except OSError:
            return
        for folder in found:
            if nfc(folder) in owned:
                continue
            name, n = os.path.basename(folder), 2
            while name in names:
                name, n = f"{os.path.basename(folder)} ({n})", n + 1
            names.add(name)
            try:
                pid = self.create_project(name)["id"]
                self.set_project_folder(pid, folder)
            except (OSError, ValueError):
                traceback.print_exc()
                continue
            for r in self.q("SELECT path FROM files WHERE path LIKE ? ESCAPE '\\'",
                            (folder.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_") + os.sep + "%",)):
                self.x("INSERT OR IGNORE INTO project_items(project_id, path, added) VALUES (?, ?, ?)", (pid, r["path"], time.time()))

    def rename_project(self, pid, name):
        self.x("UPDATE projects SET name=? WHERE id=?", (name.strip(), pid))

    def delete_project(self, pid):
        folder = self.project_folder(pid)
        if folder:
            gone = sorted(self._dismissed_folders() | {folder})
            self.x("INSERT OR REPLACE INTO meta VALUES ('projects_dismissed', ?)", (json.dumps(gone),))
        with self.tx() as db:
            db.execute("DELETE FROM project_items WHERE project_id=?", (pid,))
            db.execute("DELETE FROM projects WHERE id=?", (pid,))

    def project_folder(self, pid):
        r = self.q("SELECT folder FROM projects WHERE id=?", (pid,))
        return r[0]["folder"] if r else None

    def project_folder_ready(self, pid):
        """The project's folder, checked: it must exist, and it must be inside the library,
        because a file outside it is never indexed and so would never show up. A folder that
        has left the library (removed on the Folders page, or restored from a backup) rejoins it."""
        folder = self.project_folder(pid)
        if not folder:
            return None
        if not os.path.isdir(folder):
            raise ValueError(f"The project folder was not found: {folder}")
        if not any(nfc(f) == nfc(folder) or is_inside(folder, f) for f in self.folders()):
            self.add_folder(folder)
        return folder

    def _into_project_folder(self, folder, path):
        """Copy path into the project folder and return the copy. A file already there with
        the same name and the same content is the same sound and is reused."""
        if is_inside(path, folder):
            return path
        target = Path(folder) / os.path.basename(path)
        size = os.path.getsize(path)
        same = target.is_file() and target.stat().st_size == size and file_hash(str(target), size) == file_hash(path, size)
        if not same:
            target = unique_path(target)
            shutil.copy2(path, target)
        target = str(target)
        self.catalog_copy(path, target)
        self.set_origin(target, path)
        self.index_file(target)
        return target

    def set_project_folder(self, pid, path):
        """Give the project a folder on disk. Its sounds are copied into it, and from then on
        every sound added to the project is too. The folder joins the library if it is not
        already inside it, so the copies can be searched and played."""
        path = str(Path(path).expanduser().resolve())
        if not os.path.isdir(path):
            raise ValueError(f"Not a folder: {path}")
        folders = self.folders()
        if not any(nfc(f) == nfc(path) or is_inside(path, f) for f in folders):
            path = self.add_folder(path)
        self.x("UPDATE projects SET folder=? WHERE id=?", (path, pid))
        for old in sorted(self.project_paths(pid)):
            if is_inside(old, path) or not os.path.exists(old):
                continue
            new = self._into_project_folder(path, old)
            with self.tx() as db:
                # The copy takes the original's place in the project, keeping its position.
                db.execute("INSERT OR IGNORE INTO project_items(project_id, path, added, pos) "
                           "SELECT project_id, ?, added, pos FROM project_items WHERE project_id=? AND path=?", (new, pid, old))
                db.execute("DELETE FROM project_items WHERE project_id=? AND path=?", (pid, old))
        return path

    def project_add(self, pid, path):
        folder = self.project_folder_ready(pid)
        if folder:
            path = self._into_project_folder(folder, path)
        self.x("INSERT OR IGNORE INTO project_items(project_id, path, added) VALUES (?, ?, ?)", (pid, path, time.time()))
        return path

    def sound_map(self):
        """Every indexed sound as a point on a flat map where sounds that sound alike sit close
        together. The layout is worked out from the same listening data search uses, and kept
        until the library changes."""
        paths = self.snapshot()[0]
        key = (len(paths), hash(tuple(paths)))
        if self._map and self._map[0] == key:
            return self._map[1]
        if self._map and self._map[1]:
            # The library changed. Hand back the last layout, minus sounds that are gone, and
            # work the new one out in the background: it is there the next time the map is opened.
            if self._maplock.acquire(blocking=False):
                def rebuild():
                    try:
                        self._sound_map()
                    except Exception:
                        traceback.print_exc()
                    finally:
                        self._maplock.release()
                threading.Thread(target=rebuild, daemon=True).start()
            alive = set(paths)
            return [p for p in self._map[1] if p["path"] in alive]
        with self._maplock:
            return self._sound_map()

    def _sound_map(self):
        paths, metas, M, cats, _ = self.snapshot()
        n = len(paths)
        key = (n, hash(tuple(paths)))
        if self._map and self._map[0] == key:
            return self._map[1]
        if n < 3:
            xy = np.array([[0.5, 0.5]] * n, dtype=np.float32).reshape(n, 2)
        else:
            X = M - M.mean(axis=0)
            # The main directions of variation first: that alone gives a rough map.
            U, S, _ = np.linalg.svd(X, full_matrices=False)
            k = min(30, U.shape[1])
            P = U[:, :k] * S[:k]
            xy = P[:, :2]
            # The finer layout takes time that grows faster than the library, so past a few
            # thousand sounds the rough map is used as it is.
            if 12 <= n <= MAP_FINE_LIMIT:
                try:
                    from sklearn.manifold import TSNE
                    xy = TSNE(n_components=2, init=P[:, :2] / (P[:, 0].std() or 1) * 1e-4, perplexity=min(30.0, (n - 1) / 3),
                              random_state=0).fit_transform(P)
                except Exception:
                    traceback.print_exc()
            lo, hi = xy.min(axis=0), xy.max(axis=0)
            xy = (xy - lo) / np.where(hi - lo > 0, hi - lo, 1)
        names = self.engine.category_names if self.engine else []
        out = [{"path": p, "name": os.path.basename(p), "x": round(float(xy[i, 0]), 4), "y": round(float(xy[i, 1]), 4),
                "category": names[cats[i]] if names else None, "duration": metas[i]["duration"], "sr": metas[i]["sr"]}
               for i, p in enumerate(paths)]
        self._map = (key, out)
        return out

    def project_order(self, pid):
        """The project's sounds in the order the user gave them. Sounds never placed by hand
        come after the placed ones, by name."""
        return [r["path"] for r in self.q(
            "SELECT path FROM project_items WHERE project_id=? ORDER BY pos IS NULL, pos, path", (pid,))]

    def set_project_order(self, pid, paths):
        with self.tx() as db:
            for n, path in enumerate(paths):
                db.execute("UPDATE project_items SET pos=? WHERE project_id=? AND path=?", (n, pid, path))

    def export_backup(self, dest):
        """Everything the user wrote about their sounds, as one file in dest."""
        data = {
            "athar_backup": 1, "made": datetime.now().isoformat(timespec="seconds"),
            "catalog": [dict(r) for r in self.q("SELECT path, tags, memo, rating FROM catalog")],
            "projects": [{"name": p["name"], "folder": p["folder"], "paths": self.project_order(p["id"])} for p in self.projects()],
            "regions": [dict(r) for r in self.q("SELECT * FROM regions")],
        }
        target = unique_path(Path(dest) / f"Athar backup {datetime.now():%Y-%m-%d}.json")
        target.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        return str(target)

    def restore_backup(self, file):
        """Bring tags, notes, ratings, regions and projects back from a backup file: one made
        with export_backup, or one of the dated files Athar writes by itself. What is in the
        file wins for the sounds it names; everything else is left as it is. The file is read
        and checked in full before anything is written."""
        try:
            data = json.loads(Path(file).read_text(encoding="utf-8"))
        except (OSError, ValueError) as e:
            raise ValueError(f"That file could not be read as a backup: {e}")
        if not isinstance(data, dict) or not isinstance(data.get("catalog"), list):
            raise ValueError("That file is not an Athar backup")
        try:
            sounds = []
            for r in data["catalog"]:
                tags = r.get("tags")
                tags = json.loads(tags) if isinstance(tags, str) else (tags or [])
                tags = sorted({nfc(str(t).strip().lower()) for t in tags if str(t).strip()})
                memo, rating = str(r.get("memo") or ""), max(0, min(5, int(r.get("rating") or 0)))
                if r.get("path") and (tags or memo or rating):
                    sounds.append((str(r["path"]), json.dumps(tags), memo, rating))
            # The dated backups list project members apart from the projects, by project id.
            members = {}
            for it in data.get("project_items") or []:
                members.setdefault(it["project_id"], []).append((it.get("added") or 0, str(it["path"])))
            projects = []
            for p in data.get("projects") or []:
                name = str(p.get("name") or "").strip()
                if not name:
                    continue
                paths = [str(x) for x in p["paths"]] if "paths" in p else [x for _, x in sorted(members.get(p.get("id"), []))]
                projects.append((name, p.get("folder") if isinstance(p.get("folder"), str) else None, paths))
            regions = [(str(r["path"]), r["data"] if isinstance(r["data"], str) else json.dumps(r["data"]))
                       for r in data.get("regions") or [] if r.get("path") and r.get("data")]
        except (AttributeError, KeyError, TypeError, ValueError) as e:
            raise ValueError(f"That backup is damaged, so nothing was restored: {e}")
        now = time.time()
        with self.tx() as db:
            for path, tags, memo, rating in sounds:
                db.execute("INSERT OR IGNORE INTO catalog(path, done, updated) VALUES (?, 0, ?)", (path, now))
                db.execute("UPDATE catalog SET tags=?, memo=?, rating=?, updated=? WHERE path=?", (tags, memo, rating, now, path))
            for path, blob in regions:
                db.execute("INSERT OR REPLACE INTO regions VALUES (?, ?)", (path, blob))
            ids = []
            for name, folder, paths in projects:
                db.execute("INSERT OR IGNORE INTO projects(name) VALUES (?)", (name,))
                pid = db.execute("SELECT id FROM projects WHERE name=?", (name,)).fetchone()[0]
                for n, path in enumerate(paths):
                    db.execute("INSERT OR IGNORE INTO project_items(project_id, path, added) VALUES (?, ?, ?)", (pid, path, now))
                    db.execute("UPDATE project_items SET pos=? WHERE project_id=? AND path=?", (n, pid, path))
                ids.append((pid, folder))
            self._catalog_changed()
        for pid, folder in ids:
            # Through the usual route, so the folder joins the library and its sounds can be found.
            if folder and os.path.isdir(folder) and not self.project_folder(pid):
                try:
                    self.set_project_folder(pid, folder)
                except (ValueError, OSError):
                    pass
        return {"sounds": len(sounds), "projects": len(projects), "regions": len(regions)}

    def project_remove(self, pid, path):
        self.x("DELETE FROM project_items WHERE project_id=? AND path=?", (pid, path))

    def project_paths(self, pid):
        return {r["path"] for r in self.q(
            "SELECT path FROM project_items WHERE project_id=?", (pid,))}

    def export_project(self, pid, dest, pattern="{project}_{category}_{n:03d}_{name}", fmt="copy"):
        check_pattern(pattern)
        name = self.q("SELECT name FROM projects WHERE id=?", (pid,))[0]["name"]
        root = sp404_dir(dest) if fmt == "sp404" else Path(dest).expanduser()
        dest = root if fmt == "sp404" else root / safe_name(name)
        dest.mkdir(parents=True, exist_ok=True)
        rows = {r["path"]: r for r in self.search(project=pid, sort="name", limit=100000)}
        written = []
        for n, path in enumerate(self.project_order(pid), 1):
            src = Path(path)
            if not src.exists():
                continue
            cat = rows.get(path, {}).get("category") or "Sound"
            base = pattern.format(project=safe_name(name), category=cat, n=n, name=src.stem)
            if fmt == "sp404":
                target = unique_path(dest / (ascii_name(base) + ".wav"))
                to_sp404(src, target, mono=self.settings()["sp404_mono"])
            else:
                target = unique_path(dest / (safe_name(base) + src.suffix.lower()))
                shutil.copy2(src, target)
            written.append(str(target))
        return {"folder": str(dest), "files": written}


def recorded_date(path):
    """Recording time from a WAV's BWF 'bext' chunk (written by field recorders),
    otherwise the file's creation time."""
    try:
        if path.lower().endswith((".wav", ".wave", ".bwf")):
            with open(path, "rb") as f:
                head = f.read(12)
                if head[:4] in (b"RIFF", b"RF64") and head[8:12] == b"WAVE":
                    for _ in range(64):
                        hdr = f.read(8)
                        if len(hdr) < 8:
                            break
                        cid, size = hdr[:4], struct.unpack("<I", hdr[4:])[0]
                        if cid == b"bext":
                            data = f.read(min(size, 340))
                            d = data[320:330].decode("ascii", "ignore").strip("\x00 ")
                            t = data[330:338].decode("ascii", "ignore").strip("\x00 ")
                            if len(d) == 10 and d[0].isdigit():
                                return f"{d.replace(':', '-').replace('/', '-')} {t[:5].replace('-', ':').replace('.', ':')}".strip()
                            break
                        f.seek(size + (size & 1), 1)
        st = os.stat(path)
        ts = getattr(st, "st_birthtime", None) or st.st_mtime
        return datetime.fromtimestamp(ts).strftime("%Y-%m-%d %H:%M")
    except OSError:
        return None


def unique_path(p):
    p = Path(p)
    if not p.exists():
        return p
    for i in range(2, 10000):
        c = p.with_name(f"{p.stem} ({i}){p.suffix}")
        if not c.exists():
            return c
    raise FileExistsError(p)


class _FolderEvents(FileSystemEventHandler):
    def __init__(self, lib):
        self.lib = lib

    def on_created(self, e):
        if not e.is_directory:
            self.lib._changed(e.src_path)

    def on_modified(self, e):
        if not e.is_directory:
            self.lib._changed(e.src_path)

    def on_deleted(self, e):
        if e.is_directory and e.src_path in self.lib.folders():
            # watchdog reports a renamed or deleted watched root as deleted; prune_folders
            # decides whether it is gone, and the next scan watches it again if it returns.
            # Unwatch from another thread: this one holds the observer's lock.
            threading.Thread(target=self.lib._unwatch, args=(e.src_path,), daemon=True).start()
            return
        self.lib.forget(e.src_path, e.is_directory)

    def on_moved(self, e):
        self.lib.move(e.src_path, e.dest_path, e.is_directory)
        if e.is_directory:
            for root, _, names in os.walk(e.dest_path):
                for n in names:
                    self.lib._changed(os.path.join(root, n))
        else:
            self.lib._changed(e.dest_path)
