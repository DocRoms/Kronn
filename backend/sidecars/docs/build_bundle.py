#!/usr/bin/env python3
"""Build the native kronn-docs executable included in Tauri installers."""

from __future__ import annotations

import argparse
import os
import platform
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent
REPO_ROOT = ROOT.parents[2]
DEFAULT_OUTPUT = REPO_ROOT / "desktop" / "src-tauri" / "resources" / "docs-sidecar"
WORK_ROOT = REPO_ROOT / "target" / "docs-sidecar-pyinstaller"

COLLECT_PACKAGES = (
    "kronn_docs",
    "weasyprint",
    "docx",
    "pypdfium2",
    "pypdfium2_raw",
    "PIL",
    "xlsxwriter",
    "pptx",
    "uvicorn",
    "fastapi",
    "pydantic",
)

NATIVE_LIBRARY_PATTERNS = {
    "Darwin": (
        "libpango-1.0*.dylib",
        "libpangoft2-1.0*.dylib",
        "libharfbuzz*.dylib",
        "libgobject-2.0*.dylib",
        "libglib-2.0*.dylib",
        "libgio-2.0*.dylib",
        "libgmodule-2.0*.dylib",
        "libfontconfig*.dylib",
        "libfreetype*.dylib",
        "libfribidi*.dylib",
        "libthai*.dylib",
        "libdatrie*.dylib",
        "libgraphite2*.dylib",
        "libpng*.dylib",
        "libpcre2-8*.dylib",
        "libintl*.dylib",
    ),
    "Linux": (
        "libpango-1.0.so*",
        "libpangoft2-1.0.so*",
        "libharfbuzz.so*",
        "libharfbuzz-subset.so*",
        "libgobject-2.0.so*",
        "libfontconfig.so*",
    ),
    "Windows": (
        "libpango-1.0-0.dll",
        "libpangoft2-1.0-0.dll",
        "libharfbuzz-0.dll",
        "libgobject-2.0-0.dll",
        "libfontconfig-1.dll",
    ),
}


def native_library_dirs(system: str) -> list[Path]:
    configured = [
        Path(value)
        for value in os.environ.get("KRONN_DOCS_NATIVE_LIB_DIRS", "").split(os.pathsep)
        if value
    ]
    defaults = {
        "Darwin": [Path("/opt/homebrew/lib"), Path("/usr/local/lib")],
        "Linux": [
            Path("/usr/lib/x86_64-linux-gnu"),
            Path("/usr/lib/aarch64-linux-gnu"),
            Path("/usr/lib64"),
            Path("/usr/lib"),
        ],
        "Windows": [Path(r"C:\msys64\mingw64\bin")],
    }
    return configured + defaults.get(system, [])


def native_libraries(system: str) -> list[Path]:
    """Find CFFI-loaded Pango roots so PyInstaller can trace their dependencies."""
    found: dict[str, Path] = {}
    for directory in native_library_dirs(system):
        if not directory.is_dir():
            continue
        for pattern in NATIVE_LIBRARY_PATTERNS.get(system, ()):
            for candidate in directory.glob(pattern):
                # Preserve every ABI-name symlink. WeasyPrint and Pango load
                # specific names (for example libharfbuzz.0.dylib), so
                # collapsing aliases by resolved inode produces a bundle that
                # silently falls back to Homebrew on the build machine.
                found.setdefault(str(candidate), candidate)
    return sorted(found.values())


def configure_loader_environment(system: str) -> dict[str, str]:
    env = os.environ.copy()
    existing_dirs = [str(path) for path in native_library_dirs(system) if path.is_dir()]
    if not existing_dirs:
        return env
    if system == "Windows":
        env["WEASYPRINT_DLL_DIRECTORIES"] = os.pathsep.join(existing_dirs)
        env["PATH"] = os.pathsep.join(existing_dirs + [env.get("PATH", "")])
    elif system == "Darwin":
        env["DYLD_FALLBACK_LIBRARY_PATH"] = os.pathsep.join(
            existing_dirs + [env.get("DYLD_FALLBACK_LIBRARY_PATH", "")]
        )
    return env


def verify_weasyprint(env: dict[str, str]) -> None:
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            "from weasyprint import HTML; HTML(string='<p>ok</p>').write_pdf()",
        ],
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise SystemExit(
            "WeasyPrint cannot load its native libraries; the desktop sidecar "
            f"would ship broken.\n{detail}"
        )


def build(output: Path) -> Path:
    system = platform.system()
    env = configure_loader_environment(system)
    verify_weasyprint(env)

    output.mkdir(parents=True, exist_ok=True)
    WORK_ROOT.mkdir(parents=True, exist_ok=True)

    args = [
        sys.executable,
        "-m",
        "PyInstaller",
        "--noconfirm",
        "--clean",
        "--onedir",
        "--name",
        "kronn-docs",
        "--distpath",
        str(output),
        "--workpath",
        str(WORK_ROOT / "work"),
        "--specpath",
        str(WORK_ROOT),
        "--additional-hooks-dir",
        str(ROOT / "hooks"),
    ]
    for package in COLLECT_PACKAGES:
        args.extend(["--collect-all", package])
    for library in native_libraries(system):
        args.extend(["--add-binary", f"{library}{os.pathsep}."])
    args.append(str(ROOT / "bundle_entry.py"))

    subprocess.run(args, cwd=ROOT, env=env, check=True)
    executable = output / "kronn-docs" / (
        "kronn-docs.exe" if system == "Windows" else "kronn-docs"
    )
    if not executable.is_file():
        raise SystemExit(f"PyInstaller completed but {executable} is missing")
    print(f"kronn-docs bundle ready: {executable}")
    return executable


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    build(args.output.resolve())


if __name__ == "__main__":
    main()
