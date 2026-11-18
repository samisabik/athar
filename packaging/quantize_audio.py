# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["onnx==1.23.2", "onnxruntime==1.31.1"]
# ///
"""Write build/model-audio-pc/audio.onnx: the CLAP audio model with int8 weights, per channel.

Against the full model its embeddings agree at cosine 0.9997 on a real library, it is a quarter
of the size, and it lets several sessions index side by side in the memory one full-size
session takes. Run packaging/export_model.py first.

uv run packaging/quantize_audio.py
"""
from pathlib import Path

from onnxruntime.quantization import QuantType, quantize_dynamic

ROOT = Path(__file__).resolve().parents[1]
FULL = ROOT / "build" / "model"
OUT = ROOT / "build" / "model-audio-pc"
# The models as the app loads them when run from source: the int8 audio model beside the
# full-size text model and the tokenizer.
FROM_SOURCE = ROOT / "build" / "model-next"


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    quantize_dynamic(FULL / "audio.onnx", OUT / "audio.onnx", weight_type=QuantType.QInt8, per_channel=True)
    print(f"Wrote {OUT / 'audio.onnx'}")

    FROM_SOURCE.mkdir(exist_ok=True)
    for name, src in (("audio.onnx", OUT), ("text.onnx", FULL), ("tokenizer.json", FULL)):
        link = FROM_SOURCE / name
        link.unlink(missing_ok=True)
        link.symlink_to((src / name).resolve())
    print(f"Wrote {FROM_SOURCE}")


if __name__ == "__main__":
    main()
