//! The sounds held in memory for playing and editing, and the keys the page names them by.
//!
//! `path:<file>` is a library file decoded whole, `cap:<file>` the start of one for the preview
//! player, `held:<n>` a recorded take kept until it is released, and `buf:<n>` a found sound
//! decoded for a preview. Decoded files are kept up to a size in memory, the least recently
//! used going first, and are decoded again once the file on disk has changed.

use athar_core::decode::{decode, resample};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

type Res<T> = Result<T, String>;
pub type Channels = Arc<Vec<Vec<f32>>>;

/// The preview player holds at most this much of a file.
pub const PREVIEW_MAX_SECONDS: usize = 600;
/// Decoded files kept for the next play, in bytes of samples. The latest one always stays.
const FILES_BYTES: usize = 512 << 20;
/// Found sounds kept for replays.
const UPLOADS_KEPT: usize = 6;
const NOT_HELD: &str = "That sound is no longer held";

/// A sound in memory, one vector per channel. Its samples are never changed, so it is shared
/// freely; a copy at the output device's rate is made when it first plays at another rate.
pub struct Sound {
    pub channels: Channels,
    pub sr: u32,
    resampled: Mutex<Option<(u32, Channels)>>,
    /// Held while a copy is being resampled, so a second caller waits for it instead of redoing it.
    resampling: Mutex<()>,
}

impl Sound {
    pub fn new(channels: Vec<Vec<f32>>, sr: u32) -> Arc<Self> {
        Arc::new(Self { channels: Arc::new(channels), sr, resampled: Mutex::new(None), resampling: Mutex::new(()) })
    }

    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / self.sr as f64
    }

    /// The memory it takes: its samples, and the copy at the device's rate when it has one.
    pub fn bytes(&self) -> usize {
        let size = |c: &Channels| c.iter().map(|ch| 4 * ch.len()).sum::<usize>();
        size(&self.channels) + self.resampled.lock().unwrap().as_ref().map_or(0, |r| size(&r.1))
    }

    /// The samples at `rate`: its own when that is its rate, otherwise a resampled copy that is
    /// kept for the next call.
    pub fn at(&self, rate: u32) -> Res<Channels> {
        if self.sr == rate {
            return Ok(self.channels.clone());
        }
        let kept = || self.resampled.lock().unwrap().as_ref().filter(|(r, _)| *r == rate).map(|(_, d)| d.clone());
        // The work is done under its own lock, not the one bytes() takes while the cache of
        // sounds is locked: a long file must not hold up every other sound.
        let _working = self.resampling.lock().unwrap();
        if let Some(d) = kept() {
            return Ok(d);
        }
        let d: Vec<Vec<f32>> = self.channels.iter().map(|ch| resample(ch, self.sr, rate)).collect::<Result<_, _>>().map_err(|e| e.to_string())?;
        let d = Arc::new(d);
        *self.resampled.lock().unwrap() = Some((rate, d.clone()));
        Ok(d)
    }
}

/// What tells one version of a file from the next: when it was modified and its size.
type Stamp = (Option<SystemTime>, u64);

fn stamp(path: &str) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok(), m.len()))
}

struct File {
    path: String,
    /// Only the first PREVIEW_MAX_SECONDS of the file.
    capped: bool,
    stamp: Option<Stamp>,
    sound: Arc<Sound>,
    /// The whole file's length in seconds.
    full_s: f64,
}

#[derive(Default)]
struct Held {
    /// Least recently used first.
    files: Vec<File>,
    uploads: VecDeque<(u64, Arc<Sound>)>,
    takes: HashMap<u64, Arc<Sound>>,
}

impl Held {
    /// Takes a decoded file out to use it; `keep` puts it back as the most recent.
    fn take(&mut self, path: &str, capped: bool) -> Option<File> {
        let i = self.files.iter().position(|f| f.path == path && f.capped == capped)?;
        Some(self.files.remove(i))
    }

    fn keep(&mut self, f: File) {
        self.files.retain(|x| !(x.path == f.path && x.capped == f.capped));
        self.files.push(f);
        let mut total: usize = self.files.iter().map(|f| f.sound.bytes()).sum();
        while total > FILES_BYTES && self.files.len() > 1 {
            total -= self.files.remove(0).sound.bytes();
        }
    }
}

#[derive(Default)]
pub struct Sounds {
    held: Mutex<Held>,
    next: AtomicU64,
}

impl Sounds {
    /// A file decoded whole, from memory when it has not changed on disk since.
    pub fn file(&self, path: &str) -> Res<Arc<Sound>> {
        let now = stamp(path);
        let mut held = self.held.lock().unwrap();
        if let Some(f) = held.take(path, false).filter(|f| f.stamp == now) {
            let s = f.sound.clone();
            held.keep(f);
            return Ok(s);
        }
        drop(held);
        let d = decode(Path::new(path)).map_err(|e| e.to_string())?;
        let full_s = d.duration();
        let sound = Sound::new(d.channels, d.sr);
        self.held.lock().unwrap().keep(File { path: path.to_string(), capped: false, stamp: now, sound: sound.clone(), full_s });
        Ok(sound)
    }

    /// The first PREVIEW_MAX_SECONDS of a file for the preview player: its key, the sound and
    /// the whole file's length in seconds.
    pub fn preview(&self, path: &str) -> Res<(String, Arc<Sound>, f64)> {
        let key = format!("cap:{path}");
        let now = stamp(path);
        let mut held = self.held.lock().unwrap();
        for capped in [true, false] {
            if let Some(f) = held.take(path, capped).filter(|f| f.stamp == now) {
                let fits = capped || f.sound.frames() <= PREVIEW_MAX_SECONDS * f.sound.sr as usize;
                let found = (f.sound.clone(), f.full_s);
                held.keep(f);
                if fits {
                    return Ok((key, found.0, found.1));
                }
            }
        }
        drop(held);
        let mut d = decode(Path::new(path)).map_err(|e| e.to_string())?;
        let full_s = d.duration();
        let cap = PREVIEW_MAX_SECONDS * d.sr as usize;
        let capped = d.frames() > cap;
        d.channels.iter_mut().for_each(|c| c.truncate(cap));
        let sound = Sound::new(d.channels, d.sr);
        self.held.lock().unwrap().keep(File { path: path.to_string(), capped, stamp: now, sound: sound.clone(), full_s });
        Ok((key, sound, full_s))
    }

    /// Keeps a found sound decoded for a preview; the last few stay for replays.
    pub fn upload(&self, sound: Arc<Sound>) -> String {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let mut held = self.held.lock().unwrap();
        held.uploads.push_back((n, sound));
        if held.uploads.len() > UPLOADS_KEPT {
            held.uploads.pop_front();
        }
        format!("buf:{n}")
    }

    /// Keeps a recorded take until `release`.
    pub fn hold(&self, sound: Arc<Sound>) -> String {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        self.held.lock().unwrap().takes.insert(n, sound);
        format!("held:{n}")
    }

    pub fn release(&self, key: &str) {
        if let Some(n) = key.strip_prefix("held:").and_then(|n| n.parse().ok()) {
            self.held.lock().unwrap().takes.remove(&n);
        }
    }

    /// The library file a key names, when it names one that would be read from disk.
    pub fn file_of(key: &str) -> Option<&str> {
        key.strip_prefix("path:")
    }

    /// The sound a key names.
    pub fn get(&self, key: &str) -> Res<Arc<Sound>> {
        let (kind, rest) = key.split_once(':').ok_or(NOT_HELD)?;
        let held = self.held.lock().unwrap();
        let found = match kind {
            "path" => {
                drop(held);
                return self.file(rest);
            }
            // What the preview player was given stays playable while it is held, whatever has
            // happened to the file since.
            "cap" => held.files.iter().rev().find(|f| f.path == rest && (f.capped || f.sound.frames() <= PREVIEW_MAX_SECONDS * f.sound.sr as usize)).map(|f| f.sound.clone()),
            "held" => rest.parse().ok().and_then(|n: u64| held.takes.get(&n).cloned()),
            "buf" => rest.parse().ok().and_then(|n: u64| held.uploads.iter().find(|u| u.0 == n).map(|u| u.1.clone())),
            _ => None,
        };
        found.ok_or_else(|| NOT_HELD.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{write_tone, Tmp};

    #[test]
    fn a_sound_at_its_own_rate_is_not_copied() {
        let s = Sound::new(vec![vec![0.0; 100]], 48_000);
        assert!(Arc::ptr_eq(&s.at(48_000).unwrap(), &s.channels));
        assert_eq!(s.bytes(), 400);
        let other = s.at(24_000).unwrap();
        assert_eq!(other[0].len(), 50);
        assert!(Arc::ptr_eq(&s.at(24_000).unwrap(), &other), "the resampled copy is kept");
        assert_eq!(s.bytes(), 4 * (100 + 50), "and counted");
    }

    #[test]
    fn a_file_is_decoded_once_until_it_changes_on_disk() {
        let t = Tmp::new();
        let path = write_tone(&t.0, "a.wav", 8000, 0.5);
        let sounds = Sounds::default();
        let first = sounds.file(&path).unwrap();
        assert!(Arc::ptr_eq(&first, &sounds.file(&path).unwrap()));
        assert!(Arc::ptr_eq(&first, &sounds.get(&format!("path:{path}")).unwrap()));
        write_tone(&t.0, "a.wav", 8000, 1.0);
        let second = sounds.file(&path).unwrap();
        assert_eq!((first.frames(), second.frames()), (4000, 8000));
    }

    #[test]
    fn a_preview_reports_the_whole_length_every_time() {
        let t = Tmp::new();
        let path = write_tone(&t.0, "long.wav", 10, PREVIEW_MAX_SECONDS as f64 + 5.0);
        let sounds = Sounds::default();
        for _ in 0..2 {
            let (key, s, full) = sounds.preview(&path).unwrap();
            assert_eq!(s.frames(), PREVIEW_MAX_SECONDS * 10);
            assert_eq!(full, PREVIEW_MAX_SECONDS as f64 + 5.0);
            assert!(Arc::ptr_eq(&s, &sounds.get(&key).unwrap()));
        }
        // A short file is one decode for the preview player and the editor.
        let short = write_tone(&t.0, "short.wav", 8000, 0.25);
        let (_, s, full) = sounds.preview(&short).unwrap();
        assert_eq!(full, 0.25);
        assert!(Arc::ptr_eq(&s, &sounds.file(&short).unwrap()));
    }

    #[test]
    fn takes_stay_until_released_and_only_the_last_found_sounds_are_kept() {
        let sounds = Sounds::default();
        let take = sounds.hold(Sound::new(vec![vec![0.0; 4]], 48_000));
        let found: Vec<String> = (0..UPLOADS_KEPT + 1).map(|_| sounds.upload(Sound::new(vec![vec![0.0; 4]], 48_000))).collect();
        assert!(sounds.get(&found[0]).is_err());
        assert!(sounds.get(&found[1]).is_ok());
        assert!(sounds.get(&take).is_ok());
        sounds.release(&take);
        assert!(sounds.get(&take).is_err());
        assert!(sounds.get("nonsense").is_err());
    }
}
