//! Search of free online sound archives that need no account, and download of a found sound
//! into a project. Only the search words and the chosen file's address leave the machine, and
//! requests only ever go to the archives themselves, redirects included.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const USER_AGENT: &str = "Athar sound browser";
const PAGE: u64 = 20;
const MAX_BYTES: u64 = 400 * 1024 * 1024;
const AUDIO_EXTS: [&str; 7] = ["wav", "aif", "aiff", "flac", "mp3", "ogg", "m4a"];
const HOSTS: [&str; 6] = ["openverse.org", "freesound.org", "archive.org", "wikimedia.org", "jamendo.com", "ccmixter.org"];

pub fn sources(freesound_key: &str) -> Value {
    let mut v = vec![
        json!({"id": "openverse", "name": "Openverse", "about": "Creative Commons sounds from Freesound, Wikimedia and others"}),
        json!({"id": "archive", "name": "Internet Archive", "about": "Field recordings, old radio, netlabels. Licences vary"}),
    ];
    if !freesound_key.trim().is_empty() {
        v.push(json!({"id": "freesound", "name": "Freesound", "about": "The largest sample archive. Uses your own API key from Settings"}));
    }
    Value::Array(v)
}

fn host_of(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let auth = rest.split(['/', '?', '#']).next()?;
    let host = auth.rsplit('@').next()?.split(':').next()?.to_lowercase();
    Some((scheme.to_lowercase(), host))
}

pub fn allowed(url: &str) -> bool {
    host_of(url).is_some_and(|(s, h)| s == "https" && HOSTS.iter().any(|a| h == *a || h.ends_with(&format!(".{a}"))))
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn query(params: &[(&str, String)]) -> String {
    params.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&")
}

fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .max_redirects(0)
            .max_redirects_will_error(false)
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .build(),
    )
}

/// GET, following redirects only to another of the archives. Returns the response and the
/// address it finally came from.
fn open(url: &str) -> Result<(ureq::http::Response<ureq::Body>, String), String> {
    let mut url = url.to_string();
    for _ in 0..6 {
        if !allowed(&url) {
            return Err("That address is not one of the sound archives".into());
        }
        let r = agent().get(&url).call().map_err(|_| "Could not reach the archive. Check the internet connection".to_string())?;
        let status = r.status().as_u16();
        if (300..400).contains(&status) {
            let loc = r.headers().get("location").and_then(|l| l.to_str().ok()).ok_or("The archive redirected without saying where")?;
            url = if loc.starts_with("http") { loc.to_string() } else { format!("https://{}{loc}", host_of(&url).map(|h| h.1).unwrap_or_default()) };
            if !allowed(&url) {
                return Err("The archive redirected somewhere unexpected".into());
            }
            continue;
        }
        if status == 401 || status == 403 {
            return Err("The archive did not accept the request. If this is Freesound, check the API key in Settings".into());
        }
        if status >= 400 {
            return Err(format!("The archive answered with an error ({status}). Try again in a moment"));
        }
        return Ok((r, url));
    }
    Err("The archive redirected too many times".into())
}

fn get_json(url: &str) -> Result<Value, String> {
    let (mut r, _) = open(url)?;
    r.body_mut().with_config().limit(8 * 1024 * 1024).read_json::<Value>().map_err(|_| "The archive sent an answer Athar could not read".into())
}

/// A short licence name from a Creative Commons address.
fn cc(url: &str) -> String {
    let Some(i) = url.find("creativecommons.org/") else { return String::new() };
    let parts: Vec<&str> = url[i + 20..].split('/').collect();
    match parts.as_slice() {
        ["publicdomain", name, ..] => if *name == "zero" { "CC0".into() } else { "Public domain".into() },
        ["licenses", name, ver, ..] if ver.contains('.') => format!("CC {} {ver}", name.to_uppercase()),
        ["licenses", name, ..] => format!("CC {}", name.to_uppercase()),
        _ => String::new(),
    }
}

fn first(v: &Value) -> String {
    match v {
        Value::Array(a) => a.first().and_then(|x| x.as_str()).unwrap_or("").into(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

pub fn search(source: &str, q: &str, page: u64, key: &str) -> Result<Value, String> {
    let q = q.split_whitespace().collect::<Vec<_>>().join(" ");
    if q.is_empty() {
        return Ok(json!({"items": [], "more": false}));
    }
    let page = page.max(1);
    match source {
        "openverse" => {
            let d = get_json(&format!("https://api.openverse.org/v1/audio/?{}", query(&[("q", q), ("page", page.to_string()), ("page_size", PAGE.to_string())])))?;
            let items: Vec<Value> = d["results"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|r| {
                    let lic = r["license"].as_str().unwrap_or("");
                    let name = match lic {
                        "cc0" => "CC0".to_string(),
                        "pdm" => "Public domain".to_string(),
                        _ => format!("CC {} {}", lic.to_uppercase(), r["license_version"].as_str().unwrap_or("")).trim().to_string(),
                    };
                    json!({"id": r["id"], "title": r["title"].as_str().unwrap_or("Untitled"), "creator": r["creator"].as_str().unwrap_or(""),
                           "duration": r["duration"].as_f64().filter(|d| *d > 0.0).map(|d| d / 1000.0), "license": name,
                           "license_url": r["license_url"].as_str().unwrap_or(""), "page": r["foreign_landing_url"].as_str().unwrap_or(""),
                           "url": r["url"].as_str().unwrap_or(""), "via": r["source"].as_str().unwrap_or("")})
                })
                .collect();
            Ok(json!({"items": items, "more": page < d["page_count"].as_u64().unwrap_or(0)}))
        }
        "archive" => {
            let qq = format!("({q}) AND mediatype:audio AND NOT collection:podcasts");
            let d = get_json(&format!(
                "https://archive.org/advancedsearch.php?{}",
                query(&[
                    ("q", qq),
                    ("rows", PAGE.to_string()),
                    ("page", page.to_string()),
                    ("output", "json".into()),
                    ("fl[]", "identifier".into()),
                    ("fl[]", "title".into()),
                    ("fl[]", "creator".into()),
                    ("fl[]", "licenseurl".into()),
                    ("sort[]", "downloads desc".into()),
                ])
            ))?;
            let resp = &d["response"];
            let items: Vec<Value> = resp["docs"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|r| {
                    let id = r["identifier"].as_str().unwrap_or("").to_string();
                    let lic = first(&r["licenseurl"]);
                    let title = first(&r["title"]);
                    let license = if cc(&lic).is_empty() { "Licence not stated".to_string() } else { cc(&lic) };
                    json!({"id": id, "title": if title.is_empty() { id.clone() } else { title }, "creator": first(&r["creator"]),
                           "duration": null, "license": license, "license_url": lic,
                           "page": format!("https://archive.org/details/{}", enc(&id)), "url": "", "via": ""})
                })
                .collect();
            Ok(json!({"items": items, "more": page * PAGE < resp["numFound"].as_u64().unwrap_or(0)}))
        }
        "freesound" => {
            if key.trim().is_empty() {
                return Err("Add your Freesound API key in Settings first".into());
            }
            let d = get_json(&format!(
                "https://freesound.org/apiv2/search/text/?{}",
                query(&[("query", q), ("page", page.to_string()), ("page_size", PAGE.to_string()), ("token", key.trim().into()), ("fields", "id,name,username,duration,license,previews,url".into())])
            ))?;
            let items: Vec<Value> = d["results"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|r| {
                    let url = r["previews"]["preview-hq-mp3"].as_str()?.to_string();
                    let id = r["id"].to_string();
                    let lic = r["license"].as_str().unwrap_or("");
                    Some(json!({"id": id, "title": r["name"].as_str().unwrap_or("Untitled"), "creator": r["username"].as_str().unwrap_or(""),
                                "duration": r["duration"].as_f64(), "license": if cc(lic).is_empty() { "Licence not stated".to_string() } else { cc(lic) },
                                "license_url": lic, "page": r["url"].as_str().map(String::from).unwrap_or(format!("https://freesound.org/s/{id}/")),
                                "url": url, "via": ""}))
                })
                .collect();
            Ok(json!({"items": items, "more": !d["next"].is_null()}))
        }
        _ => Err("Unknown archive".into()),
    }
}

/// The outline of an Openverse sound, as `bars` heights from 0 to 255.
pub fn waveform(id: &str, bars: usize) -> Value {
    if id.len() != 36 || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return json!([]);
    }
    let Ok(d) = get_json(&format!("https://api.openverse.org/v1/audio/{id}/waveform/")) else { return json!([]) };
    let pts: Vec<f64> = d["points"].as_array().cloned().unwrap_or_default().iter().filter_map(|v| v.as_f64()).collect();
    if pts.is_empty() {
        return json!([]);
    }
    let top = pts.iter().cloned().fold(0.0, f64::max);
    let top = if top == 0.0 { 1.0 } else { top };
    let n = pts.len();
    Value::Array(
        (0..bars)
            .map(|i| {
                let a = i * n / bars;
                let b = (a + 1).max((i + 1) * n / bars).min(n);
                json!((255.0 * pts[a..b].iter().cloned().fold(0.0, f64::max) / top) as i64)
            })
            .collect(),
    )
}

fn seconds(v: &Value) -> Option<f64> {
    match v {
        Value::String(s) if s.contains(':') => s.split(':').try_fold(0.0, |acc, p| p.parse::<f64>().ok().map(|x| acc * 60.0 + x)),
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

/// The audio files inside an Internet Archive item.
pub fn files(source: &str, id: &str) -> Result<Value, String> {
    if source != "archive" {
        return Err("Unknown archive".into());
    }
    let d = get_json(&format!("https://archive.org/metadata/{}/files", enc(id)))?;
    let rows = d["result"].as_array().cloned().unwrap_or_default();
    let is_audio = |f: &Value| {
        let n = f["name"].as_str().unwrap_or("").to_lowercase();
        AUDIO_EXTS.iter().any(|e| n.ends_with(&format!(".{e}")))
    };
    let audio: Vec<&Value> = rows.iter().filter(|f| is_audio(f)).collect();
    let own: Vec<&Value> = audio.iter().copied().filter(|f| f["source"] == "original").collect();
    let pick = if own.is_empty() { audio } else { own };
    Ok(Value::Array(
        pick.iter()
            .take(200)
            .map(|f| {
                let name = f["name"].as_str().unwrap_or("");
                json!({"name": name, "url": format!("https://archive.org/download/{}/{}", enc(id), name.split('/').map(enc).collect::<Vec<_>>().join("/")),
                       "size": f["size"].as_str().and_then(|s| s.parse::<u64>().ok()), "duration": seconds(&f["length"])})
            })
            .collect(),
    ))
}

/// Fetch one sound into dest. The file appears under its final name only once all of it has
/// arrived.
pub fn download(url: &str, dest: &Path, name: &str, progress: &dyn Fn(u64, u64)) -> Result<PathBuf, String> {
    download_part(url, dest, name, None, progress).map(|(p, _)| p)
}

/// How much of a file to fetch: given its whole size (0 when unknown), the bytes wanted.
pub type Limit<'a> = &'a dyn Fn(u64) -> u64;

/// Like `download`, but with `limit` it stops after that many bytes: enough of a long file to
/// preview its start. `progress` is told the bytes so far and the whole size (0 when unknown).
/// Returns the file and whether it holds only the start of the sound.
pub fn download_part(url: &str, dest: &Path, name: &str, limit: Option<Limit>, progress: &dyn Fn(u64, u64)) -> Result<(PathBuf, bool), String> {
    let (r, final_url) = open(url)?;
    let path_ext = final_url.split(['?', '#']).next().unwrap_or("").rsplit('/').next().and_then(|f| f.rsplit_once('.')).map(|(_, e)| e.to_lowercase());
    let ext = match path_ext.filter(|e| AUDIO_EXTS.contains(&e.as_str())) {
        Some(e) => e,
        None => match r.headers().get("content-type").and_then(|h| h.to_str().ok()).unwrap_or("").split(';').next().unwrap_or("").trim() {
            "audio/mpeg" | "audio/mp3" => "mp3".into(),
            "audio/wav" | "audio/x-wav" => "wav".into(),
            "audio/flac" => "flac".into(),
            "audio/ogg" => "ogg".into(),
            "audio/mp4" | "audio/x-m4a" => "m4a".into(),
            "audio/aiff" | "audio/x-aiff" => "aiff".into(),
            _ => return Err("That file is not a sound Athar can read".into()),
        },
    };
    let expected: u64 = r.headers().get("content-length").and_then(|h| h.to_str().ok()).and_then(|s| s.parse().ok()).unwrap_or(0);
    if expected > MAX_BYTES && limit.is_none() {
        return Err("That file is larger than 400 MB".into());
    }
    let limit = limit.map(|f| f(expected));
    let want = limit.map_or(expected, |l| if expected == 0 { l } else { expected.min(l) });
    let mut partial = false;
    let stem_src = match name.rsplit_once('.') {
        Some((s, e)) if AUDIO_EXTS.contains(&e.to_lowercase().as_str()) => s,
        _ => name,
    };
    let stem: String = crate::library::safe_name(stem_src).chars().take(120).collect();
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let tmp = dest.join(format!(".athar-{}-{}.part", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut reader = r.into_body().into_with_config().limit(MAX_BYTES + 1).reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut got: u64 = 0;
        let mut told = std::time::Instant::now();
        loop {
            let n = reader.read(&mut buf).map_err(|_| "The connection to the archive dropped. Try again".to_string())?;
            if n == 0 {
                break;
            }
            got += n as u64;
            if got > MAX_BYTES {
                return Err("That file is larger than 400 MB".to_string());
            }
            f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            if told.elapsed().as_millis() >= 100 {
                told = std::time::Instant::now();
                progress(got, want);
            }
            if limit.is_some_and(|l| got >= l) {
                partial = expected == 0 || got < expected;
                break;
            }
        }
        progress(got, want.max(got));
        if got == 0 || (!partial && expected != 0 && got != expected) {
            return Err("The download stopped before the whole file arrived. Try again".to_string());
        }
        let target = crate::library::unique_path(&dest.join(format!("{stem}.{ext}")))?;
        std::fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
        Ok((target, partial))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_archives_are_allowed() {
        assert!(super::allowed("https://api.openverse.org/v1/audio/"));
        assert!(super::allowed("https://ia800.us.archive.org/x.mp3"));
        assert!(!super::allowed("http://archive.org/x"));
        assert!(!super::allowed("https://archive.org.evil.com/x"));
        assert!(!super::allowed("https://192.168.1.1/"));
        assert!(!super::allowed("https://evil.com@archive.org.evil.com/"));
    }

    #[test]
    fn licence_names() {
        assert_eq!(super::cc("https://creativecommons.org/licenses/by-nc/4.0/"), "CC BY-NC 4.0");
        assert_eq!(super::cc("http://creativecommons.org/publicdomain/zero/1.0/"), "CC0");
    }
}
