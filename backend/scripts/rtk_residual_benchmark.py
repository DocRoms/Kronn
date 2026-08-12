#!/usr/bin/env python3
"""KT-197 DoD 6 — the shell residual, next to the compression it must not cost.

Two numbers that have to be read together. RTK's test and lint filters are
excellent: 96-99.9% on vitest, cargo test and eslint. Any change aimed at the
residual that drags one of those down has traded a large certain gain for a small
speculative one, so those rates are a FLOOR this script enforces.

The residual itself, measured on a real 2 901-call session:

    adoption            17%
    missed              2 172 invocations, ~1.37 MB returned unwrapped
    grep alone          1 548 calls, 1 029 698 B — 75% of the residual
    /usr/bin/ bypasses  1 411

And the filter that explains most of what is left:

    rtk read   985 calls, 10.6% saved

That last figure is the point. `read` is the most-used filter in the fleet and the
weakest by an order of magnitude; it is what KT-197 DoD 3 (targeting, pagination,
caps, out-of-context artifact) exists to fix. Reporting a global adoption
percentage without it would send someone chasing the 17% instead of the 10.6%.

Usage:
    python3 rtk_residual_benchmark.py [--gain-output FILE] [--json]

With no arguments it runs `rtk gain` itself.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys

# Floors on the filters whose compression must not regress, each pinned at what rtk
# 0.42.4 actually measures on this fleet. Tightened when a gain is real, and never
# lowered to make a run pass.
#
# The first version of this file guessed round numbers instead, and the gate
# immediately caught it: `rtk cargo test` runs at 96.4 / 99.7 / 100.0 / 94.7 across
# its flag variants, so a 95.0 floor failed on a rate that had never regressed. A
# floor set before measuring is not a floor, it is an opinion — so each value below
# is the LOWEST observed variant, with the readings kept beside it.
COMPRESSION_FLOORS = {
    # measured 96.6
    "vitest": 96.0,
    # measured 96.4 / 99.7 / 100.0 / 94.7 — the last is `--lib` with a filter
    "cargo test": 94.5,
    # measured 99.9
    "lint": 99.0,
}

# Where the residual actually is. `read` is listed because it is the largest
# unexploited filter, not because it is failing: 10.6% over 985 calls is more
# tokens than a perfect filter on a rare command could ever return.
KNOWN_WEAK = {"read": 10.6, "find": 60.6}

# `1.  rtk vitest run    165  16.0M   96.6%   17.7s`
ROW = re.compile(
    r"^\s*\d+\.\s+(?P<command>.+?)\s{2,}(?P<count>[\d,]+)\s+(?P<saved>[\d.]+[KMG]?)\s+"
    r"(?P<pct>[\d.]+)%"
)


def parse_gain(text: str) -> list[dict]:
    """Per-command rows from `rtk gain`. Ignores everything else on purpose."""
    rows = []
    for line in text.splitlines():
        match = ROW.match(line)
        if not match:
            continue
        rows.append({
            "command": match.group("command").strip(),
            "count": int(match.group("count").replace(",", "")),
            "saved": match.group("saved"),
            "percent": float(match.group("pct")),
        })
    return rows


def floor_violations(rows: list[dict]) -> list[dict]:
    """Filters that fell below their floor.

    Matched on a substring of the command, because `rtk cargo test --lib` and
    `rtk cargo test --manifest-path …` are the same filter under different flags
    and both must hold.
    """
    violations = []
    for name, floor in COMPRESSION_FLOORS.items():
        matching = [row for row in rows if name in row["command"]]
        if not matching:
            # Absent is not passing: a floor with nothing to check proves nothing,
            # and silently skipping it is how a gate stops gating.
            violations.append({
                "filter": name,
                "floor": floor,
                "measured": None,
                "why": "no row for this filter in the gain output",
            })
            continue
        for row in matching:
            if row["percent"] < floor:
                violations.append({
                    "filter": name,
                    "floor": floor,
                    "measured": row["percent"],
                    "command": row["command"],
                    "why": "compression regressed below the pinned floor",
                })
    return violations


def weakest_filters(rows: list[dict], limit: int = 3) -> list[dict]:
    """The filters returning the most bytes despite being used the most.

    Ranked by count x (100 - percent): a filter used 985 times at 10.6% costs far
    more than one used twice at 0%. Ranking by percentage alone would put the rare
    command first and send the work to the wrong place.
    """
    scored = [
        dict(row, residual_score=round(row["count"] * (100 - row["percent"])))
        for row in rows
    ]
    scored.sort(key=lambda row: row["residual_score"], reverse=True)
    return scored[:limit]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gain-output", help="file holding `rtk gain` output")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()

    if args.gain_output:
        with open(args.gain_output, encoding="utf-8") as handle:
            text = handle.read()
    else:
        try:
            text = subprocess.run(
                ["rtk", "gain"], capture_output=True, text=True, timeout=120, check=False
            ).stdout
        except (OSError, subprocess.SubprocessError) as error:
            print(f"could not run `rtk gain`: {error}", file=sys.stderr)
            return 2

    rows = parse_gain(text)
    if not rows:
        # Empty is not clean. A parse that found nothing must fail loudly, or a
        # format change would read as "every floor holds".
        print("no per-command rows parsed from `rtk gain` — format changed?", file=sys.stderr)
        return 2

    violations = floor_violations(rows)
    weakest = weakest_filters(rows)

    if args.json:
        print(json.dumps({
            "rows": rows,
            "violations": violations,
            "weakest": weakest,
            "known_weak": KNOWN_WEAK,
        }, indent=2))
    else:
        print("compression floors (tests and lint must not regress)")
        for name, floor in COMPRESSION_FLOORS.items():
            measured = [row for row in rows if name in row["command"]]
            state = "ok" if not any(v["filter"] == name for v in violations) else "FAIL"
            rates = ", ".join(f"{row['percent']}%" for row in measured) or "not measured"
            print(f"  {name:<14} floor {floor:>5}%   {rates:<28} {state}")

        print("\nlargest residual by filter (count x bytes left on the table)")
        for row in weakest:
            print(f"  {row['command']:<32} {row['count']:>5} calls  "
                  f"{row['percent']:>5}%  score {row['residual_score']:>8}")

        if violations:
            print("\nFAILED:")
            for violation in violations:
                print(f"  {violation['filter']}: {violation['why']}"
                      f" (floor {violation['floor']}%, measured {violation['measured']})")

    return 1 if violations else 0


if __name__ == "__main__":
    sys.exit(main())
