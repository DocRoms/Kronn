#!/usr/bin/env python3
"""Plan, record and verify the partitioned backend test run (KT-1118).

One job builds a single instrumented nextest archive; N jobs each run one
partition of it and upload an artifact holding `manifest.json`, `junit.xml`
and `profiles/*.profraw`. The aggregate job calls `verify`, which fails unless
every partition 1..N is present, built from the same archive and commit, and
together ran each test of the archive's inventory exactly once and green.
`stage` then copies the profiles where `cargo llvm-cov report` reads them.

Subcommands:
  plan --count N                      GitHub outputs: matrix=[1..N], count=N
  manifest --partition K --count N --archive F --sha S --dir D
                                      write D/manifest.json for D's junit/profiles
  verify --count N --artifacts D --inventory LIST.json --archive F --sha S
  stage --count N --artifacts D --dest DIR
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import shutil
import sys
import xml.etree.ElementTree as ET

SCHEMA = 1
MAX_PARTITIONS = 16
ARTIFACT_PREFIX = "backend-partition-"
MANIFEST = "manifest.json"
JUNIT = "junit.xml"
PROFILES = "profiles"
PROFILE_SUFFIX = ".profraw"


class PartitionError(Exception):
    pass


def partition_count(raw: str | int) -> int:
    try:
        count = int(raw)
    except (TypeError, ValueError):
        raise PartitionError(f"partition count must be an integer, got {raw!r}") from None
    if not 1 <= count <= MAX_PARTITIONS:
        raise PartitionError(f"partition count must be between 1 and {MAX_PARTITIONS}, got {count}")
    return count


def plan(count: int) -> str:
    return f"matrix={json.dumps(list(range(1, count + 1)))}\ncount={count}\n"


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def inventory(list_json: dict) -> set[str]:
    """Tests the run must execute: those `nextest list` reports as matching."""
    tests: set[str] = set()
    suites = list_json.get("rust-suites")
    if not isinstance(suites, dict) or not suites:
        raise PartitionError("nextest list output has no rust-suites")
    for binary_id, suite in suites.items():
        for name, case in (suite.get("testcases") or {}).items():
            if (case.get("filter-match") or {}).get("status") == "matches":
                tests.add(f"{binary_id} {name}")
    if not tests:
        raise PartitionError("nextest list output matches no test")
    return tests


def junit_results(text: str) -> tuple[list[str], list[str]]:
    """(every test case run, the failed ones), keyed `<binary-id> <test name>`."""
    root = ET.fromstring(text)
    ran: list[str] = []
    failed: list[str] = []
    for suite in root.iter("testsuite"):
        binary_id = suite.get("name", "")
        for case in suite.iter("testcase"):
            key = f"{binary_id} {case.get('name', '')}"
            ran.append(key)
            if case.find("failure") is not None or case.find("error") is not None:
                failed.append(key)
    return ran, failed


def write_manifest(partition: int, count: int, archive: pathlib.Path, sha: str, directory: pathlib.Path) -> dict:
    if not 1 <= partition <= count:
        raise PartitionError(f"partition {partition} is outside 1..{count}")
    junit = directory / JUNIT
    if not junit.is_file():
        raise PartitionError(f"{junit} is missing")
    profiles = sorted(p.name for p in (directory / PROFILES).glob(f"*{PROFILE_SUFFIX}"))
    if not profiles:
        raise PartitionError(f"no {PROFILE_SUFFIX} file under {directory / PROFILES}")
    manifest = {
        "schema": SCHEMA,
        "partition": partition,
        "count": count,
        "sha": sha,
        "archive_sha256": sha256(archive),
        "profiles": profiles,
    }
    (directory / MANIFEST).write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest


def verify(count: int, artifacts: pathlib.Path, expected: set[str], archive_digest: str, sha: str) -> list[str]:
    """Every problem found; an empty list means the partitions form one whole run."""
    problems: list[str] = []
    seen: dict[str, int] = {}
    incomplete = False
    present = sorted(p.name for p in artifacts.glob(f"{ARTIFACT_PREFIX}*") if p.is_dir())
    wanted = [f"{ARTIFACT_PREFIX}{k}" for k in range(1, count + 1)]
    for extra in sorted(set(present) - set(wanted)):
        problems.append(f"{extra}: not one of the {count} planned partitions")
    for partition, name in enumerate(wanted, start=1):
        directory = artifacts / name
        if not directory.is_dir():
            problems.append(f"{name}: artifact missing")
            incomplete = True
            continue
        try:
            manifest = json.loads((directory / MANIFEST).read_text())
        except (OSError, ValueError) as error:
            problems.append(f"{name}: unreadable {MANIFEST} ({error})")
            incomplete = True
            continue
        for field, value in (("schema", SCHEMA), ("partition", partition), ("count", count),
                             ("sha", sha), ("archive_sha256", archive_digest)):
            if manifest.get(field) != value:
                problems.append(f"{name}: {field} is {manifest.get(field)!r}, expected {value!r}")
        listed = manifest.get("profiles") or []
        if not listed:
            problems.append(f"{name}: no coverage profile recorded")
        for profile in listed:
            if not (directory / PROFILES / profile).is_file():
                problems.append(f"{name}: profile {profile} missing")
        try:
            ran, failed = junit_results((directory / JUNIT).read_text())
        except (OSError, ET.ParseError) as error:
            problems.append(f"{name}: unreadable {JUNIT} ({error})")
            incomplete = True
            continue
        for test in failed:
            problems.append(f"{name}: failed {test}")
        for test in ran:
            if test in seen:
                problems.append(f"{name}: {test} also ran in partition {seen[test]}")
            else:
                seen[test] = partition
    # A lost partition already fails; listing its thousands of tests adds nothing.
    missing = sorted(expected - seen.keys())
    if incomplete and missing:
        problems.append(f"{len(missing)} inventory test(s) without a result")
    else:
        problems.extend(f"inventory test never ran: {test}" for test in missing)
    for test in sorted(seen.keys() - expected):
        problems.append(f"ran a test outside the inventory: {test}")
    return problems


def stage(count: int, artifacts: pathlib.Path, dest: pathlib.Path) -> int:
    """Copy each partition's profiles into DEST, prefixed so equal names never collide."""
    dest.mkdir(parents=True, exist_ok=True)
    stale = sorted(dest.glob(f"*{PROFILE_SUFFIX}"))
    if stale:
        raise PartitionError(f"{dest} already holds {len(stale)} profile(s); refusing to mix runs")
    copied = 0
    for partition in range(1, count + 1):
        source = artifacts / f"{ARTIFACT_PREFIX}{partition}" / PROFILES
        for profile in sorted(source.glob(f"*{PROFILE_SUFFIX}")):
            shutil.copy2(profile, dest / f"p{partition}-{profile.name}")
            copied += 1
    if copied == 0:
        raise PartitionError("no profile to stage")
    return copied


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    p_plan = sub.add_parser("plan")
    p_plan.add_argument("--count", required=True)
    p_manifest = sub.add_parser("manifest")
    p_manifest.add_argument("--partition", type=int, required=True)
    p_manifest.add_argument("--count", required=True)
    p_manifest.add_argument("--archive", type=pathlib.Path, required=True)
    p_manifest.add_argument("--sha", required=True)
    p_manifest.add_argument("--dir", type=pathlib.Path, required=True)
    p_verify = sub.add_parser("verify")
    p_verify.add_argument("--count", required=True)
    p_verify.add_argument("--artifacts", type=pathlib.Path, required=True)
    p_verify.add_argument("--inventory", type=pathlib.Path, required=True)
    p_verify.add_argument("--archive", type=pathlib.Path, required=True)
    p_verify.add_argument("--sha", required=True)
    p_stage = sub.add_parser("stage")
    p_stage.add_argument("--count", required=True)
    p_stage.add_argument("--artifacts", type=pathlib.Path, required=True)
    p_stage.add_argument("--dest", type=pathlib.Path, required=True)
    args = parser.parse_args(argv)
    try:
        count = partition_count(args.count)
        if args.command == "plan":
            sys.stdout.write(plan(count))
        elif args.command == "manifest":
            manifest = write_manifest(args.partition, count, args.archive, args.sha, args.dir)
            print(f"partition {args.partition}/{count}: {len(manifest['profiles'])} profile(s) recorded")
        elif args.command == "verify":
            expected = inventory(json.loads(args.inventory.read_text()))
            problems = verify(count, args.artifacts, expected, sha256(args.archive), args.sha)
            for problem in problems:
                print(f"::error::{problem}")
            if problems:
                print(f"{len(problems)} problem(s): the partitions do not form one complete run.")
                return 1
            print(f"{count} partition(s) ran all {len(expected)} inventory tests exactly once, all green.")
        else:
            print(f"staged {stage(count, args.artifacts, args.dest)} profile(s) into {args.dest}")
    except PartitionError as error:
        print(f"::error::{error}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
