#!/usr/bin/env python3
"""Every backend/scripts/ci/test_*.py is run by a workflow.

`discover -s backend/scripts` finds none of them (no __init__.py), so a test
only ran if a workflow step named it, and test_mcp_catalogue_census.py went
unrun that way. ci-test.yml now has ONE step that globs `scripts/ci/test_*.py`
and runs each file, so a new test is wired by existing.

This guard does not read shell. It asserts that this step is present, verbatim,
at step level, in the job that has the dependencies and the working directory:
a line-for-line match cannot be faked by a comment, an `env` value or a heredoc,
since those are indented deeper or begin with `#`.
"""

from __future__ import annotations

import fnmatch
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
CI_DIR = ROOT / "backend" / "scripts" / "ci"
WORKFLOW = ROOT / ".github" / "workflows" / "ci-test.yml"

JOB = "test-backend-quality"
JOB_DEFAULTS = [
    "    defaults:",
    "      run:",
    "        working-directory: backend",
]
GLOB = "scripts/ci/test_*.py"
STEP = [
    "      - name: CI tests — run every backend/scripts/ci/test_*.py",
    "        if: github.event_name != 'push'",
    "        run: |",
    "          fail=0",
    f"          for t in {GLOB}; do",
    '            echo "::group::$t"',
    '            python3 "$t" || fail=1',
    '            echo "::endgroup::"',
    "          done",
    '          exit "$fail"',
]
JOB_KEY = re.compile(r"^  ([A-Za-z0-9_-]+):\s*$")


def find_sublist(lines: list[str], block: list[str], start: int = 0) -> int:
    """Index where `block` appears as consecutive whole lines, or -1."""
    for i in range(start, len(lines) - len(block) + 1):
        if lines[i : i + len(block)] == block:
            return i
    return -1


def discovery_problems(text: str) -> list[str]:
    """Why `text` does not run every CI test, as human-readable lines."""
    lines = text.splitlines()
    at = find_sublist(lines, STEP)
    if at < 0:
        return ["the discovery step is missing or altered, expected:\n" + "\n".join(STEP)]
    job_at = next((i for i in range(at, -1, -1) if JOB_KEY.match(lines[i])), -1)
    job = JOB_KEY.match(lines[job_at]).group(1) if job_at >= 0 else None
    if job != JOB:
        return [f"the discovery step is in job {job!r}, expected {JOB!r}"]
    if find_sublist(lines[job_at:at], JOB_DEFAULTS) < 0:
        return [f"job {JOB} no longer runs in backend/ ({' '.join(s.strip() for s in JOB_DEFAULTS)})"]
    return []


def workflow(step: list[str], job: str = JOB, defaults: list[str] | None = None) -> str:
    defaults = JOB_DEFAULTS if defaults is None else defaults
    head = [f"  {job}:", "    runs-on: ubuntu-latest", *defaults, "    steps:"]
    return "\n".join(["jobs:", *head, *step, ""])


class CiTestsWiredTest(unittest.TestCase):
    def test_the_workflow_runs_every_ci_test(self):
        self.assertEqual(discovery_problems(WORKFLOW.read_text()), [])

    def test_the_glob_reaches_every_ci_test(self):
        tests = sorted(p.name for p in CI_DIR.rglob("test_*.py"))
        self.assertTrue(tests, "no test_*.py found: CI_DIR is wrong")
        pattern = pathlib.PurePosixPath(GLOB)
        self.assertEqual(f"backend/{pattern.parent}", CI_DIR.relative_to(ROOT).as_posix())
        missed = [t for t in tests if not fnmatch.fnmatchcase(t, pattern.name)]
        nested = [str(p.relative_to(CI_DIR)) for p in CI_DIR.rglob("test_*.py") if p.parent != CI_DIR]
        self.assertEqual((missed, nested), ([], []), "the step's glob is not recursive")

    def test_every_ci_test_runs_standalone(self):
        for path in sorted(CI_DIR.glob("test_*.py")):
            with self.subTest(test=path.name):
                self.assertIn("__main__", path.read_text(), "the loop runs `python3 <file>`")

    def test_the_exact_step_passes(self):
        self.assertEqual(discovery_problems(workflow(STEP)), [])

    def test_a_removed_or_altered_step_is_reported(self):
        altered = [
            [],
            [line.replace("test_*.py", "test_a.py") for line in STEP],
            [line.replace(" || fail=1", "") for line in STEP],
            [line.replace('exit "$fail"', "exit 0") for line in STEP],
            [line.replace("if: github.event_name != 'push'", "if: false") for line in STEP],
            ["      - name: x", "        run: true"],
        ]
        for step in altered:
            with self.subTest(step=step):
                self.assertTrue(discovery_problems(workflow(step)))

    def test_text_that_only_resembles_the_step_does_not_count(self):
        commented = ["      # " + line.strip() for line in STEP]
        deeper = ["    " + line for line in STEP]
        carrier = ["      - name: x", "        run: echo success", "        env:", "          NOTE: |", *deeper]
        heredoc = ["      - name: x", "        run: |", "          cat <<'NOTE'", *deeper, "          NOTE"]
        quoted = ['      - name: x\n        env: "note', *deeper, '      "']
        for name, step in (
            ("commented out", commented),
            ("multiline env value", carrier),
            ("heredoc payload", heredoc),
            ("quoted scalar", quoted),
        ):
            with self.subTest(case=name):
                self.assertTrue(discovery_problems(workflow(step)))

    def test_the_step_in_the_wrong_job_is_reported(self):
        self.assertTrue(discovery_problems(workflow(STEP, job="docs")))

    def test_a_job_that_left_backend_is_reported(self):
        for defaults in ([], ["    defaults:", "      run:", "        working-directory: ."]):
            with self.subTest(defaults=defaults):
                self.assertTrue(discovery_problems(workflow(STEP, defaults=defaults)))

    def test_a_step_in_an_earlier_job_does_not_borrow_the_next_ones_defaults(self):
        text = workflow(STEP, job="docs", defaults=[]) + workflow([], job=JOB)
        self.assertTrue(discovery_problems(text))


if __name__ == "__main__":
    unittest.main()
