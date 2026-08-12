#!/usr/bin/env python3
"""KT-196 DoD 7 — before/after benchmark on the two reference review discussions.

The first version of this script measured the wrong thing. It compared the BYTES
OF THE PUBLISHED MESSAGES, and found a negative saving — correctly, because the
published report is the output of a review, and the delta architecture does not
make a review report shorter. What it changes is what a review pass has to be
SENT in order to produce that report.

So this measures input per review pass. A "review pass" is an agent message that
names at least one source file: the message where a reviewer says something about
code. For each one:

  BEFORE, cold  — everything in the discussion up to that point. What a fresh pass
                  costs: a returning agent, a new session, a `disc_load_other`.
                  This is the pattern the reference discussions actually show.
  BEFORE, warm  — only what arrived since that author's previous pass. What a live
                  session pays at the margin.
  AFTER         — the bounded review payload, at its enforced ceiling.

Both before-figures are measured from the recorded discussion. The after-figure is
the CAP the shipped code enforces, so it is the pessimistic case: a real payload
carrying a handful of findings is far smaller.

Prints aggregates only — no message content.
"""

import re
import sqlite3
import sys
from collections import defaultdict

DB = sys.argv[1] if len(sys.argv) > 1 else "bench.db"
DISCS = {
    "095dfee0-a727-41c2-a00c-1c25ad93fbcd": "Review next step 3.3",
    "8490d400-1537-46d8-83a5-12fdae06551f": "front_euronews review",
}

# Enforced in code: core::review_payload::REVIEW_PAYLOAD_MAX_BYTES.
REVIEW_PAYLOAD_MAX_BYTES = 24_576
# What the shipped renderer actually produces at the shape measured below, taken
# from the two pinned benchmark tests in review_payload_test.rs. The cap above is a
# backstop; these are what a pass costs.
PAYLOAD_AT_P90 = 564
PAYLOAD_AT_WORST = 2_199

FILE_PATH = re.compile(r"\b([\w./-]+\.(?:rs|ts|tsx|js|jsx|py|sql|toml|yml|yaml|md))\b")
# Same 10-line bucketing as db::review_ledger::fingerprint.
SITE = re.compile(r"\b([\w./-]+\.(?:rs|ts|tsx|js|jsx|py|sql|toml|yml|yaml|md))[:(](\d{1,6})")


def measure(rows):
    """Accounting for one discussion, from `(role, author, content)` in order.

    A pass pays for what was in the discussion BEFORE it, not including itself:
    an agent is not sent its own not-yet-written message. The warm figure is the
    same accounting per author, which is why a first pass costs the same either
    way and only later ones diverge.
    """
    cumulative = 0
    last_pass_offset = defaultdict(int)  # per author: cumulative at their last pass
    cold = warm = 0
    passes = 0
    mentions = defaultdict(set)
    sites = defaultdict(set)

    for index, (role, author, content) in enumerate(rows):
        size = len(content.encode())
        if role == "Agent" and FILE_PATH.search(content):
            passes += 1
            cold += cumulative
            warm += cumulative - last_pass_offset[author]
            last_pass_offset[author] = cumulative
            for path in set(FILE_PATH.findall(content)):
                mentions[path].add(index)
            for path, line in SITE.findall(content):
                sites[(path, int(line) // 10)].add(index)
        cumulative += size

    return {
        "messages": len(rows),
        "bytes": cumulative,
        "passes": passes,
        "cold": cold,
        "warm": warm,
        "files": len(mentions),
        "repeated": sum(1 for m in mentions.values() if len(m) > 1),
        "mentions": sum(len(m) for m in mentions.values()),
        "sites": len(sites),
        "repeated_sites": sum(1 for m in sites.values() if len(m) > 1),
    }


def main():
    conn = sqlite3.connect(DB)
    grand = defaultdict(int)

    for disc_id, label in DISCS.items():
        rows = conn.execute(
            """SELECT role, COALESCE(agent_type, '-'), content
                 FROM messages
                WHERE discussion_id = ? AND channel = 'main'
                ORDER BY sort_order""",
            (disc_id,),
        ).fetchall()

        m = measure(rows)
        cumulative, passes = m["bytes"], m["passes"]
        cold, warm = m["cold"], m["warm"]
        repeated_paths, path_mentions = m["repeated"], m["mentions"]
        repeated_sites = m["repeated_sites"]
        file_count, site_count = m["files"], m["sites"]
        after = passes * REVIEW_PAYLOAD_MAX_BYTES

        print(f"\n=== {label} ({disc_id[:8]}) ===")
        print(f"  messages                          {len(rows):>12,}")
        print(f"  discussion content bytes          {cumulative:>12,}")
        print(f"  review passes (agent names a file){passes:>12,}")
        print(f"  distinct files named              {file_count:>12,}")
        print(f"  files named in >1 pass            {repeated_paths:>12,}")
        print(f"  file mentions across passes       {path_mentions:>12,}")
        if file_count:
            print(f"  mentions per file                 {path_mentions / file_count:>11.2f}x")
        print(f"  distinct root-cause sites         {site_count:>12,}")
        print(f"  sites named in >1 pass            {repeated_sites:>12,}")
        print(f"  BEFORE input, cold passes         {cold:>12,}")
        print(f"  BEFORE input, warm passes         {warm:>12,}")
        print(f"  AFTER input, at the cap           {after:>12,}")
        if cold:
            print(f"  saving vs cold                    {cold - after:>12,}"
                  f"  ({100 * (cold - after) / cold:.1f}%)")
        if warm:
            delta = warm - after
            sign = "saving" if delta >= 0 else "COST"
            print(f"  {sign} vs warm{'':<20}{delta:>12,}"
                  f"  ({100 * delta / warm:+.1f}%)")

        grand["msgs"] += len(rows)
        grand["bytes"] += cumulative
        grand["passes"] += passes
        grand["cold"] += cold
        grand["warm"] += warm
        grand["files"] += file_count
        grand["repeated"] += repeated_paths
        grand["mentions"] += path_mentions
        grand["sites"] += site_count
        grand["repeated_sites"] += repeated_sites

    print("\n=== both discussions ===")
    print(f"  messages                          {grand['msgs']:>12,}")
    print(f"  discussion content bytes          {grand['bytes']:>12,}")
    print(f"  review passes                     {grand['passes']:>12,}")
    print(f"  distinct files named              {grand['files']:>12,}")
    print(f"  files named in >1 pass            {grand['repeated']:>12,}")
    print(f"  file mentions across passes       {grand['mentions']:>12,}")
    print(f"  dedup ratio (mentions per file)   {grand['mentions'] / grand['files']:>11.2f}x")
    print(f"  distinct root-cause sites         {grand['sites']:>12,}")
    print(f"  sites named in >1 pass            {grand['repeated_sites']:>12,}")
    print(f"  BEFORE input, cold passes         {grand['cold']:>12,}")
    print(f"  BEFORE input, warm passes         {grand['warm']:>12,}")

    print("\n  AFTER, three ways of costing a pass:")
    for name, per_pass, note in (
        ("measured p90 payload", PAYLOAD_AT_P90, "what a normal pass costs"),
        ("measured worst payload", PAYLOAD_AT_WORST, "the worst shape observed"),
        ("enforced cap", REVIEW_PAYLOAD_MAX_BYTES, "backstop, not a target"),
    ):
        after = grand["passes"] * per_pass
        cold_delta = grand["cold"] - after
        warm_delta = grand["warm"] - after
        print(f"\n    {name} ({per_pass:,} B/pass — {note})")
        print(f"      after total                   {after:>12,}")
        print(f"      vs cold  {cold_delta:>+14,}  ({100 * cold_delta / grand['cold']:+.1f}%)")
        print(f"      vs warm  {warm_delta:>+14,}  ({100 * warm_delta / grand['warm']:+.1f}%)")

    print("\n  Read the cap row: costing every pass at the ceiling makes a bounded")
    print("  payload look MORE expensive than a warm session's incremental context.")
    print("  The ceiling is a backstop; the p90 and worst rows are what the shipped")
    print("  renderer produces, pinned by two tests so growth fails a build.")


if __name__ == "__main__":
    main()
