#!/usr/bin/env python3
"""Contract of scripts/ci/supersede_verdicts.sh against a fake GitHub API."""

from __future__ import annotations

import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/ci/supersede_verdicts.sh"

# `gh api [-X PATCH -f k=v ...] <path> [--jq <expr>]`: a GET answers from
# responses.json, filtered by the caller's own jq; a path listed in "fail"
# exits 1. A PATCH is logged as one JSON line with its fields.
FAKE_GH = """#!/usr/bin/env python3
import json, os, subprocess, sys
args = sys.argv[1:]
path = next(a for a in args if a.startswith("repos/"))
bare = path.split("?")[0]
with open(os.environ["FAKE_GH_STATE"]) as handle:
    state = json.load(handle)
log = os.environ["FAKE_GH_STATE"] + ".calls"
if bare in state.get("fail", []):
    sys.exit(1)
if "-X" in args:
    fields = {}
    for index, arg in enumerate(args):
        if arg == "-f":
            key, _, value = args[index + 1].partition("=")
            fields[key] = value
    with open(log, "a") as handle:
        handle.write(json.dumps({"method": args[args.index("-X") + 1], "path": bare, "fields": fields}) + "\\n")
    sys.exit(0)
expr = args[args.index("--jq") + 1]
result = subprocess.run(["jq", "-r", expr], input=json.dumps(state[bare]), capture_output=True, text=True, check=True)
sys.stdout.write(result.stdout)
"""

REPO = "DocRoms/Kronn"
RUN = 300
OWN = 500  # this run's gate job id, which is also its check run id
HEAD = "abc123"
GATE = "ci-quality-gates"
RUNS = f"repos/{REPO}/actions/workflows/ci-verdict.yml/runs"
JOBS = f"repos/{REPO}/actions/runs/{RUN}/jobs"
CHECKS = f"repos/{REPO}/commits/{HEAD}/check-runs"


def verdict_run(run_id, status="completed"):
    return {"id": run_id, "head_sha": HEAD, "event": "pull_request", "status": status}


def check(check_id, name=GATE, conclusion="failure", status="completed", run_id=100):
    return {
        "id": check_id,
        "name": name,
        "status": status,
        "conclusion": conclusion,
        "details_url": f"https://github.com/{REPO}/actions/runs/{run_id}/job/{check_id}",
    }


def api(checks, runs=(), **extra):
    """The fake API: this run's gate job is OWN; `runs` are earlier verdict runs."""
    return {
        JOBS: {"jobs": [{"id": 9, "name": "ci-build-gates"}, {"id": OWN, "name": GATE}]},
        RUNS: {"workflow_runs": list(runs)},
        CHECKS: {"check_runs": checks},
        **extra,
    }


@unittest.skipUnless(shutil.which("jq"), "jq is required")
class SupersedeVerdictsTests(unittest.TestCase):
    def supersede(self, responses_body: dict, gate: str = GATE):
        """Returns (completed process, PATCH calls)."""
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            gh = tmp_path / "gh"
            gh.write_text(FAKE_GH)
            gh.chmod(0o755)
            responses = tmp_path / "responses.json"
            responses.write_text(json.dumps(responses_body))
            env = {
                **os.environ,
                "PATH": f"{tmp_path}:{os.environ['PATH']}",
                "FAKE_GH_STATE": str(responses),
                "GITHUB_REPOSITORY": REPO,
                "GITHUB_RUN_ID": str(RUN),
                "HEAD_SHA": HEAD,
                # Never sleep in a test: a run that does not finish is given up on.
                "SUPERSEDE_WAIT_SECONDS": "0",
            }
            result = subprocess.run(
                ["bash", str(SCRIPT), gate], capture_output=True, text=True, env=env, timeout=30
            )
            log = pathlib.Path(str(responses) + ".calls")
            calls = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
            return result, calls

    @staticmethod
    def patched(calls):
        return sorted(int(call["path"].rsplit("/", 1)[1]) for call in calls)

    def test_earlier_checks_become_neutral_and_name_the_latest_run(self):
        result, calls = self.supersede(api([
            check(1, conclusion="failure"),
            check(2, conclusion="failure"),
            check(3, conclusion="success"),
        ]))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.patched(calls), [1, 2, 3])
        for call in calls:
            self.assertEqual(call["method"], "PATCH")
            self.assertEqual(call["fields"]["status"], "completed")
            self.assertEqual(call["fields"]["conclusion"], "neutral")
            self.assertEqual(call["fields"]["output[title]"], f"Superseded by run {RUN}")
            self.assertIn(f"/actions/runs/{RUN}", call["fields"]["output[summary]"])

    def test_this_check_and_later_ones_are_never_touched(self):
        result, calls = self.supersede(api([
            check(200),
            check(OWN, conclusion=None, status="in_progress", run_id=RUN),
            check(OWN + 1, conclusion="success", run_id=400),
        ]))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.patched(calls), [200])

    def test_a_ci_tests_gate_of_the_same_name_that_predates_this_verdict_is_superseded(self):
        # ci-test.yml publishes `ci-quality-gates` too: the red one of the
        # `opened` run (no label yet) must not outlive the verdict that replaced it.
        result, calls = self.supersede(api([
            check(1, conclusion="failure", run_id=150),
            check(2, conclusion="cancelled", run_id=160),
            check(OWN + 5, conclusion="success", run_id=170),
        ]))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.patched(calls), [1, 2])

    def test_another_gate_is_left_alone(self):
        result, calls = self.supersede(api([check(1, name="ci-build-gates")]))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(calls, [])

    def test_already_superseded_or_unfinished_checks_are_skipped(self):
        result, calls = self.supersede(api([
            check(1, conclusion="neutral"),
            check(2, conclusion=None, status="in_progress"),
        ]))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(calls, [])

    def test_a_cancelled_earlier_verdict_is_superseded_too(self):
        result, calls = self.supersede(api([check(1, conclusion="cancelled")]))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.patched(calls), [1])

    def test_a_run_still_winding_down_is_awaited_only_for_the_set_time(self):
        # The cancelled run never completes in this fake: the wait is bounded.
        result, calls = self.supersede(
            api([check(1, conclusion="cancelled")], runs=[verdict_run(200, status="in_progress")])
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.patched(calls), [1])

    def test_no_earlier_check_is_a_no_op(self):
        result, calls = self.supersede(api([]))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("No earlier", result.stdout)
        self.assertEqual(calls, [])

    def test_a_refused_update_warns_and_never_fails_the_verdict(self):
        result, calls = self.supersede(api([check(1)], fail=[f"repos/{REPO}/check-runs/1"]))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("::warning::", result.stdout)
        self.assertEqual(calls, [])

    def test_an_unreadable_listing_warns_and_exits_green(self):
        result, calls = self.supersede(api([check(1)], fail=[CHECKS]))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("::warning::", result.stdout)
        self.assertEqual(calls, [])

    def test_an_unknown_own_job_warns_and_exits_green(self):
        result, calls = self.supersede(api([check(1)]), gate="ci-other-gates")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("::warning::", result.stdout)
        self.assertEqual(calls, [])


if __name__ == "__main__":
    unittest.main()
