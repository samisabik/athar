"""Compare smaller CLAP models with the full one (build/model) on a sample of the library:
embedding agreement per sound, and how much the top results of text searches change. Each
candidate is a folder under build/ holding audio.onnx, text.onnx and tokenizer.json. Reads
the library's own file list, read-only.

uv run python packaging/compare_int8.py SAMPLE_SIZE FOLDER [FOLDER...]
"""
import random
import sqlite3
import sys
import time
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from athar import engine as eng
from athar.library import data_dir

QUERIES = ["deep punchy kick drum", "crisp snare", "closed hi-hat", "warm synth pad", "808 sub bass",
           "rain on a window", "a person talking", "birds singing", "metallic impact", "vinyl crackle texture",
           "piano chord", "acoustic guitar strum", "riser sweep", "hand clap", "ambient drone"]


def engine_for(folder):
    eng.model_dir = lambda: ROOT / "build" / folder
    return eng.Engine()


def main():
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 200
    candidates = sys.argv[2:] or ["model-int8"]
    db = sqlite3.connect(f"file:{data_dir() / 'index.db'}?mode=ro", uri=True)
    paths = [r[0] for r in db.execute("SELECT path FROM files WHERE error IS NULL")]
    paths = [p for p in paths if Path(p).exists()]
    random.Random(0).shuffle(paths)
    paths = paths[:n]
    print(f"{len(paths)} sounds")
    clips = []
    for p in paths:
        y = eng.load_audio(p)
        clips.append(eng.clips(p, y, eng.file_info(p)[0])[0])

    ref = engine_for("model")
    t = time.time()
    a32 = ref.embed_audio(clips)
    print(f"model: {(time.time() - t) / len(clips) * 1000:.0f} ms a clip")
    q32 = ref.embed_texts(QUERIES)
    del ref
    for folder in candidates:
        e = engine_for(folder)
        t = time.time()
        a8 = e.embed_audio(clips)
        ms = (time.time() - t) / len(clips) * 1000
        q8 = e.embed_texts(QUERIES)
        del e
        size = sum((ROOT / "build" / folder / f).resolve().stat().st_size for f in ("audio.onnx", "text.onnx")) / 1e6
        cos_a, cos_t = np.sum(a32 * a8, axis=1), np.sum(q32 * q8, axis=1)
        line = [f"{folder}: {size:.0f} MB, {ms:.0f} ms a clip, audio cosine {cos_a.mean():.4f} (min {cos_a.min():.4f}), text cosine {cos_t.mean():.4f}"]
        for k in (1, 5, 10):
            shared = [len(set(np.argsort(-(a32 @ q32[i]))[:k]) & set(np.argsort(-(a8 @ q8[i]))[:k])) / k for i in range(len(QUERIES))]
            line.append(f"top-{k} shared {np.mean(shared) * 100:.0f}%")
        print(", ".join(line))


if __name__ == "__main__":
    main()
