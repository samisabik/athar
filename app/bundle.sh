#!/bin/zsh
# Builds Athar.app (target/release/bundle/macos) with the interface and the model inside.
set -e
cd "${0:A:h}"
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
[[ -f ../build/model/text.onnx ]] || { echo "Run: uv run packaging/export_model.py"; exit 1; }
[[ -f ../build/model-audio-pc/audio.onnx ]] || { echo "Run: uv run packaging/quantize_audio.py"; exit 1; }
cargo tauri build --bundles app
APP=target/release/bundle/macos/Athar.app
codesign --force --deep --sign - "$APP"
echo "Built $APP ($(du -sh "$APP" | cut -f1))"
