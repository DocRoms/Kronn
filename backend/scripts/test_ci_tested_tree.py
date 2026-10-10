#!/usr/bin/env python3
"""Contract of scripts/ci/tested_tree_proof.sh against a fake GitHub API."""

from __future__ import annotations

import json
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/ci/tested_tree_proof.sh"
WORKFLOWS = ROOT / ".github/workflows"

# `gh api <path> --jq <expr>`: answers from responses.json filtered by jq, and
# exits 1 for a path it does not know (an API error).
FAKE_GH = """#!/usr/bin/env python3
import json, os, subprocess, sys
args = sys.argv[1:]
path = next(a for a in args if a.startswith("repos/")).split("?")[0]
with open(os.environ["FAKE_GH_STATE"]) as handle:
    state = json.load(handle)
with open(os.environ["FAKE_GH_STATE"] + ".calls", "a") as handle:
    handle.write(path + "\\n")
if path not in state:
    sys.stderr.write("HTTP 404: " + path + "\\n")
    sys.exit(1)
expr = args[args.index("--jq") + 1]
result = subprocess.run(["jq", "-r", expr], input=json.dumps(state[path]), capture_output=True, text=True, check=True)
sys.stdout.write(result.stdout)
"""

REPO = "DocRoms/Kronn"
TREE = "7" * 40
OTHER_TREE = "8" * 40
HEAD = "a" * 40
OLD_HEAD = "b" * 40
BASE = "c" * 40
GATE = "ci-quality-gates"
RUNS = f"repos/{REPO}/actions/workflows/ci-test.yml/runs"


def identity(head=HEAD, base=BASE, pr=42):
    return f"Gate verdict for pull_request #{pr} head {head} base {base}"


def run(run_id, head=HEAD, event="pull_request"):
    return {"id": run_id, "head_sha": head, "event": event}


def gate(conclusion="success", head=HEAD, base=BASE, status="completed", name=None):
    return {"jobs": [
        {"name": "test-backend", "status": "completed", "steps": []},
        {"name": GATE, "status": status, "steps": [
            {"name": "Require the ci-test label", "conclusion": "skipped"},
            {"name": name or identity(head, base), "conclusion": conclusion},
        ]},
    ]}


def commit(sha, tree=TREE):
    return {f"repos/{REPO}/git/commits/{sha}": {"sha": sha, "tree": {"sha": tree}}}


def compare(head=HEAD, base=BASE, status="ahead"):
    return {f"repos/{REPO}/compare/{base}...{head}": {"status": status}}


def jobs(run_id, payload):
    return {f"repos/{REPO}/actions/runs/{run_id}/jobs": payload}


@unittest.skipUnless(shutil.which("jq"), "jq is required")
class TestedTreeProofTests(unittest.TestCase):
    def proof(self, state: dict, calls: list | None = None) -> subprocess.CompletedProcess:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            gh = tmp_path / "gh"
            gh.write_text(FAKE_GH)
            gh.chmod(0o755)
            responses = tmp_path / "responses.json"
            responses.write_text(json.dumps(state))
            env = {
                **os.environ,
                "PATH": f"{tmp_path}:{os.environ['PATH']}",
                "FAKE_GH_STATE": str(responses),
                "GITHUB_REPOSITORY": REPO,
                "RELEASE_TREE": TREE,
            }
            result = subprocess.run(
                ["bash", str(SCRIPT), "ci-test.yml", GATE],
                capture_output=True, text=True, env=env, timeout=30,
            )
            log = pathlib.Path(str(responses) + ".calls")
            if calls is not None and log.exists():
                calls.extend(log.read_text().splitlines())
            return result

    def test_a_green_pull_request_run_on_the_same_tree_proves_it(self):
        result = self.proof({
            RUNS: {"workflow_runs": [run(300)]},
            **commit(HEAD), **jobs(300, gate()), **compare(),
        })
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Run 300 passed ci-quality-gates", result.stdout)
        self.assertIn(f"pull_request #42 head {HEAD} base {BASE}", result.stdout)

    def test_another_tree_is_not_proof(self):
        result = self.proof({
            RUNS: {"workflow_runs": [run(300)]},
            **commit(HEAD, OTHER_TREE), **jobs(300, gate()), **compare(),
        })
        self.assertEqual(result.returncode, 1)
        self.assertIn("No completed pull-request run", result.stderr)

    def test_the_newest_verdict_on_the_tree_decides(self):
        result = self.proof({
            RUNS: {"workflow_runs": [run(200, OLD_HEAD), run(300)]},
            **commit(HEAD), **commit(OLD_HEAD),
            **jobs(300, gate("failure")), **jobs(200, gate(head=OLD_HEAD)),
            **compare(), **compare(OLD_HEAD),
        })
        self.assertEqual(result.returncode, 1)
        self.assertIn("Run 300, the newest verdict", result.stderr)

    def test_a_base_that_is_not_an_ancestor_tested_another_tree(self):
        calls = []
        result = self.proof({
            RUNS: {"workflow_runs": [run(300), run(200, OLD_HEAD)]},
            **commit(HEAD), **commit(OLD_HEAD),
            **jobs(300, gate()), **jobs(200, gate(head=OLD_HEAD)),
            **compare(status="diverged"), **compare(OLD_HEAD),
        }, calls)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Run 300: base", result.stderr)
        self.assertIn("Run 200 passed", result.stdout)

    def test_a_behind_base_is_not_an_ancestor_either(self):
        result = self.proof({
            RUNS: {"workflow_runs": [run(300)]},
            **commit(HEAD), **jobs(300, gate()), **compare(status="behind"),
        })
        self.assertEqual(result.returncode, 1)

    def test_an_identical_base_counts_as_an_ancestor(self):
        result = self.proof({
            RUNS: {"workflow_runs": [run(300)]},
            **commit(HEAD), **jobs(300, gate()), **compare(status="identical"),
        })
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_a_run_without_the_label_is_passed_over(self):
        result = self.proof({
            RUNS: {"workflow_runs": [run(300), run(200)]},
            **commit(HEAD), **jobs(300, gate("skipped")), **jobs(200, gate()), **compare(),
        })
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Run 200 passed", result.stdout)

    def test_a_verdict_for_another_head_is_not_evidence(self):
        result = self.proof({
            RUNS: {"workflow_runs": [run(300)]},
            **commit(HEAD), **jobs(300, gate(name=identity(head=OLD_HEAD))), **compare(),
        })
        self.assertEqual(result.returncode, 1)
        self.assertIn("No completed", result.stderr)

    def test_a_dispatch_or_push_verdict_is_not_evidence(self):
        for name in (
            "Gate verdict for workflow_dispatch # head  base ",
            "Gate verdict for push # head  base ",
        ):
            result = self.proof({
                RUNS: {"workflow_runs": [run(300)]},
                **commit(HEAD), **jobs(300, gate(name=name)), **compare(),
            })
            self.assertEqual(result.returncode, 1, name)

    def test_only_pull_request_runs_are_read(self):
        calls = []
        result = self.proof({
            RUNS: {"workflow_runs": [run(300, event="workflow_dispatch")]},
            **commit(HEAD), **jobs(300, gate()), **compare(),
        }, calls)
        self.assertEqual(result.returncode, 1)
        self.assertNotIn(f"repos/{REPO}/actions/runs/300/jobs", calls)

    def test_an_unfinished_gate_is_passed_over(self):
        result = self.proof({
            RUNS: {"workflow_runs": [run(300)]},
            **commit(HEAD), **jobs(300, gate(status="in_progress")), **compare(),
        })
        self.assertEqual(result.returncode, 1)

    def test_an_unreadable_head_is_not_proof(self):
        result = self.proof({
            RUNS: {"workflow_runs": [run(300)]},
            **jobs(300, gate()), **compare(),
        })
        self.assertEqual(result.returncode, 1)

    def test_an_api_error_is_never_proof(self):
        for missing in (RUNS, f"repos/{REPO}/compare/{BASE}...{HEAD}"):
            state = {
                RUNS: {"workflow_runs": [run(300)]},
                **commit(HEAD), **jobs(300, gate()), **compare(),
            }
            del state[missing]
            result = self.proof(state)
            self.assertNotEqual(result.returncode, 0, missing)

    def test_a_head_shared_by_several_runs_is_read_once(self):
        calls = []
        self.proof({
            RUNS: {"workflow_runs": [run(300), run(299), run(298)]},
            **commit(HEAD, OTHER_TREE),
        }, calls)
        self.assertEqual(calls.count(f"repos/{REPO}/git/commits/{HEAD}"), 1)

    def test_the_script_reads_the_identity_the_aggregates_record(self):
        source = SCRIPT.read_text()
        self.assertIn("Gate verdict for pull_request #[0-9]+ head $head base [0-9a-f]{40}$", source)
        for name, job in (("ci-test.yml", "ci-quality-gates"), ("ci-build.yml", "ci-build-gates")):
            workflow = (WORKFLOWS / name).read_text()
            self.assertIn(
                '- name: "Gate verdict for ${{ github.event_name }} #${{ github.event.pull_request.number }} '
                'head ${{ github.event.pull_request.head.sha }} base ${{ github.event.pull_request.base.sha }}"',
                workflow, name,
            )
            self.assertRegex(
                (WORKFLOWS / "desktop-build.yml").read_text(),
                rf"ci_\w+:{re.escape(name)}:{job}",
            )


if __name__ == "__main__":
    unittest.main()
