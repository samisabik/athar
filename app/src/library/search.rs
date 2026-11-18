//! Search by words, by likeness and by filters, and the counts shown beside the categories.

use rayon::prelude::*;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use super::paths::{basename, is_inside, nfc};
use super::store::{Item, dot, sub_of};
use super::{Library, Res};

/// Above nonsense: the best a query must score to count as finding anything, and what each sound keeps.
const QUERY_FOUND: f32 = 0.25;

const QUERY_KEEP: f32 = 0.15;

#[derive(serde::Deserialize, Default)]
pub struct SearchQuery {
    pub query: Option<String>,
    pub similar_to: Option<String>,
    pub category: Option<String>,
    pub folder: Option<String>,
    pub under: Option<String>,
    pub project: Option<i64>,
    pub tag: Option<String>,
    pub min_rating: Option<i64>,
    pub sort: Option<String>,
    pub limit: Option<i64>,
}

impl Library {
    pub fn embedding_of(&self, path: &str) -> Option<Vec<f32>> {
        let s = self.store.read().unwrap();
        s.pos.get(path).map(|&i| s.items[i].emb.clone())
    }

    pub fn embed_text(&self, words: &str) -> Res<Vec<f32>> {
        let clap = self.engine.get().ok_or("Model is still loading")?;
        clap.embed_texts(&[words]).map(|mut v| v.remove(0)).map_err(|e| e.to_string())
    }

    pub fn search(&self, q: &SearchQuery, vec: Option<Vec<f32>>, exclude: Option<&str>, words: Option<&str>) -> Res<Value> {
        let catalog = self.catalog_all();
        let names = self.category_names();
        let members: Option<HashSet<String>> = q.project.map(|pid| self.project_paths(pid));
        let sort = if vec.is_some() || q.sort.as_deref() != Some("score") { q.sort.clone().unwrap_or("score".into()) } else { "name".into() };
        // A project's own order, as arranged on the Projects page; sounds without a place go last.
        let order: HashMap<String, usize> = match (sort.as_str(), q.project) {
            ("project", Some(pid)) => self.project_order(pid).as_array().into_iter().flatten().enumerate().filter_map(|(k, p)| Some((p.as_str()?.to_string(), k))).collect(),
            _ => HashMap::new(),
        };
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
                // Name and subfolder, then tags and note for a sound that has any.
                let noted;
                let hay: &str = match catalog.get(&it.meta.path) {
                    Some(c) => {
                        noted = nfc(&format!("{} {} {}", it.words, c.tags.join(" "), c.memo).to_lowercase());
                        &noted
                    }
                    None => &it.words,
                };
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
                let c = catalog.get(&m.path);
                (cat_idx.is_none() || it.cat == cat_idx)
                    && q.folder.as_ref().is_none_or(|f| f.is_empty() || &m.folder == f)
                    && q.under.as_ref().is_none_or(|u| u.is_empty() || is_inside(&m.path, u))
                    && members.as_ref().is_none_or(|ms| ms.contains(&m.path))
                    && q.tag.as_ref().is_none_or(|t| t.is_empty() || c.is_some_and(|c| c.tags.contains(t)))
                    && q.min_rating.is_none_or(|r| r == 0 || c.map(|c| c.rating).unwrap_or(0) >= r)
                    && exclude.is_none_or(|e| m.path != e)
                    && relevant.as_ref().is_none_or(|r| r[i])
            })
            .collect();
        let limit = q.limit.unwrap_or(200).max(0) as usize;
        let name_of = |i: usize| basename(&store.items[i].meta.path).to_lowercase();
        match sort.as_str() {
            "project" => idx.sort_by_cached_key(|&i| (order.get(&store.items[i].meta.path).copied().unwrap_or(usize::MAX), name_of(i))),
            "score" if vec.is_some() => idx.sort_by(|&a, &b| scores[b].total_cmp(&scores[a])),
            "rating" => idx.sort_by_cached_key(|&i| std::cmp::Reverse(catalog.get(&store.items[i].meta.path).map(|c| c.rating).unwrap_or(0))),
            "recorded" => idx.sort_by_cached_key(|&i| {
                let d: String = store.items[i].meta.recorded.as_deref().unwrap_or("").chars().filter(|c| c.is_ascii_digit()).collect();
                std::cmp::Reverse(d.parse::<i64>().unwrap_or(0))
            }),
            "duration" => idx.sort_by(|&a, &b| store.items[a].meta.duration.unwrap_or(0.0).total_cmp(&store.items[b].meta.duration.unwrap_or(0.0))),
            "category" => idx.sort_by_cached_key(|&i| (store.items[i].cat.map(|c| names[c].to_lowercase()).unwrap_or_default(), name_of(i))),
            "brightness" => idx.sort_by(|&a, &b| store.items[a].meta.brightness.unwrap_or(0.0).total_cmp(&store.items[b].meta.brightness.unwrap_or(0.0))),
            "loudness" => idx.sort_by(|&a, &b| store.items[b].meta.rms_db.unwrap_or(-200.0).total_cmp(&store.items[a].meta.rms_db.unwrap_or(-200.0))),
            "pitch" => idx.sort_by(|&a, &b| store.items[a].meta.pitch_hz.unwrap_or(1e9).total_cmp(&store.items[b].meta.pitch_hz.unwrap_or(1e9))),
            "tempo" => idx.sort_by(|&a, &b| store.items[a].meta.tempo.unwrap_or(1e9).total_cmp(&store.items[b].meta.tempo.unwrap_or(1e9))),
            _ => idx.sort_by_cached_key(|&i| name_of(i)),
        }
        idx.truncate(limit);
        let rows: Vec<Value> = idx
            .iter()
            .map(|&i| {
                let it = &store.items[i];
                let m = &it.meta;
                let c = catalog.get(&m.path);
                json!({
                    "tags": c.map(|c| c.tags.clone()).unwrap_or_default(), "memo": c.map(|c| c.memo.clone()).unwrap_or_default(),
                    "rating": c.map(|c| c.rating).unwrap_or(0), "origin": c.and_then(|c| c.origin.clone()), "recorded": m.recorded,
                    "path": m.path, "name": basename(&m.path), "folder": m.folder, "sub": sub_of(&m.path, &m.folder),
                    "score": scores[i], "duration": m.duration, "sr": m.sr, "channels": m.channels, "rms_db": m.rms_db,
                    "brightness": m.brightness, "pitch_hz": m.pitch_hz, "note": note_name(m.pitch_hz), "tempo": m.tempo,
                    "is_loop": m.is_loop, "category": it.cat.map(|c| names[c].clone()), "peaks": m.peaks, "text_match": boost[i],
                })
            })
            .collect();
        // The disk is asked about each file, and missing ones are forgotten, with the store free
        // for the indexer.
        drop(store);
        let (found, missing): (Vec<Value>, Vec<Value>) = rows.into_iter().partition(|r| r["path"].as_str().is_some_and(|p| Path::new(p).exists()));
        // Files gone from a folder that is still there are forgotten; a whole folder offline
        // (an unplugged drive) stays indexed and hidden.
        for r in &missing {
            if let (Some(p), Some(folder)) = (r["path"].as_str(), r["folder"].as_str()) {
                if Path::new(folder).is_dir() {
                    self.forget(p, false);
                }
            }
        }
        Ok(Value::Array(found))
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
                    let its: Vec<&Item> = store.items.iter().filter(|it| it.cat == Some(ci)).collect();
                    json!({
                        "name": n, "count": its.len(),
                        "brightness": med(its.iter().filter_map(|it| it.meta.brightness.filter(|b| *b != 0.0)).collect()),
                        "duration": med(its.iter().filter_map(|it| it.meta.duration.filter(|d| *d != 0.0)).collect()),
                    })
                })
                .collect(),
        )
    }
}

fn note_name(hz: Option<f64>) -> Option<String> {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let hz = hz.filter(|h| *h > 0.0)?;
    let midi = (12.0 * (hz / 440.0).log2() + 69.0).round_ties_even() as i64;
    Some(format!("{}{}", NAMES[midi.rem_euclid(12) as usize], midi.div_euclid(12) - 1))
}
