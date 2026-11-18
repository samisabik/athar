//! The `athar://` protocol the window loads the interface from: index.html at the root and the
//! other files of app/ui under /static/. Everything else the page needs is a command.

use std::path::{Path, PathBuf};
use tauri::http::{Request, Response, StatusCode};

/// What the page may load and run: its own files, styles written inline, pictures in data
/// addresses (style.css has one), and requests to the commands. Text that reaches the page from
/// file names, tags or the archives can then never run as a script.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self' ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'";

/// Where the interface files are: in the app bundle's resources, or in the repository when run
/// from source.
pub struct Ui(pub PathBuf);

pub fn ui_dir(resources: Option<&Path>) -> PathBuf {
    // Built into the app, the interface is in its resources. Run from the build folder, it is the
    // repository's own files, not the copy the build leaves next to the binary, which is only
    // refreshed when the app is bundled.
    let in_app = std::env::current_exe().is_ok_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"));
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    if !in_app && repo.join("index.html").exists() {
        return repo;
    }
    resources.map(|r| r.join("ui")).filter(|u| u.join("index.html").exists()).unwrap_or(repo)
}

fn respond(status: StatusCode, mime: &str, body: impl Into<Vec<u8>>) -> Response<Vec<u8>> {
    let mut r = Response::builder().status(status).header("Content-Type", mime).header("Cache-Control", "no-store");
    if mime.starts_with("text/html") {
        r = r.header("Content-Security-Policy", CONTENT_SECURITY_POLICY);
    }
    r.body(body.into()).unwrap()
}

fn mime_of(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

pub fn serve(ui: &Path, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let path = req.uri().path();
    let file = match path {
        "/" | "/index.html" => "index.html",
        _ => match path.strip_prefix("/static/") {
            Some(rel) if rel.split('/').all(|part| !part.is_empty() && part != "..") => rel,
            Some(_) => return respond(StatusCode::FORBIDDEN, "text/plain", "Forbidden"),
            None => return respond(StatusCode::NOT_FOUND, "text/plain", "Not found"),
        },
    };
    match std::fs::read(ui.join(file)) {
        Ok(body) => respond(StatusCode::OK, mime_of(file), body),
        Err(_) => respond(StatusCode::NOT_FOUND, "text/plain", "Not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(path: &str) -> Response<Vec<u8>> {
        let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        serve(&ui, &Request::builder().uri(format!("athar://localhost{path}")).body(vec![]).unwrap())
    }

    #[test]
    fn the_page_and_its_files_are_served_and_nothing_outside_them() {
        let page = get("/");
        assert_eq!(page.status(), StatusCode::OK);
        assert!(page.headers()["Content-Security-Policy"].to_str().unwrap().contains("script-src 'self'"));
        let script = get("/static/main.js");
        assert_eq!(script.status(), StatusCode::OK);
        assert_eq!(script.headers()["Content-Type"], "text/javascript; charset=utf-8");
        assert_eq!(get("/static/fonts/../../Cargo.toml").status(), StatusCode::FORBIDDEN);
        assert_eq!(get("/static//etc/passwd").status(), StatusCode::FORBIDDEN);
        assert_eq!(get("/static/%2e%2e/Cargo.toml").status(), StatusCode::NOT_FOUND);
        assert_eq!(get("/Cargo.toml").status(), StatusCode::NOT_FOUND);
    }
}
