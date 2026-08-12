#!/usr/bin/env python3
"""Backfill token telemetry for sessions that ended before the collector existed.

The live collector only measures the session it runs in. Every CLI session that
joined a room before KT-190 landed is therefore unattributed — 32 of 33 on the
first real database — even though the vendor transcripts are usually still on
disk. This reads those and fills the gap.

It is deliberately NOT a "make coverage look good" tool:

  - it only writes a row when a transcript is actually found and parsed. A
    session whose transcript is gone stays UNATTRIBUTED, because inventing a
    zero for it is the exact failure this ticket exists to remove.
  - it never lowers a `read_offset`, so re-running it cannot double-count a
    session the live collector has since advanced past.
  - it reports what it could NOT do, in the same breath as what it did.

Provenance is marked `claude-code-transcript-backfill`, distinct from the live
`claude-code-transcript`: a number recovered after the fact and a number
measured as it happened are not the same evidence, and a reader deserves to
know which they are looking at.

Usage:
    python3 telemetry_backfill.py --db <path> [--apply]

Dry-run by default. Nothing is written without --apply.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import pathlib
import sqlite3
import sys


def _load_collector():
    path = pathlib.Path(__file__).with_name("cli_token_collector.py")
    spec = importlib.util.spec_from_file_location("cli_token_collector", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# Vendors this can reach retroactively. Vibe is excluded on purpose: its
# `meta.json` is a snapshot keyed to a working directory, and resolving an OLD
# session from a cwd that has since been reused would attribute someone else's
# tokens. Live resolution refuses on ambiguity; a backfill cannot even detect it.
BACKFILLABLE = {"ClaudeCode": "claude-code"}


def plan(db_path: str) -> dict:
    collector = _load_collector()
    connection = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    connection.row_factory = sqlite3.Row
    try:
        rows = connection.execute(
            "SELECT s.id, s.agent_type, s.conversation_id, s.status,"
            "       t.cli_session_pk IS NOT NULL AS already,"
            "       COALESCE(t.read_offset, 0) AS stored_offset"
            "  FROM discussion_sessions s"
            "  LEFT JOIN cli_session_telemetry t ON t.cli_session_pk = s.id"
            " ORDER BY s.id"
        ).fetchall()
    finally:
        connection.close()

    # ONE CLI conversation can join SEVERAL rooms, and each join is its own
    # session row. The transcript counts the conversation, not the room — it
    # cannot say which tokens went where. Writing the full amount against every
    # session would therefore multiply it: on the first real database, one
    # conversation of 4 308 007 075 tokens was shared by SEVEN sessions, so a
    # naive backfill would have reported over 30 billion and it would have looked
    # like a measurement.
    #
    # So the counters are written ONCE per conversation, against its earliest
    # session, and the later ones are reported as sharing it. That keeps the sum
    # correct and makes the shape visible instead of hiding it in a total.
    owner_of_conversation: dict[str, int] = {}
    for row in rows:
        key = row["conversation_id"]
        if key and row["agent_type"] in BACKFILLABLE:
            owner_of_conversation.setdefault(key, row["id"])

    measured, skipped = [], []
    for row in rows:
        vendor = BACKFILLABLE.get(row["agent_type"])
        if vendor is None:
            skipped.append({"session": row["id"], "agent": row["agent_type"],
                            "why": "no retroactive collector for this vendor"})
            continue
        key = row["conversation_id"]
        if not key:
            skipped.append({"session": row["id"], "agent": row["agent_type"],
                            "why": "no conversation id was captured at join"})
            continue
        owner = owner_of_conversation.get(key)
        if owner != row["id"]:
            skipped.append({
                "session": row["id"],
                "agent": row["agent_type"],
                "why": (
                    f"shares one CLI conversation with session {owner}; its cost "
                    "is counted there and cannot be split between rooms"
                ),
            })
            continue
        result = collector.collect_for_session(vendor, key, since_offset=0)
        if result["status"] != "measured":
            skipped.append({"session": row["id"], "agent": row["agent_type"],
                            "why": result.get("reason", "not measured")})
            continue
        counters = result.get("counters") or {}
        measured.append({
            "session": row["id"],
            "agent": row["agent_type"],
            "vendor": result["vendor"],
            # Distinct from the live provenance: recovered after the fact is not
            # the same evidence as measured as it happened.
            "provenance": f"{result['provenance']}-backfill",
            "counters": counters,
            "measured_responses": result.get("measured_responses"),
            "models": result.get("models") or {},
            "window_start": result.get("window_start"),
            "window_end": result.get("window_end"),
            "next_offset": result.get("next_offset", 0),
            "stored_offset": row["stored_offset"],
            "already_attributed": bool(row["already"]),
        })

    return {"measured": measured, "skipped": skipped}


def apply(db_path: str, measured: list[dict]) -> int:
    connection = sqlite3.connect(db_path)
    written = 0
    try:
        for item in measured:
            counters = item["counters"]
            connection.execute(
                """
                INSERT INTO cli_session_telemetry (
                    cli_session_pk, vendor, provenance, input_tokens,
                    cache_creation_tokens, cache_read_tokens, output_tokens,
                    measured_responses, models_json, window_start, window_end,
                    vendor_cost_usd, read_offset, updated_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, datetime('now'))
                ON CONFLICT(cli_session_pk) DO UPDATE SET
                    provenance = excluded.provenance,
                    input_tokens = excluded.input_tokens,
                    cache_creation_tokens = excluded.cache_creation_tokens,
                    cache_read_tokens = excluded.cache_read_tokens,
                    output_tokens = excluded.output_tokens,
                    measured_responses = excluded.measured_responses,
                    models_json = excluded.models_json,
                    window_start = COALESCE(cli_session_telemetry.window_start,
                                            excluded.window_start),
                    window_end = excluded.window_end,
                    -- Never rewind: the live collector may already be further
                    -- along, and lowering this would re-collect a counted span.
                    read_offset = MAX(cli_session_telemetry.read_offset,
                                      excluded.read_offset),
                    updated_at = datetime('now')
                """,
                (
                    item["session"], item["vendor"], item["provenance"],
                    # A vendor that does not publish a counter yields None here,
                    # which becomes NULL — never 0.
                    counters.get("input"), counters.get("cache_creation"),
                    counters.get("cache_read"), counters.get("output"),
                    item["measured_responses"],
                    json.dumps(item["models"], ensure_ascii=False),
                    item["window_start"], item["window_end"],
                    item["next_offset"],
                ),
            )
            written += 1
        connection.commit()
    finally:
        connection.close()
    return written


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--db", required=True)
    parser.add_argument("--apply", action="store_true",
                        help="Write. Without it, nothing is modified.")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()

    result = plan(args.db)
    measured, skipped = result["measured"], result["skipped"]

    if args.json:
        print(json.dumps(result, ensure_ascii=False, indent=1))
    else:
        print(f"backfillable: {len(measured)} session(s)")
        for item in measured:
            counters = item["counters"]
            traffic = sum(v for v in counters.values() if isinstance(v, int))
            flag = " (already attributed — will refresh)" if item["already_attributed"] else ""
            print(f"  session {item['session']:<4} {item['agent']:<12}"
                  f" traffic={traffic:>15,} responses={item['measured_responses']}{flag}")
        print(f"\nnot backfillable: {len(skipped)} session(s) — these stay "
              f"UNATTRIBUTED, which is the honest state, not zero")
        reasons: dict[str, int] = {}
        for item in skipped:
            reasons[item["why"]] = reasons.get(item["why"], 0) + 1
        for why, count in sorted(reasons.items(), key=lambda kv: -kv[1]):
            print(f"  {count:>3}  {why}")

    if not args.apply:
        print("\ndry run — nothing written. Pass --apply to write.", file=sys.stderr)
        return 0

    written = apply(args.db, measured)
    print(f"\nwrote {written} row(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
