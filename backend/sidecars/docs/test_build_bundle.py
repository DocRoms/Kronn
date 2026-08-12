"""Regression tests for the cross-platform docs-sidecar build bootstrap."""

from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import build_bundle


class NativeLibraryEnvironmentTests(unittest.TestCase):
    def test_windows_prefers_ucrt_before_legacy_mingw(self) -> None:
        with patch.dict(os.environ, {"KRONN_DOCS_NATIVE_LIB_DIRS": ""}, clear=False):
            directories = build_bundle.native_library_dirs("Windows")

        self.assertEqual(directories[0], Path(r"C:\msys64\ucrt64\bin"))
        self.assertEqual(directories[1], Path(r"C:\msys64\mingw64\bin"))

    def test_windows_exports_configured_directory_to_both_loaders(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with patch.dict(
                os.environ,
                {
                    "KRONN_DOCS_NATIVE_LIB_DIRS": directory,
                    "PATH": "existing-path",
                },
                clear=False,
            ):
                env = build_bundle.configure_loader_environment("Windows")

        self.assertEqual(env["WEASYPRINT_DLL_DIRECTORIES"], directory)
        self.assertEqual(env["PATH"].split(os.pathsep)[0], directory)
        self.assertIn("existing-path", env["PATH"])

    def test_nonexistent_configured_directory_is_not_exported(self) -> None:
        missing = str(Path(tempfile.gettempdir()) / "kronn-missing-native-dir")
        with patch.dict(
            os.environ,
            {"KRONN_DOCS_NATIVE_LIB_DIRS": missing, "PATH": "existing-path"},
            clear=False,
        ):
            env = build_bundle.configure_loader_environment("Windows")

        self.assertNotIn("WEASYPRINT_DLL_DIRECTORIES", env)
        self.assertEqual(env["PATH"], "existing-path")


if __name__ == "__main__":
    unittest.main()
