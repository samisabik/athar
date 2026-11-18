//! Exporting a project as a folder of files or as an SP-404MKII project.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use super::paths::{ascii_name, basename, expand, safe_name, unique_path};
use super::projects::SP404_PADS;
use super::{Library, Res};

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
        s.items.iter().filter(|it| members.contains(&it.meta.path)).filter_map(|it| it.cat.map(|c| (it.meta.path.clone(), names[c].clone()))).collect()
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
        let folder = sp404::import_dir(&expand(dest)).join(ascii_name(name));
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
            let data = sp404::smp_bytes(&sp404::file_channels(Path::new(path), mono)?);
            let f = folder.join("SMPL").join(sp404::bank_file(*n));
            std::fs::write(&f, &data).map_err(|e| e.to_string())?;
            files.push(f.to_string_lossy().into_owned());
            let stem = Path::new(path).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
            pads.insert(*n, (data.len(), stem, loops.get(path).copied().flatten()));
        }
        std::fs::write(folder.join("PADCONF.BIN"), sp404::padconf_bytes(name, &pads, 90.0)).map_err(|e| e.to_string())?;
        Ok(json!({"folder": folder, "files": files}))
    }
}

/// A file-naming pattern filled in. Its keys, each in braces, are project, category, n and
/// name; n takes a width such as {n:03d}.
fn fill_pattern(pattern: &str, project: &str, category: &str, n: usize, name: &str) -> Res<String> {
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
