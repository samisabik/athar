//! Paths and names: comparing and cleaning them, and where the library and the model live.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf, MAIN_SEPARATOR};
use super::Res;
use unicode_normalization::UnicodeNormalization;

const AUDIO_EXTS: [&str; 9] = ["wav", "aif", "aiff", "flac", "mp3", "ogg", "m4a", "aac", "wv"];

pub(super) fn nfc(s: &str) -> String {
    s.nfc().collect()
}

/// True when path lies strictly below folder, comparing NFC forms.
pub fn is_inside(path: &str, folder: &str) -> bool {
    let (p, f) = (nfc(path), nfc(folder));
    let f = format!("{}{}", f.trim_end_matches(MAIN_SEPARATOR), MAIN_SEPARATOR);
    p != f && p.starts_with(&f)
}

/// Modification time in seconds, the same value on every read, so an unchanged file compares equal.
pub(super) fn mtime_of(md: &std::fs::Metadata) -> f64 {
    md.mtime() as f64 + md.mtime_nsec() as f64 * 1e-9
}

pub(super) fn basename(p: &str) -> String {
    Path::new(p).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

pub(super) fn is_audio(path: &Path) -> bool {
    path.extension().is_some_and(|x| AUDIO_EXTS.contains(&x.to_string_lossy().to_lowercase().as_str()))
}

pub(super) fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

pub(super) fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(p),
    }
}

/// The characters `keep` accepts, with each run of other characters as one underscore.
fn squash(s: impl Iterator<Item = char>, keep: impl Fn(char) -> bool) -> String {
    let mut out = String::new();
    let mut in_bad = false;
    for c in s {
        if keep(c) {
            out.push(c);
            in_bad = false;
        } else if !in_bad {
            out.push('_');
            in_bad = true;
        }
    }
    out
}

pub fn safe_name(s: &str) -> String {
    let out = squash(s.chars(), |c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ' '));
    let t = out.trim().trim_start_matches(['.', ' ']).to_string();
    if t.is_empty() { "untitled".into() } else { t }
}

pub fn ascii_name(s: &str) -> String {
    let out = squash(s.nfkd().filter(char::is_ascii), |c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
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

/// Where the library lives. ATHAR_HOME points at another folder.
pub(super) fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("ATHAR_HOME") {
        return PathBuf::from(d);
    }
    settle_home(&home().join("Library/Application Support"))
}

/// The library folder under `base`, which is `Athar`. Version 2.0.0 kept it in `Athar Next`,
/// beside the Python app's `Athar`; on the first launch after that it moves back, and the Python
/// app's folder is kept as `Athar 1.x backup`. A library from the Python app used in place has
/// its index copied first, as `index-1.x.db`. The marker file records that this is done.
pub(super) fn settle_home(base: &Path) -> PathBuf {
    const MARK: &str = ".native";
    let home = base.join("Athar");
    let next = base.join("Athar Next");
    if home.join(MARK).exists() {
        return home;
    }
    if next.join("index.db").exists() {
        if home.exists() {
            let mut backup = base.join("Athar 1.x backup");
            let mut n = 2;
            while backup.exists() {
                backup = base.join(format!("Athar 1.x backup {n}"));
                n += 1;
            }
            if std::fs::rename(&home, &backup).is_err() {
                return next;
            }
        }
        if std::fs::rename(&next, &home).is_err() {
            return next;
        }
    } else if home.join("index.db").exists() {
        for f in ["index.db", "index.db-wal"] {
            if home.join(f).exists() && std::fs::copy(home.join(f), home.join(f.replace("index", "index-1.x"))).is_err() {
                return home;
            }
        }
    }
    let _ = std::fs::create_dir_all(&home);
    let _ = std::fs::write(home.join(MARK), "");
    home
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
