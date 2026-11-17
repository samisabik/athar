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
OUT = ROOT / "build" / "model-audio-pc"
OUT.mkdir(parents=True, exist_ok=True)
quantize_dynamic(ROOT / "build/model/audio.onnx", OUT / "audio.onnx", weight_type=QuantType.QInt8, per_channel=True)
print(f"Wrote {OUT / 'audio.onnx'}")

# build/model-next: the int8 audio model with the full-size text model, as the Rust app uses them.
NEXT = ROOT / "build" / "model-next"
NEXT.mkdir(exist_ok=True)
for name, src in (("audio.onnx", OUT / "audio.onnx"), ("text.onnx", ROOT / "build/model/text.onnx"), ("tokenizer.json", ROOT / "build/model/tokenizer.json")):
    link = NEXT / name
    link.unlink(missing_ok=True)
    link.symlink_to(src.resolve())
print(f"Wrote {NEXT}")
