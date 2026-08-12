#!/usr/bin/env python3
"""Context Budget Gate — KT-191.

Every agent session pays for the mandatory bootstrap before doing any useful
work, so that bootstrap is a product cost, not documentation hygiene. This gate
does two things:

  * RATCHET — a file may not exceed `max_bytes`, pinned at its current size.
    Growth is refused until the re-split lands. This is the part that stops the
    bleeding today.
  * TARGET — `target_bytes` records where the file must end up. The gate prints
    the remaining gap on every run so the debt stays visible instead of being
    silently normalised.

Bytes are enforced because they are exact. Token counts are PRINTED as an
estimate and never gate anything: a real tokenizer is model-specific, and a
ceiling must not rest on an approximation. `BYTES_PER_TOKEN_ESTIMATE` is a
coarse average for English prose + code; treat the figure as indicative.

Usage: python3 backend/scripts/ci/context_budget.py [--repo-root PATH]
Exit 1 on any overrun, with the offending file and the excess.
"""

from __future__ import annotations

import argparse
import pathlib
import sys

BYTES_PER_TOKEN_ESTIMATE = 3.7

# Ratchet ceilings. `max_bytes` is the CURRENT measured size (2026-08-03) —
# raising one is a deliberate act that must be justified in review, never a
# side effect of adding a section. `target_bytes` comes from KT-191's DoD.
BUDGETS: list[dict[str, object]] = [
    {
        # Lowered from 84 224 as sections moved to their canonical homes
        # (UI structure, prerequisites, RTK internals, release history, stack).
        # A ratchet must tighten on every gain, or the slack gets refilled.
        "path": "docs/AGENTS.md",
        "max_bytes": 13_471,
        "target_bytes": 16 * 1024,
        # Stretch goal beyond the DoD ceiling: 12 KiB / ~3 000 tokens. Recorded
        # here so the next objective stays visible once the target is met —
        # "at target" is where files quietly stop improving.
        "stretch_bytes": 12 * 1024,
        "why": "Tier 1 — read in full by every session before any work",
    },
    {
        "path": "CLAUDE.md",
        "max_bytes": 5_311,
        "target_bytes": 5_311,
        "why": "auto-loaded by Claude Code on every session",
    },
    {
        "path": "AGENTS.md",
        "max_bytes": 225,
        "target_bytes": 225,
        "why": "root pointer to docs/AGENTS.md",
    },
]

# Aggregate ceiling for everything an agent must read before starting (DoD 5).
# Lowered from 89 760 by the same moves.
AGGREGATE_MAX_BYTES = 19_007
AGGREGATE_TARGET_BYTES = 24 * 1024

# Files whose content was deliberately moved OUT of the bootstrap. If a paragraph
# reappears in both places, the split has been undone — usually by someone adding
# "just a reminder" back into AGENTS.md. That is how 84 KiB accumulated.
CANONICAL_DESTINATIONS = [
    "docs/architecture/ui-structure.md",
    "docs/architecture/rtk-integration.md",
    "docs/operations/running-the-stack.md",
    "docs/release-notes-archive.md",
    "docs/stack.md",
]

# A paragraph shorter than this is not evidence of duplication: shared headings,
# table separators and one-line pointers legitimately appear in both files.
DUPLICATE_MIN_BYTES = 240

# Time-boxed waivers. Each needs an ISO date; the gate FAILS once that date has
# passed, so a waiver cannot quietly become permanent. Empty is the good state.
#   {"paragraph_startswith": "...", "reason": "...", "expires": "2026-09-01"}
EXCEPTIONS: list[dict[str, str]] = []


def est_tokens(n_bytes: int) -> int:
    return int(n_bytes / BYTES_PER_TOKEN_ESTIMATE)


def sections(text: str) -> list[tuple[str, int]]:
    """Split a markdown doc into (heading, bytes) pairs, preamble included.

    Per-section figures are what make a regression legible: "the file grew 3 KiB"
    sends you diffing, "section 9 grew 3 KiB" names the culprit.
    """
    import re

    parts = re.split(r"^(#{1,2} .*)$", text, flags=re.M)
    out: list[tuple[str, int]] = []
    if parts[0].strip():
        out.append(("(preamble)", len(parts[0])))
    for i in range(1, len(parts), 2):
        out.append((parts[i].strip(), len(parts[i]) + len(parts[i + 1])))
    return out


def paragraphs(text: str) -> set[str]:
    """Substantial paragraphs, whitespace-normalised so reflowing is not a diff."""
    import re

    out = set()
    for block in re.split(r"\n\s*\n", text):
        norm = re.sub(r"\s+", " ", block).strip()
        if len(norm.encode()) >= DUPLICATE_MIN_BYTES:
            out.add(norm)
    return out


def waived(paragraph: str, today: str) -> tuple[bool, str | None]:
    """Return (is_waived, error). An expired waiver is itself a failure."""
    for exc in EXCEPTIONS:
        if not paragraph.startswith(exc["paragraph_startswith"]):
            continue
        if exc["expires"] < today:
            return False, (
                f"exception expired on {exc['expires']} ({exc['reason']}) — "
                "re-justify it or remove the duplicated content"
            )
        return True, None
    return False, None


def check(repo_root: pathlib.Path) -> int:
    failures: list[str] = []
    total = 0
    print("Context Budget Gate — mandatory agent bootstrap")
    print(f"{'file':<24} {'bytes':>8} {'ceiling':>8} {'target':>8}  {'~tok':>6}  state")

    for entry in BUDGETS:
        path = repo_root / str(entry["path"])
        if not path.is_file():
            # A missing bootstrap file is a real failure: the router would send
            # agents to a dead reference.
            failures.append(f"{entry['path']}: missing")
            continue
        size = path.stat().st_size
        total += size
        ceiling = int(entry["max_bytes"])  # type: ignore[arg-type]
        target = int(entry["target_bytes"])  # type: ignore[arg-type]
        if size > ceiling:
            state = f"OVER by {size - ceiling} B"
            failures.append(
                f"{entry['path']}: {size} B exceeds ceiling {ceiling} B "
                f"(+{size - ceiling}) — {entry['why']}"
            )
        elif size > target:
            state = f"debt {size - target} B to target"
        else:
            # At target, so name the stretch goal instead of going quiet: a file
            # that reports only "at target" is a file that stops improving.
            stretch = entry.get("stretch_bytes")
            state = (
                f"at target, {size - int(stretch)} B to stretch"
                if stretch and size > int(stretch)
                else "at target"
            )
        print(
            f"{str(entry['path']):<24} {size:>8} {ceiling:>8} {target:>8} "
            f" {est_tokens(size):>6}  {state}"
        )

    if total > AGGREGATE_MAX_BYTES:
        failures.append(
            f"aggregate bootstrap {total} B exceeds ceiling "
            f"{AGGREGATE_MAX_BYTES} B (+{total - AGGREGATE_MAX_BYTES})"
        )
    gap = max(0, total - AGGREGATE_TARGET_BYTES)
    print(
        f"{'AGGREGATE':<24} {total:>8} {AGGREGATE_MAX_BYTES:>8} "
        f"{AGGREGATE_TARGET_BYTES:>8}  {est_tokens(total):>6}  "
        + (f"debt {gap} B to target" if gap else "at target")
    )

    # ── Per-section deltas (DoD 6) ─────────────────────────────────
    tier1 = repo_root / "docs/AGENTS.md"
    if tier1.is_file():
        print("\nsections of docs/AGENTS.md, largest first:")
        for heading, n in sorted(sections(tier1.read_text()), key=lambda r: -r[1]):
            print(f"{n:>8} {est_tokens(n):>7}  {heading[:58]}")

    # ── Duplication (DoD 6) ────────────────────────────────────────
    # Content moved out must not reappear in the bootstrap. Compared paragraph by
    # paragraph rather than by title, because content creeps back under a new one.
    from datetime import date

    today = date.today().isoformat()
    if tier1.is_file():
        boot = paragraphs(tier1.read_text())
        for rel in CANONICAL_DESTINATIONS:
            dest = repo_root / rel
            if not dest.is_file():
                continue
            for shared in boot & paragraphs(dest.read_text()):
                is_waived, exc_err = waived(shared, today)
                if exc_err:
                    failures.append(exc_err)
                elif not is_waived:
                    failures.append(
                        f"duplicated with {rel}: {shared[:70]}… — the split was "
                        "undone; keep the pointer, drop the copy"
                    )

    # An expired waiver fails even when nothing is duplicated any more: a stale
    # entry means nobody re-read it, and the next one will be trusted blindly.
    for exc in EXCEPTIONS:
        if exc["expires"] < today:
            failures.append(
                f"exception for {exc['paragraph_startswith'][:40]}… expired on "
                f"{exc['expires']} — remove it or re-date it with a reason"
            )

    if failures:
        print("\nContext budget exceeded:", file=sys.stderr)
        for f in failures:
            print(f"  ✗ {f}", file=sys.stderr)
        print(
            "\nDo not raise a ceiling to make this pass. Move the content to its "
            "canonical source and let the router load it on demand.",
            file=sys.stderr,
        )
        return 1

    print("\n✓ within ceilings — remaining debt to target is printed above")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo-root", default=None)
    args = ap.parse_args()
    root = (
        pathlib.Path(args.repo_root)
        if args.repo_root
        else pathlib.Path(__file__).resolve().parents[3]
    )
    return check(root)


if __name__ == "__main__":
    sys.exit(main())
