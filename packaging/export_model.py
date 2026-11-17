# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["torch==2.15.0", "transformers==5.22.2", "onnx==1.23.2", "onnxruntime==1.31.1", "numpy==2.4.6", "huggingface-hub==2.4.0"]
# ///
"""Export the CLAP model to ONNX for Athar (build/model), then check it against PyTorch.

uv run packaging/export_model.py
"""
import shutil
import sys
from pathlib import Path

import numpy as np
import onnxruntime as ort
import torch
from huggingface_hub import hf_hub_download
from transformers import ClapModel, ClapProcessor

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "build" / "model"
MODEL_ID = "laion/larger_clap_music_and_speech"
CLAP_SR = 48000


class Audio(torch.nn.Module):
    def __init__(self, m):
        super().__init__()
        self.m = m

    def forward(self, input_features):
        longer = torch.zeros(input_features.shape[0], 1, dtype=torch.bool)
        o = self.m.get_audio_features(input_features=input_features, is_longer=longer)
        return o if isinstance(o, torch.Tensor) else o.pooler_output


class Text(torch.nn.Module):
    def __init__(self, m):
        super().__init__()
        self.m = m

    def forward(self, input_ids, attention_mask):
        o = self.m.get_text_features(input_ids=input_ids, attention_mask=attention_mask)
        return o if isinstance(o, torch.Tensor) else o.pooler_output


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    model = ClapModel.from_pretrained(MODEL_ID).eval()
    proc = ClapProcessor.from_pretrained(MODEL_ID)
    rng = np.random.default_rng(0)
    t = np.arange(CLAP_SR * 4) / CLAP_SR
    clips = [np.sin(2 * np.pi * 220 * t).astype(np.float32), (rng.standard_normal(CLAP_SR * 2) * 0.3).astype(np.float32)]
    texts = ["a kick drum hit", "rain on a tin roof"]
    a_in = proc(audio=clips, sampling_rate=CLAP_SR, return_tensors="pt")
    t_in = proc(text=texts, return_tensors="pt", padding=True)

    with torch.no_grad():
        ref_a = torch.nn.functional.normalize(Audio(model)(a_in["input_features"]), dim=-1).numpy()
        ref_t = torch.nn.functional.normalize(Text(model)(t_in["input_ids"], t_in["attention_mask"]), dim=-1).numpy()
    torch.onnx.export(Audio(model), (a_in["input_features"],), OUT / "audio.onnx", input_names=["input_features"],
                      output_names=["embedding"], dynamic_axes={"input_features": {0: "batch"}, "embedding": {0: "batch"}},
                      opset_version=17, dynamo=False)
    torch.onnx.export(Text(model), (t_in["input_ids"], t_in["attention_mask"]), OUT / "text.onnx",
                      input_names=["input_ids", "attention_mask"], output_names=["embedding"],
                      dynamic_axes={"input_ids": {0: "batch", 1: "tokens"}, "attention_mask": {0: "batch", 1: "tokens"},
                                    "embedding": {0: "batch"}}, opset_version=17, dynamo=False)
    shutil.copy(hf_hub_download(MODEL_ID, "tokenizer.json"), OUT / "tokenizer.json")

    unit = lambda v: v / np.linalg.norm(v, axis=-1, keepdims=True)
    audio = ort.InferenceSession(str(OUT / "audio.onnx")).run(None, {"input_features": a_in["input_features"].numpy()})[0]
    text = ort.InferenceSession(str(OUT / "text.onnx")).run(None, {"input_ids": t_in["input_ids"].numpy(), "attention_mask": t_in["attention_mask"].numpy()})[0]
    cos_a = float((unit(audio) * ref_a).sum(1).min())
    cos_t = float((unit(text) * ref_t).sum(1).min())
    print(f"Exported to {OUT}: audio match {cos_a:.5f}, text match {cos_t:.5f}")
    if min(cos_a, cos_t) < 0.999:
        sys.exit("The ONNX model does not match PyTorch; do not ship it")


if __name__ == "__main__":
    main()
