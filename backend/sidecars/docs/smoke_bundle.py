#!/usr/bin/env python3
"""Behavior smoke test for the frozen kronn-docs executable."""

from __future__ import annotations

import argparse
import json
import os
import platform
import socket
import subprocess
import tempfile
import time
import urllib.request
import zipfile
from pathlib import Path


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def post_json(url: str, payload: dict[str, object]) -> dict[str, object]:
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=15) as response:
        return json.load(response)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("executable", type=Path)
    args = parser.parse_args()

    port = free_port()
    env = os.environ.copy()
    env["KRONN_DOCS_PORT"] = str(port)
    if platform.system() == "Darwin":
        # `cargo run` sets this variable to Rust build directories. More
        # importantly, this simulates an installed Mac without Homebrew's
        # `/opt/homebrew/lib` fallback: the frozen exporter must use only the
        # libraries shipped inside its own `_internal` directory.
        env["DYLD_FALLBACK_LIBRARY_PATH"] = str(
            Path(tempfile.gettempdir()) / "kronn-no-external-dylibs"
        )
    child = subprocess.Popen(
        [str(args.executable.resolve())],
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            try:
                with urllib.request.urlopen(
                    f"http://127.0.0.1:{port}/health", timeout=1
                ) as response:
                    assert json.load(response)["ok"] is True
                    break
            except Exception:
                if child.poll() is not None:
                    stdout, stderr = child.communicate()
                    raise AssertionError(
                        f"sidecar exited before ready\nstdout: {stdout}\nstderr: {stderr}"
                    )
                time.sleep(0.1)
        else:
            raise AssertionError("sidecar did not become healthy within 20 seconds")

        with tempfile.TemporaryDirectory() as tmp:
            output_dir = Path(tmp)
            docx = output_dir / "smoke.docx"
            pdf = output_dir / "smoke.pdf"
            styled_html = """
                <style>
                  :root { --bg: #0b1020; --text: #eef3ff; }
                  html, body { margin: 0; background: var(--bg); color: var(--text); }
                  .hero {
                    display: flex;
                    background: linear-gradient(135deg, #6d5dfc, #38bdf8);
                    border-radius: 18px;
                    padding: 22px;
                  }
                </style>
                <div class="hero"><h1>Kronn styled export</h1></div>
            """
            docx_response = post_json(
                f"http://127.0.0.1:{port}/docx",
                {"html": styled_html, "output_path": str(docx)},
            )
            pdf_response = post_json(
                f"http://127.0.0.1:{port}/pdf",
                {"html": styled_html, "output_path": str(pdf), "page_size": "A4"},
            )
            assert docx.is_file() and docx.stat().st_size > 0, docx_response
            assert pdf.is_file() and pdf.stat().st_size > 0, pdf_response

            with zipfile.ZipFile(docx) as archive:
                document_xml = archive.read("word/document.xml").decode()
                page_images = [
                    name
                    for name in archive.namelist()
                    if name.startswith("word/media/")
                ]
            assert len(page_images) == 1
            assert "<w:drawing>" in document_xml
            assert 'w:top="0"' in document_xml
            assert 'w:right="0"' in document_xml
            assert 'w:bottom="0"' in document_xml
            assert 'w:left="0"' in document_xml
    finally:
        child.terminate()
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()

    print("kronn-docs frozen bundle smoke test passed (DOCX + PDF)")


if __name__ == "__main__":
    main()
