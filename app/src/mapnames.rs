//! The names written on the Map. The map is cut into squares that halve each time the zoom
//! doubles; in each square the most common family (far out) or kind (closer in) is named at the
//! middle of its sounds, and closer in still a word the file names there share takes its place.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// The broad family each kind of sound belongs to, for the names shown zoomed out.
const FAMILIES: &[(&str, &[&str])] = &[
    ("Drums", &["Kick", "Snare", "Clap", "Hi-hat", "Cymbal", "Tom", "Percussion", "Drum loop"]),
    ("Bass", &["Bass", "808"]),
    ("Synth", &["Synth lead", "Synth pad", "Pluck", "Chord"]),
    ("Instruments", &["Keys", "Guitar", "Strings", "Brass", "Bells"]),
    ("Voice", &["Vocal", "Voices"]),
    ("FX", &["FX", "Riser", "Impact"]),
    ("Texture", &["Texture", "Room tone"]),
    ("Nature", &["Birds", "Insects", "Animals", "Water", "Rain", "Wind", "Thunder", "Fire"]),
    ("Foley", &["Foley", "Footsteps", "Doors", "Metal", "Wood", "Paper"]),
    ("Machines", &["Traffic", "Machinery"]),
];

/// Words of a file name that describe nothing: formats, tempo and the like.
const DULL: &[&str] = &["wav", "aif", "aiff", "flac", "mp3", "ogg", "m4a", "bpm", "one", "shot", "oneshot", "loop", "the", "and", "edit", "min", "maj", "sample"];

/// The family a kind belongs to; a kind outside every family is its own.
pub fn family(kind: &str) -> &str {
    FAMILIES.iter().find(|(_, kinds)| kinds.contains(&kind)).map_or(kind, |f| f.0)
}

/// The words of a name that can describe a sound: letters only, longer than two, not dull.
fn words_of(name: &str) -> Vec<String> {
    name.to_lowercase().split(|c: char| !c.is_ascii_lowercase()).filter(|w| w.len() > 2 && !DULL.contains(w)).map(String::from).collect()
}

fn unique_words(name: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    words_of(name).into_iter().filter(|w| seen.insert(w.clone())).collect()
}

struct Point<'a> {
    name: &'a str,
    category: Option<&'a str>,
    x: f64,
    y: f64,
}

/// Values by key in first-seen order, so ties go to what came first.
struct Ordered<K, V> {
    index: HashMap<K, usize>,
    items: Vec<(K, V)>,
}

impl<K, V> Default for Ordered<K, V> {
    fn default() -> Self {
        Self { index: HashMap::new(), items: vec![] }
    }
}

impl<K: std::hash::Hash + Eq + Clone, V: Default> Ordered<K, V> {
    fn new() -> Self {
        Self::default()
    }

    fn entry(&mut self, key: K) -> &mut V {
        let i = *self.index.entry(key.clone()).or_insert_with(|| {
            self.items.push((key, V::default()));
            self.items.len() - 1
        });
        &mut self.items[i].1
    }
}

/// Words in more than a third of all file names (a pack's own name, say) describe nothing.
fn common_words(points: &[Point]) -> HashSet<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for p in points {
        for w in unique_words(p.name) {
            *counts.entry(w).or_default() += 1;
        }
    }
    counts.into_iter().filter(|(_, n)| *n as f64 > points.len() as f64 / 3.0).map(|(w, _)| w).collect()
}

/// A word that most file names in a group share, if there is one, leaving out the kind's own
/// name and the words every file has.
fn shared_word(group: &[&Point], common: &HashSet<String>) -> Option<String> {
    let kind: HashSet<String> = words_of(group[0].category.unwrap_or("")).into_iter().collect();
    let mut counts: Ordered<String, usize> = Ordered::new();
    for p in group {
        for w in unique_words(p.name) {
            if !common.contains(&w) && !kind.contains(&w) {
                *counts.entry(w) += 1;
            }
        }
    }
    let best = counts.items.iter().fold(None::<&(String, usize)>, |b, c| if b.is_none_or(|b| c.1 > b.1) { Some(c) } else { b })?;
    (best.1 >= 2 && best.1 as f64 >= group.len() as f64 * 0.5).then(|| best.0.clone())
}

/// The names to show at zoom `level` (0 to 5), biggest group first, in map units:
/// `[{ key, text, n, x, y }]`, where key is `g:` and a family or `c:` and a kind.
pub fn names(map: &Value, level: u32) -> Value {
    let level = level.min(5);
    let points: Vec<Point> = map
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| Point { name: p["name"].as_str().unwrap_or(""), category: p["category"].as_str(), x: p["x"].as_f64().unwrap_or(0.0), y: p["y"].as_f64().unwrap_or(0.0) })
        .collect();
    let cell = 0.26 / 2f64.powi(level as i32);
    let broad = level == 0;
    let common = if level >= 2 { common_words(&points) } else { HashSet::new() };
    // Squares in first-seen order, each with its groups in first-seen order.
    let mut cells: Ordered<(i64, i64), Ordered<String, Vec<&Point>>> = Ordered::new();
    for p in &points {
        let Some(kind) = p.category else { continue };
        let key = if broad { format!("g:{}", family(kind)) } else { format!("c:{kind}") };
        let id = ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
        cells.entry(id).entry(key).push(p);
    }
    let least = if broad { 3 } else { 2 };
    let mut all: Vec<(String, String, usize, f64, f64)> = vec![];
    for (_, groups) in &cells.items {
        let groups = &groups.items;
        let (key, group) = groups.iter().fold(&groups[0], |b, g| if g.1.len() > b.1.len() { g } else { b });
        if group.len() < least {
            continue;
        }
        let text = match if level >= 2 { shared_word(group, &common) } else { None } {
            Some(w) => w[..1].to_uppercase() + &w[1..],
            None => key[2..].to_string(),
        };
        let n = group.len();
        let x = group.iter().map(|p| p.x).sum::<f64>() / n as f64;
        let y = group.iter().map(|p| p.y).sum::<f64>() / n as f64;
        all.push((key.clone(), text, n, x, y));
    }
    // The same name in neighbouring squares is one group cut in two: the bigger half keeps it.
    all.sort_by(|a, b| b.2.cmp(&a.2));
    let mut list: Vec<&(String, String, usize, f64, f64)> = vec![];
    for l in &all {
        if !list.iter().any(|k| k.1 == l.1 && (k.3 - l.3).abs() < cell * 1.2 && (k.4 - l.4).abs() < cell * 1.2) {
            list.push(l);
        }
    }
    Value::Array(list.into_iter().map(|(key, text, n, x, y)| json!({"key": key, "text": text, "n": n, "x": x, "y": y})).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(name: &str, cat: &str, x: f64, y: f64) -> Value {
        json!({"name": name, "category": cat, "x": x, "y": y})
    }

    #[test]
    fn far_out_a_square_is_named_after_its_family() {
        let map = json!([pt("a.wav", "Kick", 0.1, 0.1), pt("b.wav", "Snare", 0.11, 0.1), pt("c.wav", "Kick", 0.12, 0.11), pt("d.wav", "Rain", 0.9, 0.9)]);
        let n = names(&map, 0);
        assert_eq!(n.as_array().unwrap().len(), 1);
        assert_eq!(n[0]["text"], "Drums");
        assert_eq!(n[0]["n"], 3);
    }

    #[test]
    fn closer_in_a_shared_word_names_the_group() {
        let map = json!([pt("Dusty Kick 1.wav", "Kick", 0.01, 0.01), pt("Dusty Kick 2.wav", "Kick", 0.02, 0.01), pt("Dusty Kick 3.wav", "Kick", 0.015, 0.02),
            pt("Snare 1.wav", "Snare", 0.8, 0.8), pt("Snare 2.wav", "Snare", 0.81, 0.8), pt("Snare 3.wav", "Snare", 0.8, 0.81),
            pt("Pad.wav", "Synth pad", 0.5, 0.5), pt("Glass.wav", "FX", 0.3, 0.7), pt("Tape.wav", "Texture", 0.7, 0.3), pt("Wood.wav", "Wood", 0.3, 0.3)]);
        let n = names(&map, 2);
        assert_eq!(n[0]["text"], "Dusty");
        assert_eq!(n[1]["text"], "Snare");
    }

    #[test]
    fn dull_and_common_words_name_nothing() {
        assert_eq!(words_of("Kick_120bpm_one shot.wav"), vec!["kick".to_string()]);
        let map = json!([pt("Pack Rain 1.wav", "Rain", 0.1, 0.1), pt("Pack Wind 2.wav", "Wind", 0.5, 0.5), pt("Pack Fire.wav", "Fire", 0.9, 0.9)]);
        let points: Vec<Point> = map.as_array().unwrap().iter().map(|p| Point { name: p["name"].as_str().unwrap(), category: p["category"].as_str(), x: 0.0, y: 0.0 }).collect();
        assert!(common_words(&points).contains("pack"));
    }
}
