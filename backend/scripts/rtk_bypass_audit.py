#!/usr/bin/env python3
"""Detect commands that bypass RTK — KT-197 DoD 2 and 5.

RTK's compression is excellent where it is used. The residual is ADOPTION: on a
real 9-day session, 2 658 Bash calls carried `rtk` only 363 times — 13% — and
2 025 of the rest were `/usr/bin/grep`, an absolute path that bypasses any shell
function or alias entirely.

So this reads a transcript and reports the bypasses, with one rule that matters
more than the counting:

    A COMMAND RTK WOULD RUN RAW ANYWAY IS NOT A MISS.

`grep -q` is a probe whose whole value is the exit code; `grep -c` and `-l`
return counts and filenames that RTK deliberately passes through. Counting those
as missed adoption would inflate the number and send someone optimising calls
that were already optimal — which is how a metric starts lying.

Usage:
    python3 rtk_bypass_audit.py <transcript.jsonl> [--json] [--min-count N]
"""

from __future__ import annotations

import argparse
import collections
import json
import pathlib
import re
import sys

# Commands RTK has a dedicated filter for. Bypassing one of these costs real
# bytes; bypassing anything else costs nothing, since RTK would pass it through.
FILTERED = (
    "cargo build", "cargo check", "cargo clippy", "cargo test",
    "go test", "jest", "vitest", "playwright test", "pytest", "rspec",
    "rake test", "tsc", "lint", "prettier", "next build",
    "git status", "git log", "git diff", "git show", "git add", "git commit",
    "git push", "git pull", "git branch", "git fetch", "git stash",
    "git worktree",
    "gh pr", "gh run", "gh issue", "gh api",
    "pnpm list", "pnpm outdated", "pnpm install", "npm run", "npx",
    "prisma", "docker ps", "docker images", "docker logs",
    "kubectl get", "kubectl logs", "curl", "wget",
    "grep", "find", "ls", "read",
)

# Ways a caller reaches the real binary past a shell function or alias. These are
# the forms KT-197 DoD 2 asks to detect: they look ordinary in a transcript and
# are invisible to `rtk discover`, which sees only the leading word.
HARD_BYPASS = (
    ("/usr/bin/", re.compile(r"(?:^|[\s|;&(])/usr/bin/(\w+)")),
    ("/bin/", re.compile(r"(?:^|[\s|;&(])/bin/(\w+)")),
    ("command ", re.compile(r"(?:^|[\s|;&(])command\s+(\w+)")),
    ("backslash", re.compile(r"(?:^|[\s|;&(])\\(\w+)")),
    ("builtin ", re.compile(r"(?:^|[\s|;&(])builtin\s+(\w+)")),
)

# Flags on which RTK runs the tool raw, so wrapping changes nothing. Sourced from
# the project's own RTK notes: "Format flags (-c, -l, -L, -o, -Z) run raw."
# Extensions whose Read cost is a rendered payload, not lines of code. Split out
# because the remedies are opposites: a source file gets `offset`/`limit`, while
# an image cannot be paginated at all — the only lever there is reading it fewer
# times. Reporting one average over both would recommend the wrong fix.
BINARY_SUFFIXES = (".png", ".jpg", ".jpeg", ".gif", ".webp", ".pdf", ".ico")

RAW_FLAGS = {"-q", "-c", "-l", "-L", "-o", "-Z", "--quiet", "--count",
             "--files-with-matches", "--files-without-match"}


def segments(command: str) -> list[str]:
    """Split a compound command into the pieces a shell would run separately.

    Needed because `rtk git add . && git commit -m x` wraps the first half only —
    a compound line is not one decision, and treating it as one hides the half
    that pays full price.
    """
    return [part.strip() for part in re.split(r"&&|\|\||;|\|", command) if part.strip()]


def is_expected_probe(segment: str) -> bool:
    """True when RTK would run this raw anyway, so wrapping saves nothing."""
    tokens = segment.split()
    for token in tokens[1:]:
        if token in RAW_FLAGS:
            return True
        # Bundled short flags, e.g. `-rn` is fine but `-rq` contains a raw flag.
        if len(token) > 1 and token[0] == "-" and token[1] != "-":
            for letter in token[1:]:
                if f"-{letter}" in RAW_FLAGS:
                    return True
    return False


def filtered_command(segment: str) -> str | None:
    """Which RTK-filtered command this segment invokes, if any."""
    stripped = segment
    # Strip a leading `rtk ` FIRST: a wrapped call still invokes the same tool,
    # and missing this reported 0% adoption on a session that used rtk hundreds
    # of times — a metric that said the opposite of the truth.
    stripped = re.sub(r"^rtk\s+", "", stripped)
    # Then strip an absolute path or shell escape, so `/usr/bin/grep` matches the
    # `grep` filter — the point is what tool RUNS, not how it was spelled.
    stripped = re.sub(r"^(?:/usr/bin/|/bin/|command\s+|builtin\s+|\\)", "", stripped)
    for name in sorted(FILTERED, key=len, reverse=True):
        if stripped.startswith(name + " ") or stripped == name:
            return name
    return None


def classify(command: str) -> list[dict]:
    """Classify every segment of one command line."""
    out = []
    for segment in segments(command):
        name = filtered_command(segment)
        if name is None:
            continue
        wrapped = re.match(r"^rtk\s", segment) is not None
        bypass_form = None
        for label, pattern in HARD_BYPASS:
            if pattern.search(segment):
                bypass_form = label
                break
        out.append({
            "command": name,
            "wrapped": wrapped,
            # A probe is NOT a miss: RTK would run it raw. Kept as its own class
            # so the headline number stays honest.
            "expected_probe": is_expected_probe(segment),
            "bypass_form": bypass_form,
        })
    return out


def audit(path: pathlib.Path) -> dict:
    """Walk a Claude Code transcript, pairing each Bash call with its result."""
    pending: dict[str, str] = {}
    reads: dict[str, dict] = {}
    findings: list[dict] = []
    result_bytes: collections.Counter = collections.Counter()
    read_stats: dict[str, list[int]] = collections.defaultdict(lambda: [0, 0])
    reread: collections.Counter = collections.Counter()
    total_calls = 0

    with path.open(errors="replace") as handle:
      for raw in handle:
        try:
            record = json.loads(raw)
        except (ValueError, UnicodeDecodeError):
            continue
        message = record.get("message")
        if not isinstance(message, dict):
            continue
        content = message.get("content")
        if not isinstance(content, list):
            continue
        for block in content:
            if not isinstance(block, dict):
                continue
            if block.get("type") == "tool_use" and block.get("name") == "Bash":
                command = (block.get("input") or {}).get("command", "")
                if command:
                    pending[block.get("id")] = command
                    total_calls += 1
            elif block.get("type") == "tool_use" and block.get("name") == "Read":
                # Tracked because on the measured session Read was 9 130 394 B —
                # 56% of ALL tool output, and ten times the grep residual. An
                # audit that only looked at shell commands would have optimised
                # the smaller half.
                reads[block.get("id")] = block.get("input") or {}
            elif block.get("type") == "tool_result":
                read_input = reads.pop(block.get("tool_use_id"), None)
                if read_input is not None:
                    payload = block.get("content")
                    text = payload if isinstance(payload, str) else json.dumps(
                        payload, ensure_ascii=False
                    )
                    path_read = str(read_input.get("file_path", ""))
                    binary = path_read.lower().endswith(BINARY_SUFFIXES)
                    targeted = "offset" in read_input or "limit" in read_input
                    bucket = ("binary" if binary else "text") + (
                        "_targeted" if targeted else "_whole"
                    )
                    read_stats[bucket][0] += 1
                    read_stats[bucket][1] += len(text.encode())
                    if path_read:
                        reread[path_read] += 1
                    continue
                command = pending.pop(block.get("tool_use_id"), None)
                if command is None:
                    continue
                payload = block.get("content")
                text = payload if isinstance(payload, str) else json.dumps(
                    payload, ensure_ascii=False
                )
                size = len(text.encode())
                for item in classify(command):
                    findings.append(item)
                    if not item["wrapped"] and not item["expected_probe"]:
                        result_bytes[item["command"]] += size

    missed = [f for f in findings if not f["wrapped"] and not f["expected_probe"]]
    probes = [f for f in findings if f["expected_probe"] and not f["wrapped"]]
    wrapped = [f for f in findings if f["wrapped"]]

    by_command = collections.Counter(f["command"] for f in missed)
    by_form = collections.Counter(
        f["bypass_form"] for f in missed if f["bypass_form"]
    )

    eligible = len(missed) + len(wrapped)
    return {
        "bash_calls": total_calls,
        "filtered_invocations": len(findings),
        "wrapped": len(wrapped),
        "missed": len(missed),
        # Reported apart, per DoD 5: RTK runs these raw, so wrapping saves
        # nothing and counting them as misses would overstate the residual.
        "expected_probes": len(probes),
        "adoption_ratio": (len(wrapped) / eligible) if eligible else None,
        "missed_by_command": dict(by_command.most_common()),
        "hard_bypass_forms": dict(by_form.most_common()),
        # Bytes returned by unwrapped calls: the upper bound on what adoption
        # could have compressed. NOT a claim that RTK would have removed all of
        # it — that would need running both, which this script does not do.
        "unwrapped_result_bytes": dict(result_bytes.most_common()),
        # Read, split four ways. On the measured session `binary_whole` alone was
        # 8 360 241 B from 23 calls — more than every other tool combined — while
        # targeted text reads averaged 2 269 B and were never the problem.
        "read_bytes": {
            bucket: {"calls": calls, "bytes": total,
                     "mean": total // calls if calls else 0}
            for bucket, (calls, total) in sorted(read_stats.items())
        },
        # Files read more than twice. Cheap individually when targeted, so this is
        # a signal about navigation, not a cost claim.
        "reread_files": {
            path.rsplit("/", 1)[-1]: count
            for path, count in reread.most_common(10)
            if count > 2
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("transcript")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--min-count", type=int, default=1)
    args = parser.parse_args()

    path = pathlib.Path(args.transcript)
    if not path.is_file():
        print(f"no such transcript: {path}", file=sys.stderr)
        return 1
    report = audit(path)

    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=1))
        return 0

    ratio = report["adoption_ratio"]
    print(f"bash calls:            {report['bash_calls']:,}")
    print(f"RTK-filtered invocations: {report['filtered_invocations']:,}")
    print(f"  wrapped in rtk:      {report['wrapped']:,}")
    print(f"  missed:              {report['missed']:,}")
    print(f"  expected probes:     {report['expected_probes']:,} "
          f"(RTK runs these raw — not misses)")
    print(f"adoption:              "
          f"{'—' if ratio is None else f'{ratio * 100:.0f}%'}")

    if report["hard_bypass_forms"]:
        print("\nforms that bypass a shell function entirely:")
        for form, count in report["hard_bypass_forms"].items():
            if count >= args.min_count:
                print(f"  {form:<12} {count:>6,}")

    if report["read_bytes"]:
        print("\nRead cost by kind:")
        for bucket, stats in report["read_bytes"].items():
            print(f"  {bucket:<16}{stats['calls']:>6} calls "
                  f"{stats['bytes']:>11,} B  mean {stats['mean']:>8,} B")
        binary = report["read_bytes"].get("binary_whole")
        if binary and binary["bytes"] > 0:
            print(f"  → images cannot be paginated; the only lever is reading "
                  f"them fewer times")

    print("\nmissed by command (bytes returned unwrapped):")
    for name, count in report["missed_by_command"].items():
        if count < args.min_count:
            continue
        size = report["unwrapped_result_bytes"].get(name, 0)
        print(f"  {name:<16} {count:>6,} calls  {size:>10,} B")
    return 0


if __name__ == "__main__":
    sys.exit(main())
