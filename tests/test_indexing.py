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


def tone(path, seconds, hz=220.0, tail=None):
    t = np.arange(int(48000 * seconds)) / 48000
    y = 0.3 * np.sin(2 * np.pi * hz * t)
    if tail:
        y[-int(48000 * 20):] = 0.3 * np.random.default_rng(0).standard_normal(48000 * 20)
    sf.write(path, y.astype(np.float32), 48000)


def test_real_index_run(tmp_path, monkeypatch):
    folder = os.path.realpath(tmp_path / "sounds")
    os.mkdir(folder)
    tone(os.path.join(folder, "short.wav"), 2)
    tone(os.path.join(folder, "long_tone_then_noise.wav"), 120, tail=True)
    tone(os.path.join(folder, "long_tone.wav"), 120)
    monkeypatch.setenv("ATHAR_HOME", str(tmp_path / "home"))
    lib = L.Library()
    assert lib.ready.wait(120)
    lib.update_settings({"rescan_on_open": False})
    lib.add_folder(folder)
    deadline = time.time() + 120
    while lib.count() < 3 and time.time() < deadline:
        time.sleep(0.2)
    rows = {os.path.basename(r["path"]): r for r in lib.search(sort="name")}
    assert set(rows) == {"short.wav", "long_tone.wav", "long_tone_then_noise.wav"}
    assert all(r["sha1"] for r in lib.q("SELECT sha1 FROM files"))
    # Peaks cover the whole file: the noisy last 20 s of 120 s is the last sixth of the bins.
    pk = np.array(rows["long_tone_then_noise.wav"]["peaks"])
    assert pk[-15:].mean() > pk[:60].mean() * 1.5
    # The noisy ending changes the embedding, which it could not when only the opening was heard.
    a = lib.embedding_of(rows["long_tone.wav"]["path"])
    b = lib.embedding_of(rows["long_tone_then_noise.wav"]["path"])
    assert abs(float(np.linalg.norm(a)) - 1) < 1e-4
    assert float(a @ b) < 0.999
