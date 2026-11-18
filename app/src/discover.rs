//! Search of free online sound archives that need no account, and download of a found sound
//! into a project. Only the search words and the chosen file's address leave the machine, and
//! requests only ever go to the archives themselves, redirects included.

use serde::Serialize;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use ureq::http::Uri;

const USER_AGENT: &str = "Athar sound browser";
/// Results per page of a search.
const PAGE: u64 = 20;
/// The largest file `download` takes.
pub const MAX_BYTES: u64 = 400 * 1024 * 1024;
/// How long an archive may take to connect and to start answering, and to send a whole answer
/// to a search.
const ANSWER: Duration = Duration::from_secs(20);
/// How long a file may take to arrive once it has started: enough for MAX_BYTES on a slow line.
const FILE_BODY: Duration = Duration::from_secs(30 * 60);
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

/// Whether an address is one of the archives, over https.
fn allowed(uri: &Uri) -> bool {
    let host = uri.host().unwrap_or("").to_lowercase();
    uri.scheme_str() == Some("https") && HOSTS.iter().any(|a| host == *a || host.ends_with(&format!(".{a}")))
}

/// Where a redirect from `from` leads: `location` may be a whole address, one without its
/// scheme (`//host/path`), a path on the same host, or a path beside the current one.
fn redirected(from: &Uri, location: &str) -> Option<Uri> {
    let host = format!("https://{}", from.authority()?);
    let target = if location.contains("://") {
        location.to_string()
    } else if location.starts_with("//") {
        format!("https:{location}")
    } else if location.starts_with('/') {
        format!("{host}{location}")
    } else {
        format!("{host}{}/{location}", from.path().rsplit_once('/').map_or("", |p| p.0))
    };
    target.parse().ok()
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn query(params: &[(&str, String)]) -> String {
    params.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&")
}

fn agent(shape: impl FnOnce(ureq::config::ConfigBuilder<ureq::typestate::AgentScope>) -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope>) -> ureq::Agent {
    let config = ureq::Agent::config_builder().max_redirects(0).max_redirects_will_error(false).http_status_as_error(false).user_agent(USER_AGENT);
    ureq::Agent::new_with_config(shape(config).build())
}

/// For searches and other short answers: the whole exchange has ANSWER to finish.
fn api_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| agent(|c| c.timeout_global(Some(ANSWER))))
}

/// For files: the archive has ANSWER to connect and to start answering, and the file itself
/// has FILE_BODY to arrive.
fn file_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| agent(|c| c.timeout_resolve(Some(ANSWER)).timeout_connect(Some(ANSWER)).timeout_send_request(Some(ANSWER)).timeout_recv_response(Some(ANSWER)).timeout_recv_body(Some(FILE_BODY))))
}

/// GET, following redirects only to another of the archives. Returns the response and the
/// address it finally came from.
fn open(agent: &ureq::Agent, url: &str) -> Result<(ureq::http::Response<ureq::Body>, Uri), String> {
    let mut uri: Uri = url.parse().map_err(|_| "That address is not one of the sound archives")?;
    for _ in 0..6 {
        if !allowed(&uri) {
            return Err("That address is not one of the sound archives".into());
        }
        let r = agent.get(&uri).call().map_err(|_| "Could not reach the archive. Check the internet connection".to_string())?;
        let status = r.status().as_u16();
        if (300..400).contains(&status) {
            let loc = r.headers().get("location").and_then(|l| l.to_str().ok()).ok_or("The archive redirected without saying where")?;
            uri = redirected(&uri, loc).filter(allowed).ok_or("The archive redirected somewhere unexpected")?;
            continue;
        }
        if status == 401 || status == 403 {
            return Err("The archive did not accept the request. If this is Freesound, check the API key in Settings".into());
        }
        if status >= 400 {
            return Err(format!("The archive answered with an error ({status}). Try again in a moment"));
        }
        return Ok((r, uri));
    }
    Err("The archive redirected too many times".into())
}

fn get_json(url: &str) -> Result<Value, String> {
    let (mut r, _) = open(api_agent(), url)?;
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

/// A sound an archive lists, as the page shows it.
#[derive(Serialize)]
struct Found {
    id: String,
    title: String,
    creator: String,
    /// In seconds, when the archive says.
    duration: Option<f64>,
    license: String,
    license_url: String,
    /// The sound's own page at the archive.
    page: String,
    /// The file to fetch; empty for an Internet Archive item, whose files are listed by `files`.
    url: String,
    /// The collection Openverse found it in.
    via: String,
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn rows(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten()
}

fn licence_or_unstated(url: &str) -> String {
    Some(cc(url)).filter(|l| !l.is_empty()).unwrap_or_else(|| "Licence not stated".into())
}

pub fn search(source: &str, q: &str, page: u64, key: &str) -> Result<Value, String> {
    let q = q.split_whitespace().collect::<Vec<_>>().join(" ");
    if q.is_empty() {
        return Ok(json!({"items": [], "more": false}));
    }
    let page = page.max(1);
    let (items, more) = match source {
        "openverse" => search_openverse(q, page)?,
        "archive" => search_archive(q, page)?,
        "freesound" => search_freesound(q, page, key.trim())?,
        _ => return Err("Unknown archive".into()),
    };
    Ok(json!({"items": items, "more": more}))
}

fn search_openverse(q: String, page: u64) -> Result<(Vec<Found>, bool), String> {
    let d = get_json(&format!("https://api.openverse.org/v1/audio/?{}", query(&[("q", q), ("page", page.to_string()), ("page_size", PAGE.to_string())])))?;
    let items = rows(&d["results"])
        .map(|r| {
            let lic = r["license"].as_str().unwrap_or("");
            let license = match lic {
                "cc0" => "CC0".to_string(),
                "pdm" => "Public domain".to_string(),
                _ => format!("CC {} {}", lic.to_uppercase(), r["license_version"].as_str().unwrap_or("")).trim().to_string(),
            };
            Found {
                id: text(&r["id"]),
                title: r["title"].as_str().unwrap_or("Untitled").into(),
                creator: text(&r["creator"]),
                duration: r["duration"].as_f64().filter(|d| *d > 0.0).map(|d| d / 1000.0),
                license,
                license_url: text(&r["license_url"]),
                page: text(&r["foreign_landing_url"]),
                url: text(&r["url"]),
                via: text(&r["source"]),
            }
        })
        .collect();
    Ok((items, page < d["page_count"].as_u64().unwrap_or(0)))
}

fn search_archive(q: String, page: u64) -> Result<(Vec<Found>, bool), String> {
    let d = get_json(&format!(
        "https://archive.org/advancedsearch.php?{}",
        query(&[
            ("q", format!("({q}) AND mediatype:audio AND NOT collection:podcasts")),
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
    let items = rows(&resp["docs"])
        .map(|r| {
            let id = text(&r["identifier"]);
            let license_url = first(&r["licenseurl"]);
            Found {
                title: Some(first(&r["title"])).filter(|t| !t.is_empty()).unwrap_or_else(|| id.clone()),
                creator: first(&r["creator"]),
                duration: None,
                license: licence_or_unstated(&license_url),
                license_url,
                page: format!("https://archive.org/details/{}", enc(&id)),
                url: String::new(),
                via: String::new(),
                id,
            }
        })
        .collect();
    Ok((items, page * PAGE < resp["numFound"].as_u64().unwrap_or(0)))
}

fn search_freesound(q: String, page: u64, key: &str) -> Result<(Vec<Found>, bool), String> {
    if key.is_empty() {
        return Err("Add your Freesound API key in Settings first".into());
    }
    let d = get_json(&format!(
        "https://freesound.org/apiv2/search/text/?{}",
        query(&[("query", q), ("page", page.to_string()), ("page_size", PAGE.to_string()), ("token", key.into()), ("fields", "id,name,username,duration,license,previews,url".into())])
    ))?;
    let items = rows(&d["results"])
        .filter_map(|r| {
            let id = r["id"].to_string();
            let license_url = text(&r["license"]);
            Some(Found {
                url: r["previews"]["preview-hq-mp3"].as_str()?.to_string(),
                title: r["name"].as_str().unwrap_or("Untitled").into(),
                creator: text(&r["username"]),
                duration: r["duration"].as_f64(),
                license: licence_or_unstated(&license_url),
                license_url,
                page: r["url"].as_str().map_or_else(|| format!("https://freesound.org/s/{id}/"), String::from),
                via: String::new(),
                id,
            })
        })
        .collect();
    Ok((items, !d["next"].is_null()))
}

/// The outline of an Openverse sound, as `bars` heights from 0 to 255.
pub fn waveform(id: &str, bars: usize) -> Value {
    if id.len() != 36 || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return json!([]);
    }
    let Ok(d) = get_json(&format!("https://api.openverse.org/v1/audio/{id}/waveform/")) else { return json!([]) };
    let pts: Vec<f64> = rows(&d["points"]).filter_map(|v| v.as_f64()).collect();
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
    let is_audio = |f: &&Value| {
        let n = f["name"].as_str().unwrap_or("").to_lowercase();
        AUDIO_EXTS.iter().any(|e| n.ends_with(&format!(".{e}")))
    };
    let audio: Vec<&Value> = rows(&d["result"]).filter(is_audio).collect();
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

/// How much of a file to fetch.
#[derive(Clone, Copy)]
pub enum Amount<'a> {
    /// All of it. A file larger than the limit in bytes is refused with the message.
    Whole(u64, &'a str),
    /// Its start, enough to preview a long file: given the file's whole size (0 when
    /// unknown), the bytes wanted.
    Start(&'a dyn Fn(u64) -> u64),
}

/// Fetches one sound whole into `dest`. The file appears under its final name only once all
/// of it has arrived.
pub fn download(url: &str, dest: &Path, name: &str, progress: &dyn Fn(u64, u64)) -> Result<PathBuf, String> {
    fetch(url, dest, name, Amount::Whole(MAX_BYTES, "That file is larger than 400 MB"), progress).map(|(p, _)| p)
}

/// Fetches `amount` of a sound into `dest` under a name made from `name`. `progress` is told
/// the bytes so far and how many are wanted (0 when unknown). Returns the file and whether it
/// holds only the start of the sound.
pub fn fetch(url: &str, dest: &Path, name: &str, amount: Amount, progress: &dyn Fn(u64, u64)) -> Result<(PathBuf, bool), String> {
    let (r, from) = open(file_agent(), url)?;
    let path_ext = from.path().rsplit('/').next().and_then(|f| f.rsplit_once('.')).map(|(_, e)| e.to_lowercase());
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
    // `stop` ends a partial fetch; `most` is where a whole one is refused.
    let (stop, most, refusal) = match amount {
        Amount::Whole(most, refusal) => (None, most, refusal),
        Amount::Start(wanted) => (Some(wanted(expected)), MAX_BYTES, ""),
    };
    if stop.is_none() && expected > most {
        return Err(refusal.into());
    }
    let want = stop.map_or(expected, |l| if expected == 0 { l } else { expected.min(l) });
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
        let mut reader = r.into_body().into_with_config().limit(most + 1).reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut got: u64 = 0;
        let mut told = std::time::Instant::now();
        loop {
            let n = reader.read(&mut buf).map_err(|_| "The connection to the archive dropped. Try again".to_string())?;
            if n == 0 {
                break;
            }
            got += n as u64;
            if got > most {
                return Err(refusal.to_string());
            }
            f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            if told.elapsed().as_millis() >= 100 {
                told = std::time::Instant::now();
                progress(got, want);
            }
            if stop.is_some_and(|l| got >= l) {
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
    use super::{cc, download, redirected, search, waveform, Uri};

    fn allowed(url: &str) -> bool {
        url.parse::<Uri>().is_ok_and(|u| super::allowed(&u))
    }

    #[test]
    fn only_archives_are_allowed() {
        assert!(allowed("https://api.openverse.org/v1/audio/"));
        assert!(allowed("https://ia800.us.archive.org/x.mp3"));
        assert!(allowed("HTTPS://Archive.ORG/x.mp3"));
        assert!(!allowed("http://archive.org/x"));
        assert!(!allowed("https://archive.org.evil.com/x"));
        assert!(!allowed("https://192.168.1.1/"));
        assert!(!allowed("https://evil.com@archive.org.evil.com/"));
        assert!(!allowed("https://evil.com/archive.org"));
        assert!(!allowed("https://evil.com#@archive.org/"));
        assert!(!allowed("https://evil.com\\@archive.org/"));
        assert!(!allowed("archive.org/x"));
    }

    #[test]
    fn a_redirect_is_followed_in_each_of_its_forms() {
        let from: Uri = "https://archive.org/download/item/a.mp3?x=1".parse().unwrap();
        let to = |loc: &str| redirected(&from, loc).map(|u| u.to_string());
        assert_eq!(to("https://ia800.us.archive.org/1/a.mp3").as_deref(), Some("https://ia800.us.archive.org/1/a.mp3"));
        assert_eq!(to("//ia800.us.archive.org/1/a.mp3").as_deref(), Some("https://ia800.us.archive.org/1/a.mp3"));
        assert_eq!(to("/1/a.mp3").as_deref(), Some("https://archive.org/1/a.mp3"));
        assert_eq!(to("files/a.mp3").as_deref(), Some("https://archive.org/download/item/files/a.mp3"));
        assert!(!redirected(&from, "http://archive.org/a.mp3").is_some_and(|u| super::allowed(&u)));
        assert!(!redirected(&from, "//evil.com/a.mp3").is_some_and(|u| super::allowed(&u)));
    }

    #[test]
    fn licence_names() {
        assert_eq!(cc("https://creativecommons.org/licenses/by-nc/4.0/"), "CC BY-NC 4.0");
        assert_eq!(cc("http://creativecommons.org/publicdomain/zero/1.0/"), "CC0");
        assert_eq!(cc("https://example.com/terms"), "");
    }

    #[test]
    fn requests_athar_cannot_make_are_refused_before_going_online() {
        assert!(download("https://example.com/a.mp3", &std::env::temp_dir(), "a", &|_, _| {}).is_err());
        assert!(download("file:///etc/passwd", &std::env::temp_dir(), "a", &|_, _| {}).is_err());
        assert_eq!(search("openverse", "   ", 1, "").unwrap(), serde_json::json!({"items": [], "more": false}));
        assert!(search("nowhere", "bell", 1, "").is_err());
        assert!(search("freesound", "bell", 1, "").is_err());
        assert_eq!(waveform("../../etc/passwd", 10), serde_json::json!([]));
    }
}
