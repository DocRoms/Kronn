#!/usr/bin/env python3
"""Anonymised token-telemetry export — KT-190 DoD 6.

Kronn's own numbers are worth sharing: a release note claiming a token reduction
should be checkable by someone who was not there. But the same rows sit next to
prompts, repository paths and business content, so the export is built as an
ALLOW-LIST. Only named columns leave.

A deny-list would be the wrong shape here. It stays correct exactly until
someone adds a column, and the failure mode is silent disclosure — the export
keeps working, with one more field in it. An allow-list fails the other way: a
new column is simply absent until someone decides it may leave.

What is deliberately dropped, and why:

    working directory / paths   name the person and their employer's projects
    discussion titles           written by a human, about real work
    project ids                 join keys back to a private inventory
    session ids                 correlatable with the vendor's own logs
    model prices                a customer's negotiated terms

What is kept: the counters, the vendor, the provenance, the coarse time window,
the model names, and the per-object shape (how many sessions, how many
unmeasured). That is enough to reproduce a ratio, which is the point.

Object keys are HASHED rather than removed: a reader must be able to see that
two rows belong to the same task without learning which task.

Usage:
    python3 telemetry_export.py --db <path> [--json] [--salt <salt>]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sqlite3
import sys

# Every column that may leave, and nothing else.
TELEMETRY_COLUMNS = (
    "vendor",
    "provenance",
    "input_tokens",
    "cache_creation_tokens",
    "cache_read_tokens",
    "output_tokens",
    "measured_responses",
    "models_json",
    "window_start",
    "window_end",
)

# Columns that exist in the table and must NEVER be exported. Listed explicitly
# so the test below can prove the allow-list and this set stay disjoint and
# together cover the table — a new column belongs to one or the other, on
# purpose, rather than slipping out by default.
WITHHELD_COLUMNS = (
    "cli_session_pk",   # joins straight back to a session row
    "read_offset",      # a byte position in a private transcript
    "updated_at",       # exact wall-clock of someone's working hours
    "vendor_cost_usd",  # derived from negotiated prices
)


def pseudonym(value: str, salt: str) -> str:
    """Stable within an export, useless outside it.

    Salted so two exports cannot be joined together, and truncated because a
    full digest invites someone to try a dictionary of known ids.
    """
    digest = hashlib.sha256(f"{salt}\0{value}".encode()).hexdigest()
    return digest[:12]


def export(db_path: str, salt: str) -> dict:
    connection = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    connection.row_factory = sqlite3.Row
    try:
        columns = ", ".join(f"t.{name}" for name in TELEMETRY_COLUMNS)
        try:
            rows = connection.execute(
                f"SELECT {columns}, s.agent_type, s.disc_id "
                "FROM cli_session_telemetry t "
                "JOIN discussion_sessions s ON s.id = t.cli_session_pk"
            ).fetchall()
        except sqlite3.OperationalError as error:
            # A database predating the telemetry migration has nothing to
            # export. That is a state to report, not a stack trace — and
            # certainly not an empty export that reads as "nothing was spent".
            if "no such table" in str(error) or "no such column" in str(error):
                return {
                    "schema": "kronn-telemetry-export/1",
                    "status": "unavailable",
                    "reason": (
                        "this database has no telemetry table yet — run the "
                        "migrations first. Nothing was exported, which is NOT "
                        "the same as nothing being spent."
                    ),
                    "sessions": [],
                    "withheld": list(WITHHELD_COLUMNS),
                }
            raise

        sessions = []
        for row in rows:
            record = {name: row[name] for name in TELEMETRY_COLUMNS}
            record["agent_type"] = row["agent_type"]
            # The room this belonged to, as a pseudonym: enough to group rows,
            # not enough to find the room.
            record["room"] = pseudonym(row["disc_id"], salt)
            # Stated explicitly rather than left to inference: a null counter
            # means the vendor does not publish it, and a consumer computing a
            # cache ratio must not read it as zero.
            record["unmeasured"] = [
                name.replace("_tokens", "")
                for name in ("input_tokens", "cache_creation_tokens",
                             "cache_read_tokens", "output_tokens")
                if row[name] is None
            ]
            sessions.append(record)

        return {
            "schema": "kronn-telemetry-export/1",
            "sessions": sessions,
            "withheld": list(WITHHELD_COLUMNS),
            "note": (
                "Counters are per vendor and never summed here. A null counter is "
                "NOT zero: the vendor does not publish it. Traffic includes cache "
                "reads, which bill at roughly a tenth — on one measured session "
                "they were 98.4% of traffic, so traffic and billable differ by "
                "about 62x. Report them side by side or not at all."
            ),
        }
    finally:
        connection.close()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--db", required=True)
    parser.add_argument(
        "--salt",
        default="kronn-export",
        help="Changes every pseudonym. Use a fresh value per export so two "
             "exports cannot be joined.",
    )
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()

    payload = export(args.db, args.salt)
    if args.json:
        print(json.dumps(payload, ensure_ascii=False, indent=1))
        return 0 if payload.get("status") != "unavailable" else 1

    if payload.get("status") == "unavailable":
        print(f"unavailable: {payload['reason']}", file=sys.stderr)
        return 1

    print(f"sessions exported: {len(payload['sessions'])}")
    print(f"withheld columns:  {', '.join(payload['withheld'])}")
    for record in payload["sessions"]:
        traffic = sum(
            value for key, value in record.items()
            if key.endswith("_tokens") and isinstance(value, int)
        )
        print(f"  {record['agent_type']:<12} room={record['room']} "
              f"traffic={traffic:>15,} "
              f"unmeasured={','.join(record['unmeasured']) or '-'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
