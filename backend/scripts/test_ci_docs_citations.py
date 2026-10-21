#!/usr/bin/env python3
"""Contract of the doc-citation gate's CI wiring (KT-1195).

scripts/ci/docs_citations_changed.sh against real merge commits, and the
workflow, Makefile and release checklist that pass its result to
backend/src/core/docs_citations.rs.
"""

from __future__ import annotations

import os
import pathlib
import re
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/ci/docs_citations_changed.sh"
CI_WORKFLOW = ROOT / ".github/workflows/ci-test.yml"
RUST = ROOT / "backend/src/core/docs_citations.rs"
GATE_TEST = "core::docs_citations::tests::docs_citations_resolve_in_the_repo"


def rust_const(name: str) -> str:
    return re.search(rf'pub const {name}: &str = "([^"]+)";', RUST.read_text())[1]


def job_section(workflow: str, job: str) -> str:
    return re.search(
        rf"^  {job}:\n(?P<section>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
        workflow,
        re.MULTILINE | re.DOTALL,
    ).group("section")


def step(section: str, name: str) -> str:
    return re.search(
        rf"^      - name: {re.escape(name)}\n(?P<body>.*?)(?=^      - |\Z)",
        section,
        re.MULTILINE | re.DOTALL,
    ).group("body")


class ChangedPathsScript(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = pathlib.Path(self.tmp.name) / "repo"
        self.repo.mkdir()
        self.git("init", "-q", "-b", "base")
        self.write("backend/src/runner.rs", "fn run() {}\n")
        self.write("docs/a.md", "doc\n")
        self.commit("base")

    def tearDown(self):
        self.tmp.cleanup()

    def git(self, *args: str) -> str:
        return subprocess.run(
            ["git", "-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", *args],
            cwd=self.repo, check=True, capture_output=True, text=True,
        ).stdout

    def write(self, path: str, text: str):
        target = self.repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)

    def commit(self, message: str):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message)

    def merge_pull_request(self, changes: dict[str, str]):
        """Builds the merge commit GitHub checks out: base tip first, head second."""
        self.git("checkout", "-q", "-b", "pr")
        for path, text in changes.items():
            self.write(path, text)
        self.commit("pr")
        self.git("checkout", "-q", "base")
        # The base moved too: its own changes must not count as the PR's.
        self.write("backend/src/other.rs", "fn other() {}\n")
        self.commit("base moved")
        self.git("merge", "-q", "--no-ff", "-m", "merge", "pr")

    def run_script(self):
        env_file = pathlib.Path(self.tmp.name) / "github_env"
        env_file.write_text("")
        out = pathlib.Path(self.tmp.name) / "changed.txt"
        result = subprocess.run(
            [str(SCRIPT), str(out)], cwd=self.repo, capture_output=True, text=True,
            env={**os.environ, "GITHUB_ENV": str(env_file)},
        )
        return result, out, env_file.read_text()

    def test_a_code_only_pull_request_lists_no_doc(self):
        self.merge_pull_request({"backend/src/runner.rs": "fn run() { 1 }\n"})
        result, out, env = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(out.read_text(), "backend/src/runner.rs\n")
        self.assertEqual(env, f"{rust_const('CHANGED_ENV')}={out}\n")
        self.assertIn("1 changed path(s), 0 under docs/", result.stdout)

    def test_a_docs_pull_request_lists_its_docs(self):
        self.merge_pull_request({"docs/a.md": "new\n", "backend/docs/b.md": "b\n"})
        result, out, _ = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sorted(out.read_text().split()), ["backend/docs/b.md", "docs/a.md"])
        self.assertIn("2 changed path(s), 2 under docs/", result.stdout)

    def test_a_non_ascii_doc_name_is_listed_unquoted(self):
        # Quoted ("docs/\303\251t\303\251.md") it would not read as a doc.
        self.merge_pull_request({"docs/été.md": "new\n"})
        result, out, _ = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(out.read_text(encoding="utf-8"), "docs/été.md\n")

    def test_a_deleted_doc_is_not_listed(self):
        self.git("checkout", "-q", "-b", "pr")
        self.git("rm", "-q", "docs/a.md")
        self.commit("pr")
        self.git("checkout", "-q", "base")
        self.git("merge", "-q", "--no-ff", "-m", "merge", "pr")
        result, out, _ = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(out.read_text(), "")
        self.assertIn("0 changed path(s), 0 under docs/", result.stdout)

    def test_a_commit_that_is_not_a_merge_fails_and_exports_nothing(self):
        self.write("docs/a.md", "edited\n")
        self.commit("plain")
        result, out, env = self.run_script()
        self.assertEqual(result.returncode, 1)
        self.assertIn("::error::HEAD is not a pull request merge commit", result.stdout)
        self.assertEqual(env, "")
        self.assertFalse(out.exists())


class Wiring(unittest.TestCase):
    def test_ci_tests_pass_the_scope_and_publish_the_report(self):
        backend = job_section(CI_WORKFLOW.read_text(), "test-backend")
        self.assertIn("fetch-depth: ${{ github.event_name == 'pull_request' && 2 || 1 }}", backend)
        scope = step(backend, "Doc citations — paths this pull request changes")
        self.assertIn("if: github.event_name == 'pull_request'", scope)
        self.assertIn('../scripts/ci/docs_citations_changed.sh "$RUNNER_TEMP/docs-citations-changed.txt"', scope)
        suite = "cargo llvm-cov nextest — measured backend critical path and coverage floor"
        self.assertLess(backend.index("Doc citations — paths this pull request changes"), backend.index(suite))
        report_dir = rust_const("REPORT_DIR_ENV")
        self.assertIn(f"{report_dir}: ${{{{ runner.temp }}}}/docs-citations", step(backend, suite))
        publish = step(backend, "Doc citations — annotations and summary")
        self.assertLess(backend.index(suite), backend.index("Doc citations — annotations and summary"))
        self.assertIn("if: always()", publish)
        self.assertIn('dir="$RUNNER_TEMP/docs-citations"', publish)
        self.assertIn('cat "$dir/summary.md" >> "$GITHUB_STEP_SUMMARY"', publish)
        self.assertIn('cat "$dir/annotations.txt"', publish)

    def test_the_repair_command_and_the_release_step_run_the_re_anchor(self):
        makefile = (ROOT / "Makefile").read_text()
        target = re.search(r"^docs-reanchor:\n(?P<body>(?:\t.*\n)+)", makefile, re.MULTILINE)["body"]
        self.assertIn(f'{rust_const("REANCHOR_ENV")}="$(or $(DOCS),all)"', target)
        self.assertIn(f"cargo test --lib {GATE_TEST}", target)
        releasing = (ROOT / "docs/operations/releasing.md").read_text()
        steps = releasing.split("## Steps", 1)[1].split("\n## ", 1)[0]
        self.assertIn("`make docs-reanchor`", steps)


if __name__ == "__main__":
    unittest.main()
