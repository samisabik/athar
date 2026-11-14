# Build: uv run pyinstaller packaging/Athar.spec --noconfirm
import re
from pathlib import Path
from PyInstaller.utils.hooks import collect_all, collect_submodules

ROOT = Path(SPECPATH).parent
VERSION = re.search(r'__version__ = "([^"]+)"', (ROOT / "athar" / "__init__.py").read_text()).group(1)
datas, binaries, hidden = [(str(ROOT / "athar" / "static"), "athar/static")], [], []
MODEL = ROOT / "build" / "model"
if not (MODEL / "audio.onnx").exists():
    raise SystemExit("Run packaging/export_model.py first")
datas += [(str(MODEL / f), "model") for f in ("audio.onnx", "text.onnx", "tokenizer.json")]
for pkg in ("onnxruntime", "librosa", "webview", "uvicorn", "soundfile", "watchdog", "tokenizers"):
    d, b, h = collect_all(pkg)
    datas += d; binaries += b; hidden += h
hidden += collect_submodules("athar") + ["AppKit", "Foundation", "WebKit"]
# The sound map lays points out with scikit-learn, which is imported lazily and so is not found on its own.
hidden += collect_submodules("sklearn.manifold") + collect_submodules("sklearn.neighbors") + collect_submodules("sklearn.decomposition")

a = Analysis([str(ROOT / "packaging" / "athar_entry.py")], pathex=[str(ROOT)], datas=datas,
             binaries=binaries, hiddenimports=hidden,
             excludes=["tkinter", "matplotlib", "IPython", "pytest", "torch", "transformers", "onnx"])
pyz = PYZ(a.pure)
exe = EXE(pyz, a.scripts, [], exclude_binaries=True, name="Athar", console=False, argv_emulation=False)
coll = COLLECT(exe, a.binaries, a.datas, name="Athar")
app = BUNDLE(coll, name="Athar.app", icon=str(ROOT / "assets" / "Athar.icns"), bundle_identifier="local.athar",
             info_plist={"CFBundleName": "Athar", "CFBundleDisplayName": "Athar", "CFBundleShortVersionString": VERSION, "CFBundleVersion": VERSION,
                         "NSHighResolutionCapable": True, "NSMicrophoneUsageDescription": "Athar records from your microphone or audio interface when you press Record.", "LSMinimumSystemVersion": "12.0"})
