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

# `gh api [-X POST] <path> [--jq <expr>]`: answer from responses.json, filtered
# by jq. A path with several answers serves them in order, then repeats the
# last one. A POST is logged as "POST <path>" and answers with the exit code
# stored under that key (0 when absent).
FAKE_GH = """#!/usr/bin/env python3
import json, os, subprocess, sys
args = sys.argv[1:]
path = next(a for a in args if a.startswith("repos/")).split("?")[0]
with open(os.environ["FAKE_GH_STATE"]) as handle:
    state = json.load(handle)
log = os.environ["FAKE_GH_STATE"] + ".calls"
if "-X" in args:
    with open(log, "a") as handle:
        handle.write("POST " + path + "\\n")
    sys.exit(state.get("POST " + path, 0))
expr = args[args.index("--jq") + 1]
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


LABEL_STEP = "Require the ci-test label"


def gate(status="completed", steps=(), attempt=1):
    return {"jobs": [{
        "name": GATE,
        "status": status,
        "run_attempt": attempt,
        "steps": [{"name": n, "conclusion": c} for n, c in steps],
    }]}


def unlabelled(attempt=1):
    return gate(steps=[(LABEL_STEP, "failure"), (identity(), "skipped")], attempt=attempt)


def jobs_path(run_id):
    return f"repos/{REPO}/actions/runs/{run_id}/jobs"


@unittest.skipUnless(shutil.which("jq"), "jq is required")
class PreviousGateVerdictTests(unittest.TestCase):
    def verdict(self, state: dict, calls: list | None = None) -> subprocess.CompletedProcess:
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
            result = subprocess.run(
                ["bash", str(SCRIPT), "ci-test.yml", GATE, "ci-test"],
                capture_output=True,
                text=True,
                env=env,
                timeout=30,
            )
            log = pathlib.Path(str(responses) + ".calls")
            if calls is not None and log.exists():
                calls.extend(log.read_text().splitlines())
            return result

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

    def test_a_run_for_another_base_is_passed_over_without_a_rerun(self):
        calls = []
        result = self.verdict({
            RUNS: [listing(run(150), run(180))],
            jobs_path(180): [gate(steps=[(identity(base="old"), "success")])],
            jobs_path(150): [gate(steps=[(identity(), "failure")])],
        }, calls)
        self.assertEqual(result.returncode, 1)
        self.assertIn("Run 150 did not pass", result.stdout)
        self.assertFalse([c for c in calls if c.startswith("POST")])

    def test_a_run_that_predates_the_label_is_rerun_then_repeated(self):
        calls = []
        result = self.verdict({
            RUNS: [listing(run(180))],
            jobs_path(180): [
                unlabelled(),
                # Right after the request, the API may still serve attempt 1.
                unlabelled(),
                gate(status="in_progress", attempt=2),
                gate(steps=[(LABEL_STEP, "skipped"), (identity(), "success")], attempt=2),
            ],
        }, calls)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("re-running it", result.stdout)
        self.assertEqual(calls.count(f"POST repos/{REPO}/actions/runs/180/rerun"), 1)

    def test_a_rerun_that_still_sees_no_label_fails(self):
        result = self.verdict({
            RUNS: [listing(run(180))],
            jobs_path(180): [unlabelled(), unlabelled(attempt=2)],
        })
        self.assertEqual(result.returncode, 1)
        self.assertIn("still saw no ci-test label", result.stdout)

    def test_a_rerun_started_by_a_sibling_verdict_is_awaited(self):
        result = self.verdict({
            RUNS: [listing(run(180))],
            f"POST {jobs_path(180)[:-5]}/rerun": 1,
            jobs_path(180): [
                unlabelled(),
                gate(steps=[(identity(), "success")], attempt=2),
            ],
            f"repos/{REPO}/actions/runs/180": [{"status": "in_progress", "run_attempt": 2}],
        })
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("already being re-run", result.stdout)

    def test_a_refused_rerun_names_the_manual_way(self):
        result = self.verdict({
            RUNS: [listing(run(180))],
            f"POST {jobs_path(180)[:-5]}/rerun": 1,
            jobs_path(180): [unlabelled()],
            f"repos/{REPO}/actions/runs/180": [{"status": "completed", "run_attempt": 1}],
        })
        self.assertEqual(result.returncode, 1)
        self.assertIn("Re-run all jobs", result.stdout)

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
