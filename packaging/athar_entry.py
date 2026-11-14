import multiprocessing
import os
from pathlib import Path

# Numba (used by librosa) caches compiled code next to its sources, which are
# read-only inside the app bundle; point it at a writable folder instead.
os.environ.setdefault("NUMBA_CACHE_DIR", str(Path.home() / "Library" / "Caches" / "Athar" / "numba"))

if __name__ == "__main__":
    multiprocessing.freeze_support()
    from athar.app import main
    main()
