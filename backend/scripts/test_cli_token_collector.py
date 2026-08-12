#!/usr/bin/env python3
"""Tests for the joined-CLI token collector — KT-190.

A telemetry collector fails in a particular way: it returns a number that looks
fine. So these tests are mostly about the cases where a wrong answer would be
invisible — a partial trailing line, a rotated file, an unsupported vendor,
counters silently summed together.

Run: python3 backend/scripts/test_cli_token_collector.py
"""

from __future__ import annotations

import importlib.util
import json
import pathlib
import tempfile
import unittest

_SCRIPT = pathlib.Path(__file__).with_name("cli_token_collector.py")


def _load():
    spec = importlib.util.spec_from_file_location("cli_token_collector", _SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _usage(**kw):
    return {
        "timestamp": kw.pop("timestamp", "2026-08-05T00:00:00.000Z"),
        "message": {
            "model": kw.pop("model", "claude-opus-5"),
            "usage": {
                "input_tokens": kw.pop("input", 0),
                "cache_creation_input_tokens": kw.pop("cache_creation", 0),
                "cache_read_input_tokens": kw.pop("cache_read", 0),
                "output_tokens": kw.pop("output", 0),
            },
        },
    }


class CollectorTests(unittest.TestCase):
    def setUp(self):
        self.mod = _load()
        self.dir = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.dir.name)
        self.project = self.root / "-some-project"
        self.project.mkdir()

    def tearDown(self):
        self.dir.cleanup()

    def _write(self, conversation_id, records, partial_tail=None):
        path = self.project / f"{conversation_id}.jsonl"
        with path.open("w") as handle:
            for record in records:
                handle.write(json.dumps(record) + "\n")
            if partial_tail is not None:
                handle.write(partial_tail)  # no newline: still being written
        return path

    # ── the counters must stay apart ────────────────────────────────

    def test_the_four_counters_are_never_merged(self):
        # Collapsing them is how a report becomes a lie: on a real session
        # cache reads were 98.4% of traffic and are billed at a tenth.
        path = self._write("c1", [_usage(input=1, cache_creation=2, cache_read=3, output=4)])
        totals = self.mod.collect_claude(path)["counters"]
        self.assertEqual(totals, {"input": 1, "cache_creation": 2,
                                  "cache_read": 3, "output": 4})

    def test_counters_accumulate_across_responses(self):
        path = self._write("c1", [_usage(output=10), _usage(output=5)])
        result = self.mod.collect_claude(path)
        self.assertEqual(result["counters"]["output"], 15)
        self.assertEqual(result["measured_responses"], 2)

    def test_a_non_integer_counter_is_ignored_not_coerced(self):
        # A vendor field that changes shape must not become a fabricated number.
        record = _usage(output=7)
        record["message"]["usage"]["input_tokens"] = "lots"
        path = self._write("c1", [record])
        totals = self.mod.collect_claude(path)["counters"]
        self.assertEqual(totals["input"], 0)
        self.assertEqual(totals["output"], 7)

    def test_a_boolean_is_not_counted_as_one(self):
        record = _usage()
        record["message"]["usage"]["output_tokens"] = True
        path = self._write("c1", [record])
        self.assertEqual(self.mod.collect_claude(path)["counters"]["output"], 0)

    # ── incremental reads must be additive ─────────────────────────

    def test_two_reads_equal_one_read(self):
        path = self._write("c1", [_usage(output=3), _usage(output=4)])
        whole = self.mod.collect_claude(path)
        first = self.mod.collect_claude(path, since_offset=0)
        # Re-reading from the returned offset must add nothing.
        again = self.mod.collect_claude(path, since_offset=first["next_offset"])
        self.assertEqual(again["measured_responses"], 0)
        self.assertEqual(again["counters"]["output"], 0)
        self.assertEqual(whole["counters"]["output"], 7)

    def test_appended_records_are_picked_up_from_the_offset(self):
        path = self._write("c1", [_usage(output=3)])
        first = self.mod.collect_claude(path)
        with path.open("a") as handle:
            handle.write(json.dumps(_usage(output=9)) + "\n")
        second = self.mod.collect_claude(path, since_offset=first["next_offset"])
        self.assertEqual(second["counters"]["output"], 9)
        self.assertEqual(second["measured_responses"], 1)

    def test_a_half_written_line_is_re_read_not_skipped(self):
        # The transcript is appended to live. Advancing past an incomplete line
        # would drop that response's counters forever, silently.
        tail = json.dumps(_usage(output=100))[:20]
        path = self._write("c1", [_usage(output=1)], partial_tail=tail)
        first = self.mod.collect_claude(path)
        self.assertEqual(first["counters"]["output"], 1)
        # Complete the line, then resume: the full record must arrive.
        with path.open("a") as handle:
            handle.write(json.dumps(_usage(output=100))[20:] + "\n")
        second = self.mod.collect_claude(path, since_offset=first["next_offset"])
        self.assertEqual(second["counters"]["output"], 100)

    def test_a_shrunken_file_reports_truncation_instead_of_double_counting(self):
        # Reset the cursor and re-read, and every token is counted twice.
        path = self._write("c1", [_usage(output=5)])
        result = self.mod.collect_claude(path, since_offset=10_000_000)
        self.assertTrue(result["truncated"])
        self.assertEqual(result["measured_responses"], 0)
        self.assertEqual(result["next_offset"], 0)

    def test_unparseable_lines_do_not_stop_the_scan(self):
        path = self.project / "c1.jsonl"
        with path.open("w") as handle:
            handle.write("{not json\n")
            handle.write(json.dumps(_usage(output=8)) + "\n")
        self.assertEqual(self.mod.collect_claude(path)["counters"]["output"], 8)

    # ── absence must never read as zero ───────────────────────────

    def test_a_missing_transcript_is_not_measured_rather_than_zero(self):
        result = self.mod.collect_for_session("claude-code", "nope", root=self.root)
        self.assertEqual(result["status"], "not_measured")
        self.assertIsNone(result["provenance"])
        self.assertNotIn("counters", result)

    def test_a_found_transcript_is_reported_as_measured_with_provenance(self):
        self._write("c1", [_usage(output=2)])
        result = self.mod.collect_for_session("claude-code", "c1", root=self.root)
        self.assertEqual(result["status"], "measured")
        self.assertEqual(result["provenance"], "claude-code-transcript")

    def test_a_records_only_transcript_reports_zero_measured_not_a_total(self):
        # Lines exist but none carry usage: that is a real zero, and it must be
        # distinguishable from "no transcript".
        path = self._write("c1", [{"timestamp": "2026-08-05T00:00:00Z", "type": "user"}])
        result = self.mod.collect_claude(path)
        self.assertEqual(result["measured_responses"], 0)
        self.assertEqual(result["lines_read"], 1)

    # ── lookup must not be steerable ──────────────────────────────

    def test_a_path_shaped_id_is_refused(self):
        # Otherwise a caller could aim the collector at any file on disk.
        for evil in ("../../etc/passwd", "a/b", "a\\b"):
            self.assertIsNone(self.mod.find_claude_transcript(evil, root=self.root))

    def test_lookup_searches_project_dirs_instead_of_deriving_a_path(self):
        # A session that moved into a git worktree lives under the WORKTREE's
        # slug, not the repo's — deriving the directory from cwd finds nothing.
        other = self.root / "-a-worktree-somewhere"
        other.mkdir()
        (other / "c9.jsonl").write_text(json.dumps(_usage(output=4)) + "\n")
        found = self.mod.find_claude_transcript("c9", root=self.root)
        self.assertIsNotNone(found)
        self.assertEqual(found.parent.name, "-a-worktree-somewhere")

    def test_a_missing_root_is_handled(self):
        self.assertIsNone(
            self.mod.find_claude_transcript("c1", root=self.root / "absent")
        )

    # ── provenance and window ─────────────────────────────────────

    def test_the_time_window_spans_first_to_last_record(self):
        path = self._write("c1", [
            _usage(timestamp="2026-08-01T00:00:00Z"),
            _usage(timestamp="2026-08-03T00:00:00Z"),
        ])
        result = self.mod.collect_claude(path)
        self.assertEqual(result["window_start"], "2026-08-01T00:00:00Z")
        self.assertEqual(result["window_end"], "2026-08-03T00:00:00Z")

    def test_models_are_counted_per_name(self):
        path = self._write("c1", [
            _usage(model="claude-opus-5"),
            _usage(model="claude-fable-5"),
            _usage(model="claude-opus-5"),
        ])
        self.assertEqual(
            self.mod.collect_claude(path)["models"],
            {"claude-opus-5": 2, "claude-fable-5": 1},
        )

    def test_a_response_without_a_model_is_labelled_not_dropped(self):
        record = _usage(output=1)
        del record["message"]["model"]
        path = self._write("c1", [record])
        result = self.mod.collect_claude(path)
        self.assertEqual(result["models"], {"unknown": 1})
        self.assertEqual(result["measured_responses"], 1)


class VibeCollectorTests(unittest.TestCase):
    """Vibe reports session TOTALS in a snapshot and gives no cache split.

    That asymmetry is the point: its cache counters are absent, not zero.
    Storing them as 0 would let a dashboard assert that Vibe performs no cache
    reads — a claim about a field nobody measured.
    """

    def setUp(self):
        self.mod = _load()
        self.dir = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.dir.name)

    def tearDown(self):
        self.dir.cleanup()

    def _session(self, session_id, stats=None, model="mistral-medium-3.5",
                 dirname=None):
        session_dir = self.root / (dirname or f"session_20260726_{session_id[:8]}")
        session_dir.mkdir(parents=True, exist_ok=True)
        meta = {
            "session_id": session_id,
            "start_time": "2026-07-26T15:46:42.578885+00:00",
            "end_time": "2026-07-26T17:12:12.440465+00:00",
            "config": {"active_model": model},
            "stats": {
                "steps": 152,
                "session_prompt_tokens": 14_126_817,
                "session_completion_tokens": 39_907,
                "session_cost": 21.489528,
                "input_price_per_million": 1.5,
                "output_price_per_million": 7.5,
            } if stats is None else stats,
        }
        (session_dir / "meta.json").write_text(json.dumps(meta))
        return session_dir / "meta.json"

    def test_cache_counters_are_absent_not_zero(self):
        result = self.mod.collect_vibe(self._session("s-1"))
        self.assertNotIn("cache_read", result["counters"])
        self.assertNotIn("cache_creation", result["counters"])
        self.assertEqual(
            sorted(result["unmeasured"]), ["cache_creation", "cache_read"]
        )

    def test_the_counters_it_does_report_are_kept(self):
        counters = self.mod.collect_vibe(self._session("s-1"))["counters"]
        self.assertEqual(counters, {"input": 14_126_817, "output": 39_907})

    def test_the_vendor_cost_is_labelled_as_the_vendors_own(self):
        # Vibe computes cost from its own per-million prices. Mixing it with a
        # Kronn estimate would make neither figure checkable.
        result = self.mod.collect_vibe(self._session("s-1"))
        self.assertEqual(result["vendor_cost_usd"], 21.489528)
        self.assertEqual(result["vendor_price_per_million"]["input"], 1.5)

    def test_the_model_and_window_come_through(self):
        result = self.mod.collect_vibe(self._session("s-1", model="mistral-large"))
        self.assertEqual(result["models"], {"mistral-large": 152})
        self.assertTrue(result["window_start"].startswith("2026-07-26"))

    def test_no_offset_is_reported_for_a_snapshot(self):
        # meta.json is rewritten in place. Handing back an offset would invite a
        # caller to resume from it and silently drop every later turn.
        self.assertNotIn("next_offset", self.mod.collect_vibe(self._session("s-1")))

    def test_lookup_matches_the_id_inside_the_file_not_the_directory_name(self):
        # Vibe truncates the id in the directory name; matching on that would
        # rely on a rule the vendor never promised.
        self._session("aabbccdd-1111-2222-3333-444455556666",
                      dirname="session_20260726_zzzzzzzz")
        found = self.mod.find_vibe_meta(
            "aabbccdd-1111-2222-3333-444455556666", root=self.root
        )
        self.assertIsNotNone(found)

    def test_an_unknown_session_is_not_measured(self):
        result = self.mod.collect_for_session("vibe", "absent", root=self.root)
        self.assertEqual(result["status"], "not_measured")
        self.assertNotIn("counters", result)

    def test_a_stats_block_without_counters_is_not_measured(self):
        # A zero here would be indistinguishable from a session that really
        # spent nothing.
        result = self.mod.collect_vibe(self._session("s-1", stats={"steps": 3}))
        self.assertEqual(result["status"], "not_measured")
        self.assertNotIn("counters", result)

    def test_unreadable_meta_is_not_measured(self):
        path = self.root / "session_broken"
        path.mkdir()
        (path / "meta.json").write_text("{not json")
        result = self.mod.collect_vibe(path / "meta.json")
        self.assertEqual(result["status"], "not_measured")

    def test_a_directory_with_broken_meta_does_not_stop_the_search(self):
        broken = self.root / "session_broken"
        broken.mkdir()
        (broken / "meta.json").write_text("{not json")
        self._session("s-good")
        self.assertIsNotNone(self.mod.find_vibe_meta("s-good", root=self.root))


class VibeSessionResolutionTests(unittest.TestCase):
    """Which Vibe session is this process in? Guessing is worse than refusing.

    Claude Code publishes its id to children. Vibe exports nothing and keeps no
    session file open, so the only signal left is the recorded cwd — weaker, so
    the rule must be stricter.
    """

    def setUp(self):
        self.mod = _load()
        self.dir = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.dir.name)

    def tearDown(self):
        self.dir.cleanup()

    def _session(self, session_id, cwd, mtime=None, dirname=None):
        import os
        session_dir = self.root / (dirname or f"session_{session_id[:8]}")
        session_dir.mkdir(parents=True, exist_ok=True)
        meta = session_dir / "meta.json"
        meta.write_text(json.dumps({
            "session_id": session_id,
            "start_time": "2026-08-05T00:00:00+00:00",
            "environment": {"working_directory": cwd},
            "config": {"active_model": "mistral-medium-3.5"},
            "stats": {"steps": 1, "session_prompt_tokens": 10,
                      "session_completion_tokens": 2},
        }))
        if mtime is not None:
            os.utime(meta, (mtime, mtime))
        return meta

    def test_one_candidate_resolves(self):
        self._session("aaaa1111-2222-3333-4444-555566667777", "/repo/a")
        out = self.mod.resolve_vibe_session_id("/repo/a", root=self.root)
        self.assertEqual(out["status"], "resolved")
        self.assertEqual(out["session_id"], "aaaa1111-2222-3333-4444-555566667777")
        self.assertEqual(out["how"], "vibe-meta-cwd-match")

    def test_a_different_cwd_is_not_adopted(self):
        self._session("aaaa1111-2222-3333-4444-555566667777", "/repo/other")
        out = self.mod.resolve_vibe_session_id("/repo/a", root=self.root)
        self.assertEqual(out["status"], "unresolved")

    def test_two_recent_sessions_in_one_cwd_are_ambiguous(self):
        # The case that would silently mis-attribute millions of tokens.
        now = 1_800_000_000.0
        self._session("aaaa1111-1111-1111-1111-111111111111", "/repo/a", mtime=now)
        self._session("bbbb2222-2222-2222-2222-222222222222", "/repo/a", mtime=now - 60)
        out = self.mod.resolve_vibe_session_id("/repo/a", root=self.root)
        self.assertEqual(out["status"], "ambiguous")
        self.assertEqual(len(out["candidates"]), 2)

    def test_a_clearly_older_session_does_not_block_resolution(self):
        # An abandoned session from hours ago is not a live competitor.
        now = 1_800_000_000.0
        self._session("aaaa1111-1111-1111-1111-111111111111", "/repo/a", mtime=now)
        self._session("bbbb2222-2222-2222-2222-222222222222", "/repo/a",
                      mtime=now - 10_000)
        out = self.mod.resolve_vibe_session_id("/repo/a", root=self.root)
        self.assertEqual(out["status"], "resolved")
        self.assertEqual(out["session_id"], "aaaa1111-1111-1111-1111-111111111111")

    def test_no_sessions_at_all_is_unresolved_not_an_error(self):
        out = self.mod.resolve_vibe_session_id("/repo/a", root=self.root)
        self.assertEqual(out["status"], "unresolved")

    def test_a_broken_meta_is_skipped_not_fatal(self):
        broken = self.root / "session_broken"
        broken.mkdir()
        (broken / "meta.json").write_text("{not json")
        self._session("aaaa1111-1111-1111-1111-111111111111", "/repo/a")
        out = self.mod.resolve_vibe_session_id("/repo/a", root=self.root)
        self.assertEqual(out["status"], "resolved")

    def test_a_path_shaped_session_id_is_not_adopted(self):
        self._session("../../evil", "/repo/a", dirname="session_evil")
        out = self.mod.resolve_vibe_session_id("/repo/a", root=self.root)
        self.assertEqual(out["status"], "unresolved")

    def test_a_missing_root_is_unresolved(self):
        out = self.mod.resolve_vibe_session_id("/repo/a", root=self.root / "absent")
        self.assertEqual(out["status"], "unresolved")


class UnsupportedVendorTests(unittest.TestCase):
    def setUp(self):
        self.mod = _load()

    def test_codex_and_copilot_report_not_measured_not_zero(self):
        # These have no collector yet. Reporting 0 for them is exactly the
        # blind spot this ticket exists to close, so it must stay loud.
        for vendor in ("codex", "copilot"):
            result = self.mod.collect_for_session(vendor, "whatever")
            self.assertEqual(result["status"], "not_measured")
            self.assertNotIn("counters", result)
            self.assertEqual(sorted(result["unmeasured"]),
                             sorted(self.mod.ALL_COUNTERS))


if __name__ == "__main__":
    unittest.main()
