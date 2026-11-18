//! The sound map: every sound on a flat map, close to the sounds it resembles.

use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use super::Library;
use super::paths::basename;
use super::store::EMB_DIM;

const MAP_FINE_LIMIT: usize = 3000;

impl Library {
    /// Every sound on a flat map, close to the sounds it resembles. Kept until the library
    /// changes; then the last map, minus sounds that are gone, is handed back while the new
    /// one is worked out in the background.
    pub fn sound_map(self: &Arc<Self>) -> Value {
        let version = self.store.read().unwrap().version;
        let cached = self.map.lock().unwrap().clone();
        match cached {
            Some((v, m)) if v == version => m,
            Some((_, m)) if m.as_array().is_some_and(|a| !a.is_empty()) => {
                if !self.map_busy.swap(true, Ordering::SeqCst) {
                    let me = self.clone();
                    std::thread::spawn(move || {
                        me.build_map();
                        me.map_busy.store(false, Ordering::SeqCst);
                    });
                }
                let s = self.store.read().unwrap();
                Value::Array(m.as_array().unwrap().iter().filter(|p| p["path"].as_str().is_some_and(|x| s.pos.contains_key(x))).cloned().collect())
            }
            _ => self.build_map(),
        }
    }

    fn build_map(&self) -> Value {
        struct Sound {
            path: String,
            duration: Option<f64>,
            sr: Option<i64>,
            cat: Option<usize>,
        }
        // What the map needs is copied out, so indexing and search go on while it is laid out.
        let (version, sounds, embs) = {
            let store = self.store.read().unwrap();
            let sounds: Vec<Sound> = store.items.iter().map(|it| Sound { path: it.meta.path.clone(), duration: it.meta.duration, sr: it.meta.sr, cat: it.cat }).collect();
            let embs: Vec<Vec<f32>> = store.items.iter().map(|it| it.emb.clone()).collect();
            (store.version, sounds, embs)
        };
        let n = sounds.len();
        let names = self.category_names();
        let mut xy = vec![[0.5f64, 0.5]; n];
        if n >= 3 {
            xy = first_two_components(&embs);
            // The finer layout costs time that grows with the square of the library, so past a
            // few thousand sounds the rough map is used as it is.
            if (12..=MAP_FINE_LIMIT).contains(&n) {
                let sd = (xy.iter().map(|p| p[0] * p[0]).sum::<f64>() / n as f64).sqrt().max(1e-12);
                let init: Vec<[f64; 2]> = xy.iter().map(|p| [p[0] / sd * 1e-4, p[1] / sd * 1e-4]).collect();
                xy = crate::tsne::tsne(&embs, &init, 30f64.min((n - 1) as f64 / 3.0), 1000);
            }
            for d in 0..2 {
                let lo = xy.iter().map(|p| p[d]).fold(f64::INFINITY, f64::min);
                let hi = xy.iter().map(|p| p[d]).fold(f64::NEG_INFINITY, f64::max);
                let span = if hi - lo > 0.0 { hi - lo } else { 1.0 };
                for p in xy.iter_mut() {
                    p[d] = (p[d] - lo) / span;
                }
            }
        }
        let out = Value::Array(
            sounds
                .iter()
                .zip(&xy)
                .map(|(s, p)| {
                    json!({"path": s.path, "name": basename(&s.path), "x": (p[0] * 1e4).round() / 1e4, "y": (p[1] * 1e4).round() / 1e4,
                           "category": s.cat.map(|c| names[c].clone()), "family": s.cat.map(|c| crate::mapnames::family(&names[c]).to_string()), "duration": s.duration, "sr": s.sr})
                })
                .collect(),
        );
        *self.map.lock().unwrap() = Some((version, out.clone()));
        out
    }
}

/// Every embedding along the two directions in which the embeddings differ most, found by
/// power iteration. The mean is taken off inside the sums, so no centred copy is kept.
fn first_two_components(embs: &[Vec<f32>]) -> Vec<[f64; 2]> {
    let dot64 = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
    let along = |e: &[f32], v: &[f64], mean_v: f64| e.iter().zip(v).map(|(a, b)| *a as f64 * b).sum::<f64>() - mean_v;
    let n = embs.len() as f64;
    let mut mean = vec![0.0f64; EMB_DIM];
    for e in embs {
        for (m, v) in mean.iter_mut().zip(e) {
            *m += *v as f64 / n;
        }
    }
    let mut comps: Vec<Vec<f64>> = Vec::new();
    for c in 0..2 {
        let mut v: Vec<f64> = (0..EMB_DIM).map(|k| ((k * 7 + c * 13) % 11) as f64 - 5.0).collect();
        for _ in 0..60 {
            let mean_v = dot64(&mean, &v);
            let proj: Vec<f64> = embs.iter().map(|e| along(e, &v, mean_v)).collect();
            let mut w = vec![0.0f64; EMB_DIM];
            for (e, p) in embs.iter().zip(&proj) {
                for (wk, ek) in w.iter_mut().zip(e) {
                    *wk += *ek as f64 * p;
                }
            }
            let total: f64 = proj.iter().sum();
            for (wk, mk) in w.iter_mut().zip(&mean) {
                *wk -= mk * total;
            }
            for prev in &comps {
                let d = dot64(&w, prev);
                for (wk, pk) in w.iter_mut().zip(prev) {
                    *wk -= d * pk;
                }
            }
            let norm = dot64(&w, &w).sqrt().max(1e-12);
            v = w.iter().map(|a| a / norm).collect();
        }
        comps.push(v);
    }
    let mean_c = [dot64(&mean, &comps[0]), dot64(&mean, &comps[1])];
    embs.iter().map(|e| [along(e, &comps[0], mean_c[0]), along(e, &comps[1], mean_c[1])]).collect()
}
