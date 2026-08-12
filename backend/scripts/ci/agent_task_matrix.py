#!/usr/bin/env python3
"""Agent task matrix — KT-191 DoD 7.

Shrinking the bootstrap is only safe if every representative task can still reach
what it needs. A prose matrix would assert that; this checks it.

For each task: a probe string that MUST be findable starting from
`docs/AGENTS.md` — either in the file itself (Tier 0/1 rules, which must never
move out) or in a document AGENTS.md links to (Tier 2 packs, loaded on demand).

The router is followed one hop, deliberately: a rule reachable only through a
chain of three documents is not reachable in practice.

This would have caught the regression it was written after — moving 53 KiB of UI
knowledge out while leaving the "Frontend UI changes" router row pointing
elsewhere, so the content existed but no task led to it.

Usage: python3 backend/scripts/ci/agent_task_matrix.py [--repo-root PATH]
Exit 1 naming any task whose probe is unreachable.
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys

# (task, probe, expected home)
#   "tier1" — must be IN docs/AGENTS.md; moving it out would drop a rule.
#   a path  — must live in THAT document, and it must be reachable in one hop.
#
# Naming the expected document matters: an earlier version accepted the probe
# anywhere in the reachable set, so the RTK task passed via a passing mention in
# running-the-stack.md. It would still have passed with rtk-integration.md
# deleted — a green test for a broken route.
MATRIX: list[tuple[str, str, str]] = [
    ("Never fabricate a technical fact", "[src: file:", "tier1"),
    ("Know what to read before acting", "Tiered context loading", "tier1"),
    ("Avoid a known trap", "make typegen", "tier1"),
    ("Regenerate types after a model change", "generated.ts", "tier1"),
    ("Frontend UI change", "Dashboard tabs", "docs/architecture/ui-structure.md"),
    ("Start or stop the stack", "./kronn start", "docs/operations/running-the-stack.md"),
    ("RTK / compression integration", "Mode économique", "docs/architecture/rtk-integration.md"),
    ("Check a dependency version", "rusqlite", "docs/stack.md"),
    ("Workflow engine work", "Key patterns", "docs/architecture/overview.md"),
    ("Look up project jargon", "RTK (Rust Token Killer)", "docs/glossary.md"),
]


def linked_docs(root: pathlib.Path, entry: pathlib.Path) -> dict[str, str]:
    """Documents reachable in ONE hop from the bootstrap, by relative path.

    Both markdown links and backticked `docs/...md` mentions count: the router
    tables use the second form, and an agent follows either.
    """
    text = entry.read_text()
    rels = set(re.findall(r"\]\(([^)]+\.md)[^)]*\)", text))
    rels |= set(re.findall(r"`((?:docs/)?[\w./-]+\.md)`", text))
    out: dict[str, str] = {}
    for rel in rels:
        for cand in (entry.parent / rel, root / rel, root / "docs" / rel):
            cand = cand.resolve()
            if cand.is_file() and root.resolve() in cand.parents:
                out[str(cand.relative_to(root.resolve()))] = cand.read_text()
                break
    return out


def routed(boot: str, rel: str) -> bool:
    """True when a router TABLE row names this document.

    Table rows are the surface an agent consults per task; a mention anywhere else
    in the file (a section stub, a prose aside) does not make a task lead there.
    """
    tail = rel[len("docs/") :] if rel.startswith("docs/") else rel
    return any(
        line.lstrip().startswith("|") and (rel in line or tail in line)
        for line in boot.split("\n")
    )


def check(root: pathlib.Path) -> int:
    entry = root / "docs/AGENTS.md"
    if not entry.is_file():
        print("docs/AGENTS.md missing — the whole router is gone", file=sys.stderr)
        return 1
    boot = entry.read_text()
    reachable = linked_docs(root, entry)

    print("Agent task matrix — can each task still reach what it needs?")
    print(f"router reaches {len(reachable)} document(s) in one hop\n")
    failures: list[str] = []

    for task, probe, where in MATRIX:
        if where == "tier1":
            ok = probe in boot
            note = "in docs/AGENTS.md" if ok else "MISSING from Tier 1"
        elif not routed(boot, where):
            # Reachable somewhere in the file is not enough: an agent looks the task
            # up in the router TABLES. A negative control proved the gap — deleting
            # the destination from the task row still passed, because a leftover
            # section stub kept linking the file.
            ok, note = False, f"no router row names {where}"
        elif where not in reachable:
            ok, note = False, f"{where} named by the router but missing on disk"
        elif probe not in reachable[where]:
            ok, note = False, f"routed, but {where} no longer holds it"
        else:
            ok, note = True, where
        print(f"{'ok ' if ok else 'FAIL'}  {task:<38} {note}")
        if not ok:
            failures.append(
                f"{task}: probe {probe!r} — {note}. Content may exist elsewhere, but "
                "this task no longer leads to it."
            )

    if failures:
        print("\nTask matrix regression:", file=sys.stderr)
        for f in failures:
            print(f"  ✗ {f}", file=sys.stderr)
        print(
            "\nEither the rule belongs back in docs/AGENTS.md, or the router needs a "
            "row naming the document that holds it.",
            file=sys.stderr,
        )
        return 1

    print(f"\n✓ all {len(MATRIX)} tasks reach their content")
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
