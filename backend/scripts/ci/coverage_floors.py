#!/usr/bin/env python3
"""The backend coverage floors, checked once on one coverage export (KT-1118).

Reads the JSON of `cargo llvm-cov report --json --summary-only` (the merged
partitions in CI, a whole run locally) and fails when the total line, function
or region coverage is under GLOBAL_FLOOR, or when a key-management file is
under its own floor or cannot be matched to exactly one file. It never runs
tests and never falls back to another report: a missing input is a failure.

Subcommands:
  check COVERAGE.json [--summary-out SUMMARY.json]
  compare EXPECTED_SUMMARY.json ACTUAL_SUMMARY.json
      Fails unless both summaries have the same region, line and function
      counts and the same covered counts, overall and per file: the partitioned
      run must measure exactly what the whole run measures.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

GLOBAL_FLOOR = 83.0
METRICS = ("lines", "functions", "regions")

# The secret-loss hardening files (2026-06-30): path suffix -> (regions %, lines %).
# Raise these with coverage, never lower them.
KEY_FILE_FLOORS = {
    "src/core/crypto.rs": (99.0, 99.0),
    "src/core/config.rs": (94.0, 96.0),
    "src/core/keyvault.rs": (93.0, 94.0),
    "src/core/keystore.rs": (94.0, 94.0),
    "src/core/recovery.rs": (97.0, 98.0),
    "src/db/mcps.rs": (93.0, 95.0),
    "src/db/migrations.rs": (94.0, 97.0),
}


class CoverageError(Exception):
    pass


def export_data(export: dict) -> dict:
    data = export.get("data")
    if not isinstance(data, list) or len(data) != 1:
        raise CoverageError("coverage export must hold exactly one data entry")
    entry = data[0]
    if not isinstance(entry.get("totals"), dict) or not isinstance(entry.get("files"), list):
        raise CoverageError("coverage export has no totals or files")
    return entry


def match_file(files: list[dict], suffix: str) -> dict:
    """The one file whose path ends with SUFFIX on a path boundary."""
    matches = [f for f in files if ("/" + f.get("filename", "").replace("\\", "/")).endswith("/" + suffix)]
    if len(matches) != 1:
        found = ", ".join(f.get("filename", "?") for f in matches) or "none"
        raise CoverageError(f"{suffix}: expected exactly one file in the report, found {found}")
    return matches[0]


def percent(summary: dict, metric: str) -> float:
    value = (summary.get(metric) or {}).get("percent")
    if not isinstance(value, (int, float)):
        raise CoverageError(f"no {metric} percentage in the report")
    return float(value)


def check(export: dict) -> tuple[list[str], list[str]]:
    """(report lines, failures) for the global floor and the key-file floors."""
    entry = export_data(export)
    lines: list[str] = []
    failures: list[str] = []
    for metric in METRICS:
        value = percent(entry["totals"], metric)
        ok = value >= GLOBAL_FLOOR
        lines.append(f"{'ok ' if ok else 'LOW'} total {metric:<9} {value:6.2f}% (floor {GLOBAL_FLOOR:g})")
        if not ok:
            failures.append(f"total {metric} coverage {value:.2f}% is under {GLOBAL_FLOOR:g}%")
    for suffix, (min_regions, min_lines) in KEY_FILE_FLOORS.items():
        try:
            summary = match_file(entry["files"], suffix)["summary"]
            regions, line_pct = percent(summary, "regions"), percent(summary, "lines")
        except (CoverageError, KeyError) as error:
            failures.append(str(error))
            lines.append(f"LOW {suffix}: {error}")
            continue
        ok = regions >= min_regions and line_pct >= min_lines
        lines.append(f"{'ok ' if ok else 'LOW'} {suffix:<22} regions {regions:6.2f}% (min {min_regions:g})"
                     f" lines {line_pct:6.2f}% (min {min_lines:g})")
        if not ok:
            failures.append(f"{suffix}: regions {regions:.2f}% (min {min_regions:g}), lines {line_pct:.2f}% (min {min_lines:g})")
    return lines, failures


def _counts(summary: dict) -> dict:
    return {m: {"count": summary[m]["count"], "covered": summary[m]["covered"]} for m in METRICS if m in summary}


def summarize(export: dict) -> dict:
    """Counts only: what `compare` holds two runs to."""
    entry = export_data(export)
    return {
        "totals": _counts(entry["totals"]),
        "files": {f["filename"]: _counts(f["summary"]) for f in entry["files"]},
    }


def compare(expected: dict, actual: dict) -> list[str]:
    differences: list[str] = []
    if expected.get("totals") != actual.get("totals"):
        differences.append(f"totals differ: expected {expected.get('totals')}, got {actual.get('totals')}")
    left, right = expected.get("files", {}), actual.get("files", {})
    for name in sorted(left.keys() - right.keys()):
        differences.append(f"{name}: only in the expected run")
    for name in sorted(right.keys() - left.keys()):
        differences.append(f"{name}: only in the actual run")
    for name in sorted(left.keys() & right.keys()):
        if left[name] != right[name]:
            differences.append(f"{name}: expected {left[name]}, got {right[name]}")
    return differences


def _load(path: pathlib.Path) -> dict:
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError) as error:
        raise CoverageError(f"cannot read {path}: {error}") from None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    p_check = sub.add_parser("check")
    p_check.add_argument("coverage", type=pathlib.Path)
    p_check.add_argument("--summary-out", type=pathlib.Path)
    p_compare = sub.add_parser("compare")
    p_compare.add_argument("expected", type=pathlib.Path)
    p_compare.add_argument("actual", type=pathlib.Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "check":
            export = _load(args.coverage)
            lines, failures = check(export)
            print("\n".join(lines))
            if args.summary_out:
                args.summary_out.write_text(json.dumps(summarize(export), indent=1, sort_keys=True) + "\n")
            for failure in failures:
                print(f"::error::{failure}")
            if failures:
                print("Coverage floor breached: add tests, do not lower a floor.")
                return 1
            print("Coverage floors OK.")
        else:
            differences = compare(_load(args.expected), _load(args.actual))
            for difference in differences[:200]:
                print(f"::error::{difference}")
            if differences:
                print(f"{len(differences)} difference(s) between the two coverage summaries.")
                return 1
            print("Both runs measure the same regions, lines and functions, with the same coverage.")
    except CoverageError as error:
        print(f"::error::{error}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
