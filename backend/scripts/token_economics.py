#!/usr/bin/env python3
"""Token Economics baseline collector (KT-188).

Aggregates LOCAL, READ-ONLY token-usage metadata for the agents that work
on this machine and emits the canonical team report defined in
docs/design/token-economics-baseline.md.

Hard rules (the contract, not aspirations):
  * Telemetry records are parsed in memory (which necessarily includes
    their content fields), but conversation/prompt content, file bodies,
    secrets and credentials are never stored, aggregated, printed or
    exported. Only usage counters, timestamps, models, tool NAMES and
    truncated/hashed identifiers leave this script.
  * Every `null`/`None` has a typed `null_reasons` entry. Source/granularity
    limitations also appear in `data_gaps`; undefined ratios, unsupported
    metrics and intentionally unconfigured costs do not masquerade as source
    gaps. A readable, sufficiently granular source with no activity reports
    measured ZEROS — the cases must never be conflated.
  * Raw traffic (dominated by cache reads) is NEVER presented as billing.

Sources (each optional — a missing source becomes a data gap, not a crash):
  * Claude Code   ~/.claude/projects/**/*.jsonl   (assistant usage records)
  * Codex         ~/.codex/sessions/**/rollout-*.jsonl (token_count events)
  * Copilot       ~/.copilot/session-store.db     (assistant_usage_events)
  * Kronn         kronn.db (read-only)            (messages.tokens_used)
  * RTK           `rtk gain` CLI                  (compaction savings)

Usage:
    python3 backend/scripts/token_economics.py report --days 30
    python3 backend/scripts/token_economics.py report \
        --from 2026-08-02T10:00:00 --to 2026-08-02T11:00:00 \
        --scenario cli-persistent-wait --json out.json

Run the tests: python3 -m unittest backend.scripts.test_token_economics
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import sqlite3
import subprocess
import sys
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Callable, Iterator

SCHEMA_VERSION = "1.3.0"

NULL_REASON_CODES = frozenset(
    {
        "source_unavailable",
        "unsupported_metric",
        "undefined_ratio",
        "unconfigured_cost",
        "insufficient_granularity",
        "missing_denominator",
        "not_requested",
        "probe_unavailable",
        "no_observed_events",
        "non_canonical_value_omitted",
    }
)

DATA_GAP_CODES = frozenset(
    {
        "source_unavailable",
        "insufficient_granularity",
        "source_quality",
        "coverage_incomplete",
        "missing_denominator",
        "non_canonical_value_omitted",
    }
)

DATA_GAP_SOURCES = frozenset(
    {"claude", "codex", "copilot", "kronn", "rtk", "report", "kpi"}
)

# The four canonical scenarios of the baseline protocol. Free-form labels
# are allowed (exploratory runs), but these four are the release contract.
CANONICAL_SCENARIOS = (
    "native-agent",
    "cli-oneshot",
    "cli-persistent",
    "cli-persistent-wait",
)


# ---------------------------------------------------------------------------
# Shared helpers
# ---------------------------------------------------------------------------


def _parse_ts(value: str | None) -> datetime | None:
    """Parse an ISO-8601 timestamp; returns None on anything unparseable."""
    if not value or not isinstance(value, str):
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def _to_utc(dt: datetime) -> datetime:
    if dt.tzinfo is None:
        return dt.replace(tzinfo=timezone.utc)
    return dt.astimezone(timezone.utc)


def _pseudonym(raw: str, keep: int = 8) -> str:
    """Stable pseudonym: SHA-256 prefix, never the raw value."""
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()[:keep]


def _pct(part: int | float | None, total: int | float | None) -> float | None:
    if part is None or not total:
        return None
    return round(100.0 * part / total, 2)


def _counter(value: Any) -> int | None:
    """Return a telemetry counter only when its type and range are trustworthy."""
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        return None
    return value


@dataclass(frozen=True)
class Window:
    """Measurement window. A scenario run = one explicit window."""

    start: datetime
    end: datetime

    def __post_init__(self) -> None:
        if not isinstance(self.start, datetime) or not isinstance(self.end, datetime):
            raise TypeError("window boundaries must be datetime values")
        start = _to_utc(self.start)
        end = _to_utc(self.end)
        if start > end:
            raise ValueError("window start must not be after window end")
        object.__setattr__(self, "start", start)
        object.__setattr__(self, "end", end)

    def contains(self, ts: datetime | None) -> bool:
        if ts is None:
            return False
        ts = _to_utc(ts)
        return self.start <= ts <= self.end

    def as_dict(self) -> dict[str, str]:
        return {"from": self.start.isoformat(), "to": self.end.isoformat()}


@dataclass
class SourceResult:
    """One collector's output: metrics + provenance + observed coverage."""

    metrics: dict[str, Any]
    provenance: str
    coverage_from: datetime | None = None
    coverage_to: datetime | None = None
    gaps: list[str] = field(default_factory=list)
    null_reason_overrides: dict[str, str] = field(default_factory=dict)

    def coverage_dict(self) -> dict[str, str | None]:
        return {
            "observed_from": self.coverage_from.isoformat() if self.coverage_from else None,
            "observed_to": self.coverage_to.isoformat() if self.coverage_to else None,
        }


def _unavailable(provenance: str, reason: str, metrics: dict[str, Any]) -> SourceResult:
    return SourceResult(metrics=metrics, provenance=provenance, gaps=[reason])


# ---------------------------------------------------------------------------
# Claude Code collector
# ---------------------------------------------------------------------------

_CLAUDE_EMPTY: dict[str, Any] = {
    "raw_traffic_tokens": None,
    "estimated_cost_usd": None,
    "non_cached_input_tokens": None,
    "cache_write_tokens": None,
    "cache_read_tokens": None,
    "output_tokens": None,
    "assistant_calls": None,
    "sessions": None,
    "top_1_share_pct": None,
    "top_4_share_pct": None,
    "repo_sessions_share_pct": None,
    "disc_wait_calls": None,
    "disc_wait_associated_tokens": None,
}


@dataclass
class JsonlReadState:
    invalid_json_lines: int = 0
    unreadable_files: int = 0


def _invalid_json_timestamp(line: str) -> datetime | None:
    """Recover only an envelope timestamp, never one from embedded content."""
    envelope = re.split(r'"(?:message|payload|content)"\s*:', line, maxsplit=1)[0]
    match = re.search(r'"timestamp"\s*:\s*"([^"\\]+)"', envelope)
    return _parse_ts(match[1]) if match else None


def _iter_jsonl(
    path: Path,
    state: JsonlReadState,
    invalid_line_is_in_scope: Callable[[str], bool],
) -> Iterator[dict[str, Any]]:
    try:
        with path.open("r", encoding="utf-8", errors="replace") as fh:
            for line in fh:
                try:
                    obj = json.loads(line)
                except json.JSONDecodeError:
                    if invalid_line_is_in_scope(line):
                        state.invalid_json_lines += 1
                    continue
                if isinstance(obj, dict):
                    yield obj
                elif invalid_line_is_in_scope(line):
                    state.invalid_json_lines += 1
    except OSError:
        state.unreadable_files += 1
        return


def _tool_names(message: dict[str, Any]) -> list[str]:
    """Tool NAMES used in an assistant message — never inputs or text."""
    content = message.get("content")
    names: list[str] = []
    if isinstance(content, list):
        for block in content:
            if isinstance(block, dict) and block.get("type") == "tool_use":
                name = block.get("name")
                if isinstance(name, str):
                    names.append(name)
    return names


def collect_claude(
    projects_dir: Path, window: Window, repo_filter: str | None = None
) -> SourceResult:
    provenance = (
        "claude-code jsonl (stream records canonicalized by requestId+message.id: "
        "earliest timestamp, largest coherent usage snapshot, union of tool names)"
    )
    if not projects_dir.is_dir():
        return _unavailable(provenance, "claude projects dir not found", dict(_CLAUDE_EMPTY))

    canonical: dict[tuple[str, str], dict[str, Any]] = {}
    malformed = False
    read_state = JsonlReadState()
    for path in sorted(projects_dir.glob("**/*.jsonl")):
        for obj in _iter_jsonl(
            path,
            read_state,
            lambda line: window.contains(_invalid_json_timestamp(line)),
        ):
            if obj.get("type") != "assistant":
                continue
            message = obj.get("message")
            if not isinstance(message, dict):
                continue
            ts = _parse_ts(obj.get("timestamp"))
            if ts is None:
                malformed = True
                continue
            ts = _to_utc(ts)
            if ts > window.end:
                continue
            request_id = obj.get("requestId")
            message_id = message.get("id")
            usage = message.get("usage")
            if (
                not isinstance(request_id, str)
                or not request_id
                or not isinstance(message_id, str)
                or not message_id
                or not isinstance(usage, dict)
            ):
                malformed = True
                continue
            vector_values = (
                _counter(usage.get("input_tokens", 0)),
                _counter(usage.get("cache_creation_input_tokens", 0)),
                _counter(usage.get("cache_read_input_tokens", 0)),
                _counter(usage.get("output_tokens", 0)),
            )
            if any(value is None for value in vector_values):
                malformed = True
                continue
            vector = {
                "input": vector_values[0],
                "cache_write": vector_values[1],
                "cache_read": vector_values[2],
                "output": vector_values[3],
            }
            key = (request_id, message_id)
            current = canonical.get(key)
            if current is None:
                canonical[key] = {
                    "timestamp": ts,
                    "usage": vector,
                    "tools": set(_tool_names(message)),
                    "session": str(obj.get("sessionId") or "unknown"),
                    "cwd": obj.get("cwd"),
                }
                continue

            current["timestamp"] = min(current["timestamp"], ts)
            current["tools"].update(_tool_names(message))
            # Streaming duplicates are cumulative snapshots. Selecting one
            # whole vector preserves coherence; field-wise maxima could
            # synthesize a usage record that never existed.
            if sum(vector.values()) >= sum(current["usage"].values()):
                current["usage"] = vector
                current["session"] = str(obj.get("sessionId") or current["session"])
                if isinstance(obj.get("cwd"), str):
                    current["cwd"] = obj["cwd"]

    totals = {"input": 0, "cache_write": 0, "cache_read": 0, "output": 0}
    per_session: dict[str, int] = {}
    repo_session_ids: set[str] = set()
    calls = 0
    wait_calls = 0
    wait_tokens = 0
    cov_from: datetime | None = None
    cov_to: datetime | None = None

    for record in canonical.values():
        ts = record["timestamp"]
        if not window.contains(ts):
            continue

        cov_from = ts if cov_from is None or ts < cov_from else cov_from
        cov_to = ts if cov_to is None or ts > cov_to else cov_to

        usage = record["usage"]
        inp = usage["input"]
        cw = usage["cache_write"]
        cr = usage["cache_read"]
        out = usage["output"]
        turn = inp + cw + cr + out
        totals["input"] += inp
        totals["cache_write"] += cw
        totals["cache_read"] += cr
        totals["output"] += out
        calls += 1

        session = record["session"]
        per_session[session] = per_session.get(session, 0) + turn
        cwd = record["cwd"]
        if repo_filter and isinstance(cwd, str) and repo_filter in cwd:
            repo_session_ids.add(session)

        if any("disc_wait_for_peer" in name for name in record["tools"]):
            wait_calls += 1
            wait_tokens += turn

    # The directory exists, so whatever we counted — including nothing —
    # is a measurement: zeros are real zeros here, never stand-ins.
    if read_state.unreadable_files:
        return _unavailable(
            provenance,
            f"claude telemetry files unreadable ({read_state.unreadable_files})",
            dict(_CLAUDE_EMPTY),
        )
    raw = sum(totals.values())
    ranked = sorted(per_session.values(), reverse=True)
    repo_traffic = sum(per_session[s] for s in repo_session_ids) if repo_filter else None
    metrics: dict[str, Any] = {
        "raw_traffic_tokens": raw,
        # Cost needs a per-model tariff table; null until one is configured.
        "estimated_cost_usd": None,
        "non_cached_input_tokens": totals["input"],
        "cache_write_tokens": totals["cache_write"],
        "cache_read_tokens": totals["cache_read"],
        "output_tokens": totals["output"],
        "assistant_calls": calls,
        "sessions": len(per_session),
        "top_1_share_pct": _pct(ranked[0] if ranked else None, raw),
        "top_4_share_pct": _pct(sum(ranked[:4]) if ranked else None, raw),
        "repo_sessions_share_pct": _pct(repo_traffic, raw),
        "disc_wait_calls": wait_calls,
        "disc_wait_associated_tokens": wait_tokens,
    }
    gaps = []
    if read_state.invalid_json_lines:
        gaps.append(
            f"claude: invalid JSON lines were ignored ({read_state.invalid_json_lines})"
        )
    if malformed:
        gaps.append("claude: malformed assistant usage records were ignored")
    overrides = (
        {"repo_sessions_share_pct": "not_requested"}
        if repo_filter is None
        else {}
    )
    return SourceResult(
        metrics, provenance, cov_from, cov_to, gaps,
        null_reason_overrides=overrides,
    )


# ---------------------------------------------------------------------------
# Codex collector
# ---------------------------------------------------------------------------

_CODEX_EMPTY: dict[str, Any] = {
    "raw_traffic_tokens": None,
    "estimated_cost_usd": None,
    "non_cached_input_tokens": None,
    "cache_write_tokens": None,
    "cache_read_tokens": None,
    "output_tokens": None,
    "reasoning_tokens": None,
    "sessions": None,
    "rollouts": None,
    "top_1_share_pct": None,
}


_CODEX_COUNTERS = ("input", "cached", "output", "reasoning")


def _codex_usage_vector(usage: dict[str, Any]) -> dict[str, int] | None:
    values = (
        _counter(usage.get("input_tokens", 0)),
        _counter(usage.get("cached_input_tokens", 0)),
        _counter(usage.get("output_tokens", 0)),
        _counter(usage.get("reasoning_output_tokens", 0)),
    )
    if any(value is None for value in values):
        return None
    vector = dict(zip(_CODEX_COUNTERS, values, strict=True))
    if vector["cached"] > vector["input"] or vector["reasoning"] > vector["output"]:
        return None
    return vector


def _codex_total(vector: dict[str, int] | None) -> int:
    if vector is None:
        return 0
    return vector["input"] + vector["output"]


def _codex_delta(
    start: dict[str, int], end: dict[str, int]
) -> dict[str, int] | None:
    """Return a coherent cumulative-counter delta, or fail closed.

    Cached input is a subset of input. Checking the derived non-cached
    cumulative counter prevents two divergent branches from being paired into
    an impossible delta even when every raw component happens to increase.
    """
    if any(end[key] < start[key] for key in _CODEX_COUNTERS):
        return None
    if end["input"] - end["cached"] < start["input"] - start["cached"]:
        return None
    delta = {key: end[key] - start[key] for key in _CODEX_COUNTERS}
    if delta["reasoning"] > delta["output"]:
        return None
    return delta


def collect_codex(sessions_dir: Path, window: Window) -> SourceResult:
    # Two traps, both measured on real data (2026-08-02):
    #  * token_count snapshots are CUMULATIVE over a session's lifetime, so
    #    the window contribution of a thread is the DELTA between its best
    #    snapshot at the window end and its best snapshot before the window
    #    start — never the raw cumulative total (a month-old session doing
    #    10k tokens today must report 10k, not its lifetime millions);
    #  * a forked/resumed rollout REPLAYS its parent's counters, so rollouts
    #    are grouped by session_meta.session_id (the ROOT thread). A fork is
    #    compared only with its own earlier snapshot or a timestamped root
    #    ancestor. Divergent/unproven ancestry fails the provider closed.
    provenance = (
        "codex rollout jsonl (per root thread: ancestry-coherent cumulative "
        "window deltas; divergent branches fail closed)"
    )
    if not sessions_dir.is_dir():
        return _unavailable(provenance, "codex sessions dir not found", dict(_CODEX_EMPTY))

    full_day_dates = {day.date().isoformat() for day in _full_utc_days(window)}
    has_partial_boundary = (
        window.start.time() != datetime.min.time()
        or window.end.time() != datetime.min.time()
    )

    # Per root thread, preserve rollout identity. Boundary snapshots from
    # different forks must never be combined merely because their totals are
    # independently maximal.
    per_thread: dict[str, list[dict[str, Any]]] = {}
    malformed = False
    read_state = JsonlReadState()
    cov_from: datetime | None = None
    cov_to: datetime | None = None

    for path in sorted(sessions_dir.glob("**/rollout-*.jsonl")):
        root_id: str | None = None
        rollout_id: str | None = None
        session_started_at: datetime | None = None
        # Fallback timestamp when an event carries none: the date encoded
        # in the rollout path (YYYY/MM/DD).
        m = re.search(r"(\d{4})/(\d{2})/(\d{2})", str(path))
        path_ts = (
            datetime(int(m[1]), int(m[2]), int(m[3]), tzinfo=timezone.utc) if m else None
        )
        if path_ts is not None and path_ts > window.end:
            continue
        path_date_is_in_scope = (
            path_ts is not None and path_ts.date().isoformat() in full_day_dates
        )

        def invalid_line_is_in_scope(line: str) -> bool:
            recovered_ts = _invalid_json_timestamp(line)
            if recovered_ts is not None:
                return window.contains(recovered_ts)
            return path_date_is_in_scope

        file_end: tuple[datetime, dict[str, int]] | None = None
        file_start: tuple[datetime, dict[str, int]] | None = None
        snapshots: list[tuple[datetime, dict[str, int]]] = []
        first_in_window: datetime | None = None
        for obj in _iter_jsonl(path, read_state, invalid_line_is_in_scope):
            payload = obj.get("payload")
            if not isinstance(payload, dict):
                continue
            # session_meta lines carry the type at the LINE level (payload
            # holds only the fields); tolerate both placements.
            is_meta = obj.get("type") == "session_meta" or payload.get("type") == "session_meta"
            if root_id is None and is_meta:
                root = payload.get("session_id") or payload.get("id")
                if isinstance(root, str):
                    root_id = root
                own = payload.get("id") or root
                if isinstance(own, str):
                    rollout_id = own
                meta_ts = _parse_ts(obj.get("timestamp"))
                if meta_ts is not None:
                    meta_ts = _to_utc(meta_ts)
                    session_started_at = (
                        meta_ts
                        if session_started_at is None or meta_ts < session_started_at
                        else session_started_at
                    )
            if payload.get("type") != "token_count":
                continue
            info = payload.get("info")
            if not isinstance(info, dict):
                continue
            usage = info.get("total_token_usage")
            if not isinstance(usage, dict):
                continue
            ts = _parse_ts(obj.get("timestamp"))
            if ts is None and path_ts is not None:
                path_date = path_ts.date().isoformat()
                path_is_fully_before_window = path_ts + timedelta(days=1) <= window.start
                if path_date not in full_day_dates and not path_is_fully_before_window:
                    continue
                ts = path_ts
            if ts is not None:
                ts = _to_utc(ts)
                if ts > window.end:
                    continue
            vector = _codex_usage_vector(usage)
            if vector is None:
                malformed = True
                continue
            if ts is None:
                malformed = True
                continue
            if ts <= window.end:
                snapshots.append((ts, vector))
                if file_end is None or ts >= file_end[0]:
                    file_end = (ts, vector)
            if ts < window.start and (file_start is None or ts >= file_start[0]):
                file_start = (ts, vector)
            if window.contains(ts):
                first_in_window = ts if first_in_window is None or ts < first_in_window else first_in_window
                cov_from = ts if cov_from is None or ts < cov_from else cov_from
                cov_to = ts if cov_to is None or ts > cov_to else cov_to
        if file_end is None and root_id is None:
            continue
        key = root_id or path.stem
        per_thread.setdefault(key, []).append(
            {
                "rollout_id": rollout_id or key,
                "start": file_start,
                "end": file_end,
                "snapshots": snapshots,
                "first_in_window": first_in_window,
                "session_started_at": session_started_at,
            }
        )

    if read_state.unreadable_files:
        return _unavailable(
            provenance,
            f"codex telemetry files unreadable ({read_state.unreadable_files})",
            dict(_CODEX_EMPTY),
        )

    deltas: list[dict[str, int]] = []
    active_rollouts = 0
    incoherent_threads = 0
    zero = {counter: 0 for counter in _CODEX_COUNTERS}
    for root_id, rollouts in per_thread.items():
        root_rollouts = [row for row in rollouts if row["rollout_id"] == root_id]
        root_snapshots = sorted(
            (snapshot for row in root_rollouts for snapshot in row["snapshots"]),
            key=lambda item: item[0],
        )
        root_before_window = [snapshot for snapshot in root_snapshots if snapshot[0] < window.start]
        root_session_starts = [
            row["session_started_at"]
            for row in root_rollouts
            if row["session_started_at"] is not None
        ]
        root_session_started_at = min(root_session_starts) if root_session_starts else None
        thread_candidates: list[dict[str, int]] = []
        thread_is_incoherent = False

        for rollout in rollouts:
            first_in_window = rollout["first_in_window"]
            if first_in_window is None:
                continue
            end = rollout["end"][1]
            own_start = rollout["start"][1] if rollout["start"] else None

            if own_start is not None:
                thread_start = own_start
                activity_start = own_start
            elif rollout["rollout_id"] == root_id:
                if root_before_window:
                    thread_start = root_before_window[-1][1]
                    activity_start = thread_start
                elif root_session_started_at is not None and root_session_started_at >= window.start:
                    thread_start = zero
                    activity_start = zero
                else:
                    thread_is_incoherent = True
                    continue
            else:
                ancestors = [
                    snapshot for snapshot in root_snapshots if snapshot[0] <= first_in_window
                ]
                if not ancestors:
                    thread_is_incoherent = True
                    continue
                activity_start = ancestors[-1][1]
                thread_start = root_before_window[-1][1] if root_before_window else zero

            thread_delta = _codex_delta(thread_start, end)
            activity_delta = _codex_delta(activity_start, end)
            if thread_delta is None or activity_delta is None:
                thread_is_incoherent = True
                continue
            if _codex_total(thread_delta) > 0:
                thread_candidates.append(thread_delta)
            if _codex_total(activity_delta) > 0:
                active_rollouts += 1

        if thread_is_incoherent:
            incoherent_threads += 1
            continue
        if thread_candidates:
            deltas.append(max(thread_candidates, key=_codex_total))

    if incoherent_threads:
        gaps = [
            f"codex: {incoherent_threads} root thread(s) had divergent or unproven branch ancestry; metrics omitted"
        ]
        if has_partial_boundary:
            gaps.append(
                "codex path-date fallback cannot allocate timestamp-less records on partial UTC boundary days"
            )
        if malformed:
            gaps.append("codex: malformed token_count records were ignored")
        if read_state.invalid_json_lines:
            gaps.append(
                f"codex: invalid JSON lines were ignored ({read_state.invalid_json_lines})"
            )
        return SourceResult(dict(_CODEX_EMPTY), provenance, cov_from, cov_to, gaps)

    input_total = sum(d["input"] for d in deltas)
    cached = sum(d["cached"] for d in deltas)
    output = sum(d["output"] for d in deltas)
    raw = input_total + output
    ranked = sorted((d["input"] + d["output"] for d in deltas), reverse=True)
    metrics: dict[str, Any] = {
        "raw_traffic_tokens": raw,
        "estimated_cost_usd": None,
        "non_cached_input_tokens": input_total - cached,
        "cache_write_tokens": None,
        "cache_read_tokens": cached,
        "output_tokens": output,
        "reasoning_tokens": sum(d["reasoning"] for d in deltas),
        # Threads and rollouts with in-window activity only.
        "sessions": len(deltas),
        "rollouts": active_rollouts,
        "top_1_share_pct": _pct(ranked[0] if ranked else None, raw),
    }
    gaps = []
    if has_partial_boundary:
        gaps.append(
            "codex path-date fallback cannot allocate timestamp-less records on partial UTC boundary days"
        )
    if malformed:
        gaps.append("codex: malformed token_count records were ignored")
    if read_state.invalid_json_lines:
        gaps.append(
            f"codex: invalid JSON lines were ignored ({read_state.invalid_json_lines})"
        )
    return SourceResult(metrics, provenance, cov_from, cov_to, gaps)


# ---------------------------------------------------------------------------
# Copilot collector
# ---------------------------------------------------------------------------

_COPILOT_EMPTY: dict[str, Any] = {
    "raw_traffic_tokens": None,
    "estimated_cost_usd": None,
    "non_cached_input_tokens": None,
    "cache_read_tokens": None,
    "cache_write_tokens": None,
    "output_tokens": None,
    "reasoning_tokens": None,
    "calls": None,
    "coverage_days": None,
    "observed_range_covers_window": None,
}


def _open_ro(db_path: Path) -> sqlite3.Connection:
    return sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)


def collect_copilot(db_path: Path, window: Window) -> SourceResult:
    provenance = "copilot session-store.db assistant_usage_events (read-only)"
    if not db_path.is_file():
        return _unavailable(provenance, "copilot db not found", dict(_COPILOT_EMPTY))
    try:
        conn = _open_ro(db_path)
        try:
            row = conn.execute(
                """
                SELECT COUNT(*),
                       SUM(input_tokens), SUM(cache_read_tokens),
                       SUM(cache_write_tokens), SUM(output_tokens),
                       SUM(reasoning_tokens),
                       COUNT(DISTINCT date(created_at))
                  FROM assistant_usage_events
                 WHERE julianday(created_at) >= julianday(?)
                   AND julianday(created_at) <= julianday(?)
                """,
                (window.start.isoformat(), window.end.isoformat()),
            ).fetchone()
            # Coverage is window-scoped. Later rows must not change a fixed
            # historical snapshot.
            cov_row = conn.execute(
                "SELECT "
                "(SELECT created_at FROM assistant_usage_events "
                " WHERE julianday(created_at) >= julianday(?) "
                "   AND julianday(created_at) <= julianday(?) "
                " ORDER BY julianday(created_at) ASC LIMIT 1), "
                "(SELECT created_at FROM assistant_usage_events "
                " WHERE julianday(created_at) >= julianday(?) "
                "   AND julianday(created_at) <= julianday(?) "
                " ORDER BY julianday(created_at) DESC LIMIT 1)",
                (
                    window.start.isoformat(), window.end.isoformat(),
                    window.start.isoformat(), window.end.isoformat(),
                ),
            ).fetchone()
        finally:
            conn.close()
    except sqlite3.Error as exc:
        return _unavailable(provenance, f"copilot db unreadable: {exc}", dict(_COPILOT_EMPTY))

    # A readable table with no rows in the window is a real measurement.
    count = _counter(row[0])
    coverage_days = _counter(row[6])
    counter_values: list[int | None] = []
    for value in row[1:6]:
        counter_values.append(0 if count == 0 and value is None else _counter(value))
    if count is None or coverage_days is None or any(value is None for value in counter_values):
        return _unavailable(
            provenance, "copilot db contains malformed counters", dict(_COPILOT_EMPTY)
        )
    inp, cr, cw, out, reasoning = counter_values
    cov_from, cov_to = _parse_ts(cov_row[0]), _parse_ts(cov_row[1])
    cov_from = _to_utc(cov_from) if cov_from else None
    cov_to = _to_utc(cov_to) if cov_to else None
    # Distinct days with data INSIDE the window — Copilot's local history is
    # sparse (single-day coverage was observed), so this is load-bearing.
    observed_range_covers_window = bool(
        cov_from
        and cov_to
        and cov_from <= window.start
        and cov_to >= window.end
    )
    metrics: dict[str, Any] = {
        "raw_traffic_tokens": inp + cr + cw + out,
        "estimated_cost_usd": None,
        "non_cached_input_tokens": inp,
        "cache_read_tokens": cr,
        "cache_write_tokens": cw,
        "output_tokens": out,
        "reasoning_tokens": reasoning,
        "calls": count,
        "coverage_days": coverage_days,
        "observed_range_covers_window": observed_range_covers_window,
    }
    gaps = []
    if not observed_range_covers_window:
        gaps.append("copilot observed coverage does not span the requested window")
    return SourceResult(metrics, provenance, cov_from, cov_to, gaps)


# ---------------------------------------------------------------------------
# Kronn collector (telemetry honesty: how much external spend Kronn sees)
# ---------------------------------------------------------------------------

_KRONN_EMPTY: dict[str, Any] = {
    "external_agent_replies": None,
    "external_replies_with_tokens_pct": None,
    "traced_tokens_used": None,
    "untraced_replies_by_agent": None,
}


def collect_kronn(db_path: Path, window: Window) -> SourceResult:
    provenance = "kronn.db messages.tokens_used (read-only)"
    if not db_path.is_file():
        return _unavailable(provenance, "kronn db not found", dict(_KRONN_EMPTY))
    try:
        conn = _open_ro(db_path)
        try:
            rows = conn.execute(
                """
                SELECT agent_type,
                       COUNT(*) AS replies,
                       SUM(CASE WHEN tokens_used > 0 THEN 1 ELSE 0 END) AS traced,
                       SUM(tokens_used) AS tokens
                  FROM messages
                 WHERE role = 'Agent' AND timestamp >= ? AND timestamp <= ?
                 GROUP BY agent_type
                """,
                (window.start.isoformat(), window.end.isoformat()),
            ).fetchall()
        finally:
            conn.close()
    except sqlite3.Error as exc:
        return _unavailable(provenance, f"kronn db unreadable: {exc}", dict(_KRONN_EMPTY))

    # A readable database with no agent replies in the window is a real
    # measurement: zeros, not nulls (the traced share stays null — 0/0).
    normalized_rows: list[tuple[Any, int, int, int]] = []
    for row in rows:
        values = (_counter(row[1]), _counter(row[2] or 0), _counter(row[3] or 0))
        if any(value is None for value in values) or values[1] > values[0]:
            return _unavailable(
                provenance, "kronn db contains malformed token counters", dict(_KRONN_EMPTY)
            )
        normalized_rows.append((row[0], values[0], values[1], values[2]))
    replies = sum(row[1] for row in normalized_rows)
    traced = sum(row[2] for row in normalized_rows)
    metrics: dict[str, Any] = {
        "external_agent_replies": replies,
        "external_replies_with_tokens_pct": _pct(traced, replies),
        "traced_tokens_used": sum(row[3] for row in normalized_rows),
        "untraced_replies_by_agent": {
            str(row[0] or "unknown"): (row[1] - row[2]) for row in normalized_rows
        },
    }
    return SourceResult(metrics, provenance)


# ---------------------------------------------------------------------------
# RTK collector (best effort — the CLI is not a stable API)
# ---------------------------------------------------------------------------

_RTK_EMPTY: dict[str, Any] = {
    "installed": None,
    "version": None,
    "commands": None,
    "raw_output_tokens": None,
    "compacted_output_tokens": None,
    "saved_tokens": None,
    "saved_pct": None,
    "granularity": "daily",
    "included_full_days": None,
    "window_coverage": None,
}


def _full_utc_days(window: Window) -> list[datetime]:
    day = datetime(window.start.year, window.start.month, window.start.day, tzinfo=timezone.utc)
    if day < window.start:
        day += timedelta(days=1)
    days: list[datetime] = []
    while day + timedelta(days=1) <= window.end:
        days.append(day)
        day += timedelta(days=1)
    return days


def collect_rtk(window: Window, runner=subprocess.run) -> SourceResult:
    """Window-scoped aggregation of `rtk gain --daily --format json`.

    RTK exposes whole UTC days, not event timestamps. Only days fully
    contained in the requested window are summed. Partial boundary days are
    omitted and disclosed instead of being misattributed to a short scenario.
    """
    provenance = "rtk gain --daily --format json (complete UTC days only)"
    try:
        proc = runner(
            ["rtk", "gain", "--daily", "--format", "json"],
            capture_output=True, text=True, timeout=30, check=False,
        )
    except FileNotFoundError as exc:
        metrics = dict(_RTK_EMPTY)
        metrics["installed"] = False
        return _unavailable(provenance, f"rtk unavailable: {exc}", metrics)
    except subprocess.TimeoutExpired as exc:
        metrics = dict(_RTK_EMPTY)
        metrics["installed"] = True
        return _unavailable(provenance, f"rtk gain timed out: {exc}", metrics)
    except OSError as exc:
        return _unavailable(provenance, f"rtk availability unknown: {exc}", dict(_RTK_EMPTY))
    unavailable_metrics = dict(_RTK_EMPTY)
    unavailable_metrics["installed"] = True
    if proc.returncode != 0:
        return _unavailable(provenance, "rtk gain failed", unavailable_metrics)
    try:
        payload = json.loads(proc.stdout)
    except json.JSONDecodeError:
        return _unavailable(provenance, "rtk gain JSON unparseable", unavailable_metrics)
    if not isinstance(payload, dict) or not isinstance(payload.get("daily"), list):
        return _unavailable(provenance, "rtk gain JSON schema is malformed", unavailable_metrics)

    full_days = _full_utc_days(window)
    included_dates = {day.date().isoformat() for day in full_days}
    rows: dict[str, dict[str, int]] = {}
    cov_from: datetime | None = None
    cov_to: datetime | None = None
    for row in payload["daily"]:
        if not isinstance(row, dict) or not isinstance(row.get("date"), str):
            return _unavailable(provenance, "rtk daily row schema is malformed", unavailable_metrics)
        try:
            day = datetime.strptime(row["date"], "%Y-%m-%d").replace(tzinfo=timezone.utc)
        except ValueError:
            return _unavailable(provenance, "rtk daily row date is malformed", unavailable_metrics)
        if row["date"] not in included_dates:
            continue
        values = {
            "commands": _counter(row.get("commands")),
            "raw_output_tokens": _counter(row.get("input_tokens")),
            "compacted_output_tokens": _counter(row.get("output_tokens")),
            "saved_tokens": _counter(row.get("saved_tokens")),
        }
        if any(value is None for value in values.values()):
            return _unavailable(provenance, "rtk daily row counters are malformed", unavailable_metrics)
        if (
            values["compacted_output_tokens"] > values["raw_output_tokens"]
            or values["saved_tokens"] > values["raw_output_tokens"]
        ):
            return _unavailable(
                provenance, "rtk daily row arithmetic is inconsistent", unavailable_metrics
            )
        if row["date"] in rows:
            return _unavailable(provenance, "rtk daily rows contain duplicate dates", unavailable_metrics)
        rows[row["date"]] = values

    totals = {"commands": 0, "raw_output_tokens": 0, "compacted_output_tokens": 0, "saved_tokens": 0}
    for day in full_days:
        row = rows.get(day.date().isoformat())
        if row:
            cov_from = day if cov_from is None or day < cov_from else cov_from
            cov_to = day if cov_to is None or day > cov_to else cov_to
            for key in totals:
                totals[key] += row[key]

    metrics: dict[str, Any] = dict(_RTK_EMPTY)
    metrics["installed"] = True
    metrics["included_full_days"] = len(full_days)
    is_complete = (
        window.start.time() == datetime.min.time()
        and window.end.time() == datetime.min.time()
    )
    metrics["window_coverage"] = "complete" if is_complete else ("partial" if full_days else "none")
    gaps: list[str] = []
    if not is_complete:
        gaps.append("rtk daily granularity excludes partial UTC boundary days")
    if full_days:
        metrics.update(totals)
        metrics["saved_pct"] = _pct(totals["saved_tokens"], totals["raw_output_tokens"])
    else:
        gaps.append("rtk has no complete UTC day inside the requested window")
    try:
        version = runner(
            ["rtk", "--version"], capture_output=True, text=True, timeout=10, check=False
        )
        if version.returncode == 0 and version.stdout.strip():
            metrics["version"] = version.stdout.strip().splitlines()[0]
    except (OSError, subprocess.TimeoutExpired):
        pass  # version stays null; the aggregates above remain valid
    return SourceResult(metrics, provenance, cov_from, cov_to, gaps)


# ---------------------------------------------------------------------------
# Report assembly
# ---------------------------------------------------------------------------


def _walk_null_paths(value: Any, prefix: str = "") -> Iterator[str]:
    if value is None:
        yield prefix
    elif isinstance(value, dict):
        for key, child in value.items():
            child_prefix = f"{prefix}.{key}" if prefix else str(key)
            yield from _walk_null_paths(child, child_prefix)
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from _walk_null_paths(child, f"{prefix}[{index}]")


def _null_reason(path: str, report: dict[str, Any], scenario_was_omitted: bool) -> str:
    if path in {"scenario", "scenario_is_canonical"}:
        return "not_requested" if scenario_was_omitted else "non_canonical_value_omitted"
    if path == "scope.repo_pseudonym":
        return "not_requested"
    if path == "scope.rtk_installed":
        return "probe_unavailable"
    if path == "kpi.completed_tasks":
        return "missing_denominator"
    if path.startswith("kpi.raw_traffic_tokens_per_completed_task_by_agent."):
        return (
            "missing_denominator"
            if not report["kpi"]["completed_tasks"]
            else "source_unavailable"
        )
    if path.endswith(".estimated_cost_usd"):
        return "unconfigured_cost"
    if path == "agents.codex.cache_write_tokens":
        return "unsupported_metric"
    if path.startswith("agents."):
        parts = path.split(".")
        agent = parts[1]
        if report["agents"][agent]["raw_traffic_tokens"] is None:
            return "source_unavailable"
        if (
            parts[-1] == "repo_sessions_share_pct"
            and report["agents"][agent]["raw_traffic_tokens"] > 0
        ):
            return "not_requested"
        if parts[-1] in {
            "top_1_share_pct", "top_4_share_pct", "repo_sessions_share_pct",
        }:
            return "undefined_ratio"
        if ".coverage." in path:
            return "no_observed_events"
        return "source_unavailable"
    if path.startswith("kronn."):
        if path == "kronn.external_replies_with_tokens_pct" and report["kronn"]["external_agent_replies"] == 0:
            return "undefined_ratio"
        return "source_unavailable"
    if path.startswith("rtk."):
        if path == "rtk.version" and report["rtk"]["installed"] is True:
            return "probe_unavailable"
        if path == "rtk.saved_pct" and report["rtk"]["raw_output_tokens"] == 0:
            return "undefined_ratio"
        if report["rtk"]["installed"] is True and report["rtk"]["window_coverage"] == "none":
            return "insufficient_granularity"
        return "source_unavailable"
    raise ValueError(f"no null reason contract for {path}")


def _build_null_reasons(
    report: dict[str, Any], *, scenario_was_omitted: bool,
    overrides: dict[str, str] | None = None,
) -> dict[str, str]:
    reasons = {
        path: _null_reason(path, report, scenario_was_omitted)
        for path in _walk_null_paths(report)
    }
    for path, reason in (overrides or {}).items():
        if path in reasons:
            reasons[path] = reason
    return reasons


def _source_core_is_unavailable(name: str, source: SourceResult) -> bool:
    key = {
        "claude": "raw_traffic_tokens",
        "codex": "raw_traffic_tokens",
        "copilot": "raw_traffic_tokens",
        "kronn": "external_agent_replies",
        "rtk": "saved_tokens",
    }[name]
    return source.metrics.get(key) is None


def _gap_code(name: str, source: SourceResult, message: str) -> str:
    lowered = message.lower()
    if "granularity" in lowered or "complete utc day" in lowered or "partial utc boundary" in lowered:
        return "insufficient_granularity"
    if _source_core_is_unavailable(name, source):
        return "source_unavailable"
    if "coverage" in lowered:
        return "coverage_incomplete"
    return "source_quality"


def _gap_source_for_null_path(path: str) -> str:
    if path.startswith("agents."):
        return path.split(".")[1]
    if path.startswith("kpi.raw_traffic_tokens_per_completed_task_by_agent."):
        return path.rsplit(".", 1)[1]
    if path.startswith("kronn."):
        return "kronn"
    if path.startswith("rtk.") or path == "scope.rtk_installed":
        return "rtk"
    raise ValueError(f"no data-gap source contract for {path}")


def _build_structured_gaps(
    sources: dict[str, SourceResult],
    *,
    scenario_was_noncanonical: bool,
    denominator_missing: bool,
) -> list[dict[str, str]]:
    details: list[dict[str, str]] = []
    for name, source in sources.items():
        messages = list(source.gaps)
        if _source_core_is_unavailable(name, source) and not messages:
            messages.append(f"{name} metrics unavailable")
        for message in messages:
            details.append(
                {
                    "id": f"gap-{len(details) + 1}",
                    "source": name,
                    "code": _gap_code(name, source, message),
                    "message": message,
                }
            )
    if scenario_was_noncanonical:
        details.append(
            {
                "id": f"gap-{len(details) + 1}",
                "source": "report",
                "code": "non_canonical_value_omitted",
                "message": "non-canonical scenario label omitted from export",
            }
        )
    if denominator_missing:
        details.append(
            {
                "id": f"gap-{len(details) + 1}",
                "source": "kpi",
                "code": "missing_denominator",
                "message": "completed task denominator was not provided or is zero",
            }
        )
    return details


def _build_null_gap_links(
    null_reasons: dict[str, str], data_gap_details: list[dict[str, str]]
) -> dict[str, str]:
    links: dict[str, str] = {}
    for path, reason in null_reasons.items():
        if reason not in {"source_unavailable", "insufficient_granularity"}:
            continue
        source = _gap_source_for_null_path(path)
        match = next(
            (
                gap
                for gap in data_gap_details
                if gap["source"] == source and gap["code"] == reason
            ),
            None,
        )
        if match is None:
            raise ValueError(f"no structured data gap for {path} ({reason})")
        links[path] = match["id"]
    return links


def build_report(
    *,
    window: Window,
    scenario: str | None,
    repo_alias: str | None,
    claude: SourceResult,
    codex: SourceResult,
    copilot: SourceResult,
    kronn: SourceResult,
    rtk: SourceResult,
    completed_tasks: int | None = None,
    generated_at: datetime | None = None,
) -> dict[str, Any]:
    """The canonical pseudonymized team JSON (docs/design/token-economics-baseline.md)."""
    if completed_tasks is not None and (
        isinstance(completed_tasks, bool) or not isinstance(completed_tasks, int) or completed_tasks < 0
    ):
        raise ValueError("completed_tasks must be a non-negative integer")
    generated_at = _to_utc(generated_at) if generated_at else window.end
    sources = {"claude": claude, "codex": codex, "copilot": copilot, "kronn": kronn, "rtk": rtk}
    canonical_scenario = scenario if scenario in CANONICAL_SCENARIOS else None
    data_gap_details = _build_structured_gaps(
        sources,
        scenario_was_noncanonical=bool(scenario and canonical_scenario is None),
        denominator_missing=not completed_tasks,
    )
    data_gaps = [gap["message"] for gap in data_gap_details]

    per_task: dict[str, float | None] = {}
    for name, source in (("claude", claude), ("codex", codex), ("copilot", copilot)):
        raw = source.metrics.get("raw_traffic_tokens")
        per_task[name] = round(raw / completed_tasks, 2) if raw is not None and completed_tasks else None
    report: dict[str, Any] = {
        "schema_version": SCHEMA_VERSION,
        "generated_at": generated_at.isoformat(),
        "window": window.as_dict(),
        "scenario": canonical_scenario,
        "scenario_is_canonical": True if canonical_scenario else None,
        "scope": {
            "repo_pseudonym": _pseudonym(repo_alias) if repo_alias else None,
            "kronn_installed": kronn.metrics.get("external_agent_replies") is not None,
            "rtk_installed": rtk.metrics.get("installed"),
        },
        "kpi": {
            "completed_tasks": completed_tasks,
            "raw_traffic_tokens_per_completed_task_by_agent": per_task,
        },
        "agents": {
            "claude": {**claude.metrics, "coverage": claude.coverage_dict(), "provenance": claude.provenance},
            "codex": {**codex.metrics, "coverage": codex.coverage_dict(), "provenance": codex.provenance},
            "copilot": {**copilot.metrics, "coverage": copilot.coverage_dict(), "provenance": copilot.provenance},
        },
        "kronn": {**kronn.metrics, "provenance": kronn.provenance},
        "rtk": {**rtk.metrics, "provenance": rtk.provenance},
        "data_gaps": data_gaps,
        "data_gap_details": data_gap_details,
        "notes": [
            "raw traffic includes cache reads and must never be read as billing",
            "every null has a typed null_reasons entry; zero is measured",
            "estimated_cost_usd stays null until a per-model tariff table is configured",
            "per-task KPI stays provider-specific; no incomplete cross-provider total is inferred",
        ],
    }
    null_reason_overrides = {
        f"agents.claude.{path}": reason
        for path, reason in claude.null_reason_overrides.items()
    }
    report["null_reasons"] = _build_null_reasons(
        report,
        scenario_was_omitted=scenario is None,
        overrides=null_reason_overrides,
    )
    report["null_gap_links"] = _build_null_gap_links(
        report["null_reasons"], data_gap_details
    )
    validate_report(report)
    return report


def validate_report(report: dict[str, Any]) -> None:
    """Reject anything outside the canonical, privacy-bounded report shape."""
    errors: list[str] = []

    def exact(value: Any, keys: set[str], path: str) -> bool:
        if not isinstance(value, dict):
            errors.append(path)
            return False
        if set(value) != keys:
            errors.append(f"{path}.keys")
            return False
        return True

    def counter(value: Any, path: str, nullable: bool = True) -> bool:
        if value is None and nullable:
            return False
        if _counter(value) is None:
            errors.append(path)
            return False
        return True

    def number(value: Any, path: str, *, percentage: bool = False) -> bool:
        if value is None:
            return False
        if (
            isinstance(value, bool)
            or not isinstance(value, (int, float))
            or not math.isfinite(value)
            or value < 0
        ):
            errors.append(path)
            return False
        if percentage and value > 100:
            errors.append(path)
            return False
        return True

    top_keys = {
        "schema_version", "generated_at", "window", "scenario",
        "scenario_is_canonical", "scope", "kpi", "agents", "kronn", "rtk",
        "data_gaps", "data_gap_details", "notes", "null_reasons", "null_gap_links",
    }
    if not exact(report, top_keys, "report"):
        raise ValueError("invalid token economics report: " + ", ".join(errors))
    if report["schema_version"] != SCHEMA_VERSION:
        errors.append("schema_version")
    generated_at = _parse_ts(report["generated_at"])
    if generated_at is None:
        errors.append("generated_at")

    requested_window: Window | None = None
    window = report["window"]
    if exact(window, {"from", "to"}, "window"):
        try:
            requested_window = Window(_parse_ts(window["from"]), _parse_ts(window["to"]))
        except (TypeError, ValueError):
            errors.append("window")
    if report["scenario"] not in (*CANONICAL_SCENARIOS, None):
        errors.append("scenario")
    scenario_flag = report["scenario_is_canonical"]
    if report["scenario"] is not None:
        if type(scenario_flag) is not bool or scenario_flag is not True:
            errors.append("scenario_is_canonical")
    elif scenario_flag is not None:
        errors.append("scenario_is_canonical")

    scope = report["scope"]
    if exact(scope, {"repo_pseudonym", "kronn_installed", "rtk_installed"}, "scope"):
        pseudonym = scope["repo_pseudonym"]
        if pseudonym is not None and not (
            isinstance(pseudonym, str) and re.fullmatch(r"[0-9a-f]{8}", pseudonym)
        ):
            errors.append("scope.repo_pseudonym")
        if type(scope["kronn_installed"]) is not bool:
            errors.append("scope.kronn_installed")
        if scope["rtk_installed"] is not None and type(scope["rtk_installed"]) is not bool:
            errors.append("scope.rtk_installed")

    kpi = report["kpi"]
    normalized: dict[str, Any] | None = None
    completed: int | None = None
    if exact(kpi, {"completed_tasks", "raw_traffic_tokens_per_completed_task_by_agent"}, "kpi"):
        completed = kpi["completed_tasks"]
        if completed is not None:
            counter(completed, "kpi.completed_tasks", nullable=False)
        normalized_value = kpi["raw_traffic_tokens_per_completed_task_by_agent"]
        if exact(normalized_value, {"claude", "codex", "copilot"}, "kpi.raw_traffic_tokens_per_completed_task_by_agent"):
            normalized = normalized_value
            for name, value in normalized.items():
                number(value, f"kpi.raw_traffic_tokens_per_completed_task_by_agent.{name}")

    coverage_keys = {"observed_from", "observed_to"}
    agent_keys = {
        "claude": {
            "raw_traffic_tokens", "estimated_cost_usd", "non_cached_input_tokens",
            "cache_write_tokens", "cache_read_tokens", "output_tokens", "assistant_calls",
            "sessions", "top_1_share_pct", "top_4_share_pct", "repo_sessions_share_pct",
            "disc_wait_calls", "disc_wait_associated_tokens", "coverage", "provenance",
        },
        "codex": {
            "raw_traffic_tokens", "estimated_cost_usd", "non_cached_input_tokens",
            "cache_write_tokens", "cache_read_tokens", "output_tokens", "reasoning_tokens",
            "sessions", "rollouts", "top_1_share_pct", "coverage", "provenance",
        },
        "copilot": {
            "raw_traffic_tokens", "estimated_cost_usd", "non_cached_input_tokens",
            "cache_write_tokens", "cache_read_tokens", "output_tokens", "reasoning_tokens",
            "calls", "coverage_days", "observed_range_covers_window", "coverage", "provenance",
        },
    }
    component_keys = {
        "claude": ("non_cached_input_tokens", "cache_write_tokens", "cache_read_tokens", "output_tokens"),
        "codex": ("non_cached_input_tokens", "cache_read_tokens", "output_tokens"),
        "copilot": ("non_cached_input_tokens", "cache_write_tokens", "cache_read_tokens", "output_tokens"),
    }
    agents = report["agents"]
    if exact(agents, set(agent_keys), "agents"):
        for name, expected_keys in agent_keys.items():
            agent = agents[name]
            if not exact(agent, expected_keys, f"agents.{name}"):
                continue
            raw = agent["raw_traffic_tokens"]
            counters = [agent[key] for key in component_keys[name]]
            values = [raw, *counters]
            if any(value is None for value in values) and not all(value is None for value in values):
                errors.append(f"agents.{name}.incomplete counters")
            for key in ("raw_traffic_tokens", *component_keys[name]):
                counter(agent[key], f"agents.{name}.{key}")
            if all(_counter(value) is not None for value in values) and raw != sum(counters):
                errors.append(f"agents.{name}.raw_traffic_tokens arithmetic")
            number(agent["estimated_cost_usd"], f"agents.{name}.estimated_cost_usd")

            count_keys = {
                "claude": ("assistant_calls", "sessions", "disc_wait_calls", "disc_wait_associated_tokens"),
                "codex": ("reasoning_tokens", "sessions", "rollouts"),
                "copilot": ("reasoning_tokens", "calls", "coverage_days"),
            }[name]
            for key in count_keys:
                counter(agent[key], f"agents.{name}.{key}")
                if raw is not None and agent[key] is None:
                    errors.append(f"agents.{name}.{key}.missing measured count")
            pct_keys = {
                "claude": ("top_1_share_pct", "top_4_share_pct", "repo_sessions_share_pct"),
                "codex": ("top_1_share_pct",),
                "copilot": (),
            }[name]
            for key in pct_keys:
                number(agent[key], f"agents.{name}.{key}", percentage=True)

            raw_is_positive = _counter(raw) is not None and raw > 0
            if name == "claude":
                calls = agent["assistant_calls"]
                sessions = agent["sessions"]
                top_1 = agent["top_1_share_pct"]
                top_4 = agent["top_4_share_pct"]
                if raw_is_positive and (
                    _counter(calls) is None or calls == 0
                    or _counter(sessions) is None or sessions == 0
                ):
                    errors.append("agents.claude.positive traffic counts")
                if (
                    _counter(calls) is not None
                    and _counter(sessions) is not None
                    and sessions > calls
                ):
                    errors.append("agents.claude.sessions arithmetic")
                if (
                    _counter(agent["disc_wait_calls"]) is not None
                    and _counter(calls) is not None
                    and agent["disc_wait_calls"] > calls
                ):
                    errors.append("agents.claude.disc_wait_calls arithmetic")
                if (
                    _counter(agent["disc_wait_associated_tokens"]) is not None
                    and _counter(raw) is not None
                    and agent["disc_wait_associated_tokens"] > raw
                ):
                    errors.append("agents.claude.disc_wait_associated_tokens arithmetic")
                if raw_is_positive and (
                    not number(top_1, "agents.claude.top_1_share_pct.required", percentage=True)
                    or not number(top_4, "agents.claude.top_4_share_pct.required", percentage=True)
                    or top_1 == 0 or top_4 == 0
                ):
                    errors.append("agents.claude.top shares required")
                top_shares_are_numeric = number(
                    top_1, "agents.claude.top_1_share_pct", percentage=True
                ) and number(
                    top_4, "agents.claude.top_4_share_pct", percentage=True
                )
                if top_shares_are_numeric:
                    if top_4 < top_1:
                        errors.append("agents.claude.top shares ordering")
                    if sessions == 1 and top_1 != 100:
                        errors.append("agents.claude.top_1_share_pct arithmetic")
                    if _counter(sessions) is not None and 0 < sessions <= 4 and top_4 != 100:
                        errors.append("agents.claude.top_4_share_pct arithmetic")
            elif name == "codex":
                sessions = agent["sessions"]
                rollouts = agent["rollouts"]
                top_1 = agent["top_1_share_pct"]
                if raw_is_positive and (
                    _counter(sessions) is None or sessions == 0
                    or _counter(rollouts) is None or rollouts == 0
                ):
                    errors.append("agents.codex.positive traffic counts")
                if (
                    _counter(sessions) is not None
                    and _counter(rollouts) is not None
                    and rollouts < sessions
                ):
                    errors.append("agents.codex.rollouts arithmetic")
                if raw_is_positive and (
                    not number(top_1, "agents.codex.top_1_share_pct.required", percentage=True)
                    or top_1 == 0
                ):
                    errors.append("agents.codex.top_1_share_pct required")
                if _counter(sessions) is not None and sessions == 1 and top_1 != 100:
                    errors.append("agents.codex.top_1_share_pct arithmetic")
            else:
                calls = agent["calls"]
                if raw_is_positive and (_counter(calls) is None or calls == 0):
                    errors.append("agents.copilot.positive traffic counts")
                if (
                    _counter(agent["coverage_days"]) is not None
                    and _counter(calls) is not None
                    and agent["coverage_days"] > calls
                ):
                    errors.append("agents.copilot.coverage_days arithmetic")
            if name == "codex" and agent["cache_write_tokens"] is not None:
                errors.append("agents.codex.cache_write_tokens")
            if name in {"codex", "copilot"}:
                reasoning = agent["reasoning_tokens"]
                output = agent["output_tokens"]
                if _counter(reasoning) is not None and _counter(output) is not None and reasoning > output:
                    errors.append(f"agents.{name}.reasoning_tokens arithmetic")
            if name == "copilot" and type(agent["observed_range_covers_window"]) is not bool and agent["observed_range_covers_window"] is not None:
                errors.append("agents.copilot.observed_range_covers_window")
            if name == "copilot" and raw is not None and agent["observed_range_covers_window"] is None:
                errors.append("agents.copilot.observed_range_covers_window.missing")

            coverage = agent["coverage"]
            if exact(coverage, coverage_keys, f"agents.{name}.coverage"):
                observed = [_parse_ts(coverage[key]) for key in ("observed_from", "observed_to")]
                if (coverage["observed_from"] is None) != (coverage["observed_to"] is None):
                    errors.append(f"agents.{name}.coverage.partial")
                elif coverage["observed_from"] is not None:
                    if any(value is None for value in observed):
                        errors.append(f"agents.{name}.coverage")
                    elif requested_window and not (
                        requested_window.start <= observed[0] <= observed[1] <= requested_window.end
                    ):
                        errors.append(f"agents.{name}.coverage.window")
            if not isinstance(agent["provenance"], str) or not agent["provenance"].strip():
                errors.append(f"agents.{name}.provenance")

            if normalized is not None:
                expected = round(raw / completed, 2) if _counter(raw) is not None and completed else None
                if normalized[name] != expected:
                    errors.append(f"kpi.{name} arithmetic")

    kronn = report["kronn"]
    kronn_keys = {
        "external_agent_replies", "external_replies_with_tokens_pct", "traced_tokens_used",
        "untraced_replies_by_agent", "provenance",
    }
    if exact(kronn, kronn_keys, "kronn"):
        counter(kronn["external_agent_replies"], "kronn.external_agent_replies")
        counter(kronn["traced_tokens_used"], "kronn.traced_tokens_used")
        number(kronn["external_replies_with_tokens_pct"], "kronn.external_replies_with_tokens_pct", percentage=True)
        untraced = kronn["untraced_replies_by_agent"]
        if untraced is not None:
            if not isinstance(untraced, dict) or not all(
                isinstance(key, str) and key and _counter(value) is not None
                for key, value in untraced.items()
            ):
                errors.append("kronn.untraced_replies_by_agent")
        if not isinstance(kronn["provenance"], str) or not kronn["provenance"].strip():
            errors.append("kronn.provenance")
        if kronn["external_agent_replies"] is not None and (
            kronn["traced_tokens_used"] is None or kronn["untraced_replies_by_agent"] is None
        ):
            errors.append("kronn.incomplete counters")
        replies = kronn["external_agent_replies"]
        if _counter(replies) is not None and isinstance(untraced, dict) and all(
            _counter(value) is not None for value in untraced.values()
        ):
            untraced_replies = sum(untraced.values())
            if untraced_replies > replies:
                errors.append("kronn.untraced_replies_by_agent arithmetic")
            else:
                expected_pct = _pct(replies - untraced_replies, replies)
                if kronn["external_replies_with_tokens_pct"] != expected_pct:
                    errors.append("kronn.external_replies_with_tokens_pct arithmetic")
        if isinstance(scope, dict) and scope.get("kronn_installed") != (
            kronn["external_agent_replies"] is not None
        ):
            errors.append("scope.kronn_installed consistency")

    rtk = report["rtk"]
    rtk_keys = {
        "installed", "version", "commands", "raw_output_tokens", "compacted_output_tokens",
        "saved_tokens", "saved_pct", "granularity", "included_full_days", "window_coverage",
        "provenance",
    }
    if exact(rtk, rtk_keys, "rtk"):
        if rtk["installed"] is not None and type(rtk["installed"]) is not bool:
            errors.append("rtk.installed")
        if rtk["version"] is not None and (not isinstance(rtk["version"], str) or not rtk["version"].strip()):
            errors.append("rtk.version")
        rtk_counter_keys = ("commands", "raw_output_tokens", "compacted_output_tokens", "saved_tokens")
        rtk_counters = [rtk[key] for key in rtk_counter_keys]
        if any(value is None for value in rtk_counters) and not all(value is None for value in rtk_counters):
            errors.append("rtk.incomplete counters")
        for key in rtk_counter_keys:
            counter(rtk[key], f"rtk.{key}")
        number(rtk["saved_pct"], "rtk.saved_pct", percentage=True)
        if _counter(rtk["raw_output_tokens"]) is not None and _counter(rtk["saved_tokens"]) is not None:
            if rtk["saved_pct"] != _pct(rtk["saved_tokens"], rtk["raw_output_tokens"]):
                errors.append("rtk.saved_pct arithmetic")
            if rtk["compacted_output_tokens"] > rtk["raw_output_tokens"] or rtk["saved_tokens"] > rtk["raw_output_tokens"]:
                errors.append("rtk.counter arithmetic")
        if any(value is not None for value in rtk_counters) and rtk["installed"] is not True:
            errors.append("rtk.installed")
        if rtk["version"] is not None and rtk["installed"] is not True:
            errors.append("rtk.version installation")
        if rtk["granularity"] != "daily":
            errors.append("rtk.granularity")
        if rtk["window_coverage"] not in ("complete", "partial", "none", None):
            errors.append("rtk.window_coverage")
        counter(rtk["included_full_days"], "rtk.included_full_days")
        if not isinstance(rtk["provenance"], str) or not rtk["provenance"].strip():
            errors.append("rtk.provenance")
        if isinstance(scope, dict) and scope.get("rtk_installed") != rtk["installed"]:
            errors.append("scope.rtk_installed consistency")

    data_gaps = report["data_gaps"]
    if not isinstance(data_gaps, list) or not all(
        isinstance(gap, str) and gap.strip() for gap in data_gaps
    ) or len(data_gaps) != len(set(data_gaps)):
        errors.append("data_gaps")
    data_gap_details = report["data_gap_details"]
    gap_by_id: dict[str, dict[str, str]] = {}
    if not isinstance(data_gap_details, list):
        errors.append("data_gap_details")
    else:
        for index, gap in enumerate(data_gap_details):
            if not isinstance(gap, dict) or set(gap) != {"id", "source", "code", "message"}:
                errors.append(f"data_gap_details[{index}]")
                continue
            if not all(isinstance(gap[key], str) and gap[key].strip() for key in gap):
                errors.append(f"data_gap_details[{index}].types")
                continue
            if gap["code"] not in DATA_GAP_CODES:
                errors.append(f"data_gap_details[{index}].code")
            if gap["source"] not in DATA_GAP_SOURCES:
                errors.append(f"data_gap_details[{index}].source")
            if gap["id"] in gap_by_id:
                errors.append("data_gap_details.duplicate id")
            gap_by_id[gap["id"]] = gap
        if isinstance(data_gaps, list) and [gap.get("message") for gap in data_gap_details if isinstance(gap, dict)] != data_gaps:
            errors.append("data_gap_details.messages")
    notes = report["notes"]
    if not isinstance(notes, list) or not notes or not all(
        isinstance(note, str) and note.strip() for note in notes
    ):
        errors.append("notes")

    null_reasons = report["null_reasons"]
    expected_null_paths = set(_walk_null_paths({
        key: value for key, value in report.items() if key != "null_reasons"
    }))
    if not isinstance(null_reasons, dict) or set(null_reasons) != expected_null_paths:
        errors.append("null_reasons.keys")
    elif not all(
        isinstance(path, str) and reason in NULL_REASON_CODES
        for path, reason in null_reasons.items()
    ):
        errors.append("null_reasons.values")
    else:
        scenario_was_omitted = not any(
            "non-canonical scenario label" in gap for gap in data_gaps
        ) if isinstance(data_gaps, list) else True
        try:
            expected_reasons = _build_null_reasons(
                {key: value for key, value in report.items() if key != "null_reasons"},
                scenario_was_omitted=scenario_was_omitted,
            )
            for path, actual_reason in null_reasons.items():
                expected_reason = expected_reasons.get(path)
                zero_traffic_repo_scope = (
                    path == "agents.claude.repo_sessions_share_pct"
                    and report["agents"]["claude"]["raw_traffic_tokens"] == 0
                    and actual_reason in {"not_requested", "undefined_ratio"}
                    and expected_reason == "undefined_ratio"
                )
                if actual_reason != expected_reason and not zero_traffic_repo_scope:
                    errors.append("null_reasons.semantics")
                    break
        except (KeyError, TypeError, ValueError):
            errors.append("null_reasons.semantics")

    null_gap_links = report["null_gap_links"]
    if isinstance(null_reasons, dict):
        required_link_paths = {
            path
            for path, reason in null_reasons.items()
            if reason in {"source_unavailable", "insufficient_granularity"}
        }
    else:
        required_link_paths = set()
    if not isinstance(null_gap_links, dict) or set(null_gap_links) != required_link_paths:
        errors.append("null_gap_links.keys")
    else:
        for path, gap_id in null_gap_links.items():
            gap = gap_by_id.get(gap_id) if isinstance(gap_id, str) else None
            if gap is None:
                errors.append(f"null_gap_links.{path}")
                continue
            try:
                expected_source = _gap_source_for_null_path(path)
            except ValueError:
                errors.append(f"null_gap_links.{path}.source")
                continue
            if gap["source"] != expected_source or gap["code"] != null_reasons[path]:
                errors.append(f"null_gap_links.{path}.mismatch")

    if errors:
        raise ValueError("invalid token economics report: " + ", ".join(errors))


def _fmt(value: Any) -> str:
    if value is None:
        return "n/a"
    if isinstance(value, int):
        return f"{value:,}".replace(",", " ")
    return str(value)


def render_text(report: dict[str, Any]) -> str:
    lines = [
        f"Token Economics report (schema {report['schema_version']})",
        f"window: {report['window']['from']} → {report['window']['to']}"
        + (f"  scenario: {report['scenario']}" if report["scenario"] else ""),
        "",
    ]
    for name in ("claude", "codex", "copilot"):
        agent = report["agents"][name]
        lines.append(
            f"{name:>8}: raw={_fmt(agent.get('raw_traffic_tokens'))}"
            f"  uncached-in={_fmt(agent.get('non_cached_input_tokens'))}"
            f"  cache-read={_fmt(agent.get('cache_read_tokens'))}"
            f"  out={_fmt(agent.get('output_tokens'))}"
            f"  sessions={_fmt(agent.get('sessions', agent.get('calls')))}"
        )
    claude = report["agents"]["claude"]
    if claude.get("disc_wait_calls") is not None:
        lines.append(
            f"          disc_wait calls={_fmt(claude['disc_wait_calls'])}"
            f"  associated tokens={_fmt(claude['disc_wait_associated_tokens'])}"
        )
    kronn = report["kronn"]
    lines.append(
        f"   kronn: external replies={_fmt(kronn.get('external_agent_replies'))}"
        f"  traced={_fmt(kronn.get('external_replies_with_tokens_pct'))}%"
    )
    rtk = report["rtk"]
    lines.append(
        f"     rtk: saved={_fmt(rtk.get('saved_tokens'))}"
        f" ({_fmt(rtk.get('saved_pct'))}%) over {_fmt(rtk.get('commands'))} commands"
    )
    if report["data_gaps"]:
        lines.append("")
        lines.append("data gaps:")
        lines.extend(f"  - {gap}" for gap in report["data_gaps"])
    lines.append("")
    lines.extend(f"note: {note}" for note in report["notes"])
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def _default_kronn_db() -> Path:
    if os.environ.get("KRONN_DATA_DIR"):
        return Path(os.environ["KRONN_DATA_DIR"]) / "kronn.db"
    if sys.platform == "darwin":
        return Path.home() / "Library/Application Support/com.kronn.kronn/kronn.db"
    return Path.home() / ".config/kronn/kronn.db"


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    report = sub.add_parser("report", help="collect and print the baseline report")
    report.add_argument("--days", type=int, default=30, help="window size ending now")
    report.add_argument("--from", dest="from_ts", help="explicit window start (ISO-8601)")
    report.add_argument("--to", dest="to_ts", help="explicit window end (ISO-8601)")
    report.add_argument(
        "--scenario", choices=CANONICAL_SCENARIOS,
        help="canonical scenario for this measurement window",
    )
    report.add_argument(
        "--completed-tasks", type=int,
        help="completed tasks in the window (required for the normalized KPI)",
    )
    report.add_argument("--repo-alias", help="repo name — stored hashed, never raw")
    report.add_argument("--repo-filter", help="cwd substring for repo session share")
    report.add_argument("--claude-dir", type=Path, default=Path.home() / ".claude/projects")
    report.add_argument("--codex-dir", type=Path, default=Path.home() / ".codex/sessions")
    report.add_argument("--copilot-db", type=Path, default=Path.home() / ".copilot/session-store.db")
    report.add_argument("--kronn-db", type=Path, default=_default_kronn_db())
    report.add_argument("--no-rtk", action="store_true", help="skip the rtk CLI probe")
    report.add_argument("--json", dest="json_out", type=Path, help="write team JSON here")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    now = datetime.now(timezone.utc)
    start = _parse_ts(args.from_ts) if args.from_ts else now - timedelta(days=args.days)
    end = _parse_ts(args.to_ts) if args.to_ts else now
    if start is None or end is None:
        print("error: --from/--to must be ISO-8601 timestamps", file=sys.stderr)
        return 2
    if args.days <= 0 or (args.completed_tasks is not None and args.completed_tasks < 0):
        print("error: --days must be positive and --completed-tasks non-negative", file=sys.stderr)
        return 2
    try:
        window = Window(start, end)
    except (TypeError, ValueError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    rtk = (
        _unavailable("rtk gain CLI", "rtk probe skipped (--no-rtk)", dict(_RTK_EMPTY))
        if args.no_rtk
        else collect_rtk(window)
    )
    report = build_report(
        window=window,
        scenario=args.scenario,
        repo_alias=args.repo_alias,
        claude=collect_claude(args.claude_dir, window, args.repo_filter),
        codex=collect_codex(args.codex_dir, window),
        copilot=collect_copilot(args.copilot_db, window),
        kronn=collect_kronn(args.kronn_db, window),
        rtk=rtk,
        completed_tasks=args.completed_tasks,
    )
    print(render_text(report))
    if args.json_out:
        args.json_out.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
        print(f"\nteam JSON written to {args.json_out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
