#!/bin/zsh
set -e
cd "${0:A:h}"
[[ -f build/model/audio.onnx ]] || uv run python packaging/export_model.py
uv run pyinstaller packaging/Athar.spec --noconfirm --distpath dist --workpath build/pyinstaller
cd dist
xattr -cr Athar.app
rm -f Athar.zip
ditto -c -k --keepParent Athar.app Athar.zip
mv -f Athar.zip ~/Desktop/Athar.zip
echo "Built ~/Desktop/Athar.zip ($(du -h ~/Desktop/Athar.zip | cut -f1))"
