"""Regression tests for the cross-platform docs-sidecar build bootstrap."""

from __future__ import annotations

import json
import os
import re
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
        self.assertIn(
            "needs: [release-checks, build-desktop, verify-installers, quality-gates]", workflow
        )

    def test_version_gate_runs_first_and_checks_the_tag(self) -> None:
        workflow = (
            Path(__file__).resolve().parents[3] / ".github" / "workflows" / "desktop-build.yml"
        ).read_text(encoding="utf-8")
        gate = workflow.index("  release-checks:")
        self.assertLess(gate, workflow.index("  build-desktop:"))
        self.assertIn("scripts/check-version-sync.sh", workflow)
        self.assertIn("needs: release-checks", workflow)

    @staticmethod
    def _checkout_refs(name: str) -> list[tuple[str, str, str]]:
        """(job, step, ref) for every actions/checkout of a workflow."""
        lines = (
            Path(__file__).resolve().parents[3] / ".github" / "workflows" / name
        ).read_text(encoding="utf-8").splitlines()
        found: list[tuple[str, str, str]] = []
        job = ""
        for index, line in enumerate(lines):
            if re.match(r"^  [A-Za-z0-9_-]+:\s*$", line):
                job = line.strip().rstrip(":")
            if "uses: actions/checkout@" not in line:
                continue
            ref = ""
            for following in lines[index + 1 :]:
                if re.match(r"^\s*- (name|uses):", following) or re.match(r"^  \S", following):
                    break
                match = re.match(r"^\s+ref:\s*(.*)$", following)
                if match:
                    ref = match.group(1).strip()
            found.append((job, line.strip(), ref))
        return found

    def test_every_checkout_uses_the_resolved_release_commit(self) -> None:
        resolved = "${{ needs.release-checks.outputs.sha }}"
        desktop = self._checkout_refs("desktop-build.yml")
        self.assertGreaterEqual(len(desktop), 4)
        for job, _step, ref in desktop:
            if job == "release-checks":
                # The one place the commit is resolved from the tag or branch.
                self.assertEqual(ref, "${{ inputs.release_tag || github.ref }}")
            else:
                self.assertEqual(ref, resolved, job)
        # The reusable workflows take the commit as an input and check it out.
        for name in ("dependency-review.yml", "ci-test.yml"):
            checkouts = self._checkout_refs(name)
            self.assertTrue(checkouts, name)
            for job, _step, ref in checkouts:
                self.assertEqual(ref, "${{ inputs.ref }}", f"{name}:{job}")

    def test_release_runs_the_reusable_checks_on_the_resolved_commit(self) -> None:
        workflow = (
            Path(__file__).resolve().parents[3] / ".github" / "workflows" / "desktop-build.yml"
        ).read_text(encoding="utf-8")
        self.assertIn("sha: ${{ steps.resolve.outputs.sha }}", workflow)
        # `with:` of the two reusable-workflow calls (job-level, 6 spaces).
        self.assertEqual(
            len(re.findall(
                r"^      ref: \$\{\{ needs\.release-checks\.outputs\.sha \}\}$", workflow, re.M
            )),
            2,
        )
        self.assertIn("uses: ./.github/workflows/ci-test.yml", workflow)
        self.assertIn("uses: ./.github/workflows/dependency-review.yml", workflow)
        release_job = workflow[workflow.index("\n  release:\n"):]
        self.assertIn("quality-gates", release_job.split("steps:")[0])
        self.assertIn("release-checks", release_job.split("steps:")[0])

    def test_ci_runs_every_gate_when_called_for_a_release(self) -> None:
        workflow = (
            Path(__file__).resolve().parents[3] / ".github" / "workflows" / "ci-test.yml"
        ).read_text(encoding="utf-8")
        self.assertIn("workflow_call:", workflow)
        # Label-gated jobs run on a release call, not only on dispatch.
        gated = re.findall(r"^    if: .*'ci-test'\)\)?$", workflow, re.MULTILINE)
        self.assertGreaterEqual(len(gated), 10)
        for condition in gated:
            if "inputs.ref == ''" in condition:
                continue  # the timing observer never runs for a release call
            self.assertIn("inputs.ref != ''", condition)
        self.assertIn('[ -n "$CI_REF" ]', workflow)

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
                "kronn-linux": "Kronn.deb",
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


    def test_installers_tauri_does_not_build_do_not_satisfy_a_platform(self) -> None:
        # tauri.conf.json bundles deb, nsis, dmg: an .msi or .AppImage alone
        # must not hide a missing .exe or .deb.
        for artifact, stray in (("kronn-windows", "Kronn.msi"), ("kronn-linux", "Kronn.AppImage")):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                for name, filename in (
                    ("kronn-windows", "Kronn.exe"),
                    ("kronn-macOS-arm64", "a.dmg"),
                    ("kronn-macOS-x64", "b.dmg"),
                    ("kronn-linux", "Kronn.deb"),
                ):
                    if name == artifact:
                        filename = stray
                    target = root / name / filename
                    target.parent.mkdir(parents=True)
                    target.write_bytes(b"installer")
                with self.assertRaises(SystemExit) as raised:
                    verify_artifacts.verify(root)
            self.assertIn(artifact, str(raised.exception))

    def test_expected_installers_are_the_configured_tauri_targets(self) -> None:
        conf = json.loads(
            (Path(__file__).resolve().parents[3] / "desktop" / "src-tauri" / "tauri.conf.json")
            .read_text(encoding="utf-8")
        )
        suffix = {"deb": ".deb", "nsis": ".exe", "dmg": ".dmg"}
        configured = {suffix[t] for t in conf["bundle"]["targets"]}
        expected = {ext for exts in verify_artifacts.EXPECTED_ARTIFACTS.values() for ext in exts}
        self.assertEqual(expected, configured)


class ReleaseAssetTests(unittest.TestCase):
    # KT-970 — 0.12.0 to 0.14.1 were published with no installer at all.
    RELEASE = [
        "Kronn_0.14.2_aarch64.dmg",
        "Kronn_0.14.2_x64.dmg",
        "Kronn_0.14.2_x64-setup.exe",
        "Kronn_0.14.2_amd64.deb",
    ]

    def test_a_complete_release_passes(self) -> None:
        self.assertEqual(verify_artifacts.missing_release_platforms(self.RELEASE), [])

    def test_a_release_without_assets_names_every_platform(self) -> None:
        self.assertEqual(
            verify_artifacts.missing_release_platforms([]),
            ["Windows", "macOS Apple Silicon", "macOS Intel", "Linux"],
        )

    def test_one_mac_build_does_not_stand_for_the_other(self) -> None:
        without_intel = [name for name in self.RELEASE if not name.endswith("_x64.dmg")]
        self.assertEqual(verify_artifacts.missing_release_platforms(without_intel), ["macOS Intel"])

    def test_msi_and_appimage_do_not_stand_for_a_platform(self) -> None:
        stray = [
            name for name in self.RELEASE if not name.endswith((".exe", ".deb"))
        ] + ["Kronn_0.14.2_x64_en-US.msi", "Kronn_0.14.2_amd64.AppImage"]
        self.assertEqual(verify_artifacts.missing_release_platforms(stray), ["Windows", "Linux"])

    def test_the_release_job_runs_on_this_repository_s_tags(self) -> None:
        # Tags are `0.14.1`, never `v0.14.1`: a `v*`-only trigger never built
        # a release, and the release job never ran.
        workflow = (
            Path(__file__).resolve().parents[3] / ".github" / "workflows" / "desktop-build.yml"
        ).read_text(encoding="utf-8")
        self.assertIn("'[0-9]+.[0-9]+.[0-9]+*'", workflow)
        self.assertNotIn("startsWith(github.ref, 'refs/tags/v')", workflow)
        self.assertIn("verify_artifacts.py --release-assets", workflow)

if __name__ == "__main__":
    unittest.main()
