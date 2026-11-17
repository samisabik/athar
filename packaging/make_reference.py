"""Write reference outputs of the Python engine for a port to check itself against.

Makes a few short test sounds (tests/reference/*.wav) and records, for each, what the engine
derives from it: file info, analysis, peaks, the CLAP input features and the audio embedding.
Also records text embeddings for a fixed set of queries.

uv run python packaging/make_reference.py
"""
import json
import sys
from pathlib import Path

import numpy as np
import soundfile as sf

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from athar import engine as eng

OUT = ROOT / "tests" / "reference"
QUERIES = ["deep punchy kick drum", "crisp snare", "warm synth pad", "rain on a window", "a person talking"]


def sounds():
    rng = np.random.default_rng(7)
    sr = 44100
    t = np.arange(int(sr * .6)) / sr
    kick = np.sin(2 * np.pi * (50 + 120 * np.exp(-t * 30)) * t) * np.exp(-t * 7)
    yield "kick_44k_mono", kick, sr
    t = np.arange(int(48000 * 2)) / 48000
    yield "tone_440_48k", 0.5 * np.sin(2 * np.pi * 440 * t), 48000
    t = np.arange(int(sr * .3)) / sr
    yield "noise_burst_44k", rng.standard_normal(t.size) * np.exp(-t * 20) * .5, sr
    t = np.arange(int(sr * 16)) / sr
    left = .4 * np.sin(2 * np.pi * (200 + 900 * t / 16) * t)
    yield "chirp_stereo_16s", np.stack([left, np.roll(left, 300)], axis=1), sr
    beat = 60 / 120
    loop = np.zeros(int(sr * beat * 16))
    for k in range(16):
        s = int(k * beat * sr)
        n = int(.08 * sr)
        loop[s:s + n] += np.sin(2 * np.pi * 90 * np.arange(n) / sr) * np.exp(-np.arange(n) / sr * 40) * (1 if k % 4 == 0 else .6)
    yield "loop_120bpm_22k", loop, 22050


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    e = eng.Engine()
    index = {"queries": QUERIES, "files": {}}
    for name, y, sr in sounds():
        if name == "loop_120bpm_22k":
            y = y[::2]
        path = OUT / f"{name}.wav"
        sf.write(path, np.asarray(y, np.float32), sr, subtype="PCM_16")
        duration, file_sr, ch = eng.file_info(str(path))
        y48 = eng.load_audio(str(path))
        clips = eng.clips(str(path), y48, duration)
        feats = eng.clap_features(clips[0])
        emb = e.embed_audio(clips).mean(axis=0)
        emb /= np.linalg.norm(emb)
        np.save(OUT / f"{name}.features.npy", feats)
        np.save(OUT / f"{name}.embedding.npy", emb.astype(np.float32))
        np.save(OUT / f"{name}.load48k.npy", y48)
        index["files"][name] = {
            "duration": duration, "sr": file_sr, "channels": ch, "clips": len(clips),
            "analysis": eng.analyze(y48, eng.CLAP_SR, duration, path.name),
            "peaks": list(eng.file_peaks(str(path), y48, duration)),
        }
    np.save(OUT / "queries.embedding.npy", e.embed_texts(QUERIES).astype(np.float32))
    (OUT / "reference.json").write_text(json.dumps(index, indent=1, default=float))
    print(f"Wrote {len(index['files'])} sounds to {OUT}")


if __name__ == "__main__":
    main()
