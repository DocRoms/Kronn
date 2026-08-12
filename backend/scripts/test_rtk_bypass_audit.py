#!/usr/bin/env python3
"""Tests for the RTK bypass audit — KT-197 DoD 2 and 5.

An adoption metric fails by producing a plausible number. This one already did:
its first run reported 0% adoption on a session that used `rtk` 445 times,
because a wrapped call was not recognised as an invocation at all. So the tests
below pin both directions — a wrapped call must count as wrapped, and an
unwrapped one must count as missed — plus the classification DoD 5 requires:
a command RTK would run raw anyway is NOT a miss.

Run: python3 backend/scripts/test_rtk_bypass_audit.py
"""

from __future__ import annotations

import importlib.util
import json
import pathlib
import tempfile
import unittest

_SCRIPT = pathlib.Path(__file__).with_name("rtk_bypass_audit.py")


def _load():
    spec = importlib.util.spec_from_file_location("rtk_bypass_audit", _SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ClassifyTests(unittest.TestCase):
    def setUp(self):
        self.mod = _load()

    def one(self, command):
        found = self.mod.classify(command)
        self.assertEqual(len(found), 1, f"{len(found)} segments for {command!r}")
        return found[0]

    # ── the bug that made this file necessary ──────────────────────

    def test_a_wrapped_call_counts_as_wrapped(self):
        # The first run of this script reported 0% adoption on a session that used
        # rtk 445 times: `rtk git status` was not recognised as a git invocation
        # at all, so wrapped calls vanished instead of counting.
        item = self.one("rtk git status")
        self.assertEqual(item["command"], "git status")
        self.assertTrue(item["wrapped"])

    def test_an_unwrapped_call_counts_as_missed(self):
        item = self.one("git status")
        self.assertFalse(item["wrapped"])
        self.assertFalse(item["expected_probe"])

    # ── DoD 2: forms that bypass a shell function ─────────────────

    def test_an_absolute_path_is_detected_as_a_bypass(self):
        # `/usr/bin/grep` reaches the real binary past any function or alias, and
        # `rtk discover` cannot see it — it reads the leading word.
        item = self.one("/usr/bin/grep -rn pattern src")
        self.assertEqual(item["command"], "grep")
        self.assertFalse(item["wrapped"])
        self.assertEqual(item["bypass_form"], "/usr/bin/")

    def test_command_prefix_is_detected(self):
        item = self.one("command grep -rn pattern src")
        self.assertEqual(item["bypass_form"], "command ")

    def test_a_backslash_escape_is_detected(self):
        item = self.one(r"\grep -rn pattern src")
        self.assertEqual(item["bypass_form"], "backslash")

    def test_a_plain_call_has_no_bypass_form(self):
        # Missed, but not a deliberate evasion — the distinction matters because
        # the remedies differ: one is a habit, the other a shell setup.
        item = self.one("grep -rn pattern src")
        self.assertIsNone(item["bypass_form"])

    # ── DoD 5: probes are not misses ──────────────────────────────

    def test_a_quiet_probe_is_not_a_miss(self):
        # `grep -q` exists for its exit code. RTK runs it raw, so wrapping saves
        # nothing and counting it would inflate the residual.
        item = self.one("grep -q needle file")
        self.assertTrue(item["expected_probe"])

    def test_count_and_list_flags_are_not_misses(self):
        for flags in ("-c", "-l", "-L", "--count"):
            item = self.one(f"grep {flags} needle file")
            self.assertTrue(item["expected_probe"], flags)

    def test_a_raw_flag_bundled_with_others_is_still_detected(self):
        # `-rq` contains `-q`; missing that would count a probe as a miss.
        self.assertTrue(self.one("grep -rq needle src")["expected_probe"])

    def test_an_ordinary_flag_bundle_is_still_a_miss(self):
        # `-rn` is the common search form and RTK compresses it — this must NOT
        # be excused as a probe, or the metric would hide its biggest item.
        self.assertFalse(self.one("grep -rn needle src")["expected_probe"])

    # ── compound commands ─────────────────────────────────────────

    def test_a_compound_line_is_judged_per_segment(self):
        # `rtk git add . && git commit` wraps the first half only. Treating the
        # line as one decision would hide the half that pays full price.
        found = self.mod.classify("rtk git add . && git commit -m x")
        self.assertEqual(len(found), 2)
        self.assertTrue(found[0]["wrapped"])
        self.assertFalse(found[1]["wrapped"])

    def test_a_pipe_splits_segments_too(self):
        found = self.mod.classify("git log | grep fix")
        self.assertEqual([f["command"] for f in found], ["git log", "grep"])

    def test_an_unfiltered_command_is_ignored_entirely(self):
        # RTK passes these through, so counting them would invent a residual.
        self.assertEqual(self.mod.classify("echo hello"), [])
        self.assertEqual(self.mod.classify("mkdir -p /tmp/x"), [])

    def test_the_longest_matching_name_wins(self):
        # "git status" must not be reported as bare "git".
        self.assertEqual(self.one("git status --short")["command"], "git status")


class AuditTests(unittest.TestCase):
    def setUp(self):
        self.mod = _load()
        self.dir = tempfile.TemporaryDirectory()
        self.path = pathlib.Path(self.dir.name) / "t.jsonl"

    def tearDown(self):
        self.dir.cleanup()

    def _transcript(self, pairs):
        """pairs: [(command, result_text)]"""
        lines = []
        for index, (command, result) in enumerate(pairs):
            call_id = f"c{index}"
            lines.append({
                "message": {"content": [
                    {"type": "tool_use", "id": call_id, "name": "Bash",
                     "input": {"command": command}}
                ]}
            })
            lines.append({
                "message": {"content": [
                    {"type": "tool_result", "tool_use_id": call_id, "content": result}
                ]}
            })
        self.path.write_text("\n".join(json.dumps(line) for line in lines) + "\n")

    def test_adoption_is_wrapped_over_eligible_excluding_probes(self):
        self._transcript([
            ("rtk git status", "ok"),
            ("git status", "a lot of output"),
            ("grep -q x f", "..."),  # probe: neither wrapped nor missed
        ])
        report = self.mod.audit(self.path)
        self.assertEqual(report["wrapped"], 1)
        self.assertEqual(report["missed"], 1)
        self.assertEqual(report["expected_probes"], 1)
        self.assertEqual(report["adoption_ratio"], 0.5)

    def test_bytes_are_attributed_to_unwrapped_calls_only(self):
        self._transcript([
            ("rtk git diff", "x" * 1000),
            ("git diff", "y" * 500),
        ])
        report = self.mod.audit(self.path)
        # Only the unwrapped call's output is counted as compressible.
        self.assertEqual(report["unwrapped_result_bytes"].get("git diff"), 500)

    def test_a_transcript_with_no_bash_calls_reports_no_adoption_ratio(self):
        # None, not 0% — nothing eligible is not the same as nothing adopted.
        self.path.write_text("")
        report = self.mod.audit(self.path)
        self.assertIsNone(report["adoption_ratio"])

    def test_unparseable_lines_do_not_stop_the_scan(self):
        self.path.write_text('{bad json\n' + json.dumps({
            "message": {"content": [
                {"type": "tool_use", "id": "c1", "name": "Bash",
                 "input": {"command": "git status"}}
            ]}
        }) + "\n")
        self.assertEqual(self.mod.audit(self.path)["bash_calls"], 1)

    def test_a_call_without_a_result_still_counts_as_a_call(self):
        # The last call of a live session has no result yet; dropping it would
        # understate adoption by exactly the calls in flight.
        self.path.write_text(json.dumps({
            "message": {"content": [
                {"type": "tool_use", "id": "c1", "name": "Bash",
                 "input": {"command": "rtk git status"}}
            ]}
        }) + "\n")
        self.assertEqual(self.mod.audit(self.path)["bash_calls"], 1)

    def test_read_cost_is_split_between_images_and_source(self):
        # THE reason for the split: on the measured session images were 8 360 241 B
        # from 23 calls while targeted source reads averaged 2 269 B. One average
        # over both would have recommended pagination for a PNG.
        lines = []
        for index, (path, extra, payload) in enumerate([
            ("/x/shot.png", {}, "P" * 400_000),
            ("/x/main.rs", {"offset": 10, "limit": 20}, "code"),
            ("/x/whole.rs", {}, "all of it"),
        ]):
            call_id = f"r{index}"
            lines.append({"message": {"content": [
                {"type": "tool_use", "id": call_id, "name": "Read",
                 "input": {"file_path": path, **extra}}
            ]}})
            lines.append({"message": {"content": [
                {"type": "tool_result", "tool_use_id": call_id, "content": payload}
            ]}})
        self.path.write_text("\n".join(json.dumps(line) for line in lines) + "\n")

        report = self.mod.audit(self.path)
        buckets = report["read_bytes"]
        self.assertEqual(buckets["binary_whole"]["calls"], 1)
        self.assertEqual(buckets["text_targeted"]["calls"], 1)
        self.assertEqual(buckets["text_whole"]["calls"], 1)
        # The image dominates, which is the finding the split exists to surface.
        self.assertGreater(
            buckets["binary_whole"]["bytes"],
            buckets["text_targeted"]["bytes"] + buckets["text_whole"]["bytes"],
        )

    def test_a_read_is_not_counted_as_a_bash_call(self):
        # Mixing them would inflate the shell figure with file reads and make the
        # adoption ratio meaningless.
        self.path.write_text(json.dumps({"message": {"content": [
            {"type": "tool_use", "id": "r1", "name": "Read",
             "input": {"file_path": "/x/a.rs"}}
        ]}}) + "\n")
        report = self.mod.audit(self.path)
        self.assertEqual(report["bash_calls"], 0)

    def test_files_read_more_than_twice_are_listed(self):
        lines = []
        for index in range(4):
            call_id = f"r{index}"
            lines.append({"message": {"content": [
                {"type": "tool_use", "id": call_id, "name": "Read",
                 "input": {"file_path": "/x/hot.rs", "offset": index}}
            ]}})
            lines.append({"message": {"content": [
                {"type": "tool_result", "tool_use_id": call_id, "content": "c"}
            ]}})
        self.path.write_text("\n".join(json.dumps(line) for line in lines) + "\n")
        self.assertEqual(self.mod.audit(self.path)["reread_files"], {"hot.rs": 4})

    def test_a_file_read_twice_is_not_flagged(self):
        # Twice is normal navigation; flagging it would bury the real hotspots.
        lines = []
        for index in range(2):
            call_id = f"r{index}"
            lines.append({"message": {"content": [
                {"type": "tool_use", "id": call_id, "name": "Read",
                 "input": {"file_path": "/x/ok.rs"}}
            ]}})
            lines.append({"message": {"content": [
                {"type": "tool_result", "tool_use_id": call_id, "content": "c"}
            ]}})
        self.path.write_text("\n".join(json.dumps(line) for line in lines) + "\n")
        self.assertEqual(self.mod.audit(self.path)["reread_files"], {})

    def test_non_bash_tools_are_ignored(self):
        self.path.write_text(json.dumps({
            "message": {"content": [
                {"type": "tool_use", "id": "c1", "name": "Read",
                 "input": {"file_path": "/tmp/x"}}
            ]}
        }) + "\n")
        self.assertEqual(self.mod.audit(self.path)["bash_calls"], 0)


if __name__ == "__main__":
    unittest.main()
