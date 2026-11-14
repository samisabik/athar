"""Real file events through the folder watcher, with the real model."""
import os
import time

import numpy as np
import pytest
import soundfile as sf

from athar import engine as eng
from athar import library as L

try:
    eng.model_dir()
except RuntimeError:
    pytestmark = pytest.mark.skip("CLAP model not exported")


def write(path, seed):
    y = np.random.default_rng(seed).standard_normal(48000).astype(np.float32) * 0.1
    sf.write(path, y, 48000, format="WAV")


def wait(cond, seconds=30):
    end = time.time() + seconds
    while time.time() < end:
        if cond():
            return True
        time.sleep(0.2)
    return False


def test_finder_style_changes_keep_metadata(tmp_path, monkeypatch):
    folder = os.path.realpath(tmp_path / "sounds")
    os.mkdir(folder)
    monkeypatch.setenv("ATHAR_HOME", str(tmp_path / "home"))
    lib = L.Library()
    assert lib.ready.wait(120)
    lib.add_folder(folder)
    assert wait(lambda: lib._observer is not None and folder in lib._watches)
    indexed = lambda p: lib.embedding_of(p) is not None
    meta = lambda p: (lib.catalog_get(p)["tags"], lib.catalog_get(p)["rating"])

    a = os.path.join(folder, "rain.wav")
    write(a, 1)
    assert wait(lambda: indexed(a)), "a new file is indexed without a rescan"
    pid = lib.create_project("kit")["id"]
    lib.catalog_set(a, {"tags": ["kyoto"], "rating": 5})
    lib.project_add(pid, a)

    # Renamed in Finder.
    b = os.path.join(folder, "rain kyoto.wav")
    os.rename(a, b)
    assert wait(lambda: indexed(b) and not indexed(a))
    assert meta(b) == (["kyoto"], 5) and lib.project_paths(pid) == {b}

    # Saved over by another app: temp file, then rename onto the original.
    stamp = lambda: [r["mtime"] for r in lib.q("SELECT mtime FROM files WHERE path=? AND emb IS NOT NULL", (b,))]
    mtime = stamp()
    time.sleep(1.1)
    tmp = os.path.join(folder, ".rain kyoto.wav.tmp")
    write(tmp, 2)
    os.replace(tmp, b)
    # The watcher may report this as a move or as delete-then-create; either way the row returns.
    assert wait(lambda: stamp() and stamp() != mtime), "the new content is re-indexed"
    assert meta(b) == (["kyoto"], 5) and lib.project_paths(pid) == {b}

    # Moved into a subfolder.
    sub = os.path.join(folder, "takes")
    os.mkdir(sub)
    c = os.path.join(sub, "rain kyoto.wav")
    os.rename(b, c)
    assert wait(lambda: indexed(c) and not indexed(b))
    assert meta(c) == (["kyoto"], 5) and lib.project_paths(pid) == {c}

    # Deleted, then put back (as from the Trash).
    data = open(c, "rb").read()
    os.unlink(c)
    assert wait(lambda: not indexed(c))
    assert lib.all_tags() == []
    with open(c, "wb") as f:
        f.write(data)
    assert wait(lambda: indexed(c))
    assert meta(c) == (["kyoto"], 5) and lib.project_paths(pid) == {c}
