#!/usr/bin/env python3
"""Native token counters for a JOINED CLI session — KT-190.

Kronn knows what the agents it SPAWNS cost: a workflow step reads stream-json,
or parses the counter the CLI prints on exit. A CLI that joined a room on its
own was never spawned, so every message it posts is recorded `tokens_used = 0`.
Measured on one real Claude Code session: 4 143 787 451 tokens of traffic,
recorded as zero.

Two vendors, and they are NOT alike — which is the whole reason this module
exists rather than one parser:

    claude-code   append-only JSONL transcript, one `usage` object per response.
                  Splits input / cache_creation / cache_read / output. Read
                  incrementally: one transcript reached 61 MB.

    vibe          a `meta.json` SNAPSHOT per session, holding session totals.
                  Gives prompt + completion + a vendor-computed cost, and NO
                  cache breakdown at all. Re-read whole; it is small.

That asymmetry is the trap this module is built around. Vibe's cache counters
are not zero, they are ABSENT. Storing them as 0 would let a dashboard state
that Vibe performs no cache reads — a fabricated claim, from a field nobody
ever measured. So a counter this vendor does not report is simply not in
`counters`, and `unmeasured` names it out loud.

Traffic is not cost. On that Claude Code session cache reads were 98.4% of the
traffic and are billed at roughly a tenth of input: traffic and billable differ
by a factor of ~62. Nothing here ever sums the counters for the caller.

Usage:
    python3 cli_token_collector.py claude-code <conversation_id> [--since-offset N]
    python3 cli_token_collector.py vibe <session_id>
    (add --json for machine output)
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

CLAUDE_CODE_ROOT = pathlib.Path.home() / ".claude" / "projects"
VIBE_SESSION_ROOT = pathlib.Path.home() / ".vibe" / "logs" / "session"

# The counter vocabulary Kronn stores. A vendor reports a subset; the rest are
# reported as unmeasured, never as zero.
ALL_COUNTERS = ("input", "cache_creation", "cache_read", "output")

_CLAUDE_USAGE_FIELDS = {
    "input_tokens": "input",
    "cache_creation_input_tokens": "cache_creation",
    "cache_read_input_tokens": "cache_read",
    "output_tokens": "output",
}


def _safe_id(value) -> bool:
    """Reject anything path-shaped: ids are opaque, and a traversal-shaped one
    would let a caller aim the collector at any file on disk."""
    return bool(value) and isinstance(value, str) and "/" not in value and "\\" not in value


def _not_measured(vendor: str, reason: str) -> dict:
    return {
        "status": "not_measured",
        "vendor": vendor,
        "provenance": None,
        "reason": reason,
        # No `counters` key at all: absence must be impossible to read as zero.
        "unmeasured": list(ALL_COUNTERS),
    }


# ── Claude Code ────────────────────────────────────────────────────


def find_claude_transcript(conversation_id: str, root: pathlib.Path | None = None):
    """Locate a transcript by conversation id.

    Searched, never derived from the working directory: the project directory
    name encodes the cwd at launch, so a session that moved into a git worktree
    lives under the worktree's slug rather than the repository's.
    """
    if not _safe_id(conversation_id):
        return None
    base = CLAUDE_CODE_ROOT if root is None else root
    if not base.is_dir():
        return None
    target = f"{conversation_id}.jsonl"
    for project_dir in base.iterdir():
        if project_dir.is_dir() and (project_dir / target).is_file():
            return project_dir / target
    return None


# KT-190 — how many timestamped responses one report may carry.
#
# The per-response timeline is what lets the BACKEND attribute tokens to
# individual messages: it knows when each message was appended, the collector
# knows when each response cost what. Bounded so a first collection over a 61 MB
# transcript cannot post a payload larger than the telemetry is worth; the
# session TOTALS are unaffected by this cap and stay exact.
TIMELINE_MAX_ENTRIES = 500


def collect_claude(path: pathlib.Path, since_offset: int = 0,
                   with_timeline: bool = False) -> dict:
    """Sum the usage objects in `path` from `since_offset` onward.

    `next_offset` advances only over lines that parsed COMPLETELY, so a
    half-written trailing line is re-read next time instead of being skipped:
    the transcript is appended to while the session runs.
    """
    counters = {name: 0 for name in ALL_COUNTERS}
    models: dict[str, int] = {}
    measured = lines = 0
    first_ts = last_ts = None
    offset = since_offset
    timeline: list[dict] = []
    timeline_truncated = False

    if since_offset > path.stat().st_size:
        # The file shrank — rotated, or the id was reused. Re-reading from zero
        # would double-count every token, so refuse and let the caller reset.
        return {
            "status": "measured",
            "vendor": "claude-code",
            "provenance": "claude-code-transcript",
            "measured_responses": 0,
            "lines_read": 0,
            "counters": counters,
            "unmeasured": [],
            "models": {},
            "next_offset": 0,
            "truncated": True,
        }

    with path.open("rb") as handle:
        handle.seek(since_offset)
        for raw in handle:
            if not raw.endswith(b"\n"):
                break  # partial line: leave the cursor before it
            offset += len(raw)
            lines += 1
            try:
                record = json.loads(raw)
            except (ValueError, UnicodeDecodeError):
                continue
            if not isinstance(record, dict):
                continue
            stamp = record.get("timestamp")
            if stamp:
                first_ts = first_ts or stamp
                last_ts = stamp
            message = record.get("message")
            if not isinstance(message, dict):
                continue
            usage = message.get("usage")
            if not isinstance(usage, dict):
                continue
            measured += 1
            model = message.get("model") or "unknown"
            models[model] = models.get(model, 0) + 1
            per_response = {}
            for field, name in _CLAUDE_USAGE_FIELDS.items():
                value = usage.get(field)
                if isinstance(value, int) and not isinstance(value, bool):
                    counters[name] += value
                    per_response[name] = value
            if with_timeline and stamp:
                # Without a timestamp a response cannot be placed against any
                # message, so it is left out of the timeline rather than guessed
                # into the wrong one. It still counts in the totals.
                if len(timeline) < TIMELINE_MAX_ENTRIES:
                    timeline.append({"at": stamp, "model": model, **per_response})
                else:
                    timeline_truncated = True

    return {
        "status": "measured",
        "vendor": "claude-code",
        "provenance": "claude-code-transcript",
        "measured_responses": measured,
        "lines_read": lines,
        "counters": counters,
        "unmeasured": [],  # this vendor reports all four
        "models": models,
        "window_start": first_ts,
        "window_end": last_ts,
        "next_offset": offset,
        "truncated": False,
        # Only present when asked for: a caller that does not attribute per
        # message should not pay for the timeline.
        **({"timeline": timeline,
            "timeline_truncated": timeline_truncated} if with_timeline else {}),
    }


# ── Vibe ───────────────────────────────────────────────────────────


def find_vibe_meta(session_id: str, root: pathlib.Path | None = None):
    """Locate a Vibe session's `meta.json` by its session id.

    Vibe names its directories `session_<date>_<short>`, so the id inside the
    file is the only reliable key — matching on the directory name would depend
    on a truncation rule the vendor never promised.
    """
    if not _safe_id(session_id):
        return None
    base = VIBE_SESSION_ROOT if root is None else root
    if not base.is_dir():
        return None
    for session_dir in base.iterdir():
        meta = session_dir / "meta.json"
        if not meta.is_file():
            continue
        try:
            if json.loads(meta.read_text()).get("session_id") == session_id:
                return meta
        except (ValueError, OSError):
            continue
    return None


def resolve_vibe_session_id(cwd: str, root: pathlib.Path | None = None,
                            now: float | None = None,
                            ambiguity_window_secs: int = 900) -> dict:
    """Work out WHICH Vibe session belongs to this process, or refuse to guess.

    Claude Code publishes `CLAUDE_CODE_SESSION_ID` to its children, so its id is
    simply read. Vibe exports nothing, and — unlike Codex — keeps no session file
    open (`SessionLogger` opens, appends and closes on every write), so there is
    no descriptor to probe either. What is left is the working directory recorded
    in each `meta.json`.

    That is a weaker signal, so the rule is deliberately strict: adopt a session
    only when ONE candidate is plausible. Two Vibe sessions in the same directory
    both touched recently are indistinguishable from here, and attributing 14
    million tokens to the wrong task is far worse than reporting nothing. So
    ambiguity returns `ambiguous`, never a coin flip.
    """
    base = VIBE_SESSION_ROOT if root is None else root
    if not base.is_dir():
        return {"status": "unresolved", "reason": "no vibe session directory"}

    candidates = []
    for session_dir in base.iterdir():
        meta = session_dir / "meta.json"
        if not meta.is_file():
            continue
        try:
            data = json.loads(meta.read_text())
        except (ValueError, OSError):
            continue
        recorded = ((data.get("environment") or {}).get("working_directory") or "")
        if recorded != cwd:
            continue
        session_id = data.get("session_id")
        if not _safe_id(session_id):
            continue
        try:
            touched = meta.stat().st_mtime
        except OSError:
            continue
        candidates.append({"session_id": session_id, "touched": touched,
                           "start_time": data.get("start_time")})

    if not candidates:
        return {"status": "unresolved",
                "reason": f"no vibe session recorded for cwd {cwd}"}

    candidates.sort(key=lambda c: c["touched"], reverse=True)
    if len(candidates) > 1:
        gap = candidates[0]["touched"] - candidates[1]["touched"]
        if gap < ambiguity_window_secs:
            # Two live-looking sessions in one directory. Picking the newest
            # would be a guess wearing the costume of a measurement.
            return {
                "status": "ambiguous",
                "reason": (
                    f"{len(candidates)} vibe sessions share cwd {cwd} and were "
                    f"touched {gap:.0f}s apart; refusing to attribute tokens"
                ),
                "candidates": [c["session_id"] for c in candidates[:4]],
            }
    return {
        "status": "resolved",
        "session_id": candidates[0]["session_id"],
        "start_time": candidates[0]["start_time"],
        "how": "vibe-meta-cwd-match",
    }


def collect_vibe(path: pathlib.Path) -> dict:
    """Read Vibe's session totals.

    A snapshot, not a log: re-read whole every time. There is no offset to
    return because the file is rewritten in place, so an offset would be
    meaningless — and pretending otherwise would silently drop later turns.

    Vibe reports NO cache split. Those counters come back unmeasured, and a
    consumer must not fill them with zeros.
    """
    try:
        meta = json.loads(path.read_text())
    except (ValueError, OSError) as error:
        return _not_measured("vibe", f"meta.json unreadable: {error}")
    stats = meta.get("stats")
    if not isinstance(stats, dict):
        return _not_measured("vibe", "meta.json carries no stats block")

    counters: dict[str, int] = {}
    prompt = stats.get("session_prompt_tokens")
    completion = stats.get("session_completion_tokens")
    if isinstance(prompt, int) and not isinstance(prompt, bool):
        # Vibe's "prompt" is everything sent, cache included and indivisible.
        # Mapped to `input` because that is what it is billed as; the cache
        # portion is unknown, not zero.
        counters["input"] = prompt
    if isinstance(completion, int) and not isinstance(completion, bool):
        counters["output"] = completion

    if not counters:
        return _not_measured("vibe", "stats block carries no token counters")

    model = (meta.get("config") or {}).get("active_model") or "unknown"
    result = {
        "status": "measured",
        "vendor": "vibe",
        "provenance": "vibe-session-meta",
        "measured_responses": stats.get("steps"),
        "counters": counters,
        "unmeasured": [name for name in ALL_COUNTERS if name not in counters],
        "models": {model: stats.get("steps") or 0},
        "window_start": meta.get("start_time"),
        "window_end": meta.get("end_time"),
        # Vibe computes its own cost from its own per-million prices. Kept as
        # the VENDOR's figure, never mixed with a Kronn estimate.
        "vendor_cost_usd": stats.get("session_cost"),
        "vendor_price_per_million": {
            "input": stats.get("input_price_per_million"),
            "output": stats.get("output_price_per_million"),
        },
    }
    return result


# ── entry point ────────────────────────────────────────────────────


def collect_for_session(vendor: str, session_key: str, since_offset: int = 0,
                        root: pathlib.Path | None = None,
                        with_timeline: bool = False) -> dict:
    """Collect for one session, or say plainly that it is not measured."""
    if vendor == "claude-code":
        path = find_claude_transcript(session_key, root=root)
        if path is None:
            return _not_measured("claude-code", "no transcript for this conversation id")
        result = collect_claude(path, since_offset=since_offset,
                                with_timeline=with_timeline)
        result["source"] = str(path)
        return result
    if vendor == "vibe":
        path = find_vibe_meta(session_key, root=root)
        if path is None:
            return _not_measured("vibe", "no session meta for this session id")
        result = collect_vibe(path)
        result["source"] = str(path)
        return result
    # An unsupported vendor is the case this ticket exists to stop reporting as
    # zero: Codex and Copilot land here until their collectors are written.
    return _not_measured(vendor, f"no collector for vendor {vendor!r}")


def _print_human(result: dict) -> None:
    print(f"vendor:     {result['vendor']}")
    print(f"source:     {result.get('source')}")
    print(f"window:     {result.get('window_start')} -> {result.get('window_end')}")
    print(f"responses:  {result.get('measured_responses')}")
    counters = result["counters"]
    traffic = sum(counters.values())
    for name in ALL_COUNTERS:
        if name in counters:
            share = f"{100 * counters[name] / traffic:.1f}%" if traffic else "—"
            print(f"  {name:<15}{counters[name]:>15,}   {share:>6}")
        else:
            print(f"  {name:<15}{'not measured':>15}")
    print(f"  {'traffic':<15}{traffic:>15,}")
    if "cache_read" in counters:
        # Side by side, never as one figure: these differ by ~62x on a real
        # Claude Code session.
        print(f"  {'billable':<15}{traffic - counters['cache_read']:>15,}"
              f"   (cache reads excluded)")
    if result.get("unmeasured"):
        print(f"unmeasured: {', '.join(result['unmeasured'])} "
              f"— absent from this vendor, NOT zero")
    if result.get("vendor_cost_usd") is not None:
        print(f"vendor cost: {result['vendor_cost_usd']} USD (vendor's own figure)")
    print(f"models:     {result['models']}")
    print(f"provenance: {result['provenance']}")
    if "next_offset" in result:
        print(f"next offset: {result['next_offset']}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("vendor", choices=["claude-code", "vibe"])
    parser.add_argument("session_key")
    parser.add_argument("--since-offset", type=int, default=0)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--root", default=None)
    args = parser.parse_args()

    result = collect_for_session(
        args.vendor,
        args.session_key,
        since_offset=args.since_offset,
        root=pathlib.Path(args.root) if args.root else None,
    )
    if args.json:
        print(json.dumps(result, ensure_ascii=False))
        return 0 if result["status"] == "measured" else 1
    if result["status"] != "measured":
        print(f"not measured: {result['reason']}", file=sys.stderr)
        return 1
    _print_human(result)
    return 0


if __name__ == "__main__":
    sys.exit(main())
