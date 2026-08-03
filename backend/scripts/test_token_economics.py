"""Deterministic tests for the Token Economics collector (KT-188).

Run from the repo root:
    python3 -m unittest backend.scripts.test_token_economics
or via the Makefile:
    make test-python

Fixtures are synthetic and built in a temp dir per test: real telemetry
never enters the repository, and each collector's contract is proven on
data whose expected numbers are computed by hand below.
"""

from __future__ import annotations

import copy
import importlib.util
import json
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path
from unittest import mock

_MODULE_PATH = Path(__file__).with_name("token_economics.py")


def _load_module():
    spec = importlib.util.spec_from_file_location("token_economics", _MODULE_PATH)
    module = importlib.util.module_from_spec(spec)
    # Required on Python ≥3.13: dataclass field resolution looks the module
    # up in sys.modules while the class body is being processed.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


te = _load_module()

WINDOW = te.Window(
    datetime(2026, 8, 1, tzinfo=timezone.utc),
    datetime(2026, 8, 3, tzinfo=timezone.utc),
)


def _claude_line(
    *,
    request_id: str,
    msg_id: str,
    session: str = "s1",
    ts: str = "2026-08-02T10:00:00Z",
    cwd: str = "/home/u/repo",
    input_tokens: int = 10,
    cache_write: int = 100,
    cache_read: int = 1000,
    output: int = 5,
    tools: list[str] | None = None,
) -> str:
    content = [{"type": "tool_use", "name": n, "input": {}} for n in (tools or [])]
    return json.dumps(
        {
            "type": "assistant",
            "timestamp": ts,
            "sessionId": session,
            "cwd": cwd,
            "requestId": request_id,
            "message": {
                "id": msg_id,
                "content": content,
                "usage": {
                    "input_tokens": input_tokens,
                    "cache_creation_input_tokens": cache_write,
                    "cache_read_input_tokens": cache_read,
                    "output_tokens": output,
                },
            },
        }
    )


class ClaudeCollectorTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)

    def _write(self, name: str, lines: list[str]):
        (self.dir / name).write_text("\n".join(lines) + "\n")

    def test_aggregates_and_deduplicates_by_request_and_message_id(self):
        self._write(
            "a.jsonl",
            [
                _claude_line(request_id="r1", msg_id="m1"),
                # Exact duplicate (same requestId + message id) must count once.
                _claude_line(request_id="r1", msg_id="m1"),
                _claude_line(request_id="r2", msg_id="m2", session="s2", input_tokens=20),
                json.dumps({"type": "user", "message": {}}),
            ],
        )
        result = te.collect_claude(self.dir, WINDOW)
        m = result.metrics
        self.assertEqual(m["assistant_calls"], 2)
        self.assertEqual(m["non_cached_input_tokens"], 30)
        self.assertEqual(m["cache_write_tokens"], 200)
        self.assertEqual(m["cache_read_tokens"], 2000)
        self.assertEqual(m["output_tokens"], 10)
        self.assertEqual(m["raw_traffic_tokens"], 2240)
        self.assertEqual(m["sessions"], 2)
        self.assertEqual(result.gaps, [])

    def test_incremental_stream_records_use_original_time_final_usage_and_all_tools(self):
        # Real Claude stream-json writes the same assistant key more than once:
        # an early partial snapshot, then a larger final snapshot which may
        # carry additional tool blocks and a later timestamp.
        narrow = te.Window(
            datetime(2026, 8, 2, 10, 0, tzinfo=timezone.utc),
            datetime(2026, 8, 2, 10, 0, 30, tzinfo=timezone.utc),
        )
        self._write(
            "stream.jsonl",
            [
                _claude_line(
                    request_id="stream-r", msg_id="stream-m",
                    ts="2026-08-02T10:00:05Z", input_tokens=10,
                    cache_write=20, cache_read=100, output=1, tools=["Bash"],
                ),
                _claude_line(
                    request_id="stream-r", msg_id="stream-m",
                    ts="2026-08-02T10:00:20Z", input_tokens=10,
                    cache_write=20, cache_read=100, output=9,
                    tools=["mcp__kronn-internal__disc_wait_for_peer"],
                ),
            ],
        )
        result = te.collect_claude(self.dir, narrow)
        self.assertEqual(result.metrics["assistant_calls"], 1)
        self.assertEqual(result.metrics["raw_traffic_tokens"], 139)
        self.assertEqual(result.metrics["disc_wait_calls"], 1)
        self.assertEqual(result.metrics["disc_wait_associated_tokens"], 139)
        self.assertEqual(result.coverage_from.isoformat(), "2026-08-02T10:00:05+00:00")

    def test_malformed_counter_record_is_ignored_and_disclosed(self):
        malformed = json.loads(_claude_line(request_id="bad", msg_id="bad"))
        malformed["message"]["usage"]["input_tokens"] = "10"
        self._write("bad.jsonl", [json.dumps(malformed)])
        result = te.collect_claude(self.dir, WINDOW)
        self.assertEqual(result.metrics["assistant_calls"], 0)
        self.assertTrue(any("malformed" in gap for gap in result.gaps))

    def test_invalid_json_is_disclosed_instead_of_becoming_a_clean_zero(self):
        self._write(
            "invalid.jsonl",
            ['{"timestamp":"2026-08-02T10:00:00Z","type":"assistant"'],
        )

        result = te.collect_claude(self.dir, WINDOW)

        self.assertEqual(result.metrics["raw_traffic_tokens"], 0)
        self.assertTrue(any("invalid JSON" in gap for gap in result.gaps))

    def test_later_unplaceable_invalid_line_does_not_mutate_a_fixed_window(self):
        path = self.dir / "fixed.jsonl"
        path.write_text(
            _claude_line(
                request_id="fixed", msg_id="fixed", ts="2026-08-02T10:00:00Z"
            ) + "\n"
        )
        before = te.collect_claude(self.dir, WINDOW)

        with path.open("a") as fh:
            fh.write("later unplaceable invalid JSON\n")
        after = te.collect_claude(self.dir, WINDOW)

        self.assertEqual(after.metrics, before.metrics)
        self.assertEqual(after.coverage_dict(), before.coverage_dict())
        self.assertEqual(after.gaps, before.gaps)

    def test_unreadable_jsonl_fails_the_source_closed(self):
        self._write("unreadable.jsonl", [_claude_line(request_id="r1", msg_id="m1")])

        with mock.patch.object(Path, "open", side_effect=PermissionError("denied")):
            result = te.collect_claude(self.dir, WINDOW)

        self.assertIsNone(result.metrics["raw_traffic_tokens"])
        self.assertTrue(any("unreadable" in gap for gap in result.gaps))

    def test_window_filter_excludes_out_of_range_turns(self):
        self._write(
            "a.jsonl",
            [
                _claude_line(request_id="r1", msg_id="m1", ts="2026-07-01T10:00:00Z"),
                _claude_line(request_id="r2", msg_id="m2"),
            ],
        )
        m = te.collect_claude(self.dir, WINDOW).metrics
        self.assertEqual(m["assistant_calls"], 1)
        # Coverage is scoped to the requested window so later telemetry cannot
        # mutate a historical snapshot.
        result = te.collect_claude(self.dir, WINDOW)
        self.assertEqual(result.coverage_from.year, 2026)
        self.assertEqual(result.coverage_from.month, 8)

    def test_disc_wait_attribution_counts_tool_name_only(self):
        self._write(
            "a.jsonl",
            [
                _claude_line(
                    request_id="r1",
                    msg_id="m1",
                    tools=["mcp__kronn-internal__disc_wait_for_peer"],
                ),
                _claude_line(request_id="r2", msg_id="m2", tools=["Bash"]),
            ],
        )
        m = te.collect_claude(self.dir, WINDOW).metrics
        self.assertEqual(m["disc_wait_calls"], 1)
        self.assertEqual(m["disc_wait_associated_tokens"], 1115)

    def test_repo_filter_computes_share(self):
        self._write(
            "a.jsonl",
            [
                _claude_line(request_id="r1", msg_id="m1", session="s1", cwd="/u/Kronn"),
                _claude_line(request_id="r2", msg_id="m2", session="s2", cwd="/u/other"),
            ],
        )
        m = te.collect_claude(self.dir, WINDOW, repo_filter="Kronn").metrics
        self.assertEqual(m["repo_sessions_share_pct"], 50.0)

    def test_missing_dir_yields_nulls_not_zeros(self):
        result = te.collect_claude(self.dir / "nope", WINDOW)
        self.assertIsNone(result.metrics["raw_traffic_tokens"])
        self.assertIsNone(result.metrics["assistant_calls"])
        self.assertTrue(result.gaps)

    def test_readable_dir_with_no_window_activity_measures_zero(self):
        self._write("a.jsonl", [_claude_line(request_id="r1", msg_id="m1", ts="2026-06-01T10:00:00Z")])
        result = te.collect_claude(self.dir, WINDOW)
        self.assertEqual(result.metrics["raw_traffic_tokens"], 0)
        self.assertEqual(result.metrics["assistant_calls"], 0)
        self.assertEqual(result.gaps, [])


def _codex_event(total: dict, ts: str = "2026-08-02T10:00:00Z") -> str:
    return json.dumps(
        {
            "timestamp": ts,
            "payload": {"type": "token_count", "info": {"total_token_usage": total}},
        }
    )


def _codex_meta(
    root: str, own: str | None = None, ts: str = "2026-08-02T09:00:00Z"
) -> str:
    # Real rollouts carry the type at the LINE level for session_meta.
    return json.dumps(
        {
            "timestamp": ts,
            "type": "session_meta",
            "payload": {"session_id": root, "id": own or root},
        }
    )


class CodexCollectorTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)

    def test_uses_last_cumulative_snapshot_per_session(self):
        day = self.dir / "2026" / "08" / "02"
        day.mkdir(parents=True)
        (day / "rollout-2026-08-02T10-00-00-abc.jsonl").write_text(
            "\n".join(
                [
                    _codex_meta("root-a"),
                    _codex_event(
                        {"input_tokens": 100, "cached_input_tokens": 50, "output_tokens": 10, "reasoning_output_tokens": 2}
                    ),
                    # Cumulative snapshots: only the LAST one may be summed.
                    _codex_event(
                        {"input_tokens": 300, "cached_input_tokens": 200, "output_tokens": 30, "reasoning_output_tokens": 6}
                    ),
                ]
            )
            + "\n"
        )
        m = te.collect_codex(self.dir, WINDOW).metrics
        self.assertEqual(m["raw_traffic_tokens"], 330)
        self.assertEqual(m["non_cached_input_tokens"], 100)
        self.assertIsNone(m["cache_write_tokens"])
        self.assertEqual(m["cache_read_tokens"], 200)
        self.assertEqual(m["reasoning_tokens"], 6)
        self.assertEqual(m["sessions"], 1)
        self.assertEqual(m["rollouts"], 1)
        self.assertEqual(m["top_1_share_pct"], 100.0)

    def test_forked_rollouts_of_one_thread_count_once(self):
        day = self.dir / "2026" / "08" / "02"
        day.mkdir(parents=True)
        # Parent thread reaches 1000 input tokens.
        (day / "rollout-2026-08-02T09-00-00-parent.jsonl").write_text(
            _codex_meta("root-a")
            + "\n"
            + _codex_event({"input_tokens": 1000, "cached_input_tokens": 900, "output_tokens": 100})
            + "\n"
        )
        # A fork REPLAYS the parent's counters and adds its own on top.
        (day / "rollout-2026-08-02T10-00-00-fork.jsonl").write_text(
            _codex_meta("root-a", own="fork-1")
            + "\n"
            + _codex_event({"input_tokens": 1200, "cached_input_tokens": 1080, "output_tokens": 120})
            + "\n"
        )
        # An unrelated thread must still be counted separately.
        (day / "rollout-2026-08-02T11-00-00-other.jsonl").write_text(
            _codex_meta("root-b")
            + "\n"
            + _codex_event({"input_tokens": 10, "cached_input_tokens": 0, "output_tokens": 1})
            + "\n"
        )
        m = te.collect_codex(self.dir, WINDOW).metrics
        # root-a keeps only its max snapshot (1200+120), not parent+fork.
        self.assertEqual(m["raw_traffic_tokens"], 1320 + 11)
        self.assertEqual(m["sessions"], 2)
        self.assertEqual(m["rollouts"], 3)

    def test_falls_back_to_path_date_when_events_lack_timestamps(self):
        day = self.dir / "2026" / "07" / "01"
        day.mkdir(parents=True)
        (day / "rollout-2026-07-01T10-00-00-old.jsonl").write_text(
            json.dumps(
                {"payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 5, "cached_input_tokens": 0, "output_tokens": 1}}}}
            )
            + "\n"
        )
        result = te.collect_codex(self.dir, WINDOW)
        # Path date (July 1st) is before the window: the source is readable,
        # so the window activity is a MEASURED zero, never null.
        self.assertEqual(result.metrics["raw_traffic_tokens"], 0)
        self.assertEqual(result.metrics["sessions"], 0)
        self.assertEqual(result.gaps, [])
        self.assertIsNone(result.coverage_from)

    def test_window_reports_delta_not_lifetime_cumulative(self):
        # Codex review P1: a thread with 1M tokens before the window and
        # 10k inside it must report 10k, not 1.01M — and a snapshot beyond
        # the window end must not leak in.
        day = self.dir / "2026" / "08" / "02"
        day.mkdir(parents=True)
        (day / "rollout-2026-08-02T10-00-00-long.jsonl").write_text(
            "\n".join(
                [
                    _codex_meta("root-long"),
                    _codex_event(
                        {"input_tokens": 1_000_000, "cached_input_tokens": 990_000, "output_tokens": 50_000},
                        ts="2026-07-20T10:00:00Z",  # before the window
                    ),
                    _codex_event(
                        {"input_tokens": 1_009_000, "cached_input_tokens": 998_500, "output_tokens": 51_000},
                        ts="2026-08-02T10:00:00Z",  # inside the window
                    ),
                    _codex_event(
                        {"input_tokens": 2_000_000, "cached_input_tokens": 1_900_000, "output_tokens": 90_000},
                        ts="2026-08-05T10:00:00Z",  # after the window end
                    ),
                ]
            )
            + "\n"
        )
        m = te.collect_codex(self.dir, WINDOW).metrics
        self.assertEqual(m["raw_traffic_tokens"], 10_000)
        self.assertEqual(m["non_cached_input_tokens"], 9_000 - 8_500)
        self.assertEqual(m["cache_read_tokens"], 8_500)
        self.assertEqual(m["output_tokens"], 1_000)
        self.assertEqual(m["sessions"], 1)

    def test_fork_delta_uses_parent_branch_as_window_start(self):
        # Parent finished at 1000 before the window; a fork REPLAYS that
        # base and adds 200 inside the window: the thread reports 200.
        old_day = self.dir / "2026" / "07" / "20"
        old_day.mkdir(parents=True)
        (old_day / "rollout-2026-07-20T10-00-00-parent.jsonl").write_text(
            _codex_meta("root-f")
            + "\n"
            + _codex_event(
                {"input_tokens": 900, "cached_input_tokens": 800, "output_tokens": 100},
                ts="2026-07-20T10:00:00Z",
            )
            + "\n"
        )
        day = self.dir / "2026" / "08" / "02"
        day.mkdir(parents=True)
        (day / "rollout-2026-08-02T10-00-00-fork.jsonl").write_text(
            _codex_meta("root-f", own="fork-1")
            + "\n"
            + _codex_event(
                {"input_tokens": 1080, "cached_input_tokens": 960, "output_tokens": 120},
                ts="2026-08-02T10:00:00Z",
            )
            + "\n"
        )
        m = te.collect_codex(self.dir, WINDOW).metrics
        self.assertEqual(m["raw_traffic_tokens"], 200)
        self.assertEqual(m["sessions"], 1)

    def test_replay_only_fork_is_not_an_active_rollout(self):
        old_day = self.dir / "2026" / "07" / "20"
        old_day.mkdir(parents=True)
        base = {"input_tokens": 100, "cached_input_tokens": 80, "output_tokens": 10}
        (old_day / "rollout-parent.jsonl").write_text(
            _codex_meta("root-replay") + "\n"
            + _codex_event(base, ts="2026-07-20T10:00:00Z") + "\n"
        )
        day = self.dir / "2026" / "08" / "02"
        day.mkdir(parents=True)
        (day / "rollout-fork.jsonl").write_text(
            _codex_meta("root-replay", own="fork-replay") + "\n"
            + _codex_event(base, ts="2026-08-02T10:00:00Z") + "\n"
        )

        result = te.collect_codex(self.dir, WINDOW)
        self.assertEqual(result.metrics["raw_traffic_tokens"], 0)
        self.assertEqual(result.metrics["rollouts"], 0)
        self.assertEqual(result.gaps, [])

    def test_divergent_branch_snapshots_fail_closed(self):
        old_day = self.dir / "2026" / "07" / "20"
        old_day.mkdir(parents=True)
        (old_day / "rollout-parent.jsonl").write_text(
            _codex_meta("root-divergent") + "\n"
            + _codex_event(
                {"input_tokens": 100, "cached_input_tokens": 0, "output_tokens": 10},
                ts="2026-07-20T10:00:00Z",
            ) + "\n"
        )
        day = self.dir / "2026" / "08" / "02"
        day.mkdir(parents=True)
        (day / "rollout-fork.jsonl").write_text(
            _codex_meta("root-divergent", own="fork-other-branch") + "\n"
            + _codex_event(
                {"input_tokens": 110, "cached_input_tokens": 100, "output_tokens": 11},
                ts="2026-08-02T10:00:00Z",
            ) + "\n"
        )

        result = te.collect_codex(self.dir, WINDOW)
        self.assertIsNone(result.metrics["raw_traffic_tokens"])
        self.assertIsNone(result.metrics["non_cached_input_tokens"])
        self.assertTrue(any("divergent" in gap for gap in result.gaps))

    def test_fork_without_a_timestamped_root_ancestor_fails_closed(self):
        day = self.dir / "2026" / "08" / "02"
        day.mkdir(parents=True)
        (day / "rollout-orphan-fork.jsonl").write_text(
            _codex_meta("missing-root", own="orphan-fork") + "\n"
            + _codex_event(
                {"input_tokens": 100, "cached_input_tokens": 80, "output_tokens": 10},
                ts="2026-08-02T10:00:00Z",
            ) + "\n"
        )

        result = te.collect_codex(self.dir, WINDOW)
        self.assertIsNone(result.metrics["raw_traffic_tokens"])
        self.assertTrue(any("unproven" in gap for gap in result.gaps))

    def test_timestamp_less_partial_boundary_append_does_not_mutate_window(self):
        partial = te.Window(
            datetime(2026, 8, 2, 10, 0, tzinfo=timezone.utc),
            datetime(2026, 8, 2, 11, 0, tzinfo=timezone.utc),
        )
        day = self.dir / "2026" / "08" / "02"
        day.mkdir(parents=True)
        rollout = day / "rollout-partial.jsonl"
        rollout.write_text(
            _codex_meta("root-partial", ts="2026-08-02T10:00:00Z") + "\n"
            + _codex_event(
                {"input_tokens": 100, "cached_input_tokens": 80, "output_tokens": 10},
                ts="2026-08-02T10:30:00Z",
            ) + "\n"
        )
        before = te.collect_codex(self.dir, partial)
        timestamp_less = json.loads(_codex_event(
            {"input_tokens": 999_999, "cached_input_tokens": 900_000, "output_tokens": 90_000}
        ))
        del timestamp_less["timestamp"]
        with rollout.open("a") as fh:
            fh.write(json.dumps(timestamp_less) + "\n")
        after = te.collect_codex(self.dir, partial)

        self.assertEqual(after.metrics, before.metrics)
        self.assertEqual(after.coverage_dict(), before.coverage_dict())
        self.assertEqual(after.gaps, before.gaps)
        self.assertTrue(any("partial UTC boundary" in gap for gap in after.gaps))

    def test_old_root_without_pre_window_counter_fails_closed(self):
        day = self.dir / "2026" / "08" / "02"
        day.mkdir(parents=True)
        (day / "rollout-old-root.jsonl").write_text(
            _codex_meta("root-old", ts="2026-07-01T09:00:00Z") + "\n"
            + _codex_event(
                {"input_tokens": 1_000_000, "cached_input_tokens": 900_000, "output_tokens": 50_000},
                ts="2026-08-02T10:00:00Z",
            ) + "\n"
        )

        result = te.collect_codex(self.dir, WINDOW)
        self.assertIsNone(result.metrics["raw_traffic_tokens"])
        self.assertTrue(any("unproven" in gap for gap in result.gaps))

    def test_old_root_metadata_in_another_file_still_requires_a_boundary(self):
        old_day = self.dir / "2026" / "07" / "01"
        old_day.mkdir(parents=True)
        (old_day / "rollout-root-meta.jsonl").write_text(
            _codex_meta("root-multi", ts="2026-07-01T09:00:00Z") + "\n"
        )
        day = self.dir / "2026" / "08" / "02"
        day.mkdir(parents=True)
        (day / "rollout-root-resumed.jsonl").write_text(
            _codex_meta("root-multi", ts="2026-08-02T09:00:00Z") + "\n"
            + _codex_event(
                {"input_tokens": 2_000_000, "cached_input_tokens": 1_800_000, "output_tokens": 100_000},
                ts="2026-08-02T10:00:00Z",
            ) + "\n"
        )

        result = te.collect_codex(self.dir, WINDOW)
        self.assertIsNone(result.metrics["raw_traffic_tokens"])
        self.assertTrue(any("unproven" in gap for gap in result.gaps))

    def test_timestamp_less_day_wholly_before_window_is_a_valid_boundary(self):
        old_day = self.dir / "2026" / "07" / "20"
        old_day.mkdir(parents=True)
        boundary = json.loads(_codex_event(
            {"input_tokens": 900, "cached_input_tokens": 800, "output_tokens": 100}
        ))
        del boundary["timestamp"]
        (old_day / "rollout-root-boundary.jsonl").write_text(
            _codex_meta("root-day-boundary", ts="2026-07-20T09:00:00Z") + "\n"
            + json.dumps(boundary) + "\n"
        )
        day = self.dir / "2026" / "08" / "02"
        day.mkdir(parents=True)
        (day / "rollout-root-resumed.jsonl").write_text(
            _codex_meta("root-day-boundary", ts="2026-08-02T09:00:00Z") + "\n"
            + _codex_event(
                {"input_tokens": 1_080, "cached_input_tokens": 960, "output_tokens": 120},
                ts="2026-08-02T10:00:00Z",
            ) + "\n"
        )

        result = te.collect_codex(self.dir, WINDOW)
        self.assertEqual(result.metrics["raw_traffic_tokens"], 200)
        self.assertEqual(result.metrics["sessions"], 1)

    def test_missing_dir_yields_nulls(self):
        result = te.collect_codex(self.dir / "nope", WINDOW)
        self.assertIsNone(result.metrics["sessions"])
        self.assertTrue(result.gaps)

    def test_invalid_json_is_disclosed_instead_of_becoming_a_clean_zero(self):
        day = self.dir / "2026" / "08" / "02"
        day.mkdir(parents=True)
        (day / "rollout-invalid.jsonl").write_text("not json at all\n")

        result = te.collect_codex(self.dir, WINDOW)

        self.assertEqual(result.metrics["raw_traffic_tokens"], 0)
        self.assertTrue(any("invalid JSON" in gap for gap in result.gaps))

    def test_future_path_day_is_not_parsed_or_added_to_fixed_window_gaps(self):
        before = te.collect_codex(self.dir, WINDOW)
        future = self.dir / "2026" / "08" / "05"
        future.mkdir(parents=True)
        (future / "rollout-invalid.jsonl").write_text("not json at all\n")

        after = te.collect_codex(self.dir, WINDOW)

        self.assertEqual(after.metrics, before.metrics)
        self.assertEqual(after.coverage_dict(), before.coverage_dict())
        self.assertEqual(after.gaps, before.gaps)

    def test_unreadable_jsonl_fails_the_source_closed(self):
        day = self.dir / "2026" / "08" / "02"
        day.mkdir(parents=True)
        (day / "rollout-unreadable.jsonl").write_text(
            _codex_meta("root-unreadable") + "\n"
        )

        with mock.patch.object(Path, "open", side_effect=PermissionError("denied")):
            result = te.collect_codex(self.dir, WINDOW)

        self.assertIsNone(result.metrics["raw_traffic_tokens"])
        self.assertTrue(any("unreadable" in gap for gap in result.gaps))


class CopilotCollectorTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.db = Path(self.tmp.name) / "session-store.db"
        conn = sqlite3.connect(self.db)
        conn.execute(
            """
            CREATE TABLE assistant_usage_events (
                id INTEGER PRIMARY KEY, session_id TEXT, model TEXT,
                input_tokens INTEGER, output_tokens INTEGER,
                cache_read_tokens INTEGER, cache_write_tokens INTEGER,
                reasoning_tokens INTEGER, created_at TEXT
            )
            """
        )
        conn.executemany(
            "INSERT INTO assistant_usage_events (session_id, model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, created_at) VALUES (?,?,?,?,?,?,?,?)",
            [
                ("s1", "m", 100, 10, 500, 20, 3, "2026-08-02 10:00:00"),
                ("s1", "m", 50, 5, 200, 10, 1, "2026-08-02 11:00:00"),
                ("s2", "m", 999, 99, 999, 99, 9, "2026-06-01 10:00:00"),  # out of window
            ],
        )
        conn.commit()
        conn.close()

    def test_sums_only_rows_in_window(self):
        m = te.collect_copilot(self.db, WINDOW).metrics
        self.assertEqual(m["calls"], 2)
        self.assertEqual(m["non_cached_input_tokens"], 150)
        self.assertEqual(m["cache_read_tokens"], 700)
        self.assertEqual(m["cache_write_tokens"], 30)
        self.assertEqual(m["output_tokens"], 15)
        self.assertEqual(m["reasoning_tokens"], 4)
        self.assertEqual(m["raw_traffic_tokens"], 895)
        self.assertEqual(m["coverage_days"], 1)
        self.assertFalse(m["observed_range_covers_window"])
        self.assertTrue(te.collect_copilot(self.db, WINDOW).gaps)

    def test_iso_t_z_rows_obey_inclusive_window_boundaries(self):
        conn = sqlite3.connect(self.db)
        conn.executemany(
            "INSERT INTO assistant_usage_events (session_id, model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, created_at) VALUES (?,?,?,?,?,?,?,?)",
            [
                ("before", "m", 1, 0, 0, 0, 0, "2026-08-02T09:59:59Z"),
                ("start", "m", 2, 0, 0, 0, 0, "2026-08-02T10:00:00Z"),
                ("end", "m", 4, 0, 0, 0, 0, "2026-08-02T11:00:00Z"),
                ("after", "m", 8, 0, 0, 0, 0, "2026-08-02T11:00:01Z"),
            ],
        )
        conn.commit()
        conn.close()
        exact = te.Window(
            datetime(2026, 8, 2, 10, 0, tzinfo=timezone.utc),
            datetime(2026, 8, 2, 11, 0, tzinfo=timezone.utc),
        )
        metrics = te.collect_copilot(self.db, exact).metrics
        # Includes the two original space-separated rows at 10:00 and 11:00
        # plus the exact ISO-Z boundaries, but neither adjacent row.
        self.assertEqual(metrics["calls"], 4)
        self.assertEqual(metrics["non_cached_input_tokens"], 156)

    def test_missing_db_yields_nulls(self):
        result = te.collect_copilot(self.db.with_name("nope.db"), WINDOW)
        self.assertIsNone(result.metrics["calls"])
        self.assertTrue(result.gaps)

    def test_readable_db_with_no_window_rows_measures_zero(self):
        june = te.Window(
            datetime(2026, 5, 1, tzinfo=timezone.utc),
            datetime(2026, 5, 2, tzinfo=timezone.utc),
        )
        result = te.collect_copilot(self.db, june)
        self.assertEqual(result.metrics["calls"], 0)
        self.assertEqual(result.metrics["raw_traffic_tokens"], 0)
        self.assertEqual(result.metrics["coverage_days"], 0)
        # Zeros are the measurement; the separate coverage gap prevents them
        # from being overclaimed as a continuously observed zero-usage window.
        self.assertTrue(any("coverage" in gap for gap in result.gaps))
        # Coverage still shows what the source has ever observed.
        self.assertIsNone(result.coverage_from)


class KronnCollectorTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.db = Path(self.tmp.name) / "kronn.db"
        conn = sqlite3.connect(self.db)
        conn.execute(
            """
            CREATE TABLE messages (
                id TEXT PRIMARY KEY, discussion_id TEXT, role TEXT,
                agent_type TEXT, timestamp TEXT, tokens_used INTEGER
            )
            """
        )
        conn.executemany(
            "INSERT INTO messages VALUES (?,?,?,?,?,?)",
            [
                ("m1", "d1", "Agent", "Codex", "2026-08-02T10:00:00+00:00", 0),
                ("m2", "d1", "Agent", "Codex", "2026-08-02T10:01:00+00:00", 0),
                ("m3", "d1", "Agent", "ClaudeCode", "2026-08-02T10:02:00+00:00", 1200),
                ("m4", "d1", "User", None, "2026-08-02T10:03:00+00:00", 0),
                ("m5", "d1", "Agent", "Copilot", "2026-06-01T10:00:00+00:00", 0),
            ],
        )
        conn.commit()
        conn.close()

    def test_telemetry_honesty_metrics(self):
        m = te.collect_kronn(self.db, WINDOW).metrics
        self.assertEqual(m["external_agent_replies"], 3)
        self.assertEqual(m["external_replies_with_tokens_pct"], 33.33)
        self.assertEqual(m["traced_tokens_used"], 1200)
        self.assertEqual(m["untraced_replies_by_agent"], {"Codex": 2, "ClaudeCode": 0})

    def test_missing_db_yields_nulls(self):
        result = te.collect_kronn(self.db.with_name("nope.db"), WINDOW)
        self.assertIsNone(result.metrics["external_agent_replies"])
        self.assertTrue(result.gaps)

    def test_readable_db_with_no_window_replies_measures_zero(self):
        may = te.Window(
            datetime(2026, 5, 1, tzinfo=timezone.utc),
            datetime(2026, 5, 2, tzinfo=timezone.utc),
        )
        result = te.collect_kronn(self.db, may)
        self.assertEqual(result.metrics["external_agent_replies"], 0)
        self.assertEqual(result.metrics["traced_tokens_used"], 0)
        # 0/0 has no defined share: null is the honest value here.
        self.assertIsNone(result.metrics["external_replies_with_tokens_pct"])
        self.assertEqual(result.gaps, [])


class RtkCollectorTests(unittest.TestCase):
    GAIN = json.dumps({
        "summary": {"total_commands": 999},
        "daily": [
            {"date": "2026-06-01", "commands": 500, "input_tokens": 9_999_999,
             "output_tokens": 999, "saved_tokens": 9_999_000},  # outside window
            {"date": "2026-08-01", "commands": 100, "input_tokens": 1_000,
             "output_tokens": 300, "saved_tokens": 700},
            {"date": "2026-08-02", "commands": 50, "input_tokens": 500,
             "output_tokens": 200, "saved_tokens": 300},
        ],
    })

    def test_sums_only_daily_rows_inside_the_window(self):
        def fake_run(cmd, **_kwargs):
            if cmd[:2] == ["rtk", "gain"]:
                return subprocess.CompletedProcess(cmd, 0, stdout=self.GAIN, stderr="")
            return subprocess.CompletedProcess(cmd, 0, stdout="rtk 1.2.3\n", stderr="")

        result = te.collect_rtk(WINDOW, runner=fake_run)
        m = result.metrics
        self.assertEqual(m["commands"], 150)
        self.assertEqual(m["raw_output_tokens"], 1_500)
        self.assertEqual(m["compacted_output_tokens"], 500)
        self.assertEqual(m["saved_tokens"], 1_000)
        self.assertEqual(m["saved_pct"], 66.67)
        self.assertEqual(m["version"], "rtk 1.2.3")
        self.assertTrue(m["installed"])
        self.assertEqual(m["included_full_days"], 2)
        self.assertEqual(m["window_coverage"], "complete")
        # Lifetime totals (the June row) must NOT leak into a windowed report.
        self.assertLess(m["raw_output_tokens"], 9_999_999)
        self.assertEqual(result.coverage_from.date().isoformat(), "2026-08-01")

    def test_rtk_missing_yields_nulls(self):
        def fake_run(cmd, **_kwargs):
            raise FileNotFoundError("no rtk")

        result = te.collect_rtk(WINDOW, runner=fake_run)
        self.assertIsNone(result.metrics["saved_tokens"])
        self.assertFalse(result.metrics["installed"])
        self.assertTrue(result.gaps)

    def test_rtk_gain_timeout_still_proves_the_executable_is_installed(self):
        def fake_run(cmd, **_kwargs):
            raise subprocess.TimeoutExpired(cmd, 30)

        result = te.collect_rtk(WINDOW, runner=fake_run)
        self.assertTrue(result.metrics["installed"])
        self.assertIsNone(result.metrics["commands"])
        self.assertTrue(any("timed out" in gap for gap in result.gaps))

    def test_unparseable_json_stays_null_never_zero(self):
        def fake_run(cmd, **_kwargs):
            return subprocess.CompletedProcess(cmd, 0, stdout="something unexpected", stderr="")

        result = te.collect_rtk(WINDOW, runner=fake_run)
        self.assertIsNone(result.metrics["commands"])
        self.assertIsNone(result.metrics["saved_pct"])
        self.assertTrue(result.metrics["installed"])
        self.assertTrue(result.gaps)

    def test_version_probe_failure_keeps_the_aggregates(self):
        def fake_run(cmd, **_kwargs):
            if cmd[:2] == ["rtk", "gain"]:
                return subprocess.CompletedProcess(cmd, 0, stdout=self.GAIN, stderr="")
            raise subprocess.TimeoutExpired(cmd, 10)

        m = te.collect_rtk(WINDOW, runner=fake_run).metrics
        self.assertEqual(m["saved_tokens"], 1_000)
        self.assertIsNone(m["version"])
        self.assertTrue(m["installed"])

    def test_partial_day_scenario_does_not_claim_whole_day_savings(self):
        def fake_run(cmd, **_kwargs):
            if cmd[:2] == ["rtk", "gain"]:
                return subprocess.CompletedProcess(cmd, 0, stdout=self.GAIN, stderr="")
            return subprocess.CompletedProcess(cmd, 0, stdout="rtk 1.2.3\n", stderr="")

        scenario = te.Window(
            datetime(2026, 8, 2, 10, 0, tzinfo=timezone.utc),
            datetime(2026, 8, 2, 11, 0, tzinfo=timezone.utc),
        )
        result = te.collect_rtk(scenario, runner=fake_run)
        self.assertIsNone(result.metrics["saved_tokens"])
        self.assertEqual(result.metrics["window_coverage"], "none")
        self.assertTrue(result.metrics["installed"])
        self.assertTrue(any("daily granularity" in gap for gap in result.gaps))

    def test_malformed_daily_schema_fails_closed(self):
        malformed_payloads = [
            {},
            {"daily": {}},
            {"daily": [{"date": "2026-08-02", "commands": "50",
                         "input_tokens": 500, "output_tokens": 200, "saved_tokens": 300}]},
            {"daily": [{"date": "2026-08-02", "commands": -1,
                         "input_tokens": 500, "output_tokens": 200, "saved_tokens": 300}]},
            {"daily": [{"date": "2026-08-02", "commands": 1,
                         "input_tokens": 500, "output_tokens": 600, "saved_tokens": 0}]},
        ]
        for payload in malformed_payloads:
            with self.subTest(payload=payload):
                def fake_run(cmd, **_kwargs):
                    return subprocess.CompletedProcess(
                        cmd, 0, stdout=json.dumps(payload), stderr=""
                    )

                result = te.collect_rtk(WINDOW, runner=fake_run)
                self.assertIsNone(result.metrics["commands"])
                self.assertTrue(result.metrics["installed"])
                self.assertTrue(result.gaps)


class WindowTests(unittest.TestCase):
    def test_normalizes_boundaries_and_rejects_invalid_windows(self):
        window = te.Window(datetime(2026, 8, 1), datetime(2026, 8, 2))
        self.assertEqual(window.start.tzinfo, timezone.utc)
        with self.assertRaises(ValueError):
            te.Window(datetime(2026, 8, 2), datetime(2026, 8, 1))
        with self.assertRaises(TypeError):
            te.Window("2026-08-01", datetime(2026, 8, 2))


class ReportTests(unittest.TestCase):
    def _empty(self, provenance="x"):
        return te.SourceResult(metrics=dict(te._CLAUDE_EMPTY), provenance=provenance, gaps=["g"])

    def test_report_shape_scenario_and_pseudonymization(self):
        report = te.build_report(
            window=WINDOW,
            scenario="cli-persistent-wait",
            repo_alias="Kronn",
            claude=self._empty(),
            codex=te.SourceResult(dict(te._CODEX_EMPTY), "codex"),
            copilot=te.SourceResult(dict(te._COPILOT_EMPTY), "copilot"),
            kronn=te.SourceResult(dict(te._KRONN_EMPTY), "kronn"),
            rtk=te.SourceResult(dict(te._RTK_EMPTY), "rtk"),
            generated_at=datetime(2026, 8, 2, 12, 0, tzinfo=timezone.utc),
        )
        self.assertEqual(report["schema_version"], te.SCHEMA_VERSION)
        self.assertTrue(report["scenario_is_canonical"])
        # A stable unsalted hash is a pseudonym, not anonymization. The raw
        # alias must still never appear anywhere in the exported JSON.
        self.assertNotIn("Kronn", json.dumps(report))
        self.assertEqual(len(report["scope"]["repo_pseudonym"]), 8)
        self.assertIn("g", report["data_gaps"])
        # Deterministic: same inputs → identical serialized output.
        again = te.build_report(
            window=WINDOW,
            scenario="cli-persistent-wait",
            repo_alias="Kronn",
            claude=self._empty(),
            codex=te.SourceResult(dict(te._CODEX_EMPTY), "codex"),
            copilot=te.SourceResult(dict(te._COPILOT_EMPTY), "copilot"),
            kronn=te.SourceResult(dict(te._KRONN_EMPTY), "kronn"),
            rtk=te.SourceResult(dict(te._RTK_EMPTY), "rtk"),
            generated_at=datetime(2026, 8, 2, 12, 0, tzinfo=timezone.utc),
        )
        self.assertEqual(json.dumps(report, sort_keys=True), json.dumps(again, sort_keys=True))
        self.assertEqual(report["generated_at"], "2026-08-02T12:00:00+00:00")
        self.assertEqual(
            report["null_reasons"]["agents.codex.cache_write_tokens"],
            "unsupported_metric",
        )
        self.assertEqual(
            report["null_reasons"]["agents.claude.estimated_cost_usd"],
            "unconfigured_cost",
        )
        self.assertEqual(
            report["null_reasons"]["kpi.completed_tasks"],
            "missing_denominator",
        )

    def test_free_form_scenario_is_not_exported(self):
        secret = "customer-acme-secret-experiment"
        report = te.build_report(
            window=WINDOW,
            scenario=secret,
            repo_alias=None,
            claude=self._empty(),
            codex=te.SourceResult(dict(te._CODEX_EMPTY), "codex"),
            copilot=te.SourceResult(dict(te._COPILOT_EMPTY), "copilot"),
            kronn=te.SourceResult(dict(te._KRONN_EMPTY), "kronn"),
            rtk=te.SourceResult(dict(te._RTK_EMPTY), "rtk"),
        )
        serialized = json.dumps(report)
        self.assertNotIn(secret, serialized)
        self.assertIsNone(report["scenario"])
        self.assertIsNone(report["scenario_is_canonical"])
        self.assertTrue(any("scenario" in gap for gap in report["data_gaps"]))

    def test_normalized_kpi_uses_explicit_completed_task_denominator(self):
        claude_metrics = dict(te._CLAUDE_EMPTY)
        claude_metrics.update({
            "raw_traffic_tokens": 1_000,
            "non_cached_input_tokens": 100,
            "cache_write_tokens": 100,
            "cache_read_tokens": 700,
            "output_tokens": 100,
            "assistant_calls": 1,
            "sessions": 1,
            "top_1_share_pct": 100.0,
            "top_4_share_pct": 100.0,
            "disc_wait_calls": 0,
            "disc_wait_associated_tokens": 0,
        })
        report = te.build_report(
            window=WINDOW,
            scenario="cli-oneshot",
            repo_alias=None,
            claude=te.SourceResult(claude_metrics, "claude"),
            codex=te.SourceResult(dict(te._CODEX_EMPTY), "codex"),
            copilot=te.SourceResult(dict(te._COPILOT_EMPTY), "copilot"),
            kronn=te.SourceResult(dict(te._KRONN_EMPTY), "kronn"),
            rtk=te.SourceResult(dict(te._RTK_EMPTY), "rtk"),
            completed_tasks=4,
        )
        kpi = report["kpi"]
        self.assertEqual(kpi["completed_tasks"], 4)
        self.assertEqual(kpi["raw_traffic_tokens_per_completed_task_by_agent"]["claude"], 250.0)
        self.assertIsNone(kpi["raw_traffic_tokens_per_completed_task_by_agent"]["codex"])

    def test_repo_share_null_reason_distinguishes_unrequested_from_zero_ratio(self):
        with tempfile.TemporaryDirectory() as tmp:
            claude_dir = Path(tmp)

            def report_for(repo_filter):
                return te.build_report(
                    window=WINDOW,
                    scenario=None,
                    repo_alias=None,
                    claude=te.collect_claude(
                        claude_dir, WINDOW, repo_filter=repo_filter
                    ),
                    codex=te.SourceResult(
                        dict(te._CODEX_EMPTY), "codex", gaps=["codex unavailable"]
                    ),
                    copilot=te.SourceResult(
                        dict(te._COPILOT_EMPTY), "copilot", gaps=["copilot unavailable"]
                    ),
                    kronn=te.SourceResult(
                        dict(te._KRONN_EMPTY), "kronn", gaps=["kronn unavailable"]
                    ),
                    rtk=te.SourceResult(
                        dict(te._RTK_EMPTY), "rtk", gaps=["rtk unavailable"]
                    ),
                )

            unrequested = report_for(None)
            requested_without_traffic = report_for("Kronn")

        path = "agents.claude.repo_sessions_share_pct"
        self.assertEqual(unrequested["null_reasons"][path], "not_requested")
        self.assertEqual(
            requested_without_traffic["null_reasons"][path], "undefined_ratio"
        )

    def test_rtk_installed_does_not_depend_on_optional_version_probe(self):
        rtk_metrics = dict(te._RTK_EMPTY)
        rtk_metrics.update({
            "installed": True,
            "commands": 0,
            "raw_output_tokens": 0,
            "compacted_output_tokens": 0,
            "saved_tokens": 0,
            "included_full_days": 2,
            "window_coverage": "complete",
        })
        report = te.build_report(
            window=WINDOW,
            scenario=None,
            repo_alias=None,
            claude=self._empty(),
            codex=te.SourceResult(dict(te._CODEX_EMPTY), "codex"),
            copilot=te.SourceResult(dict(te._COPILOT_EMPTY), "copilot"),
            kronn=te.SourceResult(dict(te._KRONN_EMPTY), "kronn"),
            rtk=te.SourceResult(rtk_metrics, "rtk"),
        )
        self.assertTrue(report["scope"]["rtk_installed"])
        self.assertIsNone(report["rtk"]["version"])

    def test_committed_baseline_schema_and_arithmetic_are_valid(self):
        baseline = Path("docs/research/token-economics-baseline-2026-08-02.json")
        report = json.loads(baseline.read_text())
        te.validate_report(report)

        bad_arithmetic = json.loads(json.dumps(report))
        bad_arithmetic["agents"]["claude"]["raw_traffic_tokens"] += 1
        with self.assertRaisesRegex(ValueError, "arithmetic"):
            te.validate_report(bad_arithmetic)

        bad_counter = json.loads(json.dumps(report))
        bad_counter["agents"]["copilot"]["output_tokens"] = "38259"
        with self.assertRaisesRegex(ValueError, "output_tokens"):
            te.validate_report(bad_counter)

    def test_validator_rejects_impossible_positive_traffic_counts_and_shares(self):
        baseline = json.loads(
            Path("docs/research/token-economics-baseline-2026-08-02.json").read_text()
        )

        def set_value(path, value):
            def mutate(candidate):
                owner = candidate
                for key in path[:-1]:
                    owner = owner[key]
                owner[path[-1]] = value
            return mutate

        def null_share(path):
            def mutate(candidate):
                set_value(path, None)(candidate)
                candidate["null_reasons"][".".join(path)] = "undefined_ratio"
            return mutate

        corruptions = {
            "claude calls": set_value(("agents", "claude", "assistant_calls"), 0),
            "claude sessions": set_value(("agents", "claude", "sessions"), 0),
            "claude missing top one": null_share(
                ("agents", "claude", "top_1_share_pct")
            ),
            "claude one-session top one": lambda value: (
                value["agents"]["claude"].__setitem__("sessions", 1),
                value["agents"]["claude"].__setitem__("top_1_share_pct", 99.0),
            ),
            "claude four-session top four": lambda value: (
                value["agents"]["claude"].__setitem__("sessions", 4),
                value["agents"]["claude"].__setitem__("top_4_share_pct", 99.0),
            ),
            "claude inverted top shares": lambda value: (
                value["agents"]["claude"].__setitem__("top_1_share_pct", 80.0),
                value["agents"]["claude"].__setitem__("top_4_share_pct", 70.0),
            ),
            "codex sessions": set_value(("agents", "codex", "sessions"), 0),
            "codex rollouts": set_value(("agents", "codex", "rollouts"), 0),
            "codex missing top one": null_share(
                ("agents", "codex", "top_1_share_pct")
            ),
            "codex one-session top one": lambda value: (
                value["agents"]["codex"].__setitem__("sessions", 1),
                value["agents"]["codex"].__setitem__("top_1_share_pct", 99.0),
            ),
            "copilot calls": set_value(("agents", "copilot", "calls"), 0),
        }
        for label, mutate in corruptions.items():
            with self.subTest(label=label):
                candidate = copy.deepcopy(baseline)
                mutate(candidate)
                with self.assertRaises(ValueError):
                    te.validate_report(candidate)

    def test_validator_rejects_table_driven_structural_corruption(self):
        report = te.build_report(
            window=WINDOW,
            scenario="native-agent",
            repo_alias="Kronn",
            claude=self._empty(),
            codex=te.SourceResult(dict(te._CODEX_EMPTY), "codex", gaps=["codex absent"]),
            copilot=te.SourceResult(dict(te._COPILOT_EMPTY), "copilot", gaps=["copilot absent"]),
            kronn=te.SourceResult(dict(te._KRONN_EMPTY), "kronn", gaps=["kronn absent"]),
            rtk=te.SourceResult(dict(te._RTK_EMPTY), "rtk", gaps=["rtk absent"]),
        )

        def delete(path):
            def mutate(candidate):
                owner = candidate
                for key in path[:-1]:
                    owner = owner[key]
                del owner[path[-1]]
            return mutate

        corruptions = {
            "missing identity": delete(("agents", "claude", "sessions")),
            "wrong count type": lambda value: value["agents"]["claude"].__setitem__("sessions", "many"),
            "wrong cost type": lambda value: value["agents"]["claude"].__setitem__("estimated_cost_usd", "unknown"),
            "missing coverage": delete(("agents", "codex", "coverage")),
            "missing provenance": delete(("agents", "copilot", "provenance")),
            "wrong rtk installation": lambda value: value["rtk"].__setitem__("installed", "yes"),
            "wrong rtk version": lambda value: value["rtk"].__setitem__("version", [1, 2, 3]),
            "corrupt data gaps": lambda value: value.__setitem__("data_gaps", "none"),
            "missing null explanation": lambda value: value["null_reasons"].pop("agents.codex.cache_write_tokens"),
            "dishonest null explanation": lambda value: value["null_reasons"].__setitem__("agents.codex.cache_write_tokens", "undefined_ratio"),
            "missing structured gap link": lambda value: value["null_gap_links"].pop("agents.codex.raw_traffic_tokens"),
            "dangling structured gap link": lambda value: value["null_gap_links"].__setitem__("agents.codex.raw_traffic_tokens", "gap-does-not-exist"),
            "wrong structured gap source": lambda value: value["data_gap_details"][0].__setitem__("source", "unknown-source"),
            "wrong structured gap code": lambda value: value["data_gap_details"][0].__setitem__("code", "unknown-code"),
            "mismatched linked gap code": lambda value: value["data_gap_details"][0].__setitem__("code", "coverage_incomplete"),
            "scenario flag integer": lambda value: value.__setitem__("scenario_is_canonical", 1),
        }
        for label, mutate in corruptions.items():
            with self.subTest(label=label):
                candidate = copy.deepcopy(report)
                mutate(candidate)
                with self.assertRaises(ValueError):
                    te.validate_report(candidate)

    def test_source_nulls_link_to_structured_gaps_but_measured_zero_does_not(self):
        with tempfile.TemporaryDirectory() as tmp:
            empty_claude_dir = Path(tmp) / "claude"
            empty_claude_dir.mkdir()
            report = te.build_report(
                window=WINDOW,
                scenario="native-agent",
                repo_alias=None,
                claude=te.collect_claude(empty_claude_dir, WINDOW),
                codex=te.SourceResult(
                    dict(te._CODEX_EMPTY), "codex", gaps=["codex source unavailable"]
                ),
                copilot=te.SourceResult(
                    dict(te._COPILOT_EMPTY), "copilot", gaps=["copilot source unavailable"]
                ),
                kronn=te.SourceResult(
                    dict(te._KRONN_EMPTY), "kronn", gaps=["kronn source unavailable"]
                ),
                rtk=te.SourceResult(
                    dict(te._RTK_EMPTY), "rtk", gaps=["rtk source unavailable"]
                ),
                completed_tasks=1,
            )

        self.assertEqual(report["agents"]["claude"]["raw_traffic_tokens"], 0)
        self.assertNotIn("agents.claude.raw_traffic_tokens", report["null_gap_links"])
        for path, reason in report["null_reasons"].items():
            if reason in {"source_unavailable", "insufficient_granularity"}:
                self.assertIn(path, report["null_gap_links"])
                gap_id = report["null_gap_links"][path]
                gap = next(item for item in report["data_gap_details"] if item["id"] == gap_id)
                self.assertEqual(gap["code"], reason)

    def test_insufficient_granularity_nulls_link_to_the_rtk_gap(self):
        rtk_metrics = dict(te._RTK_EMPTY)
        rtk_metrics.update({
            "installed": True,
            "version": "rtk test",
            "included_full_days": 0,
            "window_coverage": "none",
        })
        report = te.build_report(
            window=WINDOW,
            scenario="native-agent",
            repo_alias=None,
            claude=self._empty(),
            codex=te.SourceResult(dict(te._CODEX_EMPTY), "codex", gaps=["codex unavailable"]),
            copilot=te.SourceResult(dict(te._COPILOT_EMPTY), "copilot", gaps=["copilot unavailable"]),
            kronn=te.SourceResult(dict(te._KRONN_EMPTY), "kronn", gaps=["kronn unavailable"]),
            rtk=te.SourceResult(
                rtk_metrics,
                "rtk",
                gaps=["rtk has no complete UTC day inside the requested window"],
            ),
        )
        path = "rtk.saved_tokens"
        self.assertEqual(report["null_reasons"][path], "insufficient_granularity")
        gap_id = report["null_gap_links"][path]
        gap = next(item for item in report["data_gap_details"] if item["id"] == gap_id)
        self.assertEqual(gap["code"], "insufficient_granularity")

    def test_kronn_traced_share_is_derived_from_untraced_reply_counts(self):
        kronn_metrics = {
            "external_agent_replies": 3,
            "external_replies_with_tokens_pct": 33.33,
            "traced_tokens_used": 1_200,
            "untraced_replies_by_agent": {"Codex": 2, "ClaudeCode": 0},
        }
        report = te.build_report(
            window=WINDOW,
            scenario="native-agent",
            repo_alias=None,
            claude=self._empty(),
            codex=te.SourceResult(dict(te._CODEX_EMPTY), "codex", gaps=["codex unavailable"]),
            copilot=te.SourceResult(dict(te._COPILOT_EMPTY), "copilot", gaps=["copilot unavailable"]),
            kronn=te.SourceResult(kronn_metrics, "kronn"),
            rtk=te.SourceResult(dict(te._RTK_EMPTY), "rtk", gaps=["rtk unavailable"]),
        )

        wrong_percentage = copy.deepcopy(report)
        wrong_percentage["kronn"]["external_replies_with_tokens_pct"] = 33.34
        with self.assertRaisesRegex(ValueError, "external_replies_with_tokens_pct arithmetic"):
            te.validate_report(wrong_percentage)

        impossible_untraced = copy.deepcopy(report)
        impossible_untraced["kronn"]["untraced_replies_by_agent"]["Codex"] = 4
        with self.assertRaisesRegex(ValueError, "untraced_replies_by_agent arithmetic"):
            te.validate_report(impossible_untraced)

    def test_fixed_window_report_is_byte_reproducible_after_later_events_arrive(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            claude_dir = root / "claude"
            codex_dir = root / "codex" / "2026" / "08" / "02"
            claude_dir.mkdir()
            codex_dir.mkdir(parents=True)
            claude_file = claude_dir / "session.jsonl"
            codex_file = codex_dir / "rollout-fixed.jsonl"
            claude_file.write_text(
                _claude_line(request_id="r-in", msg_id="m-in", ts="2026-08-02T10:00:00Z") + "\n"
            )
            codex_file.write_text(
                _codex_meta("root-fixed") + "\n"
                + _codex_event(
                    {"input_tokens": 100, "cached_input_tokens": 80, "output_tokens": 10},
                    ts="2026-08-02T10:00:00Z",
                ) + "\n"
            )

            def snapshot():
                report = te.build_report(
                    window=WINDOW,
                    scenario="cli-persistent",
                    repo_alias=None,
                    claude=te.collect_claude(claude_dir, WINDOW),
                    codex=te.collect_codex(root / "codex", WINDOW),
                    copilot=te.SourceResult(dict(te._COPILOT_EMPTY), "copilot", gaps=["copilot absent"]),
                    kronn=te.SourceResult(dict(te._KRONN_EMPTY), "kronn", gaps=["kronn absent"]),
                    rtk=te.SourceResult(dict(te._RTK_EMPTY), "rtk", gaps=["rtk absent"]),
                )
                return json.dumps(report, sort_keys=True, separators=(",", ":"))

            before = snapshot()
            with claude_file.open("a") as fh:
                fh.write(_claude_line(
                    request_id="r-in", msg_id="m-in", ts="2026-08-05T10:00:00Z",
                    input_tokens=999_999,
                ) + "\n")
                malformed_after = json.loads(_claude_line(
                    request_id="bad-after", msg_id="bad-after", ts="2026-08-05T11:00:00Z",
                ))
                malformed_after["message"]["usage"]["input_tokens"] = "invalid"
                fh.write(json.dumps(malformed_after) + "\n")
                fh.write("later unplaceable invalid JSON\n")
            with codex_file.open("a") as fh:
                fh.write(_codex_event(
                    {"input_tokens": 999_999, "cached_input_tokens": 900_000, "output_tokens": 90_000},
                    ts="2026-08-05T10:00:00Z",
                ) + "\n")
            self.assertEqual(snapshot(), before)

    def test_privacy_canaries_never_reach_the_export(self):
        canaries = {
            "claude text": "CANARY_CLAUDE_TEXT_7d4f",
            "tool input": "CANARY_TOOL_INPUT_9a2e",
            "cwd": "CANARY_CWD_1b6c",
            "session": "CANARY_SESSION_55aa",
            "request": "CANARY_REQUEST_cc09",
            "codex content": "CANARY_CODEX_CONTENT_a80d",
            "repo": "CANARY_REPO_ALIAS_883e",
        }
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            claude_dir = root / "claude"
            codex_dir = root / "codex" / "2026" / "08" / "02"
            claude_dir.mkdir()
            codex_dir.mkdir(parents=True)
            claude = json.loads(_claude_line(
                request_id=canaries["request"], msg_id="message-secret",
                session=canaries["session"], cwd=canaries["cwd"],
            ))
            claude["message"]["content"] = [
                {"type": "text", "text": canaries["claude text"]},
                {
                    "type": "tool_use", "name": "Bash",
                    "input": {"command": canaries["tool input"]},
                },
            ]
            (claude_dir / "private.jsonl").write_text(json.dumps(claude) + "\n")
            codex_payload = json.loads(_codex_event(
                {"input_tokens": 10, "cached_input_tokens": 5, "output_tokens": 2}
            ))
            codex_payload["payload"]["content"] = canaries["codex content"]
            codex_payload["payload"]["cwd"] = canaries["cwd"]
            codex_payload["payload"]["request_id"] = canaries["request"]
            (codex_dir / "rollout-private.jsonl").write_text(
                _codex_meta("root-private") + "\n" + json.dumps(codex_payload) + "\n"
            )
            report = te.build_report(
                window=WINDOW,
                scenario="native-agent",
                repo_alias=canaries["repo"],
                claude=te.collect_claude(claude_dir, WINDOW),
                codex=te.collect_codex(root / "codex", WINDOW),
                copilot=te.SourceResult(dict(te._COPILOT_EMPTY), "copilot", gaps=["copilot absent"]),
                kronn=te.SourceResult(dict(te._KRONN_EMPTY), "kronn", gaps=["kronn absent"]),
                rtk=te.SourceResult(dict(te._RTK_EMPTY), "rtk", gaps=["rtk absent"]),
            )
            serialized = json.dumps(report)
            for label, canary in canaries.items():
                with self.subTest(label=label):
                    self.assertNotIn(canary, serialized)

    def test_render_text_shows_na_for_nulls(self):
        report = te.build_report(
            window=WINDOW,
            scenario=None,
            repo_alias=None,
            claude=self._empty(),
            codex=te.SourceResult(dict(te._CODEX_EMPTY), "codex"),
            copilot=te.SourceResult(dict(te._COPILOT_EMPTY), "copilot"),
            kronn=te.SourceResult(dict(te._KRONN_EMPTY), "kronn"),
            rtk=te.SourceResult(dict(te._RTK_EMPTY), "rtk"),
        )
        text = te.render_text(report)
        self.assertIn("raw=n/a", text)
        self.assertIn("data gaps:", text)
        self.assertNotIn("raw=0 ", text)


class CliTests(unittest.TestCase):
    def test_end_to_end_report_with_fixture_dirs(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            claude = root / "claude"
            claude.mkdir()
            (claude / "p.jsonl").write_text(_claude_line(request_id="r1", msg_id="m1") + "\n")
            out = root / "team.json"
            code = te.main(
                [
                    "report",
                    "--from", "2026-08-01T00:00:00+00:00",
                    "--to", "2026-08-03T00:00:00+00:00",
                    "--claude-dir", str(claude),
                    "--codex-dir", str(root / "missing-codex"),
                    "--copilot-db", str(root / "missing.db"),
                    "--kronn-db", str(root / "missing-kronn.db"),
                    "--no-rtk",
                    "--json", str(out),
                    "--scenario", "cli-oneshot",
                ]
            )
            self.assertEqual(code, 0)
            report = json.loads(out.read_text())
            self.assertEqual(report["agents"]["claude"]["raw_traffic_tokens"], 1115)
            self.assertIsNone(report["agents"]["codex"]["raw_traffic_tokens"])
            self.assertEqual(report["scenario"], "cli-oneshot")

    def test_bad_timestamp_is_a_usage_error(self):
        self.assertEqual(te.main(["report", "--from", "not-a-date"]), 2)


if __name__ == "__main__":
    unittest.main()
