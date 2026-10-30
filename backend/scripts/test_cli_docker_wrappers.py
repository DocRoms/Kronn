#!/usr/bin/env python3
"""Regression contracts for trusted CLI credentials in the Docker image."""

from __future__ import annotations

import os
import pathlib
import re
import shutil
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
WORKFLOWS = ROOT / ".github/workflows"
IMAGES_WORKFLOW = WORKFLOWS / "docker-images.yml"
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
                "require-ci-label", "test-backend-build", "test-backend-shard",
                "test-backend", "test-backend-quality",
                "duplication-check", "test-python", "test-frontend", "test-e2e",
                "test-shell", "security-scan", "ci-quality-gates",
                "backend-ci-performance",
            ),
            BUILD_WORKFLOW: (
                "require-ci-build-label", "test-desktop-compile", "test-docs-sidecar-windows",
                "test-backend-portability", "build-release", "ci-build-gates",
            ),
            VERDICT_WORKFLOW: ("ci-quality-gates", "ci-build-gates"),
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
                "test-backend-build": 40,
                "test-backend": 20,
                "test-e2e": 45,
                "build-release": 45,
                "test-backend-portability": "${{ matrix.os == 'macos-latest' && 100 || 45 }}",
            }.get(job, 60 if path == VERDICT_WORKFLOW else 30)
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
        def ci_job(name):
            return re.search(
                rf"^  {re.escape(name)}:\n(?P<section>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
                workflow,
                re.MULTILINE | re.DOTALL,
            ).group("section")

        # The suite is one build, three shards and one merging report.
        build_job = ci_job("test-backend-build")
        shard_job = ci_job("test-backend-shard")
        report_job = ci_job("test-backend")
        backend = build_job + shard_job + report_job
        self.assertIn("timeout-minutes: 40", build_job)
        self.assertIn("cargo nextest archive --workspace --locked", build_job)
        self.assertIn('eval "$(cargo llvm-cov show-env --sh)"', build_job)
        self.assertIn("shard: [1, 2, 3]", shard_job)
        self.assertIn("--partition slice:${{ matrix.shard }}/3", shard_job)
        self.assertIn("KRONN_TEMPLATES_DIR: ${{ github.workspace }}/templates", shard_job)
        self.assertIn("KRONN_TEMPLATES_DIR: ${{ github.workspace }}/templates", report_job)
        self.assertIn("needs: [require-ci-label, test-backend-build, test-backend-shard]", report_job)
        self.assertIn("pattern: backend-profraw-*", report_job)
        self.assertIn("cargo llvm-cov report --summary-only", report_job)
        self.assertIn("-E 'test(export_bindings)'", report_job)
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
            build_job.index("Build the instrumented test archive"),
            build_job.index("Mark bounded compiled backend cache ready"),
        )
        # One test pass: every test runs once, in exactly one shard, under
        # nextest's `ci` profile and coverage.
        self.assertIn("NEXTEST_PROFILE: ci", shard_job)
        # Without a pool, one profraw per test process fills the runner disk.
        self.assertIn("shard-${{ matrix.shard }}-%8m.profraw", shard_job)
        self.assertNotIn("%p", shard_job)
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
        # Any complete non-hit tree (miss or partial restore) is marked so the
        # push to main can save it, even when the suite itself failed.
        self.assertIn("always()", mark_section)
        self.assertIn("steps.verify-backend-cache.outputs.state != 'hit'", mark_section)
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
        self.assertIn("CI_COMPILED_CACHE_HIT: ${{ needs.test-backend-build.outputs.compiled_cache_hit }}", workflow)
        self.assertIn("CI_COMPILED_CACHE_STATE: ${{ needs.test-backend-build.outputs.compiled_cache_state }}", workflow)
        hot_cache = re.search(
            r"Restore cargo registry and bounded backend build \(hot\)(?P<section>.*?)(?=^      - |\Z)",
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
        # cargo clean refuses a restored target directory without its tag.
        self.assertIn("target/llvm-cov-target/CACHEDIR.TAG", hot_cache)
        self.assertIn("Signature: 8a477f597d28d172789f06886806bc55", backend)
        self.assertIn("cargo-hot-v3-", hot_cache)
        # Pull requests restore (exact key, then the same-OS prefix) and never
        # save: an Actions cache saved by a pull request is private to it and
        # only evicts the shared main cache from the 10 GB budget.
        self.assertIn("uses: actions/cache/restore@v4", hot_cache)
        self.assertIn("restore-keys: cargo-hot-v3-${{ runner.os }}-", hot_cache)
        for workflow_text in (workflow, build):
            saves = list(re.finditer(r"uses: actions/cache/save@v4", workflow_text))
            self.assertTrue(saves)
            for save in saves:
                step = workflow_text.rfind("      - name:", 0, save.start())
                condition = workflow_text[step:save.start()]
                self.assertIn("github.event_name == 'push'", condition)
                self.assertIn("github.ref == 'refs/heads/main'", condition)
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
            self.assertIn("checks: write", section)
            self.assertIn(
                f"        if: \"!contains(github.event.pull_request.labels.*.name, '{label}')\"\n"
                f"        run: |\n", section,
            )
            self.assertIn(f"run: scripts/ci/previous_gate_verdict.sh {path.name} {gate} {label}\n", section)
            for name in ("PR_NUMBER", "HEAD_SHA", "BASE_SHA"):
                self.assertIn(f"          {name}: ${{{{ github.event.pull_request.", section)
            # Only the label guard and the superseding step are conditional;
            # the verdict always runs. Superseding must not run on a cancel.
            self.assertEqual(section.count("        if: "), 2, gate)
            self.assertIn(
                "        if: success() || failure()\n"
                "        env:\n"
                "          GH_TOKEN: ${{ github.token }}\n"
                "          HEAD_SHA: ${{ github.event.pull_request.head.sha }}\n"
                f"        run: scripts/ci/supersede_verdicts.sh {gate}\n", section,
            )
            self.assertLess(
                section.index("previous_gate_verdict.sh"), section.index("supersede_verdicts.sh"), gate
            )

    def test_a_push_to_main_only_builds_and_saves_the_rust_caches(self):
        # The merged tree was tested on its pull request: a push to main
        # compiles what the pull requests restore, saves it, and runs nothing.
        no_push = "github.event_name != 'push'"
        workflow = CI_WORKFLOW.read_text()
        build = BUILD_WORKFLOW.read_text()
        jobs = {**job_sections(workflow), **job_sections(build)}
        for name in (
            "duplication-check", "diff-hygiene", "test-python", "test-frontend",
            "test-shell", "security-scan", "backend-ci-performance",
            "test-desktop-compile", "test-docs-sidecar-windows",
            "test-backend-shard", "test-backend",
        ):
            condition = re.search(r"^    if: (.*)$", jobs[name], re.MULTILINE).group(1)
            self.assertIn(no_push, condition, name)

        def step(job, name):
            match = re.search(
                rf"^      - name: {re.escape(name)}\n(?P<body>.*?)(?=^      - |\Z)",
                jobs[job],
                re.MULTILINE | re.DOTALL,
            )
            self.assertIsNotNone(match, f"{job}: {name}")
            return match.group("body")

        for job, name in (
            ("test-backend-build", "cargo fmt"),
            ("test-backend-build", "Lint — no raw Command::new in prod code"),
            ("test-backend-build", "Upload the instrumented test archive"),
            ("test-backend-quality", "cargo test --doc"),
            ("test-e2e", "Run E2E tests"),
            ("test-backend-portability", "Worktree lifecycle and disk guards"),
            ("test-backend-portability", "Windows to WSL agent launches"),
        ):
            self.assertIn(f"if: {no_push}", step(job, name), f"{job}: {name}")
        # The same archive build runs on a pull request and on a push: the
        # cache main saves is exactly what a pull request compiles.
        archive_build = step("test-backend-build", "Build the instrumented test archive")
        self.assertNotIn("if:", archive_build)
        self.assertIn("cargo nextest archive --workspace --locked", archive_build)
        lib_build = step("test-backend-portability", "Build the lib tests without running them (push to main)")
        self.assertIn("cargo test --lib --no-run --locked", lib_build)
        # A failed build must never be saved under the exact key: the next
        # run would see a hit and never replace it. The backend's own save is
        # gated by its verified `ready` marker instead.
        for text in (workflow, build):
            for save in re.finditer(r"uses: actions/cache/save@v4", text):
                start = text.rfind("      - name:", 0, save.start())
                condition = text[start:save.start()]
                if "steps.mark-backend-cache.outputs.ready == 'true'" in condition:
                    continue
                self.assertIn("if: success() &&", condition)

    def test_each_test_workflow_reads_its_label_live_and_aggregates_every_job(self):
        for path, label, route, gate in TEST_WORKFLOWS:
            workflow = path.read_text()
            jobs = job_sections(workflow)
            self.assertIn("  push:\n    branches: [main]", workflow)
            # A tag reruns nothing: no workflow calls these.
            self.assertNotIn("workflow_call:", workflow)
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
            # Only a push to main may count a skipped job: it refreshes caches.
            # The build gate also counts the portability job skipped because
            # the change does not touch what it tests, and only that one.
            if path == BUILD_WORKFLOW:
                self.assertIn('or ($event == "push" and .value.result == "skipped")', aggregate)
                self.assertIn(
                    '(.key == "test-backend-portability" and .value.result == "skipped" and $portability == "false")',
                    aggregate,
                )
                self.assertIn('.value.result == "success"', aggregate)
            else:
                self.assertIn(
                    """'all(.[]; . == "success" or ($event == "push" and . == "skipped"))'""",
                    aggregate,
                )
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
        desktop = (WORKFLOWS / "desktop-build.yml").read_text()
        for name in ("ci-test.yml", "ci-build.yml"):
            self.assertNotIn(f"uses: ./.github/workflows/{name}", desktop)

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


class DockerImagesWorkflowTests(unittest.TestCase):
    """KT-1468: a release publishes the images `make start` pulls, so nobody
    compiles the 19 GB backend crate on their own machine."""

    def setUp(self):
        self.workflow = IMAGES_WORKFLOW.read_text()
        self.jobs = job_sections(self.workflow)

    def test_it_runs_on_release_tags_and_can_republish_a_tag(self):
        self.assertRegex(self.workflow, r"(?m)^  push:\n(?:    #.*\n)*    tags: \[")
        self.assertIn("'[0-9]+.[0-9]+.[0-9]+*'", self.workflow)
        dispatch = re.search(r"(?m)^  workflow_dispatch:\n(?P<body>(?:    .*\n)+)", self.workflow)
        self.assertIsNotNone(dispatch)
        self.assertRegex(dispatch.group("body"), r"(?m)^      tag:\n(?:        .*\n)*        required: true")
        self.assertEqual(
            list(self.jobs), ["resolve", "build", "manifest", "latest", "verify-public"],
        )

    def test_it_may_write_packages_and_nothing_else(self):
        permissions = re.search(r"(?m)^permissions:\n(?P<body>(?:  .*\n)+)", self.workflow).group("body")
        self.assertEqual(
            sorted(line.strip() for line in permissions.splitlines()),
            ["contents: read", "packages: write"],
        )
        self.assertIn("password: ${{ secrets.GITHUB_TOKEN }}", self.workflow)

    def test_the_tag_is_checked_against_version_before_anything_is_built(self):
        self.assertIn('scripts/check-version-sync.sh "$RELEASE_TAG"', self.jobs["resolve"])
        self.assertIn("needs: resolve", self.jobs["build"])
        # `latest` is never written by the manifest job, only by the serialized one.
        self.assertNotIn(":latest", self.jobs["manifest"])

    def step_script(self, job: str, name: str) -> str:
        step = re.search(
            rf"(?ms)^      - name: {re.escape(name)}\n.*?^        run: \|\n(?P<body>(?:          [^\n]*\n|\n)+)",
            self.jobs[job],
        )
        self.assertIsNotNone(step, name)
        return "\n".join(line[10:] for line in step.group("body").splitlines())

    def manifest_script(self) -> str:
        return self.step_script("manifest", "Create the multi-arch manifest")

    def latest_script(self) -> str:
        return self.step_script("latest", "Point latest at the highest published stable release")

    def test_the_manifest_job_writes_only_the_version_tag(self):
        image = "ghcr.io/docroms/kronn-backend"
        self.assertEqual(
            self.run_manifest("0.15.2"),
            [["buildx", "imagetools", "create", "-t", f"{image}:0.15.2",
              f"{image}@sha256:{'a' * 64}", f"{image}@sha256:{'b' * 64}"]],
        )

    def run_manifest(self, version: str) -> list[list[str]]:
        """Run the real 'Create the multi-arch manifest' step with a recording
        `docker`. Returns each docker invocation."""
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            digests = root / "digests"
            digests.mkdir()
            for digest in ("a" * 64, "b" * 64):
                (digests / digest).touch()
            bin_dir = root / "bin"
            bin_dir.mkdir()
            calls = root / "docker.log"
            docker = bin_dir / "docker"
            docker.write_text(f'#!/bin/sh\necho "$@" >> "{calls}"\n')
            docker.chmod(0o755)
            result = subprocess.run(
                ["bash", "-e", "-c", self.manifest_script()],
                cwd=digests,
                env={
                    **os.environ,
                    "PATH": f"{bin_dir}{os.pathsep}{os.environ['PATH']}",
                    "IMAGE": "ghcr.io/docroms/kronn-backend",
                    "VERSION": version,
                },
                capture_output=True, text=True, check=False,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            return [line.split() for line in calls.read_text().splitlines()]

    class Registry:
        """A fake GHCR and git remote shared by simulated runs of the `latest`
        step: the repository's git tags, the image tags that exist, and the
        version the `latest` image tag points at."""

        DOCKER = """#!/bin/sh
echo "$@" >> "@ROOT@/docker.log"
case "$3" in
  inspect)
    if [ -f "@ROOT@/inspect_error" ]; then cat "@ROOT@/inspect_error" >&2; exit 1; fi
    grep -qxF "${4##*:}" "@ROOT@/published" || { echo "ERROR: $4: not found" >&2; exit 1; } ;;
  create) if [ "${5##*:}" = latest ]; then echo "${6##*:}" > "@ROOT@/latest"; fi ;;
esac
"""

        def __init__(self, root: pathlib.Path, tags: list[str], published: list[str], latest: str | None = None):
            self.root = root
            self.published = set(published)
            self.env = {
                **os.environ,
                "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.com",
                "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.com",
                "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_SYSTEM": os.devnull,
            }
            self.origin = root / "origin"
            self.origin.mkdir()
            self.git("init", "-q")
            self.git("commit", "-q", "--allow-empty", "-m", "release")
            for tag in tags:
                self.git("tag", tag)
            self.bin = root / "bin"
            self.bin.mkdir()
            docker = self.bin / "docker"
            docker.write_text(self.DOCKER.replace("@ROOT@", str(root)))
            docker.chmod(0o755)
            self.write_published()
            if latest is not None:
                (root / "latest").write_text(latest + "\n")

        def git(self, *args: str) -> None:
            subprocess.run(["git", *args], cwd=self.origin, env=self.env, check=True, capture_output=True)

        def write_published(self) -> None:
            (self.root / "published").write_text("".join(v + "\n" for v in sorted(self.published)))

        def publish(self, version: str) -> None:
            self.published.add(version)
            self.write_published()

        def read_latest(self) -> str | None:
            path = self.root / "latest"
            return path.read_text().strip() if path.exists() else None

    def run_latest(
        self, registry: "DockerImagesWorkflowTests.Registry", version: str,
        expect_rc: int = 0, remote: str | None = None,
    ) -> subprocess.CompletedProcess:
        """One run of the real `latest` step, as GitHub runs it once the run
        holds the concurrency group."""
        result = subprocess.run(
            ["bash", "-e", "-c", self.latest_script()],
            env={
                **registry.env,
                "PATH": f"{registry.bin}{os.pathsep}{os.environ['PATH']}",
                "IMAGE": "ghcr.io/docroms/kronn-backend",
                "VERSION": version,
                "TAGS_REMOTE": remote or str(registry.origin),
                "INSPECT_RETRY_DELAY": "0",
            },
            capture_output=True, text=True, check=False,
        )
        self.assertEqual(result.returncode, expect_rc, result.stdout + result.stderr)
        return result

    def test_a_failing_inspect_of_the_image_just_published_fails_the_job_after_retries(self):
        # The run just wrote 0.15.2 and it is the highest release: an inspect
        # error (auth, network) must not read as "not published yet" in a green run.
        with tempfile.TemporaryDirectory() as tmp:
            registry = self.Registry(pathlib.Path(tmp), ["0.15.1", "0.15.2"], ["0.15.1", "0.15.2"], latest="0.15.1")
            (registry.root / "inspect_error").write_text("denied: requested access to the resource is denied\n")
            result = self.run_latest(registry, "0.15.2", expect_rc=1)
            self.assertIn("denied: requested access to the resource is denied", result.stderr)
            self.assertIn("::error::", result.stdout)
            self.assertNotIn("not published yet", result.stdout)
            self.assertEqual(registry.read_latest(), "0.15.1")
            calls = (registry.root / "docker.log").read_text().splitlines()
            self.assertEqual(len(calls), 3, calls)  # short retry, then fatal

    def test_a_transient_inspect_error_on_the_image_just_published_is_retried(self):
        with tempfile.TemporaryDirectory() as tmp:
            registry = self.Registry(pathlib.Path(tmp), ["0.15.1", "0.15.2"], ["0.15.1", "0.15.2"], latest="0.15.1")
            # Fails once, then the registry answers.
            docker = registry.bin / "docker"
            docker.write_text(
                docker.read_text().replace(
                    '  inspect)\n',
                    '  inspect)\n    n=$(wc -l < "@ROOT@/docker.log"); '
                    'if [ "$n" -le 1 ]; then echo "i/o timeout" >&2; exit 1; fi\n'.replace("@ROOT@", str(registry.root)),
                    1,
                )
            )
            result = self.run_latest(registry, "0.15.2")
            self.assertIn("i/o timeout", result.stderr)
            self.assertEqual(registry.read_latest(), "0.15.2")

    def test_an_inspect_error_on_the_highest_release_of_another_run_fails_the_job(self):
        # 0.16.0 is the highest and published (or so the tags say), but the
        # inspect of it errors out: that is not an absence. Reading it as one
        # would end green with `latest` stuck on 0.15.1 and no job left to fix it.
        for error in ("i/o timeout", "denied: requested access to the resource is denied", "500 Internal Server Error"):
            with self.subTest(error=error), tempfile.TemporaryDirectory() as tmp:
                registry = self.Registry(
                    pathlib.Path(tmp), ["0.15.1", "0.15.2", "0.16.0"], ["0.15.1", "0.15.2", "0.16.0"], latest="0.15.1")
                (registry.root / "inspect_error").write_text(error + "\n")
                result = self.run_latest(registry, "0.15.2", expect_rc=1)
                self.assertIn(error, result.stderr)
                self.assertIn("::error::Cannot inspect", result.stdout)
                self.assertNotIn("not published yet", result.stdout)
                self.assertEqual(registry.read_latest(), "0.15.1")
                calls = (registry.root / "docker.log").read_text().splitlines()
                self.assertEqual(len(calls), 3, calls)  # short retry, then fatal

    def test_a_transient_inspect_error_on_the_highest_release_of_another_run_is_retried(self):
        with tempfile.TemporaryDirectory() as tmp:
            registry = self.Registry(
                pathlib.Path(tmp), ["0.15.1", "0.15.2", "0.16.0"], ["0.15.1", "0.15.2", "0.16.0"], latest="0.15.1")
            docker = registry.bin / "docker"
            docker.write_text(
                docker.read_text().replace(
                    '  inspect)\n',
                    '  inspect)\n    n=$(wc -l < "@ROOT@/docker.log"); '
                    'if [ "$n" -le 1 ]; then echo "i/o timeout" >&2; exit 1; fi\n'.replace("@ROOT@", str(registry.root)),
                    1,
                )
            )
            self.run_latest(registry, "0.15.2")
            self.assertEqual(registry.read_latest(), "0.16.0")

    def test_a_registry_that_confirms_the_highest_release_is_absent_leaves_latest_alone(self):
        # The highest release (0.16.0) belongs to another run that has not
        # published it yet: the registry says "not found", which is no failure.
        for answer in (None, "manifest unknown"):
            with self.subTest(answer=answer), tempfile.TemporaryDirectory() as tmp:
                registry = self.Registry(
                    pathlib.Path(tmp), ["0.15.1", "0.15.2", "0.16.0"], ["0.15.1", "0.15.2"], latest="0.15.1")
                if answer:
                    (registry.root / "inspect_error").write_text(answer + "\n")
                result = self.run_latest(registry, "0.15.2")
                self.assertIn("not published yet", result.stdout)
                self.assertEqual(registry.read_latest(), "0.15.1")
                calls = (registry.root / "docker.log").read_text().splitlines()
                self.assertEqual(len(calls), 1, calls)  # an absence is final, not retried

    def test_the_image_this_run_just_published_missing_is_fatal(self):
        # Even a "not found" is a failure for the release this run wrote.
        with tempfile.TemporaryDirectory() as tmp:
            registry = self.Registry(pathlib.Path(tmp), ["0.15.1", "0.15.2"], ["0.15.1"], latest="0.15.1")
            result = self.run_latest(registry, "0.15.2", expect_rc=1)
            self.assertIn("::error::Cannot inspect", result.stdout)
            self.assertEqual(registry.read_latest(), "0.15.1")

    def test_a_failing_git_ls_remote_fails_the_job(self):
        with tempfile.TemporaryDirectory() as tmp:
            registry = self.Registry(pathlib.Path(tmp), ["0.15.2"], ["0.15.2"], latest="0.15.1")
            missing = pathlib.Path(tmp) / "no-such-remote"
            result = self.run_latest(registry, "0.15.2", expect_rc=1, remote=str(missing))
            self.assertIn("fatal:", result.stderr)  # git's own diagnostic is shown
            self.assertIn("::error::git ls-remote failed", result.stdout)
            self.assertEqual(registry.read_latest(), "0.15.1")
            self.assertFalse((registry.root / "docker.log").exists())  # nothing written

    def test_latest_follows_only_the_highest_stable_release_tag(self):
        tags = ["0.9.7", "0.15.1", "0.15.2", "0.16.0-rc1", "v0.15.0"]
        published = ["0.9.7", "0.15.0", "0.15.1", "0.15.2", "0.16.0-rc1"]
        with tempfile.TemporaryDirectory() as tmp:
            # The newest stable release (0.15.2 > 0.15.1 > 0.9.7, as versions, not text).
            registry = self.Registry(pathlib.Path(tmp), tags, published)
            self.run_latest(registry, "0.15.2")
            self.assertEqual(registry.read_latest(), "0.15.2")
        for older in ("0.15.1", "0.9.7", "0.15.0"):
            with tempfile.TemporaryDirectory() as tmp:
                # Republishing an older stable release must not move `latest` backwards.
                registry = self.Registry(pathlib.Path(tmp), tags, published, latest="0.15.2")
                self.run_latest(registry, older)
                self.assertEqual(registry.read_latest(), "0.15.2", older)
        with tempfile.TemporaryDirectory() as tmp:
            # A prerelease never takes `latest`, even above every stable tag.
            registry = self.Registry(pathlib.Path(tmp), tags, published, latest="0.15.2")
            self.run_latest(registry, "0.16.0-rc1")
            self.assertEqual(registry.read_latest(), "0.15.2")
        with tempfile.TemporaryDirectory() as tmp:
            # A `v` prefix on the git tag is the same release: the image tag has none.
            registry = self.Registry(pathlib.Path(tmp), ["0.15.1", "v0.15.2"], ["0.15.2"])
            self.run_latest(registry, "0.15.2")
            self.assertEqual(registry.read_latest(), "0.15.2")
        with tempfile.TemporaryDirectory() as tmp:
            # The first release of a repository is the highest one.
            registry = self.Registry(pathlib.Path(tmp), ["0.1.0"], ["0.1.0"])
            self.run_latest(registry, "0.1.0")
            self.assertEqual(registry.read_latest(), "0.1.0")

    def test_two_overlapping_releases_end_with_latest_on_the_newest(self):
        # 0.15.2 is tagged, then 0.16.0 is tagged while 0.15.2 is still building.
        # The group runs their `latest` steps one at a time, in whichever order
        # they finish: the last write must be 0.16.0 either way.
        for order in (("0.15.2", "0.16.0"), ("0.16.0", "0.15.2")):
            with self.subTest(order=order), tempfile.TemporaryDirectory() as tmp:
                registry = self.Registry(
                    pathlib.Path(tmp), ["0.15.1", "0.15.2", "0.16.0"], ["0.15.1"], latest="0.15.1")
                for version in order:
                    registry.publish(version)  # the manifest job ran just before its latest step
                    self.run_latest(registry, version)
                self.assertEqual(registry.read_latest(), "0.16.0")

    def test_a_newer_tag_without_images_leaves_latest_alone_until_it_publishes(self):
        # 0.16.0 is tagged but its images are not published when the 0.15.2 run
        # reaches `latest`: pointing at 0.16.0 would point at nothing, and
        # pointing at 0.15.2 would be overwritten later anyway. It stays; the
        # 0.16.0 run moves it once its manifest exists.
        with tempfile.TemporaryDirectory() as tmp:
            registry = self.Registry(
                pathlib.Path(tmp), ["0.15.1", "0.15.2", "0.16.0"], ["0.15.1", "0.15.2"], latest="0.15.1")
            result = self.run_latest(registry, "0.15.2")
            self.assertEqual(registry.read_latest(), "0.15.1")
            self.assertIn("not published yet", result.stdout)
            registry.publish("0.16.0")
            self.run_latest(registry, "0.16.0")
            self.assertEqual(registry.read_latest(), "0.16.0")

    def test_latest_is_serialized_and_never_cancels_the_run_that_is_writing(self):
        latest = self.jobs["latest"]
        # Queue, do not cancel; one group per image so backend and frontend do not wait on each other.
        self.assertRegex(
            latest,
            r"(?m)^    concurrency:\n      group: docker-images-latest-\$\{\{ matrix\.service \}\}\n      cancel-in-progress: false\n",
        )
        self.assertIn("needs: [resolve, manifest]", latest)
        # The decision is made inside the group, not frozen by an earlier job.
        self.assertNotIn("latest", self.workflow_outputs("resolve"))
        self.assertNotIn("needs.resolve.outputs.latest", self.workflow)
        self.assertNotRegex(self.jobs["manifest"], r"(?m)^    concurrency:")

    def workflow_outputs(self, job: str) -> str:
        return re.search(r"(?ms)^    outputs:\n(?P<body>(?:      [^\n]*\n)+)", self.jobs[job]).group("body")

    def test_each_architecture_builds_natively_never_under_qemu(self):
        build = self.jobs["build"]
        for service in ("backend", "frontend"):
            self.assertIn(
                f"service: {service}, arch: amd64, runner: ubuntu-24.04, platform: linux/amd64", build)
            self.assertIn(
                f"service: {service}, arch: arm64, runner: ubuntu-24.04-arm, platform: linux/arm64", build)
        self.assertNotIn("setup-qemu-action", self.workflow)
        # One platform per build: a multi-platform build would emulate the other.
        self.assertIn("platforms: ${{ matrix.platform }}", build)
        self.assertNotRegex(build, r"platforms:.*,")
        self.assertIn("matrix.service == 'backend' && 'CARGO_PROFILE=release'", build)

    def test_republishing_an_old_tag_uses_the_current_runtime_user_packaging(self):
        build = self.jobs["build"]
        # A tag older than the UID/GID remap would otherwise ship an image that
        # only works for host uid 1000.
        self.assertIn("ref: ${{ github.sha }}", build)
        self.assertIn("backend/Dockerfile\n            backend/entrypoint.sh", build)
        self.assertIn("cp packaging/backend/Dockerfile backend/Dockerfile", build)
        self.assertIn("cp packaging/backend/entrypoint.sh backend/entrypoint.sh", build)
        self.assertIn("grep -q kronn_drop_to_host_user backend/entrypoint.sh", build)
        self.assertEqual(build.count("if: matrix.service == 'backend'"), 2)

    def test_the_manifest_ties_both_architectures_under_one_tag(self):
        build, manifest = self.jobs["build"], self.jobs["manifest"]
        self.assertIn("push-by-digest=true", build)
        self.assertIn("provenance: false", build)
        self.assertIn("docker buildx imagetools create", manifest)
        self.assertIn('"${#refs[@]}" -ne 2', manifest)
        self.assertIn("needs: [resolve, build]", manifest)
        self.assertIn("service: [backend, frontend]", manifest)

    def test_the_images_use_the_path_compose_pulls(self):
        self.assertIn("IMAGE_PREFIX: ghcr.io/docroms", self.workflow)
        compose = COMPOSE.read_text()
        for service in ("backend", "frontend"):
            self.assertRegex(
                compose,
                rf"image: ghcr\.io/docroms/kronn-{service}:\$\{{KRONN_IMAGE_TAG:-\d+\.\d+\.\d+\}}",
            )
            self.assertIn(f"kronn-${{{{ matrix.service }}}}", self.workflow)
        # The `build:` stays: `make start-build` and an untagged checkout use it.
        self.assertGreaterEqual(len(re.findall(r"(?m)^    build:$", compose)), 2)

    def test_an_anonymous_pull_of_both_architectures_is_verified(self):
        verify = self.jobs["verify-public"]
        self.assertIn("needs: [resolve, manifest]", verify)
        self.assertNotIn("login-action", verify)
        self.assertIn("linux/amd64 linux/arm64", verify)
        self.assertIn("Change visibility > Public", verify)

    def test_every_job_is_bounded(self):
        for name, section in self.jobs.items():
            self.assertRegex(section, r"(?m)^    timeout-minutes: \d+$", name)

    def test_actionlint_accepts_it_when_installed(self):
        actionlint = shutil.which("actionlint")
        if actionlint is None:
            self.skipTest("actionlint is not installed; the structural tests above run everywhere")
        result = subprocess.run(
            [actionlint, str(IMAGES_WORKFLOW)], capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
