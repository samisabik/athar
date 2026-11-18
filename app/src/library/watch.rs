//! Live watching: new, changed, moved and deleted files show up without a rescan.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, MAIN_SEPARATOR};
use std::sync::Arc;
use std::sync::mpsc::channel;
use super::paths::{is_audio, mtime_of, nfc};
use super::{Job, Library};

impl Library {
    pub(super) fn start_watching(self: &Arc<Self>) {
        let changed = self.changed.clone();
        let unwatched = self.unwatched.clone();
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
                if !on {
                    let _ = w.unwatch(Path::new(&folder));
                } else if w.watch(Path::new(&folder), notify::RecursiveMode::Recursive).is_err() && !Path::new(&folder).is_dir() {
                    unwatched.lock().unwrap().insert(folder);
                }
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
            me.watch_returned_folders();
            me.flush_changes();
        });
    }

    pub(super) fn watch(&self, folder: &str) {
        if let Some(w) = self.watcher.lock().unwrap().as_ref() {
            let _ = w.send((true, folder.to_string()));
        }
    }

    /// A folder that was missing when its watch was asked for is watched once it is there,
    /// and walked for what changed while it was away.
    fn watch_returned_folders(&self) {
        let back: Vec<String> = {
            let mut away = self.unwatched.lock().unwrap();
            let back: Vec<String> = away.iter().filter(|f| Path::new(f).is_dir()).cloned().collect();
            away.retain(|f| !back.contains(f));
            back
        };
        if back.is_empty() {
            return;
        }
        let folders = self.folders();
        for f in back.iter().filter(|f| folders.contains(f)) {
            self.watch(f);
            self.enqueue_scan(f, f);
        }
    }

    pub(super) fn flush_changes(&self) {
        let batch: Vec<String> = std::mem::take(&mut *self.changed.lock().unwrap()).into_iter().collect();
        if batch.is_empty() {
            return;
        }
        let folders = self.folders();
        let mut by_folder: HashMap<String, Vec<String>> = HashMap::new();
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
                self.enqueue_scan(&folder, &p);
                continue;
            }
            if !path.exists() {
                // A file that is gone, or a folder that is gone with every file in it.
                let gone: Vec<String> = {
                    let s = self.store.read().unwrap();
                    if s.pos.contains_key(&p) {
                        vec![p.clone()]
                    } else {
                        let below = format!("{}{MAIN_SEPARATOR}", nfc(&p));
                        let inside = |x: &str| if x.is_ascii() { x.starts_with(&below) } else { nfc(x).starts_with(&below) };
                        s.items.iter().map(|it| &it.meta.path).filter(|x| inside(x)).cloned().collect()
                    }
                };
                for x in gone {
                    self.forget(&x, false);
                }
                continue;
            }
            if !is_audio(path) {
                continue;
            }
            let Ok(md) = std::fs::metadata(path) else { continue };
            let known: Option<(f64, i64)> = self.one("SELECT mtime, size FROM files WHERE path=?", [&p], |r| Ok((r.get(0)?, r.get(1)?)));
            if known != Some((mtime_of(&md), md.size() as i64)) {
                by_folder.entry(folder).or_default().push(p);
            }
        }
        for (f, paths) in by_folder {
            self.enqueue(Job::Files(f, paths));
        }
    }
}
