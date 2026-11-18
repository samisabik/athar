//! Exporting a project as files and as an SP-404MKII project.

use crate::library::paths::basename;
use crate::library::projects::SP404_PADS;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use super::{lab, write_wav};

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
