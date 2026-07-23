"""PyInstaller entrypoint for the desktop-bundled document sidecar."""

from __future__ import annotations

import os
import sys
from pathlib import Path


def _bootstrap_macos_loader() -> None:
    """Re-exec once so CFFI can resolve Pango from the frozen bundle.

    PyInstaller rewrites the collected dylibs to use ``@rpath`` but
    WeasyPrint opens the root libraries by basename. macOS must therefore
    receive the bundle's internal directory in its loader environment before
    process startup; changing it later, immediately before the lazy import,
    is too late for dyld.
    """
    if sys.platform != "darwin" or not getattr(sys, "frozen", False):
        return
    if os.environ.get("KRONN_DOCS_DYLD_READY") == "1":
        return

    bundle_root = Path(getattr(sys, "_MEIPASS"))
    env = os.environ.copy()
    env["DYLD_FALLBACK_LIBRARY_PATH"] = str(bundle_root)
    env["KRONN_DOCS_DYLD_READY"] = "1"
    os.execve(sys.executable, [sys.executable, *sys.argv[1:]], env)


if __name__ == "__main__":
    _bootstrap_macos_loader()
    from kronn_docs.server import main

    main()
