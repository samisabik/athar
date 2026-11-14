import os

import numpy as np
import pytest
import soundfile as sf

from athar import library as L


@pytest.fixture
def lib(tmp_path, monkeypatch):
    """A Library on a temp database with one folder, and no model, watcher or worker."""
    monkeypatch.setattr(L.Library, "_boot", lambda self: None)
    monkeypatch.setattr(L.Library, "_worker", lambda self: None)
    root = os.path.realpath(tmp_path)
    folder = os.path.join(root, "sounds")
    os.mkdir(folder)
    lib = L.Library(os.path.join(root, "index.db"))
    lib.x("INSERT INTO folders VALUES (?)", (folder,))
    lib.folder = folder
    return lib


@pytest.fixture
def add(lib):
    """Write a WAV into the library folder and index it as the worker would, minus the model."""
    def add(name, seed=0, seconds=0.1, tags=None, rating=None, project=None):
        path = os.path.join(lib.folder, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        rng = np.random.default_rng(seed)
        sf.write(path, rng.standard_normal(int(48000 * seconds)).astype(np.float32) * 0.1, 48000)
        index(lib, path)
        if tags or rating:
            lib.catalog_set(path, {k: v for k, v in (("tags", tags), ("rating", rating)) if v})
        if project:
            lib.project_add(project, path)
        return path
    return add


def index(lib, path):
    st = os.stat(path)
    emb = np.zeros(L.EMB_DIM, np.float32)
    emb[hash(os.path.basename(path)) % L.EMB_DIM] = 1
    row = {"path": path, "folder": lib.folder, "mtime": st.st_mtime, "size": st.st_size,
           "sha1": L.file_hash(path, st.st_size), "duration": 0.1, "emb": emb.tobytes()}
    with lib.dblock:
        lib._save(row)
        lib._reattach(lib.db, path, row["sha1"])
        lib.db.commit()
    lib._put_mem({k: row.get(k) for k in L.META_COLS} | {"emb": row["emb"]})
