"""Search of free online sound archives that need no account or key, and download of a
found sound into the library. Only the search words and the chosen file's address leave the
machine."""
import http.client
import json
import os
import re
import ssl
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

USER_AGENT = "Athar sound browser"
TIMEOUT = 20
PAGE = 20
MAX_BYTES = 400 * 1024 * 1024
SYSTEM_CERTS = "/etc/ssl/cert.pem"
AUDIO_EXTS = (".wav", ".aif", ".aiff", ".flac", ".mp3", ".ogg", ".m4a")
TYPES = {"audio/mpeg": ".mp3", "audio/mp3": ".mp3", "audio/wav": ".wav", "audio/x-wav": ".wav", "audio/flac": ".flac",
         "audio/ogg": ".ogg", "audio/mp4": ".m4a", "audio/x-m4a": ".m4a", "audio/aiff": ".aiff", "audio/x-aiff": ".aiff"}
# Files and pages are only fetched or opened from the archives themselves.
HOSTS = ("openverse.org", "freesound.org", "archive.org", "wikimedia.org", "jamendo.com", "ccmixter.org")

SOURCES = [
    {"id": "openverse", "name": "Openverse", "about": "Creative Commons sounds from Freesound, Wikimedia and others"},
    {"id": "archive", "name": "Internet Archive", "about": "Field recordings, old radio, netlabels. Licences vary"},
]


class DiscoverError(Exception):
    pass


def _tls():
    """Certificates for the archives. The packaged app carries a Python whose usual certificate
    file may not exist on this Mac, so the one macOS ships is used when that is the case."""
    ctx = ssl.create_default_context()
    paths = ssl.get_default_verify_paths()
    if not (paths.cafile and os.path.exists(paths.cafile)) and os.path.exists(SYSTEM_CERTS):
        ctx.load_verify_locations(SYSTEM_CERTS)
    return ctx


def allowed(url):
    try:
        u = urllib.parse.urlsplit(url)
        host = (u.hostname or "").lower()
    except ValueError:
        return False
    return u.scheme == "https" and any(host == h or host.endswith("." + h) for h in HOSTS)


class _ArchivesOnly(urllib.request.HTTPRedirectHandler):
    """A redirect is followed only to another of the archives, so a link on an allowed site
    cannot send a request anywhere else, such as to a machine on the local network."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not allowed(newurl):
            raise DiscoverError("The archive redirected somewhere unexpected")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


# Everything that can go wrong on the wire, at connect time or halfway through a body.
NETWORK_ERRORS = (urllib.error.URLError, http.client.HTTPException, TimeoutError, OSError)


def _open(url):
    if not allowed(url):
        raise DiscoverError("That address is not one of the sound archives")
    opener = urllib.request.build_opener(urllib.request.HTTPSHandler(context=_tls()), _ArchivesOnly())
    try:
        return opener.open(urllib.request.Request(url, headers={"User-Agent": USER_AGENT}), timeout=TIMEOUT)
    except urllib.error.HTTPError as e:
        if e.code in (401, 403):
            raise DiscoverError("The archive did not accept the request. If this is Freesound, check the API key in Settings") from e
        raise DiscoverError(f"The archive answered with an error ({e.code}). Try again in a moment") from e
    except NETWORK_ERRORS + (ValueError,) as e:
        raise DiscoverError("Could not reach the archive. Check the internet connection") from e


def _json(url):
    try:
        with _open(url) as r:
            return json.loads(r.read(8 * 1024 * 1024))
    except ValueError as e:
        raise DiscoverError("The archive sent an answer Athar could not read") from e
    except NETWORK_ERRORS as e:
        raise DiscoverError("The connection to the archive dropped. Try again") from e


def _q(**params):
    return urllib.parse.urlencode({k: v for k, v in params.items() if v is not None}, doseq=True)


def _cc(url):
    """A short licence name from a Creative Commons address."""
    m = re.search(r"creativecommons\.org/(licenses|publicdomain)/([a-z\-]+)(?:/(\d\.\d))?", url or "")
    if not m:
        return ""
    kind, name, ver = m.groups()
    if kind == "publicdomain":
        return "CC0" if name == "zero" else "Public domain"
    return f"CC {name.upper()}" + (f" {ver}" if ver else "")


def _first(v):
    return (v[0] if v else "") if isinstance(v, list) else (v or "")


def _openverse(query, page):
    d = _json("https://api.openverse.org/v1/audio/?" + _q(q=query, page=page, page_size=PAGE))
    out = []
    for r in d.get("results", []):
        lic = r.get("license") or ""
        name = {"cc0": "CC0", "pdm": "Public domain"}.get(lic, f"CC {lic.upper()} {r.get('license_version') or ''}".strip())
        out.append({
            "id": r["id"], "title": r.get("title") or "Untitled", "creator": r.get("creator") or "",
            "duration": (r.get("duration") or 0) / 1000 or None, "license": name, "license_url": r.get("license_url") or "",
            "page": r.get("foreign_landing_url") or "", "url": r.get("url") or "", "via": r.get("source") or "",
        })
    return {"items": out, "more": page < (d.get("page_count") or 0)}


def _archive(query, page):
    q = f"({query}) AND mediatype:audio AND NOT collection:podcasts"
    d = _json("https://archive.org/advancedsearch.php?" + _q(
        q=q, rows=PAGE, page=page, output="json", **{"fl[]": ["identifier", "title", "creator", "licenseurl"], "sort[]": "downloads desc"}))
    resp = d.get("response", {})
    out = [{
        "id": r["identifier"], "title": _first(r.get("title")) or r["identifier"], "creator": _first(r.get("creator")),
        "duration": None, "license": _cc(_first(r.get("licenseurl"))) or "Licence not stated",
        "license_url": _first(r.get("licenseurl")), "page": f"https://archive.org/details/{urllib.parse.quote(r['identifier'])}",
        "url": "", "via": "",
    } for r in resp.get("docs", [])]
    return {"items": out, "more": page * PAGE < (resp.get("numFound") or 0)}


FREESOUND = {"id": "freesound", "name": "Freesound", "about": "The largest sample archive. Uses your own API key from Settings"}


def _freesound(query, page, key):
    """Freesound's own search. With an API key (no login) only the previews can be fetched,
    so an added sound is the high-quality MP3 preview, not the original upload."""
    d = _json("https://freesound.org/apiv2/search/text/?" + _q(
        query=query, page=page, page_size=PAGE, token=key, fields="id,name,username,duration,license,previews,url"))
    out = []
    for r in d.get("results", []):
        url = (r.get("previews") or {}).get("preview-hq-mp3") or ""
        if not url:
            continue
        out.append({
            "id": str(r["id"]), "title": r.get("name") or "Untitled", "creator": r.get("username") or "",
            "duration": r.get("duration") or None, "license": _cc(r.get("license")) or "Licence not stated",
            "license_url": r.get("license") or "", "page": r.get("url") or f"https://freesound.org/s/{r['id']}/",
            "url": url, "via": "",
        })
    return {"items": out, "more": bool(d.get("next"))}


def waveform(ident, bars=48):
    """The outline of an Openverse sound, as `bars` heights from 0 to 255."""
    if not re.fullmatch(r"[0-9a-f\-]{36}", ident or ""):
        raise DiscoverError("Unknown sound")
    pts = _json(f"https://api.openverse.org/v1/audio/{ident}/waveform/").get("points") or []
    if not pts:
        return []
    top = max(pts) or 1
    out = []
    for i in range(bars):
        a, b = i * len(pts) // bars, max(i * len(pts) // bars + 1, (i + 1) * len(pts) // bars)
        out.append(int(255 * max(pts[a:b]) / top))
    return out


def search(source, query, page=1, key=""):
    query = " ".join((query or "").split())
    if not query:
        return {"items": [], "more": False}
    page = max(1, int(page))
    if source == "freesound":
        if not key:
            raise DiscoverError("Add your Freesound API key in Settings first")
        return _freesound(query, page, key)
    fn = {"openverse": _openverse, "archive": _archive}.get(source)
    if fn is None:
        raise DiscoverError("Unknown archive")
    return fn(query, page)


def _seconds(v):
    try:
        if isinstance(v, str) and ":" in v:
            s = 0.0
            for part in v.split(":"):
                s = s * 60 + float(part)
            return s
        return float(v)
    except (TypeError, ValueError):
        return None


def files(source, ident):
    """The audio files inside an Internet Archive item."""
    if source == "archive":
        rows = _json(f"https://archive.org/metadata/{urllib.parse.quote(ident)}/files").get("result", [])
        audio = [f for f in rows if f.get("name", "").lower().endswith(AUDIO_EXTS)]
        own = [f for f in audio if f.get("source") == "original"]
        return [{
            "name": f["name"], "url": f"https://archive.org/download/{urllib.parse.quote(ident)}/{urllib.parse.quote(f['name'])}",
            "size": int(f["size"]) if str(f.get("size", "")).isdigit() else None, "duration": _seconds(f.get("length")),
        } for f in (own or audio)[:200]]
    raise DiscoverError("Unknown archive")


def download(url, dest_dir, name):
    """Fetch one sound into dest_dir. Returns the path it was written to. The file appears
    under its final name only once all of it has arrived."""
    from .library import safe_name, unique_path
    tmp = None
    try:
        with _open(url) as r:
            ext = Path(urllib.parse.unquote(urllib.parse.urlsplit(r.geturl()).path)).suffix.lower()
            if ext not in AUDIO_EXTS:
                ext = TYPES.get((r.headers.get("Content-Type") or "").split(";")[0].strip().lower())
            if not ext:
                raise DiscoverError("That file is not a sound Athar can read")
            expected = int(r.headers.get("Content-Length") or 0)
            if expected > MAX_BYTES:
                raise DiscoverError("That file is larger than 400 MB")
            stem = safe_name(Path(name).stem if Path(name).suffix.lower() in AUDIO_EXTS else name)[:120]
            dest = Path(dest_dir)
            dest.mkdir(parents=True, exist_ok=True)
            # A private temporary file, so two downloads with the same title cannot write into each other.
            fd, tmp = tempfile.mkstemp(dir=dest, prefix=".athar-", suffix=".part")
            got = 0
            with os.fdopen(fd, "wb") as f:
                while chunk := r.read(256 * 1024):
                    got += len(chunk)
                    if got > MAX_BYTES:
                        raise DiscoverError("That file is larger than 400 MB")
                    f.write(chunk)
            # A dropped connection ends the reading early without an error, so the size is checked.
            if not got or (expected and got != expected):
                raise DiscoverError("The download stopped before the whole file arrived. Try again")
            for _ in range(50):
                target = unique_path(dest / f"{stem}{ext}")
                try:
                    os.link(tmp, target)
                    return str(target)
                except FileExistsError:
                    continue
                except OSError:
                    # A drive that cannot link (exFAT, some network shares): move the file instead.
                    os.replace(tmp, target)
                    tmp = None
                    return str(target)
            raise DiscoverError("Could not find a free name for the file")
    except NETWORK_ERRORS as e:
        raise DiscoverError("The connection to the archive dropped. Try again") from e
    finally:
        if tmp:
            Path(tmp).unlink(missing_ok=True)
