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
BUILD_WORKFLOW = ROOT / ".github/workflows/ci-build.yml"
VERDICT_WORKFLOW = ROOT / ".github/workflows/ci-verdict.yml"
CACHE_WORKFLOW = ROOT / ".github/workflows/ci-cache.yml"
WORKFLOWS = ROOT / ".github/workflows"
# Each test workflow, its label, the job that reads it live, and its aggregate.
TEST_WORKFLOWS = (
    (CI_WORKFLOW, "ci-test", "require-ci-label", "ci-quality-gates"),
    (BUILD_WORKFLOW, "ci-build", "require-ci-build-label", "ci-build-gates"),
)
LABEL_EVENTS = ("labeled", "unlabeled", "edited")
IDENTITY_STEP = (
    '- name: "Gate verdict for ${{ github.event_name }} #${{ github.event.pull_request.number }} '
    'head ${{ github.event.pull_request.head.sha }} base ${{ github.event.pull_request.base.sha }}"'
)


def job_sections(workflow: str) -> dict[str, str]:
    """Top-level job ids of a workflow, each with its YAML section."""
    body = workflow[workflow.index("\njobs:\n") + len("\njobs:\n"):]
    return {
        match.group("name"): match.group("section")
        for match in re.finditer(
            r"^  (?P<name>[A-Za-z0-9_-]+):\n(?P<section>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
            body,
            re.MULTILINE | re.DOTALL,
        )
    }


def job_steps(section: str) -> list[str]:
    """Each step of a job section, as its raw text."""
    if "\n    steps:\n" not in section:
        return []
    body = section.split("\n    steps:\n", 1)[1]
    return re.split(r"(?m)^      - ", body)[1:]


def step_inputs(step: str) -> dict[str, str]:
    """The `with:` inputs of a step; a block scalar keeps one line per entry."""
    inputs: dict[str, str] = {}
    current = None
    inside = False
    for line in step.splitlines():
        if line == "        with:":
            inside = True
        elif not inside:
            continue
        elif line.startswith("            ") and current:
            inputs[current] += line.strip() + "\n"
        elif line.startswith("          ") and not line.startswith("           "):
            current, _, value = line.strip().partition(":")
            value = value.strip()
            inputs[current] = "" if value == "|" else value
        else:
            break
    return inputs


def cache_steps(workflow: str) -> list[dict[str, str]]:
    """Every actions/cache step, with its inputs and the runner its job uses."""
    found = []
    for job, section in job_sections(workflow).items():
        runner = tuple(
            (re.search(pattern, section, re.MULTILINE) or [None, None])[1]
            for pattern in (r"^    runs-on: (.*)$", r"^      image: (.*)$", r"^        os: (.*)$")
        )
        for step in job_steps(section):
            uses = re.search(r"(?m)^(?:        )?uses: (\S+)$", step)
            if uses and uses.group(1).startswith("actions/cache"):
                found.append({
                    "job": job,
                    "uses": uses.group(1),
                    "if": (re.search(r"(?m)^        if: (.*)$", step) or [None, ""])[1],
                    "runner": runner,
                    **step_inputs(step),
                })
    return found


def cache_identity(step: dict[str, str]) -> tuple:
    """What GitHub matches a cache on: its key, restore keys and paths."""
    return step["key"], step.get("restore-keys", ""), step.get("path", "")


def pull_request_types(workflow: str) -> list[str]:
    match = re.search(r"^  pull_request:\n(?:    #.*\n)*    types: \[(?P<types>[^\]]*)\]", workflow, re.MULTILINE)
    if match:
        return [kind.strip() for kind in match.group("types").split(",")]
    # No `types`: GitHub's default, which carries no label event.
    return ["opened", "reopened", "synchronize"] if re.search(r"^  pull_request:", workflow, re.MULTILINE) else []


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
    def test_ci_jobs_have_hard_bounded_timeouts(self):
        jobs = {
            CI_WORKFLOW: (
                "require-ci-label", "test-backend", "test-backend-quality",
                "duplication-check", "test-python", "test-frontend", "test-e2e",
                "test-shell", "security-scan", "ci-quality-gates",
                "backend-ci-performance",
            ),
            BUILD_WORKFLOW: (
                "require-ci-build-label", "test-desktop-compile", "test-docs-sidecar-windows",
                "test-backend-portability", "build-release", "ci-build-gates",
            ),
            VERDICT_WORKFLOW: ("ci-quality-gates", "ci-build-gates"),
            CACHE_WORKFLOW: (
                "warm-test-backend", "warm-test-backend-quality", "warm-test-e2e",
                "warm-security-scan", "warm-test-backend-portability",
                "warm-build-release", "warm-test-docs-sidecar-windows",
            ),
        }
        for path, job in ((path, job) for path, names in jobs.items() for job in names):
            workflow = path.read_text()
            match = re.search(
                rf"^  {re.escape(job)}:\n(?P<section>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
                workflow,
                re.MULTILINE | re.DOTALL,
            )
            self.assertIsNotNone(match, job)
            section = match.group("section")
            # Cold compilation/coverage need bounded room before suites/cache save.
            expected_timeout = {
                "test-backend": 40,
                "test-e2e": 45,
                "build-release": 45,
                "test-backend-portability": "${{ matrix.os == 'macos-latest' && 45 || 30 }}",
            }.get(job.removeprefix("warm-"), 60 if path == VERDICT_WORKFLOW else 30)
            self.assertIn(f"timeout-minutes: {expected_timeout}", section, job)

    def test_backend_slo_observer_is_non_blocking_and_uses_hot_cold_measurements(self):
        workflow = CI_WORKFLOW.read_text()
        self.assertIn("workflow_dispatch:", workflow)
        self.assertIn("options: [hot, cold]", workflow)
        self.assertIn("backend-ci-performance:", workflow)
        self.assertIn("ci-quality-gates:", workflow)
        for gate in (
            "test-backend", "test-backend-quality", "duplication-check",
            "test-python", "test-frontend", "test-e2e", "test-shell",
            "security-scan",
        ):
            self.assertIn(f"      - {gate}", workflow)
        build = BUILD_WORKFLOW.read_text()
        for gate in ("test-desktop-compile", "test-docs-sidecar-windows", "test-backend-portability"):
            self.assertIn(f"      - {gate}", build)
            self.assertNotIn(f"  {gate}:\n", workflow)
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
        self.assertIn("timeout-minutes: 40", backend)
        self.assertIn("target/llvm-cov-target/debug/.fingerprint", backend)
        self.assertIn("target/llvm-cov-target/debug/build", backend)
        self.assertIn("target/llvm-cov-target/debug/deps", backend)
        self.assertIn("Record compiled cache hit", backend)
        self.assertIn("Record compiled cache warmup miss", backend)
        self.assertIn("Verify bounded compiled backend cache", backend)
        self.assertIn("Reject invalid compiled cache hit", backend)
        self.assertIn("Mark bounded compiled backend cache ready", backend)
        self.assertIn("../target/llvm-cov-target/debug/$directory", backend)
        self.assertIn(".kronn-backend-cache-v3", backend)
        cargo_config = (ROOT / ".cargo" / "config.toml").read_text()
        self.assertIn('target-dir = "target"', cargo_config)
        self.assertLess(
            backend.index("cargo llvm-cov nextest — measured backend critical path"),
            backend.index("Mark bounded compiled backend cache ready"),
        )
        # One test pass: the suite runs once, under nextest and coverage, with
        # the same configuration as `make test-backend-cov`.
        self.assertIn("cargo llvm-cov nextest --workspace", backend)
        self.assertIn("NEXTEST_PROFILE: ci", backend)
        # Without a pool, one profraw per test process fills the runner disk.
        self.assertIn("LLVM_PROFILE_FILE_NAME: kronn-%8m.profraw", backend)
        # The drift check and the raw-command lint reuse the suite's build.
        self.assertIn("assemble-generated-types.mjs", backend)
        self.assertIn("llvm-cov-target/debug/examples/lint-no-raw-command", backend)
        self.assertIn("--fail-under-lines 83", backend)
        self.assertIn("check-keymgmt-coverage.sh", backend)
        self.assertIn("cargo fmt --all -- --check", backend)
        self.assertNotIn("cargo test", backend)
        self.assertNotIn("test-backend-coverage", workflow)
        self.assertTrue((ROOT / "backend/.config/nextest.toml").is_file())
        self.assertNotIn("cargo check — desktop crate", backend)
        # KT-533 — clippy moved off the measured critical path: a warm
        # `test-backend` run still exceeded the 15-minute SLO (17m41 on
        # https://github.com/DocRoms/Kronn/actions/runs/33368662050) with
        # clippy inline. It now runs in the parallel, still-required
        # `test-backend-quality` gate instead of being removed.
        self.assertNotIn("cargo clippy", backend)
        # KT-533 — the measured backend job never spends several minutes
        # deleting unrelated runner toolchains. The bounded compiled cache is
        # restored and saved in place, so neither a hit nor a warmup copies the
        # whole debug tree a second time.
        self.assertNotIn("Free disk space", backend)
        verify_index = backend.index("Verify bounded compiled backend cache")
        mark_index = backend.index("Mark bounded compiled backend cache ready")
        mark_section = re.search(
            r"Mark bounded compiled backend cache ready(?P<section>.*?)(?=^      - |\Z)",
            backend,
            re.DOTALL,
        ).group("section")
        self.assertIn("steps.verify-backend-cache.outputs.state == 'miss'", mark_section)
        self.assertNotIn("cp -a", backend)
        self.assertLess(verify_index, mark_index)
        quality = re.search(
            r"^  test-backend-quality:\n(?P<section>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
            workflow,
            re.MULTILINE | re.DOTALL,
        ).group("section")
        self.assertIn("components: clippy", quality)
        self.assertIn("cargo clippy --all-targets -- -D warnings", quality)
        # nextest skips doctests: they run here so a new one is never lost.
        self.assertIn("run: cargo test --doc", quality)
        desktop = re.search(
            r"^  test-desktop-compile:\n(?P<section>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
            build,
            re.MULTILINE | re.DOTALL,
        ).group("section")
        # Clippy type-checks the crate; a check step compiled it twice.
        self.assertIn("cargo clippy --locked -- -D warnings", desktop)
        self.assertNotIn("cargo check", desktop)
        self.assertIn("CI_COMPILED_CACHE_HIT: ${{ needs.test-backend.outputs.compiled_cache_hit }}", workflow)
        self.assertIn("CI_COMPILED_CACHE_STATE: ${{ needs.test-backend.outputs.compiled_cache_state }}", workflow)
        hot_cache = re.search(
            r"Cache cargo registry and bounded backend build \(hot\)(?P<section>.*?)(?=^      - |\Z)",
            backend,
            re.DOTALL | re.MULTILINE,
        ).group("section")
        self.assertNotIn("backend/target", hot_cache)
        # Only the bounded build directories: never profraw files, the
        # nextest store, or the whole instrumented target tree.
        self.assertNotIn("target/llvm-cov-target\n", hot_cache)
        self.assertNotIn("profraw", hot_cache)
        self.assertNotIn("nextest", hot_cache)
        self.assertIn("target/llvm-cov-target/debug/.fingerprint", hot_cache)
        self.assertIn("cargo-hot-v3-", hot_cache)
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

    def test_no_label_event_starts_a_test_job(self):
        # GitHub shows the latest check of each name: a label event that
        # skipped a test job would hide its real result.
        test_jobs = {
            name
            for path, _label, _route, gate in TEST_WORKFLOWS
            for name in job_sections(path.read_text())
            if name != gate
        }
        self.assertIn("test-backend", test_jobs)
        self.assertIn("test-docs-sidecar-windows", test_jobs)
        label_triggered = []
        for path in sorted(WORKFLOWS.glob("*.yml")):
            workflow = path.read_text()
            if not set(LABEL_EVENTS) & set(pull_request_types(workflow)):
                continue
            label_triggered.append(path.name)
            self.assertFalse(test_jobs & set(job_sections(workflow)), path.name)
        self.assertEqual(label_triggered, ["ci-verdict.yml"])
        for path, *_ in TEST_WORKFLOWS:
            self.assertEqual(
                pull_request_types(path.read_text()), ["opened", "reopened", "synchronize"], path.name
            )

    def test_the_verdict_workflow_publishes_exactly_the_two_aggregates(self):
        workflow = VERDICT_WORKFLOW.read_text()
        self.assertEqual(pull_request_types(workflow), list(LABEL_EVENTS))
        self.assertNotIn("  push:", workflow)
        self.assertNotIn("workflow_call:", workflow)
        self.assertIn("permissions: {}", workflow)
        jobs = job_sections(workflow)
        self.assertEqual(sorted(jobs), ["ci-build-gates", "ci-quality-gates"])
        for path, label, _route, gate in TEST_WORKFLOWS:
            section = jobs[gate]
            # A skipped required check counts as passing: these always run.
            self.assertNotRegex(section, r"(?m)^    (if|needs|uses|strategy):", gate)
            self.assertIn("actions: write", section)
            self.assertIn(
                f"        if: \"!contains(github.event.pull_request.labels.*.name, '{label}')\"\n"
                f"        run: |\n", section,
            )
            self.assertIn(f"run: scripts/ci/previous_gate_verdict.sh {path.name} {gate} {label}\n", section)
            for name in ("PR_NUMBER", "HEAD_SHA", "BASE_SHA"):
                self.assertIn(f"          {name}: ${{{{ github.event.pull_request.", section)
            # Only the label guard is conditional; the verdict always runs.
            self.assertEqual(section.count("        if: "), 1, gate)

    def test_each_test_workflow_reads_its_label_live_and_aggregates_every_job(self):
        for path, label, route, gate in TEST_WORKFLOWS:
            workflow = path.read_text()
            jobs = job_sections(workflow)
            # A push to main warms caches instead (ci-cache.yml).
            self.assertNotIn("  push:", workflow)
            self.assertIn("workflow_call:", workflow)
            self.assertNotIn("github.event.label", workflow)
            self.assertNotIn("github.event.action", workflow)
            self.assertNotIn("previous_gate_verdict", workflow)
            route_job = jobs[route]
            self.assertNotRegex(route_job, r"(?m)^    (if|needs):")
            self.assertIn("pull-requests: read", route_job)
            self.assertIn("run: ${{ steps.label.outputs.run }}", route_job)
            self.assertIn(f"--jq '.labels[].name' | grep -qx '{label}'", route_job)
            self.assertIn('echo "run=true" >> "$GITHUB_OUTPUT"', route_job)
            live = f"needs.{route}.outputs.run == 'true'"
            aggregate = jobs[gate]
            needs = re.findall(r"^      - ([A-Za-z0-9_-]+)$", aggregate, re.MULTILINE)
            self.assertEqual(
                sorted(needs), sorted(name for name in jobs if name not in (gate, "backend-ci-performance")),
                path.name,
            )
            self.assertIn("    if: always()\n", aggregate)
            self.assertIn(f"        if: needs.{route}.outputs.run != 'true'\n", aggregate)
            self.assertIn(f"        if: {live}\n", aggregate)
            self.assertIn(IDENTITY_STEP, aggregate)
            self.assertIn("jq -e 'all(.[]; . == \"success\")'", aggregate)
            for name, section in jobs.items():
                if name in (route, gate):
                    continue
                condition = re.search(r"^    if: (.*)$", section, re.MULTILINE).group(1)
                self.assertIn(live, condition, name)
                self.assertRegex(section, rf"(?m)^    needs: (\[[^\]]*\b)?{route}\b", name)
        build = BUILD_WORKFLOW.read_text()
        self.assertIn("Add the ci-build label before merging", build)
        # ci-test serves a cheaper e2e build: the real release profile is
        # built here, for every pull request.
        self.assertIn("cargo build --release --locked --bin kronn", build)
        self.assertIn("      - build-release", build)
        # Desktop Build calls both: the label job's permission must be granted.
        desktop = (WORKFLOWS / "desktop-build.yml").read_text()
        for name in ("ci-test.yml", "ci-build.yml"):
            self.assertIn(f"      pull-requests: read\n    uses: ./.github/workflows/{name}", desktop)

    def test_a_push_to_main_only_warms_the_caches_pull_requests_read(self):
        cache = CACHE_WORKFLOW.read_text()
        self.assertIn("on:\n  push:\n    branches: [main]\n  workflow_dispatch:\n\n", cache)
        self.assertNotIn("pull_request", cache)
        self.assertNotIn("workflow_call", cache)
        self.assertIn("\npermissions:\n  contents: read\n", cache)
        # Nothing else that builds or tests starts on main; the site deploy is
        # filtered on site/.
        pushed = sorted(
            path.name for path in WORKFLOWS.glob("*.yml")
            if re.search(r"(?m)^  push:\n    branches: \[main\]", path.read_text())
        )
        self.assertEqual(pushed, ["ci-cache.yml", "deploy-site.yml"])
        self.assertIn("      - 'site/**'", (WORKFLOWS / "deploy-site.yml").read_text())

        shared = [
            step
            for path in (CI_WORKFLOW, BUILD_WORKFLOW)
            for step in cache_steps(path.read_text())
            # The cold measurement cache belongs to its own run.
            if "github.run_id" not in step["key"]
        ]
        self.assertEqual(len(shared), 6)
        warm = cache_steps(cache)
        saves = [step for step in warm if step["uses"] != "actions/cache/restore@v4"]
        lookups = [step for step in warm if step["uses"] == "actions/cache/restore@v4"]
        self.assertEqual(
            sorted(map(cache_identity, saves)), sorted(map(cache_identity, shared))
        )
        for step in shared:
            twin = next(save for save in saves if cache_identity(save) == cache_identity(step))
            # Same action and runner (OS, container, matrix): GitHub's cache
            # version hashes the paths and the archive format.
            self.assertEqual(twin["uses"], step["uses"], step["key"])
            self.assertEqual(twin["runner"], step["runner"], step["key"])
            self.assertEqual(twin["job"], f"warm-{step['job']}")
            self.assertEqual(twin["if"], "steps.lookup.outputs.cache-hit != 'true'")
        # A lookup per cache, on main only, that never downloads.
        self.assertEqual(sorted(step["job"] for step in lookups), sorted(step["job"] for step in saves))
        for lookup in lookups:
            twin = next(save for save in saves if save["job"] == lookup["job"])
            self.assertEqual(lookup["lookup-only"], "true")
            self.assertEqual((lookup["key"], lookup["path"]), (twin["key"], twin["path"]))
            self.assertNotIn("restore-keys", lookup)
            self.assertEqual(lookup["if"], "")

        jobs = job_sections(cache)
        for name, section in jobs.items():
            steps = job_steps(section)
            gate = "steps.python.outputs.cache-hit" if name.endswith("docs-sidecar-windows") else "steps.lookup.outputs.cache-hit"
            probe = next(i for i, step in enumerate(steps) if f"id: {gate.split('.')[1]}" in step)
            # Every step after the probe runs only on a miss.
            for step in steps[probe + 1:]:
                self.assertIn(f"        if: {gate} != 'true'\n", step, name)
        # It builds what the jobs build; it runs no suite.
        self.assertIn(
            "cargo llvm-cov nextest-archive --workspace", jobs["warm-test-backend"]
        )
        code = "\n".join(line for line in cache.splitlines() if not line.lstrip().startswith("#"))
        self.assertNotRegex(code, r"llvm-cov nextest(?!-archive)|nextest run|\bpnpm |\bmake |smoke_bundle")
        self.assertEqual(
            re.findall(r"cargo test[^\n]*", code), ["cargo test --doc", "cargo test --lib --no-run"]
        )
        self.assertIn("cargo clippy --all-targets -- -D warnings", jobs["warm-test-backend-quality"])
        self.assertIn("cargo build --locked --bin kronn", jobs["warm-test-e2e"])
        self.assertIn("cargo build --release --locked --bin kronn", jobs["warm-build-release"])
        self.assertIn("cargo install cargo-audit --locked", jobs["warm-security-scan"])
        marker = "echo 'schema=3' > ../target/llvm-cov-target/debug/.kronn-backend-cache-v3"
        self.assertIn(marker, jobs["warm-test-backend"])
        self.assertIn(marker, job_sections(CI_WORKFLOW.read_text())["test-backend"])
        # The tool-managed pip and MSYS2 caches of the Windows exporter job.
        sidecar = job_sections(BUILD_WORKFLOW.read_text())["test-docs-sidecar-windows"]
        warm_sidecar = jobs["warm-test-docs-sidecar-windows"]
        for action in ("actions/setup-python@", "msys2/setup-msys2@"):
            pr_step = next(step for step in job_steps(sidecar) if action in step)
            warm_step = next(step for step in job_steps(warm_sidecar) if action in step)
            self.assertEqual(step_inputs(warm_step), step_inputs(pr_step), action)
            self.assertEqual(re.search(r"uses: (\S+)", warm_step)[1], re.search(r"uses: (\S+)", pr_step)[1])
        self.assertIn("node desktop/scripts/build-docs-sidecar.mjs", warm_sidecar)

    def test_e2e_serves_the_dev_build_and_ci_build_the_release(self):
        e2e = re.search(
            r"^  test-e2e:\n(?P<section>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
            CI_WORKFLOW.read_text(),
            re.MULTILINE | re.DOTALL,
        ).group("section")
        self.assertIn("run: cargo build --locked --bin kronn", e2e)
        self.assertIn("./target/debug/kronn", e2e)
        self.assertNotIn("--release", e2e)
        self.assertIn("target/debug/deps", e2e)
        release = re.search(
            r"^  build-release:\n(?P<section>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
            BUILD_WORKFLOW.read_text(),
            re.MULTILINE | re.DOTALL,
        ).group("section")
        self.assertIn("cargo build --release --locked --bin kronn", release)

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
