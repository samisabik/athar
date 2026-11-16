import io
import mimetypes
import os
import secrets
import sqlite3
import subprocess
from contextlib import asynccontextmanager
from pathlib import Path

import soundfile as sf
from fastapi import FastAPI, File, Form, HTTPException, UploadFile
from fastapi.responses import FileResponse, HTMLResponse, PlainTextResponse, RedirectResponse, Response
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel
from starlette.middleware.trustedhost import TrustedHostMiddleware

from . import discover
from . import engine as eng
from .library import SAVE_FORMATS, Library, ascii_name, is_inside, safe_name, sp404_card, sp404_dir, to_sp404, transcode, unique_path
from . import __version__

STATIC = Path(__file__).parent / "static"
PREVIEW_SR = 48000
PREVIEW_MAX_SECONDS = 600
TOKEN = secrets.token_urlsafe(32)
mimetypes.add_type("audio/aiff", ".aif")
mimetypes.add_type("audio/aiff", ".aiff")
mimetypes.add_type("audio/flac", ".flac")

lib: Library | None = None


def library():
    global lib
    if lib is None:
        lib = Library()
    return lib


@asynccontextmanager
async def lifespan(app):
    library()
    yield


app = FastAPI(title="Athar", lifespan=lifespan, docs_url=None, redoc_url=None, openapi_url=None)
app.mount("/static", StaticFiles(directory=STATIC), name="static")


def _matches(value, expected):
    return secrets.compare_digest(value.encode(), expected.encode())


@app.middleware("http")
async def guard(request, call_next):
    # Cookies are shared across ports on 127.0.0.1, so the name carries the port.
    cookie = f"athar_{request.url.port}"
    if not _matches(request.cookies.get(cookie, ""), TOKEN):
        if request.url.path == "/" and _matches(request.query_params.get("token", ""), TOKEN):
            response = RedirectResponse("/", status_code=303)
            response.set_cookie(cookie, TOKEN, httponly=True, samesite="strict")
            return response
        return PlainTextResponse("Forbidden", status_code=403)
    response = await call_next(request)
    # The UI files change between launches; make the webview revalidate them every time.
    if request.url.path == "/" or request.url.path.startswith("/static/"):
        response.headers["Cache-Control"] = "no-cache"
    return response


app.add_middleware(TrustedHostMiddleware, allowed_hosts=["127.0.0.1", "localhost"])


def need_engine():
    L = library()
    if not L.ready.is_set():
        raise HTTPException(503, "Model is still loading")
    return L


def known_path(path):
    L = library()
    if not L.q("SELECT 1 FROM files WHERE path=?", (path,)):
        real = os.path.realpath(path)
        if not any(is_inside(real, os.path.realpath(f)) for f in L.folders()):
            raise HTTPException(403, "Path is outside your library folders")
    if not os.path.isfile(path):
        raise HTTPException(404, "File not found")
    return path


class Filters(BaseModel):
    category: str | None = None
    min_dur: float | None = None
    folder: str | None = None
    under: str | None = None
    project: int | None = None
    tag: str | None = None
    min_rating: int | None = None
    sort: str = "score"
    limit: int = 200


class SearchReq(Filters):
    query: str | None = None
    similar_to: str | None = None


@app.get("/", response_class=HTMLResponse)
def index():
    return (STATIC / "index.html").read_text(encoding="utf-8")


@app.get("/api/status")
def status():
    L = library()
    return {**L.status, "indexed": L.count(), "pending_jobs": L.jobs.qsize(),
            "device": L.engine.device if L.engine else None}


@app.get("/api/sp404-card")
def find_sp404_card():
    card = sp404_card()
    if not card:
        return {"card": None}
    return {"card": card, "import": os.path.join(card, "IMPORT")}


@app.get("/api/settings")
def get_settings():
    L = library()
    return {**L.settings(), "_data_dir": str(Path(L.db_path).parent), "_version": __version__,
            "_preview_max_s": PREVIEW_MAX_SECONDS}


@app.post("/api/settings")
def set_settings(changes: dict):
    try:
        return library().update_settings(changes)
    except (TypeError, ValueError) as e:
        raise HTTPException(400, f"Invalid setting: {e}")


@app.post("/api/reveal-data")
def reveal_data():
    subprocess.Popen(["open", str(Path(library().db_path).parent)])
    return {"ok": True}


class PathReq(BaseModel):
    path: str


class CatalogReq(BaseModel):
    path: str
    tags: list[str] | None = None
    memo: str | None = None
    rating: int | None = None


@app.post("/api/catalog")
def catalog_set(req: CatalogReq):
    known_path(req.path)
    return library().catalog_set(req.path, req.model_dump(exclude={"path"}, exclude_none=True))


@app.get("/api/tags")
def tags():
    return library().all_tags()


@app.post("/api/trash")
def trash(req: PathReq):
    known_path(req.path)
    try:
        library().trash(req.path)
    except OSError as e:
        raise HTTPException(400, str(e))
    return {"ok": True}


class RenameReq(BaseModel):
    path: str
    name: str


@app.post("/api/rename")
def rename(req: RenameReq):
    known_path(req.path)
    try:
        return {"path": library().rename(req.path, req.name)}
    except (FileExistsError, ValueError, FileNotFoundError) as e:
        raise HTTPException(400, str(e))


@app.get("/api/folders")
def folders():
    return library().folder_counts()


@app.post("/api/subfolders")
def subfolders(req: PathReq):
    return library().subfolders(req.path)


@app.post("/api/folders")
def add_folder(req: PathReq):
    try:
        return {"path": library().add_folder(req.path)}
    except ValueError as e:
        raise HTTPException(400, str(e))


@app.delete("/api/folders")
def remove_folder(req: PathReq):
    library().remove_folder(req.path)
    return {"ok": True}


@app.post("/api/rescan")
def rescan():
    library().rescan()
    return {"ok": True}


@app.get("/api/categories")
def categories():
    return library().category_counts()


@app.post("/api/search")
def search(req: SearchReq):
    L = library()
    vec, exclude, words = None, None, None
    if req.similar_to:
        vec = L.embedding_of(req.similar_to)
        if vec is None:
            raise HTTPException(404, "That file is not indexed yet")
        exclude = req.similar_to
    elif req.query and req.query.strip():
        words = req.query.strip()
        vec = need_engine().engine.embed_texts([words])[0]
    sort = req.sort if vec is not None or req.sort != "score" else "name"
    f = req.model_dump(exclude={"query", "similar_to", "sort"})
    return L.search(vec, sort=sort, exclude=exclude, words=words, **f)


@app.get("/api/audio")
def audio(path: str):
    known_path(path)
    return FileResponse(path, media_type=mimetypes.guess_type(path)[0] or "application/octet-stream")


@app.get("/api/decoded")
def decoded(path: str, full: bool = False):
    """WAV render for formats the webview cannot decode itself (ogg, wv, some aiff). The
    preview player gets up to PREVIEW_MAX_SECONDS in mono; the editor asks for the full file, with
    every channel at its own sample rate."""
    known_path(path)
    if full:
        y, sr = eng.read_any(path)
        y = y.T
    else:
        y, sr = eng.decode_mono(path, PREVIEW_SR, PREVIEW_MAX_SECONDS), PREVIEW_SR
    buf = io.BytesIO()
    sf.write(buf, y, sr, format="WAV", subtype="FLOAT")
    return Response(buf.getvalue(), media_type="audio/wav")


@app.post("/api/reveal")
def reveal(req: PathReq):
    known_path(req.path)
    subprocess.Popen(["open", "-R", req.path])
    return {"ok": True}


@app.get("/api/map")
def sound_map():
    return library().sound_map()


@app.post("/api/reveal-folder")
def reveal_folder(req: PathReq):
    """Open a library folder, or a folder inside one, in Finder."""
    path = os.path.realpath(req.path)
    roots = [os.path.realpath(f) for f in library().folders()]
    if not os.path.isdir(path) or not any(path == r or is_inside(path, r) for r in roots):
        raise HTTPException(403, "That folder is not part of your library")
    subprocess.Popen(["open", path])
    return {"ok": True}


@app.get("/api/projects")
def projects():
    L = library()
    L.adopt_project_folders()
    return L.projects()


class ProjectReq(BaseModel):
    name: str


@app.post("/api/projects")
def create_project(req: ProjectReq):
    if not req.name.strip():
        raise HTTPException(400, "Name required")
    L = library()
    fresh = not any(p["name"] == req.name.strip() for p in L.projects())
    p = L.create_project(req.name)
    if fresh:
        try:
            p["folder"] = L.give_project_folder(p["id"], p["name"])
        except (OSError, ValueError) as e:
            p["folder_error"] = str(e)
    return p


@app.post("/api/projects/{pid}/rename")
def rename_project(pid: int, req: ProjectReq):
    if not req.name.strip():
        raise HTTPException(400, "Name required")
    try:
        library().rename_project(pid, req.name)
    except sqlite3.IntegrityError:
        raise HTTPException(400, "A project with that name already exists")
    return {"ok": True}


@app.delete("/api/projects/{pid}")
def delete_project(pid: int):
    library().delete_project(pid)
    return {"ok": True}


class ItemReq(BaseModel):
    path: str
    remove: bool = False


@app.post("/api/projects/{pid}/items")
def project_item(pid: int, req: ItemReq):
    L = library()
    if req.remove:
        L.project_remove(pid, req.path)
    else:
        known_path(req.path)
        try:
            if not L.project_folder(pid):
                p = next((p for p in L.projects() if p["id"] == pid), None)
                if not p:
                    raise HTTPException(404, "Project not found")
                L.give_project_folder(pid, p["name"])
            L.project_add(pid, req.path)
        except (ValueError, OSError) as e:
            raise HTTPException(400, f"Could not copy the sound into the project folder: {e}")
    return {"ok": True}


@app.post("/api/projects/{pid}/folder")
def project_folder(pid: int, req: PathReq):
    L = library()
    if not L.project_exists(pid):
        raise HTTPException(404, "Project not found")
    try:
        return {"folder": L.set_project_folder(pid, req.path)}
    except (ValueError, OSError) as e:
        raise HTTPException(400, str(e))


class OrderReq(BaseModel):
    paths: list[str]


@app.get("/api/projects/{pid}/order")
def project_order(pid: int):
    return library().project_order(pid)


@app.post("/api/projects/{pid}/order")
def set_project_order(pid: int, req: OrderReq):
    library().set_project_order(pid, req.paths)
    return {"ok": True}


class PadsReq(BaseModel):
    pads: dict[str, int | None]


@app.get("/api/projects/{pid}/pads")
def project_pads(pid: int):
    return library().project_pads(pid)


@app.post("/api/projects/{pid}/pads")
def set_project_pads(pid: int, req: PadsReq):
    L = library()
    if not L.project_exists(pid):
        raise HTTPException(404, "Project not found")
    try:
        L.set_project_pads(pid, req.pads)
    except ValueError as e:
        raise HTTPException(400, str(e))
    return {"ok": True}


@app.post("/api/backup")
def backup(req: PathReq):
    d = Path(req.path).expanduser()
    if not d.is_dir():
        raise HTTPException(400, f"Not a folder: {d}")
    try:
        return {"path": library().export_backup(d)}
    except OSError as e:
        raise HTTPException(400, f"Could not write the backup: {e}")


@app.post("/api/restore")
def restore(req: PathReq):
    try:
        return library().restore_backup(Path(req.path).expanduser())
    except ValueError as e:
        raise HTTPException(400, str(e))


class ExportReq(BaseModel):
    dest: str
    pattern: str = "{project}_{name}"
    format: str = "copy"


@app.post("/api/projects/{pid}/export")
def export_project(pid: int, req: ExportReq):
    L = library()
    if not L.project_exists(pid):
        raise HTTPException(404, "Project not found")
    try:
        return L.export_project(pid, req.dest, req.pattern, req.format)
    except ValueError as e:
        raise HTTPException(400, str(e))
    except (OSError, RuntimeError) as e:
        raise HTTPException(400, f"Export failed: {e}")


@app.post("/api/save")
def save(file: UploadFile = File(...), dest_dir: str = Form(...), name: str = Form(...),
         sp404: bool = Form(False), format: str = Form("wav"), source: str = Form(""),
         project: int = Form(0)):
    """Save a WAV rendered by the editor, always as a new file. format rewrites it as FLAC or
    MP3; source names the file it was edited from, whose tags, note and rating the new file
    inherits; project saves it into that project's folder and adds it to the project."""
    L = library()
    if project:
        p = next((p for p in L.projects() if p["id"] == project), None)
        if not p:
            raise HTTPException(404, "Project not found")
        try:
            dest_dir = L.project_folder_ready(project) or L.give_project_folder(project, p["name"])
        except (ValueError, OSError) as e:
            raise HTTPException(400, f"The project folder cannot be used: {e}")
    d = Path(dest_dir).expanduser()
    if not d.is_dir():
        raise HTTPException(400, f"Not a folder: {d}")
    if format not in SAVE_FORMATS or (sp404 and format != "wav"):
        raise HTTPException(400, f"Cannot save as {format}")
    if sp404:
        d = sp404_dir(d)
    stem = ascii_name(Path(name).stem) if sp404 else safe_name(Path(name).stem)
    data = file.file.read()
    try:
        target = unique_path(d / f"{stem}.{format}")
        if sp404 or format != "wav":
            tmp = target.with_suffix(".tmp.wav")
            tmp.write_bytes(data)
            try:
                if sp404:
                    to_sp404(tmp, target, mono=library().settings()["sp404_mono"])
                else:
                    transcode(tmp, target, format)
            finally:
                tmp.unlink(missing_ok=True)
        else:
            target.write_bytes(data)
    except (OSError, RuntimeError) as e:
        raise HTTPException(400, f"Save failed: {e}")
    if source and L.q("SELECT 1 FROM files WHERE path=?", (source,)):
        L.catalog_copy(source, str(target))
        L.set_origin(str(target), source)
    L.index_file(str(target))
    if project:
        L.project_add(project, str(target))
    return {"path": str(target)}


class DiscoverSearchReq(BaseModel):
    source: str
    query: str
    page: int = 1


class DiscoverItemReq(BaseModel):
    source: str
    id: str


class DiscoverSaveReq(BaseModel):
    url: str
    title: str
    project: int
    note: str = ""
    source: str = ""


@app.get("/api/discover/sources")
def discover_sources():
    key = library().settings()["freesound_key"].strip()
    return discover.SOURCES + ([discover.FREESOUND] if key else [])


@app.post("/api/discover/search")
def discover_search(req: DiscoverSearchReq):
    try:
        return discover.search(req.source, req.query, req.page, library().settings()["freesound_key"].strip())
    except discover.DiscoverError as e:
        raise HTTPException(502, str(e))


@app.post("/api/discover/waveform")
def discover_waveform(req: DiscoverItemReq):
    try:
        return discover.waveform(req.id)
    except discover.DiscoverError:
        return []


@app.post("/api/discover/files")
def discover_files(req: DiscoverItemReq):
    try:
        return discover.files(req.source, req.id)
    except discover.DiscoverError as e:
        raise HTTPException(502, str(e))


@app.post("/api/discover/save")
def discover_save(req: DiscoverSaveReq):
    """Download a found sound into a project's folder and add it to the project, with where
    it came from and its licence written into its note."""
    L = library()
    try:
        dest = L.project_folder_ready(req.project)
    except (ValueError, OSError) as e:
        raise HTTPException(400, str(e))
    if not dest:
        raise HTTPException(400, "That project has no folder yet. Choose one on the Projects page")
    try:
        path = discover.download(req.url, dest, req.title)
    except discover.DiscoverError as e:
        raise HTTPException(502, str(e))
    L.catalog_set(path, {"memo": req.note[:2000], "tags": ["discovered"] + ([req.source] if req.source else [])})
    L.index_file(path)
    L.project_add(req.project, path)
    return {"path": path}


@app.get("/api/arrangement")
def get_arrangement():
    return library().arrangement_get()


@app.post("/api/arrangement")
def set_arrangement(data: dict):
    try:
        return library().arrangement_set(data)
    except (AttributeError, KeyError, TypeError, ValueError) as e:
        raise HTTPException(400, f"Bad arrangement: {e}")


class RegionsReq(BaseModel):
    path: str
    regions: list[dict]


@app.get("/api/regions")
def get_regions(path: str):
    return library().regions_get(path)


@app.post("/api/regions")
def set_regions(req: RegionsReq):
    known_path(req.path)
    try:
        return library().regions_set(req.path, req.regions)
    except (KeyError, TypeError, ValueError) as e:
        raise HTTPException(400, f"Bad region: {e}")
