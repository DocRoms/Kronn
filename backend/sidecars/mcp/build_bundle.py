#!/usr/bin/env python3
"""Freeze the shared MCP bridge, then exercise the distributable executable."""

import os
import subprocess
import sys
from pathlib import Path

from smoke_bundle import smoke_bundle


ROOT = Path(__file__).resolve().parent
REPO_ROOT = ROOT.parents[2]


def build():
    scripts = REPO_ROOT / "backend" / "scripts"
    output = REPO_ROOT / "desktop" / "src-tauri" / "resources" / "mcp-sidecar"
    work = REPO_ROOT / "target" / "mcp-sidecar-pyinstaller"
    args = [
        sys.executable, "-m", "PyInstaller", "--noconfirm", "--clean", "--onedir",
        "--name", "kronn-mcp", "--distpath", str(output),
        "--workpath", str(work / "work"), "--specpath", str(work),
        "--paths", str(scripts), "--hidden-import", "cli_token_collector",
    ]
    # The bridge fingerprints its source and loads the collector beside it.
    for filename in ("disc-introspection-mcp.py", "cli_token_collector.py"):
        args.extend(["--add-data", f"{scripts / filename}{os.pathsep}."])
    args.append(str(scripts / "disc-introspection-mcp.py"))
    subprocess.run(args, cwd=ROOT, check=True)
    executable = output / "kronn-mcp" / (
        "kronn-mcp.exe" if os.name == "nt" else "kronn-mcp"
    )
    smoke_bundle(executable)
    print(f"MCP bundle ready: {executable}")


if __name__ == "__main__":
    build()
