#!/usr/bin/env python3
"""Regression contracts for trusted CLI credentials in the Docker image."""

from __future__ import annotations

import os
import pathlib
import re
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
WRAPPER = ROOT / "backend/scripts/azure-docker-wrapper.sh"
DOCKERFILE = ROOT / "backend/Dockerfile"
COMPOSE = ROOT / "docker-compose.yml"
CI_WORKFLOW = ROOT / ".github/workflows/ci-test.yml"


class AzureDockerWrapperTests(unittest.TestCase):
    def test_image_pins_azure_cli_and_wraps_the_real_binary(self):
        dockerfile = DOCKERFILE.read_text()
        self.assertIn("ARG AZURE_CLI_VERSION=2.88.0", dockerfile)
        self.assertIn("azure-cli=${AZURE_CLI_VERSION}-1~bookworm", dockerfile)
        self.assertIn("/usr/bin/az-real", dockerfile)
        self.assertIn("azure-docker-wrapper.sh /usr/bin/az", dockerfile)

    def test_compose_mounts_the_host_home_read_only(self):
        compose = COMPOSE.read_text()
        self.assertIn("${HOME}:/host-home:ro", compose)

    def test_wrapper_points_azure_cli_at_host_credentials(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            host_creds = root / ".azure"
            host_creds.mkdir()
            fake_bin = root / "bin"
            fake_bin.mkdir()
            fake_real = fake_bin / "az-real"
            fake_real.write_text("#!/bin/sh\nprintf '%s' \"$AZURE_CONFIG_DIR\"")
            fake_real.chmod(0o755)
            wrapper = (root / "az")
            wrapper.write_text(
                WRAPPER.read_text().replace("/usr/bin/az-real", str(fake_real))
            )
            wrapper.chmod(0o755)
            env = os.environ.copy()
            env["KRONN_AZURE_CONFIG_DIR"] = str(host_creds)
            result = subprocess.run(
                [str(wrapper), "account", "get-access-token"],
                check=True,
                capture_output=True,
                text=True,
                env=env,
            )
            self.assertEqual(result.stdout, str(host_creds))

    def test_wrapper_explains_how_to_restore_missing_host_credentials(self):
        env = os.environ.copy()
        env["KRONN_AZURE_CONFIG_DIR"] = "/definitely/missing/kronn-azure"
        result = subprocess.run(
            ["sh", str(WRAPPER), "account", "get-access-token"],
            capture_output=True,
            text=True,
            env=env,
        )
        self.assertEqual(result.returncode, 78)
        self.assertIn("az login", result.stderr)
        self.assertIn("host", result.stderr)


class E2eContainerWorkflowTests(unittest.TestCase):
    def test_ci_jobs_have_a_hard_thirty_minute_timeout(self):
        workflow = CI_WORKFLOW.read_text()
        jobs = (
            "require-ci-label", "test-backend", "test-backend-coverage",
            "test-backend-quality", "test-desktop-compile", "duplication-check", "test-python",
            "test-docs-sidecar-windows", "test-frontend", "test-e2e", "test-shell",
            "security-scan", "test-backend-portability", "ci-quality-gates",
            "backend-ci-performance",
        )
        for job in jobs:
            match = re.search(
                rf"^  {re.escape(job)}:\n(?P<section>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
                workflow,
                re.MULTILINE | re.DOTALL,
            )
            self.assertIsNotNone(match, job)
            section = match.group("section")
            self.assertIn("timeout-minutes: 30", section, job)

    def test_backend_slo_observer_is_non_blocking_and_uses_hot_cold_measurements(self):
        workflow = CI_WORKFLOW.read_text()
        self.assertIn("workflow_dispatch:", workflow)
        self.assertIn("options: [hot, cold]", workflow)
        self.assertIn("unlabeled", workflow)
        self.assertIn("backend-ci-performance:", workflow)
        self.assertIn("ci-quality-gates:", workflow)
        for gate in (
            "test-backend", "test-backend-coverage", "test-backend-quality",
            "test-desktop-compile", "duplication-check", "test-python",
            "test-docs-sidecar-windows", "test-frontend", "test-e2e",
            "test-shell", "security-scan", "test-backend-portability",
        ):
            self.assertIn(f"      - {gate}", workflow)
        aggregate = re.search(
            r"^  ci-quality-gates:\n(?P<section>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
            workflow,
            re.MULTILINE | re.DOTALL,
        ).group("section")
        self.assertIn("if: always()", aggregate)
        self.assertIn("      - require-ci-label", aggregate)
        self.assertIn("node scripts/ci/backend_ci_slo.mjs", workflow)
        backend = re.search(
            r"^  test-backend:\n(?P<section>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
            workflow,
            re.MULTILINE | re.DOTALL,
        ).group("section")
        self.assertIn(".ci-cache/backend-compiled", backend)
        self.assertIn("Record compiled cache hit", backend)
        self.assertIn("Record compiled cache warmup miss", backend)
        self.assertIn("Verify bounded compiled backend cache", backend)
        self.assertIn("Reject invalid compiled cache hit", backend)
        self.assertIn("Stage bounded compiled backend artifacts", backend)
        self.assertIn("../target/debug/$directory", backend)
        self.assertIn(".kronn-backend-cache-v1", backend)
        self.assertNotIn('"target/debug/$directory"', backend)
        cargo_config = (ROOT / ".cargo" / "config.toml").read_text()
        self.assertIn('target-dir = "target"', cargo_config)
        self.assertLess(
            backend.index("cargo test — measured backend critical path"),
            backend.index("Stage bounded compiled backend artifacts"),
        )
        self.assertNotIn("cargo llvm-cov", backend)
        self.assertNotIn("cargo check — desktop crate", backend)
        coverage = re.search(
            r"^  test-backend-coverage:\n(?P<section>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
            workflow,
            re.MULTILINE | re.DOTALL,
        ).group("section")
        self.assertIn("cargo llvm-cov — enforce coverage floor", coverage)
        desktop = re.search(
            r"^  test-desktop-compile:\n(?P<section>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
            workflow,
            re.MULTILINE | re.DOTALL,
        ).group("section")
        self.assertIn("cargo check — desktop crate", desktop)
        self.assertIn("CI_COMPILED_CACHE_HIT: ${{ needs.test-backend.outputs.compiled_cache_hit }}", workflow)
        self.assertIn("CI_COMPILED_CACHE_STATE: ${{ needs.test-backend.outputs.compiled_cache_state }}", workflow)
        hot_cache = re.search(
            r"Cache cargo registry and bounded backend build \(hot\)(?P<section>.*?)(?=^      - |\Z)",
            backend,
            re.DOTALL,
        ).group("section")
        self.assertNotIn("backend/target", hot_cache)
        self.assertNotIn("llvm-cov-target", hot_cache)
        cold_cache = re.search(
            r"Cache cargo registry \(cold, isolated\)(?P<section>.*?)(?=^      - |\Z)",
            backend,
            re.DOTALL,
        ).group("section")
        self.assertIn("github.run_attempt", cold_cache)
        self.assertNotIn("restore-keys", cold_cache)
        python_job = re.search(
            r"^  test-python:\n(?P<section>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
            workflow,
            re.MULTILINE | re.DOTALL,
        ).group("section")
        self.assertIn("node scripts/ci/test_backend_ci_slo.mjs", python_job)

    def test_backend_readiness_wait_is_posix_and_latched(self):
        workflow = CI_WORKFLOW.read_text()
        self.assertNotIn(
            "for i in {1..60}",
            workflow,
            "container steps use /bin/sh, where Bash brace expansion runs once",
        )
        self.assertIn('while [ "$attempt" -le 60 ]', workflow)
        self.assertIn("backend_ready=1", workflow)
        self.assertIn('if [ "$backend_ready" -ne 1 ]', workflow)
        self.assertIn('kill -0 "$(cat /tmp/backend.pid)"', workflow)


if __name__ == "__main__":
    unittest.main()
