#!/usr/bin/env python3
"""Contract of scripts/ci/previous_gate_verdict.sh against a fake GitHub API."""

from __future__ import annotations

import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/ci/previous_gate_verdict.sh"

# `gh api <path> --jq <expr>`: answer from responses.json, filtered by jq. A
# path with several answers serves them in order, then repeats the last one.
FAKE_GH = """#!/usr/bin/env python3
import json, os, subprocess, sys
args = sys.argv[1:]
path = args[1].split("?")[0]
expr = args[args.index("--jq") + 1]
with open(os.environ["FAKE_GH_STATE"]) as handle:
    state = json.load(handle)
log = os.environ["FAKE_GH_STATE"] + ".calls"
with open(log, "a") as handle:
    handle.write(path + "\\n")
calls = sum(1 for line in open(log) if line.strip() == path)
answers = state[path]
answer = answers[min(calls, len(answers)) - 1]
result = subprocess.run(["jq", "-r", expr], input=json.dumps(answer), capture_output=True, text=True, check=True)
sys.stdout.write(result.stdout)
"""

REPO = "DocRoms/Kronn"
RUN = 200
PR = 42
HEAD = "abc123"
BASE = "base111"
RUNS = f"repos/{REPO}/actions/workflows/ci-test.yml/runs"
GATE = "ci-quality-gates"
SAME_SECOND = "2026-10-08T10:00:00Z"


def identity(pr=PR, head=HEAD, base=BASE):
    return f"Gate verdict for pull_request #{pr} head {head} base {base}"


def run(run_id, event="pull_request", head=HEAD):
    return {"id": run_id, "event": event, "head_sha": head, "created_at": SAME_SECOND}


def listing(*runs):
    return {"workflow_runs": list(runs)}


def gate(status="completed", steps=()):
    return {"jobs": [{"name": GATE, "status": status, "steps": [{"name": n, "conclusion": c} for n, c in steps]}]}


def jobs_path(run_id):
    return f"repos/{REPO}/actions/runs/{run_id}/jobs"


@unittest.skipUnless(shutil.which("jq"), "jq is required")
class PreviousGateVerdictTests(unittest.TestCase):
    def verdict(self, state: dict) -> subprocess.CompletedProcess:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            gh = tmp_path / "gh"
            gh.write_text(FAKE_GH)
            gh.chmod(0o755)
            (tmp_path / "sleep").write_text("#!/bin/sh\nexit 0\n")
            (tmp_path / "sleep").chmod(0o755)
            responses = tmp_path / "responses.json"
            responses.write_text(json.dumps(state))
            env = {
                **os.environ,
                "PATH": f"{tmp_path}:{os.environ['PATH']}",
                "FAKE_GH_STATE": str(responses),
                "GITHUB_REPOSITORY": REPO,
                "GITHUB_RUN_ID": str(RUN),
                "PR_NUMBER": str(PR),
                "HEAD_SHA": HEAD,
                "BASE_SHA": BASE,
            }
            return subprocess.run(
                ["bash", str(SCRIPT), "ci-test.yml", GATE],
                capture_output=True,
                text=True,
                env=env,
                timeout=30,
            )

    def test_repeats_the_verdict_of_the_run_that_tested_this_state(self):
        result = self.verdict({
            RUNS: [listing(run(150), run(180))],
            jobs_path(180): [gate(steps=[(identity(), "success")])],
        })
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Run 180 passed", result.stdout)

    def test_repeats_an_earlier_failure(self):
        result = self.verdict({
            RUNS: [listing(run(180))],
            jobs_path(180): [gate(steps=[(identity(), "failure")])],
        })
        self.assertEqual(result.returncode, 1)
        self.assertIn("did not pass", result.stdout)

    def test_a_predecessor_created_in_the_same_second_counts(self):
        result = self.verdict({
            RUNS: [listing(run(199))],
            jobs_path(199): [gate(steps=[(identity(), "success")])],
        })
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_never_reads_a_later_run(self):
        result = self.verdict({
            RUNS: [listing(run(210))],
            jobs_path(210): [gate(steps=[(identity(), "success")])],
        })
        self.assertEqual(result.returncode, 1)
        self.assertIn("No earlier ci-test.yml run", result.stdout)

    def test_a_green_dispatch_on_the_same_commit_is_not_evidence(self):
        result = self.verdict({
            RUNS: [listing(run(180, event="workflow_dispatch"))],
            jobs_path(180): [gate(steps=[(identity(), "success")])],
        })
        self.assertEqual(result.returncode, 1)
        self.assertIn("No earlier", result.stdout)

    def test_another_pull_request_or_base_is_not_evidence(self):
        for other in (identity(pr=7), identity(base="base222")):
            result = self.verdict({
                RUNS: [listing(run(180))],
                jobs_path(180): [gate(steps=[(other, "success")])],
            })
            self.assertEqual(result.returncode, 1, other)
            self.assertIn("No earlier", result.stdout)

    def test_a_run_that_only_repeated_a_verdict_is_skipped(self):
        result = self.verdict({
            RUNS: [listing(run(150), run(180))],
            jobs_path(180): [gate(steps=[
                ("Repeat the verdict already given for this pull request", "success"),
                (identity(), "skipped"),
            ])],
            jobs_path(150): [gate(steps=[(identity(), "failure")])],
        })
        self.assertEqual(result.returncode, 1)
        self.assertIn("Run 150 did not pass", result.stdout)

    def test_waits_for_a_running_verdict(self):
        result = self.verdict({
            RUNS: [listing(run(180))],
            jobs_path(180): [
                {"jobs": [{"name": "test-backend", "status": "in_progress", "steps": []}]},
                gate(status="in_progress"),
                gate(steps=[(identity(), "success")]),
            ],
            f"repos/{REPO}/actions/runs/180": [{"status": "in_progress"}],
        })
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout.count("Waiting for run 180"), 2)

    def test_a_finished_run_without_the_gate_is_not_evidence(self):
        result = self.verdict({
            RUNS: [listing(run(180))],
            jobs_path(180): [{"jobs": [{"name": "test-backend", "status": "completed", "steps": []}]}],
            f"repos/{REPO}/actions/runs/180": [{"status": "completed"}],
        })
        self.assertEqual(result.returncode, 1)
        self.assertIn("No earlier", result.stdout)


if __name__ == "__main__":
    unittest.main()
