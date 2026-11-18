//! Names, and where the library lives.

use crate::library::paths::{safe_name, settle_home};
use std::path::Path;
use super::{Tmp};

#[test]
fn names_never_start_with_a_dot() {
    assert_eq!(safe_name(".hidden take"), "hidden take");
    assert_eq!(safe_name("."), "untitled");
    assert_eq!(safe_name(".."), "untitled");
}

fn put(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

#[test]
fn a_new_user_starts_in_an_empty_athar_folder() {
    let t = Tmp::new();
    let home = settle_home(&t.0);
    assert_eq!(home, t.0.join("Athar"));
    assert!(home.is_dir());
    assert!(!t.0.join("Athar Next").exists());
    assert_eq!(settle_home(&t.0), home);
}

#[test]
fn a_2_0_0_library_moves_back_to_athar_and_the_python_one_is_kept() {
    let t = Tmp::new();
    put(&t.0.join("Athar/index.db"), "python");
    put(&t.0.join("Athar/backups/b.json"), "old backup");
    put(&t.0.join("Athar Next/index.db"), "native");
    put(&t.0.join("Athar Next/backups/b.json"), "new backup");
    let home = settle_home(&t.0);
    assert_eq!(home, t.0.join("Athar"));
    assert_eq!(read(&home.join("index.db")), "native");
    assert_eq!(read(&home.join("backups/b.json")), "new backup");
    assert!(!t.0.join("Athar Next").exists());
    assert_eq!(read(&t.0.join("Athar 1.x backup/index.db")), "python");
    assert_eq!(read(&t.0.join("Athar 1.x backup/backups/b.json")), "old backup");
    // Later launches leave everything where it is.
    assert_eq!(settle_home(&t.0), home);
    assert_eq!(read(&home.join("index.db")), "native");
    assert!(!t.0.join("Athar 1.x backup 2").exists());
}

#[test]
fn a_2_0_0_library_without_a_python_one_moves_back_to_athar() {
    let t = Tmp::new();
    put(&t.0.join("Athar Next/index.db"), "native");
    let home = settle_home(&t.0);
    assert_eq!(read(&home.join("index.db")), "native");
    assert!(!t.0.join("Athar Next").exists());
    assert!(!t.0.join("Athar 1.x backup").exists());
}

#[test]
fn a_python_library_is_used_in_place_after_a_copy_of_its_index() {
    let t = Tmp::new();
    put(&t.0.join("Athar/index.db"), "python");
    put(&t.0.join("Athar/index.db-wal"), "wal");
    let home = settle_home(&t.0);
    assert_eq!(home, t.0.join("Athar"));
    assert_eq!(read(&home.join("index.db")), "python");
    assert_eq!(read(&home.join("index-1.x.db")), "python");
    assert_eq!(read(&home.join("index-1.x.db-wal")), "wal");
    // The copy is made once: a later launch keeps the first copy, not the changed index.
    put(&home.join("index.db"), "changed");
    settle_home(&t.0);
    assert_eq!(read(&home.join("index-1.x.db")), "python");
}

#[test]
fn an_earlier_backup_folder_is_not_overwritten() {
    let t = Tmp::new();
    put(&t.0.join("Athar 1.x backup/keep"), "keep");
    put(&t.0.join("Athar/index.db"), "python");
    put(&t.0.join("Athar Next/index.db"), "native");
    settle_home(&t.0);
    assert_eq!(read(&t.0.join("Athar 1.x backup/keep")), "keep");
    assert_eq!(read(&t.0.join("Athar 1.x backup 2/index.db")), "python");
    assert_eq!(read(&t.0.join("Athar/index.db")), "native");
}
