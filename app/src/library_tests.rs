//! The library's behaviour, ported from the Python app's test suite. Each test runs on its own
//! temporary data folder and sound folder. Most index with a stand-in embedding, so the CLAP
//! model is loaded only by the tests that index and watch for real.

use super::*;
use std::collections::BTreeMap;

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("athar-library-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Tmp(std::fs::canonicalize(&p).unwrap())
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An embedding that depends only on the audio, unit length, without the model.
fn stand_in(clips: &[Vec<f32>]) -> Option<Vec<f32>> {
    let bytes: Vec<u8> = clips.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    let d = sha1_smol::Sha1::from(&bytes).digest().bytes();
    let mut seed = u64::from_le_bytes(d[..8].try_into().unwrap()) | 1;
    let v: Vec<f32> = (0..EMB_DIM)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 40) as f32 / (1u64 << 23) as f32 - 1.0
        })
        .collect();
    let norm = dot(&v, &v).sqrt();
    Some(v.into_iter().map(|x| x / norm).collect())
}

fn noise(seed: u64, frames: usize) -> Vec<f32> {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..frames)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 40) as f32 / (1u64 << 23) as f32 - 1.0
        })
        .collect()
}

fn write_wav(path: &Path, samples: &[f32], sr: u32, bits: u16) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let float = bits == 32;
    let spec = hound::WavSpec { channels: 1, sample_rate: sr, bits_per_sample: bits, sample_format: if float { hound::SampleFormat::Float } else { hound::SampleFormat::Int } };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let scale = ((1i64 << (bits - 1)) - 1) as f32;
    for &v in samples {
        if float {
            w.write_sample(v).unwrap();
        } else {
            w.write_sample((v.clamp(-1.0, 1.0) * scale).round() as i32).unwrap();
        }
    }
    w.finalize().unwrap();
}

fn wav_bytes(samples: &[f32], sr: u32, bits: u16) -> Vec<u8> {
    let t = Tmp::new();
    let p = t.0.join("x.wav");
    write_wav(&p, samples, sr, bits);
    std::fs::read(p).unwrap()
}

fn names(rows: &[Value]) -> Vec<String> {
    let mut n: Vec<String> = rows.iter().map(|r| basename(r["path"].as_str().unwrap())).collect();
    n.sort();
    n
}

fn set(paths: &[&str]) -> HashSet<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

/// A library on a temporary data folder with one sound folder, and no model, watcher or worker:
/// jobs wait in `rx` until `drain` runs them with the stand-in embedding.
struct Lab {
    lib: Arc<Library>,
    rx: Receiver<Job>,
    root: PathBuf,
    folder: String,
    _tmp: Tmp,
}

fn lab() -> Lab {
    let tmp = Tmp::new();
    let root = tmp.0.clone();
    let folder = root.join("sounds");
    std::fs::create_dir(&folder).unwrap();
    let (lib, rx) = Library::create(root.join("home"), root.join("no model")).unwrap();
    lib.update_settings(&json!({"projects_dir": root.join("Projects")})).unwrap();
    let folder = folder.to_string_lossy().into_owned();
    lib.db().execute("INSERT INTO folders VALUES (?)", [&folder]).unwrap();
    Lab { lib, rx, root, folder, _tmp: tmp }
}

impl Lab {
    fn path(&self, rel: &str) -> String {
        Path::new(&self.folder).join(rel).to_string_lossy().into_owned()
    }

    /// A tenth of a second of 16-bit noise at 48 kHz.
    fn write(&self, rel: &str, seed: u64) -> String {
        let p = self.path(rel);
        write_wav(Path::new(&p), &noise(seed, 4800).iter().map(|v| v * 0.1).collect::<Vec<_>>(), 48_000, 16);
        p
    }

    fn add(&self, rel: &str, seed: u64) -> String {
        let p = self.write(rel, seed);
        self.index(&[&p]);
        p
    }

    fn index(&self, paths: &[&str]) {
        self.lib.index(&self.folder, paths.iter().map(|p| p.to_string()).collect(), &stand_in);
    }

    fn drain(&self) {
        while let Ok(job) = self.rx.try_recv() {
            match job {
                Job::Folder(f) => self.lib.scan(&f, &stand_in),
                Job::Files(f, paths) => self.lib.index(&f, paths, &stand_in),
            }
            self.lib.pending.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// What the folder watcher does when it reports these paths as changed.
    fn events(&self, paths: &[&str]) {
        self.lib.changed.lock().unwrap().extend(paths.iter().map(|p| p.to_string()));
        self.lib.flush_changes();
        self.drain();
    }

    fn tag(&self, path: &str, tags: &[&str], rating: i64) {
        self.lib.catalog_set(path, &json!({"tags": tags, "rating": rating})).unwrap();
    }

    fn meta(&self, path: &str) -> (Vec<String>, i64) {
        let c = self.lib.catalog_get(path);
        (serde_json::from_value(c["tags"].clone()).unwrap(), c["rating"].as_i64().unwrap())
    }

    /// A project whose folder is the sound folder, so adding a sound keeps it where it is.
    fn kit(&self, name: &str) -> i64 {
        let pid = self.lib.create_project(name).unwrap()["id"].as_i64().unwrap();
        self.lib.set_project_folder(pid, &self.folder).unwrap();
        self.drain();
        pid
    }

    fn count(&self) -> usize {
        self.lib.store.read().unwrap().items.len()
    }

    fn search(&self, q: SearchQuery, words: Option<&str>) -> Vec<Value> {
        self.lib.search(&q, None, None, words).unwrap().as_array().unwrap().clone()
    }

    fn by_name(&self) -> Vec<Value> {
        self.search(SearchQuery { sort: Some("name".into()), ..Default::default() }, None)
    }
}

// ---------- the catalog follows files (test_metadata.py)

#[test]
fn an_atomic_replace_keeps_tags_and_project() {
    let l = lab();
    let pid = l.kit("kit");
    let p = l.add("kick.wav", 0);
    l.tag(&p, &["keeper"], 5);
    l.lib.project_add(pid, &p).unwrap();
    let tmp = l.write("kick.wav.tmp", 7);
    std::fs::rename(&tmp, &p).unwrap();
    l.events(&[&tmp, &p]);
    assert_eq!(l.meta(&p), (vec!["keeper".to_string()], 5));
    assert_eq!(l.lib.project_paths(pid), set(&[&p]));
    assert_eq!(l.count(), 1);
    let sha: Option<String> = l.lib.db().query_row("SELECT sha1 FROM files WHERE path=?", [&p], |r| r.get(0)).unwrap();
    assert_eq!(sha, file_hash(&p, std::fs::metadata(&p).unwrap().len()).ok(), "the new content is indexed");
}

#[test]
fn a_file_moved_over_another_brings_its_own_tags() {
    let l = lab();
    let a = l.add("a.wav", 1);
    let b = l.add("b.wav", 2);
    l.tag(&a, &["from-a"], 4);
    l.tag(&b, &["from-b"], 1);
    std::fs::rename(&a, &b).unwrap();
    l.events(&[&a, &b]);
    assert_eq!(l.meta(&b), (vec!["from-a".to_string()], 4));
    assert_eq!(l.count(), 1);
}

#[test]
fn a_folder_move_rekeys_everything() {
    let l = lab();
    let pid = l.kit("kit");
    let a = l.add("old/a.wav", 0);
    l.tag(&a, &["x"], 3);
    l.lib.project_add(pid, &a).unwrap();
    let (old, new) = (l.path("old"), l.path("new"));
    std::fs::rename(&old, &new).unwrap();
    l.events(&[&old, &new]);
    let moved = l.path("new/a.wav");
    assert_eq!(l.meta(&moved), (vec!["x".to_string()], 3));
    assert_eq!(l.lib.project_paths(pid), set(&[&moved]));
    assert!(l.lib.embedding_of(&moved).is_some() && l.lib.embedding_of(&a).is_none());
}

#[test]
fn a_file_moved_while_athar_was_closed_gets_its_tags_back() {
    let l = lab();
    let pid = l.kit("kit");
    let old = l.add("kick.wav", 0);
    l.tag(&old, &["keeper"], 5);
    l.lib.project_add(pid, &old).unwrap();
    let new = l.path("drums/kick 01.wav");
    std::fs::create_dir(l.path("drums")).unwrap();
    std::fs::rename(&old, &new).unwrap();
    l.lib.scan(&l.folder, &stand_in);
    assert_eq!(l.meta(&new), (vec!["keeper".to_string()], 5));
    assert_eq!(l.lib.project_paths(pid), set(&[&new]));
    let gone: i64 = l.lib.db().query_row("SELECT COUNT(*) FROM gone", [], |r| r.get(0)).unwrap();
    assert_eq!(gone, 0);
}

#[test]
fn a_deleted_file_keeps_its_tags_for_its_return() {
    let l = lab();
    let p = l.add("rain.wav", 0);
    l.tag(&p, &["kyoto"], 4);
    let data = std::fs::read(&p).unwrap();
    std::fs::remove_file(&p).unwrap();
    l.events(&[&p]);
    assert_eq!(l.count(), 0);
    assert_eq!(l.lib.all_tags(), json!([]));
    std::fs::write(&p, data).unwrap();
    l.index(&[&p]);
    assert_eq!(l.meta(&p), (vec!["kyoto".to_string()], 4));
    assert_eq!(l.lib.all_tags(), json!([{"tag": "kyoto", "count": 1}]));
}

#[test]
fn search_hides_a_missing_file_without_dropping_its_tags() {
    let l = lab();
    let p = l.add("rain.wav", 0);
    l.tag(&p, &["kyoto"], 4);
    std::fs::remove_file(&p).unwrap();
    assert!(l.by_name().is_empty());
    assert_eq!(l.meta(&p), (vec!["kyoto".to_string()], 4));
}

#[test]
fn a_missing_folder_is_offline_and_kept() {
    let l = lab();
    let p = l.add("rain.wav", 0);
    l.tag(&p, &["kyoto"], 0);
    let size = std::fs::metadata(&p).unwrap().len();
    let away = format!("{}-away", l.folder);
    std::fs::rename(&l.folder, &away).unwrap();
    assert_eq!(l.lib.folder_counts(), json!([{"path": l.folder, "files": 1, "indexed": 1, "bytes": size, "offline": true}]));
    assert!(l.by_name().is_empty());
    assert_eq!(l.count(), 1);
    std::fs::rename(&away, &l.folder).unwrap();
    assert_eq!(names(&l.by_name()), vec!["rain.wav"]);
}

#[test]
fn trash_purges_tags_and_project_places() {
    let l = lab();
    let pid = l.kit("kit");
    let p = l.add("bad.wav", 0);
    l.tag(&p, &["x"], 0);
    l.lib.project_add(pid, &p).unwrap();
    l.lib.forget(&p, true);
    assert_eq!(l.meta(&p), (vec![], 0));
    assert!(l.lib.project_paths(pid).is_empty());
}

#[test]
fn search_follows_catalog_writes() {
    let l = lab();
    let p = l.add("rain.wav", 0);
    l.tag(&p, &["one"], 0);
    let q = || SearchQuery { sort: Some("name".into()), ..Default::default() };
    assert_eq!(l.search(q(), Some("one"))[0]["tags"], json!(["one"]));
    l.lib.catalog_set(&p, &json!({"tags": ["two"]})).unwrap();
    assert!(l.search(q(), Some("one")).is_empty());
    assert_eq!(l.search(q(), Some("two"))[0]["tags"], json!(["two"]));
}

#[test]
fn a_backup_is_written_once_per_day() {
    let l = lab();
    let p = l.add("rain.wav", 0);
    l.tag(&p, &["kyoto"], 0);
    l.lib.daily_backup();
    l.lib.daily_backup();
    let files: Vec<PathBuf> = std::fs::read_dir(l.root.join("home/backups")).unwrap().flatten().map(|e| e.path()).collect();
    assert_eq!(files.len(), 1);
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("catalog-") && name.ends_with(".json"));
    assert!(std::fs::read_to_string(&files[0]).unwrap().contains("kyoto"));
}

#[test]
fn an_sp404_export_is_16_bit_48k_and_dithered() {
    let t = Tmp::new();
    let out = t.0.join("out.wav");
    let quiet: Vec<f32> = (0..44_100).map(|i| 0.4 / 32768.0 * (std::f32::consts::TAU * 440.0 * i as f32 / 44_100.0).sin()).collect();
    crate::audio::write_sp404(&wav_bytes(&quiet, 44_100, 32), &out, false).unwrap();
    let mut r = hound::WavReader::open(&out).unwrap();
    let spec = r.spec();
    assert_eq!((spec.sample_rate, spec.bits_per_sample, spec.sample_format), (48_000, 16, hound::SampleFormat::Int));
    assert!(r.samples::<i16>().any(|s| s.unwrap() != 0), "a tone below one step is kept as dithered signal");

    let exact: Vec<i16> = (0..48_000).map(|i| (i % 200 - 100) as i16).collect();
    let src = wav_bytes(&exact.iter().map(|&v| v as f32 / 32767.0).collect::<Vec<_>>(), 48_000, 16);
    let want: Vec<i16> = hound::WavReader::new(std::io::Cursor::new(&src)).unwrap().samples::<i16>().map(Result::unwrap).collect();
    crate::audio::write_sp404(&src, &out, false).unwrap();
    let got: Vec<i16> = hound::WavReader::open(&out).unwrap().samples::<i16>().map(Result::unwrap).collect();
    assert_eq!(got, want, "already 16-bit 48 kHz: untouched");
}

#[test]
fn regions_are_kept_and_follow_a_move() {
    let l = lab();
    let a = l.add("take.wav", 0);
    let saved = l.lib.regions_set(&a, &json!([{"start": 0.5, "end": 1.25, "name": "hit"}, {"start": 3, "end": 2}])).unwrap();
    assert_eq!(saved, json!([{"start": 0.5, "end": 1.25, "name": "hit"}]));
    let b = l.path("take 2.wav");
    std::fs::rename(&a, &b).unwrap();
    l.events(&[&a, &b]);
    assert_eq!(l.lib.regions_get(&b), saved);
    assert_eq!(l.lib.regions_get(&a), json!([]));
    l.lib.regions_set(&b, &json!([])).unwrap();
    assert_eq!(l.lib.regions_get(&b), json!([]));
}

#[test]
fn a_saved_edit_inherits_metadata_once() {
    let l = lab();
    let src = l.add("rain.wav", 0);
    l.tag(&src, &["kyoto"], 4);
    let dst = l.add("rain_edit.wav", 2);
    l.lib.after_save(&dst, &src, 0).unwrap();
    l.drain();
    assert_eq!(l.meta(&dst), (vec!["kyoto".to_string()], 4));
    let rows = l.search(SearchQuery::default(), Some("rain_edit"));
    assert_eq!(rows.iter().find(|r| r["path"] == dst.as_str()).unwrap()["origin"], json!(src));
    l.lib.catalog_set(&dst, &json!({"tags": ["mine"]})).unwrap();
    l.lib.after_save(&dst, &src, 0).unwrap();
    assert_eq!(l.meta(&dst).0, vec!["mine"]);
}

#[test]
fn saving_writes_flac_mp3_and_wav() {
    let l = lab();
    let dest = l.root.join("saved");
    std::fs::create_dir(&dest).unwrap();
    let tone: Vec<f32> = (0..48_000).map(|i| 0.3 * (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin()).collect();
    let wav = wav_bytes(&tone, 48_000, 24);
    for (fmt, magic) in [("flac", &b"fLaC"[..]), ("mp3", &b"ID3"[..]), ("wav", &b"RIFF"[..])] {
        let path = crate::save_wav(&l.lib, &wav, &json!({"dest_dir": dest, "name": "tone.wav", "format": fmt})).unwrap();
        assert_eq!(path, dest.join(format!("tone.{fmt}")).to_string_lossy());
        let bytes = std::fs::read(&path).unwrap();
        let mpeg_frame = fmt == "mp3" && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0;
        assert!(bytes.starts_with(magic) || mpeg_frame, "{fmt} file");
        let d = decode(Path::new(&path)).unwrap();
        assert_eq!(d.sr, 48_000, "{fmt}");
        assert!((d.duration() - 1.0).abs() < 0.06, "{fmt} lasts {}", d.duration());
    }
    // STREAMINFO: 20 bits of rate, 3 of channels, 5 of bits per sample.
    let flac = std::fs::read(dest.join("tone.flac")).unwrap();
    assert_eq!((((flac[20] & 1) << 4) | (flac[21] >> 4)) + 1, 24);
    assert!(crate::save_wav(&l.lib, &wav, &json!({"dest_dir": dest, "name": "x", "format": "exe"})).is_err());
}

// ---------- folders, search, projects, backups (test_features.py)

#[test]
fn subfolders_are_listed_with_counts() {
    let l = lab();
    l.add("top.wav", 0);
    l.add("Hits/Kicks/k1.wav", 1);
    l.add("Hits/Snares/s1.wav", 2);
    l.add("Hits/Snares/s2.wav", 3);
    l.add("Loops/l1.wav", 4);
    let row = |s: &Value| (s["name"].as_str().unwrap().to_string(), s["files"].as_i64().unwrap(), s["nested"].as_bool().unwrap());
    let top: Vec<_> = l.lib.subfolders(&l.folder).as_array().unwrap().iter().map(row).collect();
    assert_eq!(top, vec![("Hits".into(), 3, true), ("Loops".into(), 1, false)]);
    let hits: Vec<_> = l.lib.subfolders(&l.path("Hits")).as_array().unwrap().iter().map(|s| (s["name"].as_str().unwrap().to_string(), s["files"].as_i64().unwrap())).collect();
    assert_eq!(hits, vec![("Kicks".into(), 1), ("Snares".into(), 2)]);
    assert_eq!(l.lib.subfolders("/nowhere/at/all"), json!([]));
}

#[test]
fn search_narrows_to_a_subfolder_and_reports_it() {
    let l = lab();
    l.add("top.wav", 0);
    l.add("Hits/Snares/s1.wav", 1);
    l.add("Hits/Kicks/k1.wav", 2);
    let rows = l.search(SearchQuery { folder: Some(l.folder.clone()), under: Some(l.path("Hits")), sort: Some("name".into()), ..Default::default() }, None);
    assert_eq!(names(&rows), vec!["k1.wav", "s1.wav"]);
    let subs: HashSet<&str> = rows.iter().map(|r| r["sub"].as_str().unwrap()).collect();
    assert_eq!(subs, HashSet::from(["Hits/Kicks", "Hits/Snares"]));
    assert_eq!(l.by_name().last().unwrap()["sub"], "");
}

#[test]
fn words_match_the_subfolder_a_sound_sits_in() {
    let l = lab();
    l.add("Hits/Snares/one.wav", 1);
    l.add("Hits/Kicks/two.wav", 2);
    assert_eq!(names(&l.search(SearchQuery::default(), Some("snares"))), vec!["one.wav"]);
}

#[test]
fn a_project_with_a_folder_copies_what_is_added() {
    let l = lab();
    let src = l.add("kick.wav", 0);
    l.tag(&src, &["keeper"], 4);
    let kit = l.path("kit");
    std::fs::create_dir(&kit).unwrap();
    let pid = l.lib.create_project("kit").unwrap()["id"].as_i64().unwrap();
    l.lib.set_project_folder(pid, &kit).unwrap();
    let copy = l.lib.project_add(pid, &src).unwrap();
    assert_eq!(copy, l.path("kit/kick.wav"));
    assert!(Path::new(&copy).exists() && Path::new(&src).exists());
    assert_eq!(l.meta(&copy).0, vec!["keeper"]);
    // The same sound a second time reuses the copy.
    assert_eq!(l.lib.project_add(pid, &src).unwrap(), copy);
    let inside: Vec<String> = std::fs::read_dir(&kit).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(inside, vec!["kick.wav"]);
    assert_eq!(l.lib.project_paths(pid), set(&[&copy]));
}

#[test]
fn setting_a_folder_moves_existing_members_into_it() {
    let l = lab();
    let pid = l.lib.create_project("kit").unwrap()["id"].as_i64().unwrap();
    let a = l.add("a.wav", 1);
    l.lib.project_add(pid, &a).unwrap();
    let kit = l.path("kit");
    std::fs::create_dir(&kit).unwrap();
    l.lib.set_project_folder(pid, &kit).unwrap();
    assert_eq!(l.lib.project_paths(pid), set(&[&l.path("kit/a.wav")]));
    assert!(Path::new(&a).exists());
}

#[test]
fn adding_to_a_project_fails_cleanly_when_its_folder_is_gone() {
    let l = lab();
    let src = l.add("kick.wav", 0);
    let kit = l.path("kit");
    std::fs::create_dir(&kit).unwrap();
    let pid = l.lib.create_project("kit").unwrap()["id"].as_i64().unwrap();
    l.lib.set_project_folder(pid, &kit).unwrap();
    std::fs::remove_dir(&kit).unwrap();
    assert!(l.lib.project_add(pid, &src).is_err());
    assert!(l.lib.project_paths(pid).is_empty());
}

#[test]
fn project_order_is_kept_and_new_sounds_go_last() {
    let l = lab();
    let pid = l.kit("kit");
    let [a, b, c] = [("a.wav", 0), ("b.wav", 1), ("c.wav", 2)].map(|(n, s)| l.add(n, s));
    for p in [&a, &b, &c] {
        l.lib.project_add(pid, p).unwrap();
    }
    assert_eq!(l.lib.project_order(pid), json!([a, b, c]));
    l.lib.set_project_order(pid, &[c.clone(), a.clone(), b.clone()]);
    assert_eq!(l.lib.project_order(pid), json!([c, a, b]));
    let d = l.add("0-first-by-name.wav", 9);
    l.lib.project_add(pid, &d).unwrap();
    assert_eq!(l.lib.project_order(pid), json!([c, a, b, d]));
}

#[test]
fn an_export_follows_the_project_order() {
    let l = lab();
    let pid = l.kit("kit");
    let (a, b) = (l.add("a.wav", 1), l.add("b.wav", 2));
    l.lib.project_add(pid, &a).unwrap();
    l.lib.project_add(pid, &b).unwrap();
    l.lib.set_project_order(pid, &[b, a]);
    let out = l.lib.export_project(pid, &l.root.join("out").to_string_lossy(), "{n:03d}_{name}", "copy").unwrap();
    let files: Vec<String> = out["files"].as_array().unwrap().iter().map(|f| basename(f.as_str().unwrap())).collect();
    assert_eq!(files, vec!["001_b.wav", "002_a.wav"]);
}

#[test]
fn pads_hold_one_sound_each_and_follow_the_project() {
    let l = lab();
    let pid = l.kit("kit");
    let (a, b) = (l.add("a.wav", 1), l.add("b.wav", 2));
    l.lib.project_add(pid, &a).unwrap();
    l.lib.project_add(pid, &b).unwrap();
    l.lib.set_project_pads(pid, &json!({&a: 0, &b: 17})).unwrap();
    assert_eq!(l.lib.project_pads(pid), json!({&a: 0, &b: 17}));
    assert!(l.lib.set_project_pads(pid, &json!({&a: 3, &b: 3})).is_err());
    assert!(l.lib.set_project_pads(pid, &json!({&a: 160})).is_err());
    assert!(l.lib.set_project_pads(pid, &json!({"/not/in/the/project.wav": 1})).is_err());
    assert_eq!(l.lib.project_pads(pid), json!({&a: 0, &b: 17}));
    l.lib.set_project_pads(pid, &json!({&b: 5})).unwrap();
    assert_eq!(l.lib.project_pads(pid), json!({&b: 5}));
    l.lib.project_remove(pid, &b);
    assert_eq!(l.lib.project_pads(pid), json!({}));
}

#[test]
fn the_old_default_name_pattern_becomes_the_new_one() {
    let l = lab();
    l.lib.update_settings(&json!({"name_pattern": "{project}_{category}_{n:03d}_{name}"})).unwrap();
    assert_eq!(l.lib.settings()["name_pattern"], "{project}_{name}");
    l.lib.update_settings(&json!({"name_pattern": "{n:03d}_{name}"})).unwrap();
    assert_eq!(l.lib.settings()["name_pattern"], "{n:03d}_{name}");
}

#[test]
fn a_backup_round_trips() {
    let l = lab();
    let pid = l.kit("kit");
    let (a, b) = (l.add("a.wav", 1), l.add("b.wav", 2));
    l.lib.project_add(pid, &a).unwrap();
    l.lib.project_add(pid, &b).unwrap();
    l.tag(&a, &["keep"], 4);
    l.lib.catalog_set(&a, &json!({"memo": "under the bridge"})).unwrap();
    l.lib.set_project_order(pid, &[b.clone(), a.clone()]);
    l.lib.set_project_pads(pid, &json!({&a: 16})).unwrap();
    let file = l.lib.export_backup(&l.root.to_string_lossy()).unwrap();
    let data: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(data["athar_backup"], 1);
    l.lib.catalog_set(&a, &json!({"tags": [], "rating": 0, "memo": ""})).unwrap();
    l.lib.delete_project(pid);
    assert_eq!(l.lib.restore_backup(&file).unwrap(), json!({"sounds": 1, "projects": 1, "regions": 0}));
    let c = l.lib.catalog_get(&a);
    assert_eq!((c["tags"].clone(), c["rating"].clone(), c["memo"].clone()), (json!(["keep"]), json!(4), json!("under the bridge")));
    let restored = l.lib.projects()[0]["id"].as_i64().unwrap();
    assert_eq!(l.lib.project_order(restored), json!([b, a]));
    assert_eq!(l.lib.project_pads(restored), json!({&a: 16}));
}

#[test]
fn restore_refuses_a_file_that_is_not_a_backup() {
    let l = lab();
    let bad = l.root.join("x.json");
    std::fs::write(&bad, "[1, 2, 3]").unwrap();
    assert!(l.lib.restore_backup(&bad.to_string_lossy()).is_err());
    std::fs::write(&bad, "not json").unwrap();
    assert!(l.lib.restore_backup(&bad.to_string_lossy()).is_err());
}

#[test]
fn the_sound_map_places_every_sound_inside_the_unit_square() {
    let l = lab();
    assert_eq!(l.lib.sound_map(), json!([]));
    let mut paths: Vec<String> = (0..14).map(|i| l.add(&format!("s{i}.wav"), i)).collect();
    let points = l.lib.sound_map();
    let pts = points.as_array().unwrap();
    let mut got: Vec<String> = pts.iter().map(|p| p["path"].as_str().unwrap().to_string()).collect();
    got.sort();
    paths.sort();
    assert_eq!(got, paths);
    let xy: Vec<(f64, f64)> = pts.iter().map(|p| (p["x"].as_f64().unwrap(), p["y"].as_f64().unwrap())).collect();
    assert!(xy.iter().all(|&(x, y)| (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)));
    assert!(xy.iter().any(|p| *p != xy[0]));
    assert_eq!(l.lib.sound_map(), points);
}

#[test]
fn folder_counts_include_the_size_on_disk() {
    let l = lab();
    let a = l.add("a.wav", 0);
    assert_eq!(l.lib.folder_counts()[0]["bytes"], std::fs::metadata(&a).unwrap().len());
}

#[test]
fn restore_brings_regions_back() {
    let l = lab();
    let a = l.add("a.wav", 0);
    l.tag(&a, &["x"], 0);
    l.lib.regions_set(&a, &json!([{"start": 0.01, "end": 0.05, "name": "hit"}])).unwrap();
    let file = l.lib.export_backup(&l.root.to_string_lossy()).unwrap();
    l.lib.regions_set(&a, &json!([])).unwrap();
    assert_eq!(l.lib.restore_backup(&file).unwrap()["regions"], 1);
    assert_eq!(l.lib.regions_get(&a)[0]["name"], "hit");
}

#[test]
fn restore_reads_the_dated_backups_too() {
    let l = lab();
    let pid = l.kit("kit");
    let (a, b) = (l.add("a.wav", 1), l.add("b.wav", 2));
    l.lib.project_add(pid, &a).unwrap();
    l.lib.project_add(pid, &b).unwrap();
    l.tag(&a, &["keep"], 0);
    l.lib.set_project_pads(pid, &json!({&b: 2})).unwrap();
    let rows = |sql: &str| -> Vec<Value> {
        let db = l.lib.db();
        let mut st = db.prepare(sql).unwrap();
        let cols: Vec<String> = st.column_names().map(|c| c.to_string()).collect();
        st.query_map([], |r| {
            let mut o = serde_json::Map::new();
            for (i, c) in cols.iter().enumerate() {
                let v = match r.get_ref(i)? {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(n) => json!(n),
                    rusqlite::types::ValueRef::Real(f) => json!(f),
                    rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    rusqlite::types::ValueRef::Blob(_) => Value::Null,
                };
                o.insert(c.clone(), v);
            }
            Ok(Value::Object(o))
        })
        .unwrap()
        .flatten()
        .collect()
    };
    let dated = json!({
        "catalog": rows("SELECT path, tags, memo, rating, updated FROM catalog"),
        "projects": [{"id": pid, "name": "kit"}],
        "project_items": rows("SELECT project_id, path, added, pad FROM project_items"),
    });
    let file = l.root.join("catalog-2026-11-13.json");
    std::fs::write(&file, dated.to_string()).unwrap();
    l.lib.delete_project(pid);
    l.lib.catalog_set(&a, &json!({"tags": []})).unwrap();
    assert_eq!(l.lib.restore_backup(&file.to_string_lossy()).unwrap()["projects"], 1);
    assert_eq!(l.meta(&a).0, vec!["keep"]);
    let restored = l.lib.projects()[0]["id"].as_i64().unwrap();
    assert_eq!(l.lib.project_paths(restored), set(&[&a, &b]));
    assert_eq!(l.lib.project_pads(restored), json!({&b: 2}));
}

#[test]
fn a_damaged_backup_restores_nothing() {
    let l = lab();
    let a = l.add("a.wav", 0);
    l.tag(&a, &["mine"], 0);
    let bad = l.root.join("bad.json");
    std::fs::write(&bad, json!({"catalog": [{"path": a, "tags": ["theirs"], "rating": 3}, "not a row"], "projects": [{"name": "kit", "paths": ["x"]}]}).to_string()).unwrap();
    assert!(l.lib.restore_backup(&bad.to_string_lossy()).is_err());
    assert_eq!(l.meta(&a).0, vec!["mine"]);
    assert_eq!(l.lib.projects(), json!([]));
}

#[test]
fn a_project_folder_that_left_the_library_rejoins_it() {
    let l = lab();
    let src = l.add("kick.wav", 0);
    let kit = l.root.join("elsewhere");
    std::fs::create_dir(&kit).unwrap();
    let kit = kit.to_string_lossy().into_owned();
    let pid = l.lib.create_project("kit").unwrap()["id"].as_i64().unwrap();
    l.lib.db().execute("UPDATE projects SET folder=? WHERE id=?", params![kit, pid]).unwrap();
    assert!(!l.lib.folders().contains(&kit));
    l.lib.project_add(pid, &src).unwrap();
    assert!(l.lib.folders().contains(&kit));
}

#[test]
fn the_same_name_and_size_with_another_sound_is_not_reused() {
    let l = lab();
    let a = l.add("one/kick.wav", 1);
    let b = l.add("two/kick.wav", 2);
    assert_eq!(std::fs::metadata(&a).unwrap().len(), std::fs::metadata(&b).unwrap().len());
    let kit = l.path("kit");
    std::fs::create_dir(&kit).unwrap();
    let pid = l.lib.create_project("kit").unwrap()["id"].as_i64().unwrap();
    l.lib.set_project_folder(pid, &kit).unwrap();
    let (first, second) = (l.lib.project_add(pid, &a).unwrap(), l.lib.project_add(pid, &b).unwrap());
    assert_ne!(first, second);
    assert_eq!(std::fs::read_dir(&kit).unwrap().count(), 2);
}

#[test]
fn setting_a_folder_keeps_the_order() {
    let l = lab();
    let pid = l.lib.create_project("kit").unwrap()["id"].as_i64().unwrap();
    let (a, b) = (l.add("a.wav", 1), l.add("b.wav", 2));
    let a = l.lib.project_add(pid, &a).unwrap();
    let b = l.lib.project_add(pid, &b).unwrap();
    l.lib.set_project_order(pid, &[b, a]);
    let kit = l.path("kit");
    std::fs::create_dir(&kit).unwrap();
    l.lib.set_project_folder(pid, &kit).unwrap();
    let order: Vec<String> = l.lib.project_order(pid).as_array().unwrap().iter().map(|p| basename(p.as_str().unwrap())).collect();
    assert_eq!(order, vec!["b.wav", "a.wav"]);
    assert!(l.lib.project_paths(pid).iter().all(|p| is_inside(p, &kit)));
}

#[test]
fn names_never_start_with_a_dot() {
    assert_eq!(safe_name(".hidden take"), "hidden take");
    assert_eq!(safe_name("."), "untitled");
    assert_eq!(safe_name(".."), "untitled");
}

#[test]
fn a_folder_made_by_hand_in_the_projects_folder_becomes_a_project() {
    let l = lab();
    let base = l.path("projects");
    std::fs::create_dir_all(l.path("projects/by hand")).unwrap();
    l.lib.update_settings(&json!({"projects_dir": base})).unwrap();
    let inside = l.add("projects/by hand/a_b.wav", 0);
    l.lib.adopt_project_folders();
    let p = l.lib.projects().as_array().unwrap().iter().find(|p| p["name"] == "by hand").cloned().unwrap();
    assert_eq!(p["folder"], l.path("projects/by hand"));
    assert_eq!(l.lib.project_paths(p["id"].as_i64().unwrap()), set(&[&inside]));
    // Deleted in the app, it stays deleted although its folder is still on disk.
    l.lib.delete_project(p["id"].as_i64().unwrap());
    l.lib.adopt_project_folders();
    assert!(!l.lib.projects().as_array().unwrap().iter().any(|q| q["name"] == "by hand"));
}

// ---------- the arrangement (test_arrangement.py)

fn clip(path: &str, over: Value) -> Value {
    let mut c = json!({"path": path, "name": "x", "duration": 1.0, "sr": 48000, "at": 0.5, "in": 0.1, "out": 0.2});
    c.as_object_mut().unwrap().extend(over.as_object().unwrap().clone());
    c
}

fn lane(clips: Vec<Value>, over: Value) -> Value {
    let mut l = json!({"name": "Drums", "gain": -3, "mute": false, "solo": true, "clips": clips});
    l.as_object_mut().unwrap().extend(over.as_object().unwrap().clone());
    l
}

#[test]
fn the_arrangement_is_kept_and_starts_empty() {
    let l = lab();
    assert_eq!(l.lib.arrangement_get()["lanes"], json!([]));
    let p = l.add("kick.wav", 0);
    l.lib.arrangement_set(&json!({"loop": false, "lanes": [lane(vec![clip(&p, json!({}))], json!({}))]})).unwrap();
    let a = l.lib.arrangement_get();
    assert_eq!(a["loop"], false);
    assert_eq!(a["lanes"], json!([lane(vec![clip(&p, json!({"name": "kick.wav"}))], json!({"gain": -3.0}))]));
}

#[test]
fn arrangement_values_are_held_in_range() {
    let l = lab();
    let p = l.add("kick.wav", 0);
    let mut lanes = vec![lane(vec![clip(&p, json!({"at": -1})), clip(&p, json!({"at": 1e9, "in": 0.8, "out": 0.9}))], json!({"gain": 40, "name": "  "}))];
    lanes.extend(std::iter::repeat_n(lane(vec![], json!({})), 40));
    let a = l.lib.arrangement_set(&json!({"lanes": lanes})).unwrap();
    assert_eq!(a["loop"], true);
    assert_eq!(a["lanes"].as_array().unwrap().len(), 16);
    let first = &a["lanes"][0];
    assert_eq!((first["gain"].as_f64(), first["name"].as_str()), (Some(6.0), Some("Lane 1")));
    let ats: Vec<f64> = first["clips"].as_array().unwrap().iter().map(|c| c["at"].as_f64().unwrap()).collect();
    assert_eq!(ats, vec![0.0, 600.0]);
    // Fades that would meet in the middle are cut back to the length of the clip.
    assert_eq!(first["clips"][1]["in"], 0.8);
    assert!((first["clips"][1]["out"].as_f64().unwrap() - 0.2).abs() < 1e-9);
    assert_eq!(a["lanes"][1]["clips"], json!([]));
}

#[test]
fn the_arrangement_rejects_a_clip_without_a_path() {
    let l = lab();
    assert!(l.lib.arrangement_set(&json!({"lanes": [{"clips": [{"at": 0}]}]})).is_err());
}

#[test]
fn an_arrangement_of_one_sound_per_lane_becomes_lanes_of_clips() {
    let l = lab();
    let p = l.add("kick.wav", 0);
    let old = json!({"bpm": 120, "grid": false, "lanes": [{"path": p, "name": "kick.wav", "duration": 0.1, "sr": 48000, "gain": -6, "mute": true, "solo": false, "hits": [0, 0.5]}]});
    l.lib.meta_set("arrangement", &old.to_string()).unwrap();
    let a = l.lib.arrangement_get();
    let lanes: Vec<_> = a["lanes"].as_array().unwrap().iter().map(|x| (x["name"].as_str().unwrap().to_string(), x["gain"].as_f64().unwrap(), x["mute"].as_bool().unwrap())).collect();
    assert_eq!(lanes, vec![("kick".to_string(), -6.0, true)]);
    let clips: Vec<_> = a["lanes"][0]["clips"].as_array().unwrap().iter().map(|c| (c["path"].as_str().unwrap().to_string(), c["at"].as_f64().unwrap(), c["in"].as_f64().unwrap(), c["out"].as_f64().unwrap())).collect();
    assert_eq!(clips, vec![(p.clone(), 0.0, 0.0, 0.0), (p.clone(), 0.5, 0.0, 0.0)]);
}

#[test]
fn the_arrangement_follows_a_renamed_file() {
    let l = lab();
    let (p, other) = (l.add("kick.wav", 1), l.add("snare.wav", 2));
    l.lib.arrangement_set(&json!({"lanes": [lane(vec![clip(&p, json!({})), clip(&other, json!({}))], json!({}))]})).unwrap();
    let new = l.lib.rename(&p, "boom").unwrap();
    let clips: Vec<_> = l.lib.arrangement_get()["lanes"][0]["clips"].as_array().unwrap().iter().map(|c| (c["path"].as_str().unwrap().to_string(), c["name"].as_str().unwrap().to_string())).collect();
    assert_eq!(clips, vec![(new, "boom.wav".to_string()), (other, "snare.wav".to_string())]);
}

#[test]
fn the_arrangement_follows_a_moved_folder() {
    let l = lab();
    let p = l.add("drums/kick.wav", 0);
    l.lib.arrangement_set(&json!({"lanes": [lane(vec![clip(&p, json!({}))], json!({}))]})).unwrap();
    let (old, new) = (l.path("drums"), l.path("kit"));
    std::fs::rename(&old, &new).unwrap();
    l.events(&[&old, &new]);
    assert_eq!(l.lib.arrangement_get()["lanes"][0]["clips"][0]["path"], l.path("kit/kick.wav"));
}

// ---------- SP-404MKII export (test_sp404.py)

#[test]
fn an_sp404_export_writes_a_project_folder_with_the_pad_layout() {
    let l = lab();
    let pid = l.kit("kit");
    let [a, b, c] = [("a.wav", 0), ("b.wav", 1), ("c.wav", 2)].map(|(n, s)| l.add(n, s));
    for p in [&a, &b, &c] {
        l.lib.project_add(pid, p).unwrap();
    }
    l.lib.set_project_pads(pid, &json!({&b: 0, &a: 20})).unwrap();
    let card = l.root.join("card");
    std::fs::create_dir_all(card.join("IMPORT")).unwrap();
    let out = l.lib.export_project(pid, &card.to_string_lossy(), "{project}_{name}", "sp404").unwrap();
    let folder = card.join("IMPORT/kit");
    assert_eq!(out["folder"], json!(folder));
    let list = |d: PathBuf| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    };
    assert_eq!(list(folder.join("SMPL")), vec!["BANK1-01.SMP", "BANK1-02.SMP", "BANK2-05.SMP"]);
    assert_eq!(list(folder.clone()), vec!["PADCONF.BIN", "PICTURE", "PTN", "SMPL"]);
    let pic = std::fs::read(folder.join("PICTURE/startup_1.bmp")).unwrap();
    assert_eq!((pic.len(), &pic[..2]), (1086, &b"BM"[..]));
    assert_eq!((i32::from_le_bytes(pic[18..22].try_into().unwrap()), i32::from_le_bytes(pic[22..26].try_into().unwrap())), (128, 64));
    let conf = std::fs::read(folder.join("PADCONF.BIN")).unwrap();
    assert_eq!(crate::sp404::padconf_name(&conf).as_deref(), Some("kit"));
    let pads = crate::sp404::pad_labels(&conf);
    let labels: BTreeMap<usize, &str> = pads.iter().map(|(n, (_, l))| (*n, l.as_str())).collect();
    assert_eq!(labels, BTreeMap::from([(0, "b"), (1, "c"), (20, "a")]));
    let smp = std::fs::read(folder.join("SMPL/BANK1-01.SMP")).unwrap();
    assert_eq!(pads[&0].0, smp.len());
    let word = |at: usize| u32::from_be_bytes(smp[at..at + 4].try_into().unwrap());
    let mut r = hound::WavReader::open(&b).unwrap();
    let (sr, ch) = (r.spec().sample_rate, r.spec().channels as u32);
    let want: Vec<i16> = r.samples::<i16>().map(Result::unwrap).collect();
    assert_eq!((&smp[..4], word(8), word(12), word(16)), (&b"RFWV"[..], sr, ch, 16));
    let got: Vec<i16> = smp[512..].chunks_exact(2).map(|w| i16::from_be_bytes([w[0], w[1]])).collect();
    assert_eq!(got, want, "16-bit 48 kHz audio goes onto the card exact");
}

#[test]
fn an_sp404_export_refuses_more_sounds_than_pads() {
    let l = lab();
    let pid = l.kit("big");
    for i in 0..=SP404_PADS {
        let p = l.path(&format!("s{i:03}.wav"));
        write_wav(Path::new(&p), &[0.0; 48], 48_000, 16);
        l.lib.project_add(pid, &p).unwrap();
    }
    assert!(l.lib.export_project(pid, &l.root.to_string_lossy(), "{project}_{name}", "sp404").is_err());
    assert!(!l.root.join("big").exists());
}

#[test]
fn an_sp404_export_keeps_another_project_with_the_same_folder_name() {
    let l = lab();
    let pid = l.kit("kit");
    let a = l.add("a.wav", 0);
    l.lib.project_add(pid, &a).unwrap();
    let card = l.root.join("card");
    let theirs = card.join("IMPORT/kit");
    std::fs::create_dir_all(&theirs).unwrap();
    std::fs::write(theirs.join("PADCONF.BIN"), crate::sp404::padconf_bytes("other", &BTreeMap::new(), 90.0)).unwrap();
    assert!(l.lib.export_project(pid, &card.to_string_lossy(), "{project}_{name}", "sp404").is_err());
    assert!(!theirs.join("SMPL").exists());
    std::fs::write(theirs.join("PADCONF.BIN"), crate::sp404::padconf_bytes("kit", &BTreeMap::new(), 90.0)).unwrap();
    l.lib.export_project(pid, &card.to_string_lossy(), "{project}_{name}", "sp404").unwrap();
    assert!(theirs.join("SMPL/BANK1-01.SMP").is_file());
}

#[test]
fn the_card_is_the_volume_with_import_and_export() {
    let t = Tmp::new();
    assert_eq!(crate::audio::sp404_card_in(&t.0), None);
    std::fs::create_dir_all(t.0.join("Backup/Photos")).unwrap();
    for v in ["CARD", "SP404MKII"] {
        std::fs::create_dir_all(t.0.join(v).join("IMPORT")).unwrap();
        std::fs::create_dir(t.0.join(v).join("EXPORT")).unwrap();
    }
    assert_eq!(crate::audio::sp404_card_in(&t.0), Some(t.0.join("SP404MKII").to_string_lossy().into_owned()));
}

// ---------- with the CLAP model (test_indexing.py, test_watching.py)

fn wait(seconds: u64, cond: impl Fn() -> bool) -> bool {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    while std::time::Instant::now() < end {
        if cond() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    cond()
}

/// A library with the real model, watcher and worker, on a temporary data folder.
fn running(root: &Path) -> Arc<Library> {
    let model = model_dir(None);
    assert!(model.join("audio.onnx").exists(), "the CLAP model is not exported to {}", model.display());
    let (lib, rx) = Library::create(root.join("home"), model).unwrap();
    lib.update_settings(&json!({"projects_dir": root.join("Projects"), "rescan_on_open": false})).unwrap();
    let l = lib.clone();
    std::thread::spawn(move || l.boot(rx));
    assert!(wait(120, || lib.status()["model"] != "loading"));
    assert_eq!(lib.status()["model"], "ready", "{}", lib.status()["error"]);
    lib
}

fn gaussian(seed: u64, n: usize) -> Vec<f32> {
    let u = noise(seed, 2 * n);
    u.chunks_exact(2).map(|p| {
        let (a, b) = ((p[0] + 1.0) / 2.0 + 1e-7, (p[1] + 1.0) / 2.0);
        (-2.0 * a.ln()).sqrt() * (std::f32::consts::TAU * b).cos()
    }).collect()
}

#[test]
fn a_real_index_run_hears_the_whole_file() {
    let t = Tmp::new();
    let folder = t.0.join("sounds");
    let tone = |seconds: usize, tail: bool| -> Vec<f32> {
        let mut y: Vec<f32> = (0..48_000 * seconds).map(|i| 0.3 * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin()).collect();
        if tail {
            let n = y.len();
            for (v, g) in y[n - 48_000 * 20..].iter_mut().zip(gaussian(0, 48_000 * 20)) {
                *v = 0.3 * g;
            }
        }
        y
    };
    write_wav(&folder.join("short.wav"), &tone(2, false), 48_000, 32);
    write_wav(&folder.join("long_tone_then_noise.wav"), &tone(120, true), 48_000, 32);
    write_wav(&folder.join("long_tone.wav"), &tone(120, false), 48_000, 32);
    let lib = running(&t.0);
    lib.add_folder(&folder.to_string_lossy()).unwrap();
    assert!(wait(120, || lib.store.read().unwrap().items.len() >= 3));
    let all = lib.search(&SearchQuery { sort: Some("name".into()), ..Default::default() }, None, None, None).unwrap();
    let rows: HashMap<String, Value> = all.as_array().unwrap().iter().map(|r| (r["name"].as_str().unwrap().to_string(), r.clone())).collect();
    let mut found: Vec<&str> = rows.keys().map(String::as_str).collect();
    found.sort();
    assert_eq!(found, vec!["long_tone.wav", "long_tone_then_noise.wav", "short.wav"]);
    let unhashed: i64 = lib.db().query_row("SELECT COUNT(*) FROM files WHERE sha1 IS NULL OR sha1 = ''", [], |r| r.get(0)).unwrap();
    assert_eq!(unhashed, 0);
    // Peaks cover the whole file: the noisy last 20 s of 120 s is the last sixth of the bins.
    let pk: Vec<f64> = rows["long_tone_then_noise.wav"]["peaks"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    assert!(mean(&pk[pk.len() - 15..]) > mean(&pk[..60]) * 1.5);
    // The noisy ending changes the embedding, which it could not when only the opening was heard.
    let a = lib.embedding_of(rows["long_tone.wav"]["path"].as_str().unwrap()).unwrap();
    let b = lib.embedding_of(rows["long_tone_then_noise.wav"]["path"].as_str().unwrap()).unwrap();
    assert!((dot(&a, &a).sqrt() - 1.0).abs() < 1e-4);
    assert!(dot(&a, &b) < 0.999);
}

#[test]
fn finder_style_changes_keep_metadata() {
    let t = Tmp::new();
    let folder = t.0.join("sounds");
    std::fs::create_dir(&folder).unwrap();
    let f = folder.to_string_lossy().into_owned();
    let lib = running(&t.0);
    lib.add_folder(&f).unwrap();
    assert!(wait(30, || lib.watcher.lock().unwrap().is_some()));
    // The watch request reaches the watcher thread on its own time.
    std::thread::sleep(std::time::Duration::from_secs(1));
    let write = |p: &Path, seed: u64| write_wav(p, &noise(seed, 48_000).iter().map(|v| v * 0.1).collect::<Vec<_>>(), 48_000, 32);
    let indexed = |p: &Path| lib.embedding_of(&p.to_string_lossy()).is_some();
    let meta = |p: &Path| {
        let c = lib.catalog_get(&p.to_string_lossy());
        (c["tags"].clone(), c["rating"].clone())
    };
    let kept = (json!(["kyoto"]), json!(5));
    let paths = |pid: i64| lib.project_paths(pid);

    let a = folder.join("rain.wav");
    write(&a, 1);
    assert!(wait(30, || indexed(&a)), "a new file is indexed without a rescan");
    let pid = lib.create_project("kit").unwrap()["id"].as_i64().unwrap();
    lib.set_project_folder(pid, &f).unwrap();
    lib.catalog_set(&a.to_string_lossy(), &json!({"tags": ["kyoto"], "rating": 5})).unwrap();
    lib.project_add(pid, &a.to_string_lossy()).unwrap();

    // Renamed in Finder.
    let b = folder.join("rain kyoto.wav");
    std::fs::rename(&a, &b).unwrap();
    assert!(wait(30, || indexed(&b) && !indexed(&a)));
    assert_eq!(meta(&b), kept);
    assert_eq!(paths(pid), set(&[&b.to_string_lossy()]));

    // Saved over by another app: a temporary file, then a rename onto the original.
    let stamp = || -> Option<f64> { lib.db().query_row("SELECT mtime FROM files WHERE path=? AND emb IS NOT NULL", [b.to_string_lossy()], |r| r.get(0)).ok() };
    let before = stamp();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let tmp = folder.join(".rain kyoto.wav.tmp");
    write(&tmp, 2);
    std::fs::rename(&tmp, &b).unwrap();
    assert!(wait(30, || stamp().is_some() && stamp() != before), "the new content is re-indexed");
    assert_eq!(meta(&b), kept);
    assert_eq!(paths(pid), set(&[&b.to_string_lossy()]));

    // Moved into a subfolder.
    let c = folder.join("takes/rain kyoto.wav");
    std::fs::create_dir(folder.join("takes")).unwrap();
    std::fs::rename(&b, &c).unwrap();
    assert!(wait(30, || indexed(&c) && !indexed(&b)));
    assert_eq!(meta(&c), kept);
    assert_eq!(paths(pid), set(&[&c.to_string_lossy()]));

    // Deleted, then put back as from the Trash.
    let data = std::fs::read(&c).unwrap();
    std::fs::remove_file(&c).unwrap();
    assert!(wait(30, || !indexed(&c)));
    assert_eq!(lib.all_tags(), json!([]));
    std::fs::write(&c, data).unwrap();
    assert!(wait(30, || indexed(&c)));
    assert_eq!(meta(&c), kept);
    assert_eq!(paths(pid), set(&[&c.to_string_lossy()]));
}
