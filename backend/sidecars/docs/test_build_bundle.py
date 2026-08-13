"""Regression tests for the cross-platform docs-sidecar build bootstrap."""

from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import build_bundle
import smoke_bundle
import verify_artifacts


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

    def test_windows_loader_diagnostics_name_missing_roots_and_loader_path(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            native_dir = Path(directory)
            (native_dir / "libgobject-2.0-0.dll").touch()
            with patch.dict(
                os.environ,
                {"KRONN_DOCS_NATIVE_LIB_DIRS": directory},
                clear=False,
            ):
                diagnostics = build_bundle.native_loader_diagnostics(
                    "Windows",
                    {
                        "PATH": directory,
                        "WEASYPRINT_DLL_DIRECTORIES": directory,
                    },
                )

        self.assertIn(f"native_dir={directory} exists=True", diagnostics)
        self.assertIn("libgobject-2.0-0.dll", diagnostics)
        self.assertIn(f"WEASYPRINT_DLL_DIRECTORIES={directory}", diagnostics)

    def test_desktop_workflow_uses_setup_msys2_output_not_a_fixed_path(self) -> None:
        workflow = (
            Path(__file__).resolve().parents[3]
            / ".github"
            / "workflows"
            / "desktop-build.yml"
        ).read_text(encoding="utf-8")

        self.assertIn("id: msys2", workflow)
        self.assertIn("steps.msys2.outputs.msys2-location", workflow)
        self.assertNotIn("'C:\\msys64\\ucrt64\\bin'", workflow)

    def test_cargo_cache_never_archives_python_or_pyinstaller_target_data(self) -> None:
        workflow = (
            Path(__file__).resolve().parents[3]
            / ".github"
            / "workflows"
            / "desktop-build.yml"
        ).read_text(encoding="utf-8")
        cache_block = workflow.split("- name: Cache cargo", 1)[1].split(
            "- name: Setup Node.js", 1
        )[0]

        self.assertNotIn("\n            target\n", cache_block)
        self.assertNotIn("desktop/src-tauri/target", cache_block)

    def test_pull_request_ci_runs_the_real_frozen_exporter_on_windows(self) -> None:
        workflow = (
            Path(__file__).resolve().parents[3]
            / ".github"
            / "workflows"
            / "ci-test.yml"
        ).read_text(encoding="utf-8")
        job = workflow.split("test-docs-sidecar-windows:", 1)[1].split(
            "\n  test-frontend:", 1
        )[0]

        self.assertIn("runs-on: windows-latest", job)
        self.assertIn("steps.msys2.outputs.msys2-location", job)
        self.assertIn("build-docs-sidecar.mjs", job)
        self.assertIn("smoke_bundle.py", job)
        self.assertIn("id: docs-smoke", job)
        self.assertIn("--diagnostics target/docs-sidecar-smoke.log", job)
        self.assertIn("steps.docs-smoke.outcome == 'failure'", job)

    def test_release_smoke_runs_before_tauri_and_preserves_diagnostics(self) -> None:
        workflow = (
            Path(__file__).resolve().parents[3]
            / ".github"
            / "workflows"
            / "desktop-build.yml"
        ).read_text(encoding="utf-8")

        smoke_position = workflow.index("- name: Smoke-test frozen PDF and DOCX exporter")
        tauri_position = workflow.index("- name: Install Tauri CLI")
        self.assertLess(smoke_position, tauri_position)
        self.assertIn("--diagnostics target/docs-sidecar-smoke.log", workflow)
        self.assertIn("steps.docs-smoke.outcome == 'failure'", workflow)

    def test_release_requires_installers_from_every_platform(self) -> None:
        workflow = (
            Path(__file__).resolve().parents[3]
            / ".github"
            / "workflows"
            / "desktop-build.yml"
        ).read_text(encoding="utf-8")

        self.assertIn("name: kronn-windows", workflow)
        self.assertIn("name: kronn-${{ matrix.label }}", workflow)
        self.assertIn("name: kronn-linux", workflow)
        self.assertIn("label: macOS-arm64", workflow)
        self.assertIn("label: macOS-x64", workflow)
        self.assertEqual(workflow.count("if-no-files-found: error"), 3)
        self.assertIn("verify_artifacts.py artifacts", workflow)
        self.assertIn("verify-installers:", workflow)
        self.assertIn("needs: [build-desktop, verify-installers]", workflow)

    def test_checkout_network_retry_is_bounded(self) -> None:
        workflow = (
            Path(__file__).resolve().parents[3]
            / ".github"
            / "workflows"
            / "desktop-build.yml"
        ).read_text(encoding="utf-8")
        checkout = workflow.split("- name: Checkout source", 1)[1].split(
            "- name: Install Linux dependencies", 1
        )[0]

        self.assertIn("uses: actions/checkout@v7", checkout)
        self.assertIn("timeout-minutes: 5", checkout)

    def test_smoke_failure_can_be_saved_for_ci_diagnostics(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            diagnostics = Path(directory) / "smoke.log"
            with patch(
                "sys.argv",
                [
                    "smoke_bundle.py",
                    "missing-sidecar",
                    "--diagnostics",
                    str(diagnostics),
                ],
            ):
                with self.assertRaises(FileNotFoundError):
                    smoke_bundle.main()

            report = diagnostics.read_text(encoding="utf-8")
            self.assertIn("FileNotFoundError", report)
            self.assertIn("missing-sidecar", report)

    def test_bootstrap_failure_can_be_saved_for_ci_diagnostics(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            diagnostics = Path(directory) / "bootstrap.log"
            with (
                patch.object(build_bundle, "DIAGNOSTICS", diagnostics),
                patch.object(build_bundle, "build", side_effect=RuntimeError("Pango missing")),
                patch("sys.argv", ["build_bundle.py"]),
            ):
                with self.assertRaisesRegex(RuntimeError, "Pango missing"):
                    build_bundle.main()

            report = diagnostics.read_text(encoding="utf-8")
            self.assertIn("RuntimeError", report)
            self.assertIn("Pango missing", report)


class DesktopArtifactTests(unittest.TestCase):
    def test_complete_nonempty_installer_matrix_passes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixtures = {
                "kronn-windows": "Kronn.exe",
                "kronn-macOS-arm64": "Kronn-arm64.dmg",
                "kronn-macOS-x64": "Kronn-x64.dmg",
                "kronn-linux": "Kronn.AppImage",
            }
            for artifact, filename in fixtures.items():
                target = root / artifact / filename
                target.parent.mkdir(parents=True)
                target.write_bytes(b"installer")

            installers = verify_artifacts.verify(root)

        self.assertEqual(len(installers), 4)

    def test_missing_and_empty_installers_fail_with_platform_names(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            empty = root / "kronn-windows" / "Kronn.exe"
            empty.parent.mkdir(parents=True)
            empty.touch()

            with self.assertRaises(SystemExit) as raised:
                verify_artifacts.verify(root)

        message = str(raised.exception)
        self.assertIn("kronn-windows", message)
        self.assertIn("kronn-macOS-arm64", message)
        self.assertIn("kronn-macOS-x64", message)
        self.assertIn("kronn-linux", message)


if __name__ == "__main__":
    unittest.main()
