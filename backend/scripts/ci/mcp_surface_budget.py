#!/usr/bin/env python3
"""MCP surface budget — KT-192.

The tool catalogue is injected into every agent session before a single word of
work is exchanged. Measured at 96 972 B (~26 200 tokens at 3.7 B/token), it was
five times the whole documentation bootstrap that KT-191 spent a day shrinking.
Nothing was watching it, so it grew.

Same ratchet as `context_budget.py`, for the same reason: a ceiling that may be
tightened when a reduction lands and NEVER raised to make a build pass. Raising
it is how the 84 KiB AGENTS.md happened — one defensible paragraph at a time.

Bytes are exact. Token figures are estimates and gate nothing.

Usage: python3 backend/scripts/ci/mcp_surface_budget.py [--repo-root PATH]
Exit 1 when the catalogue, or any single declaration, exceeds its ceiling.
"""

from __future__ import annotations

import argparse
import ast
import json
import pathlib
import sys

BRIDGE = "backend/scripts/disc-introspection-mcp.py"

# Whole `tools/list` payload, serialised as the client receives it.
#
# The measurement serialises with `ensure_ascii=True`, exactly like the wire:
# every accented character in a French description travels as a 6-byte
# `\uXXXX` escape, not as its 2 bytes of UTF-8. The ceiling is pinned to this
# tree's 95-tool payload after moving reference material to on-demand manuals
# and keeping field descriptions to the information needed at selection time.
# It carries no slack: lower it whenever the catalogue shrinks, never raise it
# to make a build pass.
CATALOGUE_MAX_BYTES = 89_588

# Per-declaration ceiling. The five heaviest tools were 29% of the catalogue for
# 6% of the tools; their descriptions had grown into manuals. A per-tool cap is
# what stops one tool from quietly becoming a book.
#
# Also corrected: this used to add raw `description` bytes to a separately
# serialised schema, which counted neither the JSON envelope nor the escaping. It
# now measures the whole declaration exactly as sent.
DECLARATION_MAX_BYTES = 4_392

# Tools allowed above DECLARATION_MAX_BYTES, with the reason. An entry here is a
# debt acknowledged in writing, not an exemption to forget: shrink the tool and
# delete the line. Empty on purpose — the ceiling is pinned to today's heaviest
# declaration, so no tool needs one yet, and a waiver that never fires would only
# imply a debt that does not exist.
DECLARATION_WAIVERS: dict[str, str] = {}

BYTES_PER_TOKEN = 3.7


def load_tools(root: pathlib.Path) -> list[dict]:
    """Read TOOLS without importing the bridge.

    Importing would run module-level setup and need the whole environment; the
    catalogue is literal data, so parsing is both cheaper and safer. JSON Schema
    bounds may reference scalar constants declared before ``TOOLS``; resolve
    those names without executing the module.
    """
    source = (root / BRIDGE).read_text()
    tree = ast.parse(source)

    class LiteralNameResolver(ast.NodeTransformer):
        def __init__(self, values: dict[str, object]) -> None:
            self.values = values

        def visit_Name(self, node: ast.Name) -> ast.AST:
            if isinstance(node.ctx, ast.Load) and node.id in self.values:
                return ast.copy_location(ast.Constant(self.values[node.id]), node)
            return node

    literals: dict[str, object] = {}
    for node in tree.body:
        if not isinstance(node, ast.Assign):
            continue
        names = [target.id for target in node.targets if isinstance(target, ast.Name)]
        resolved = LiteralNameResolver(literals).visit(node.value)
        if "TOOLS" in names:
            try:
                return ast.literal_eval(resolved)
            except (ValueError, TypeError) as exc:
                raise SystemExit(
                    f"TOOLS in {BRIDGE} must contain only literals or references "
                    f"to prior scalar constants: {exc}"
                ) from exc
        if len(names) != 1:
            continue
        try:
            value = ast.literal_eval(resolved)
        except (ValueError, TypeError):
            continue
        if isinstance(value, (str, int, float, bool, type(None))):
            literals[names[0]] = value
    raise SystemExit(f"TOOLS literal not found in {BRIDGE}")


def wire_bytes(value: object) -> int:
    """Bytes this value occupies on the wire.

    `ensure_ascii=True` is not a stylistic choice: it is what the transport sends,
    so an accented character costs 6 bytes as `\\uXXXX` rather than its 2 bytes of
    UTF-8. Measuring the other way under-reported the Kronn catalogue by 792 B.
    """
    return len(json.dumps(value, ensure_ascii=True).encode())


def declaration_bytes(tool: dict) -> tuple[int, int, int]:
    """Description, schema and the WHOLE declaration, as sent.

    The total is measured on the complete object rather than summed from its
    parts, because the parts omit the JSON envelope — the `name`, the keys, the
    braces. That envelope is 45 B on the heaviest tool and is really transmitted.
    """
    description = wire_bytes(tool.get("description", ""))
    schema = wire_bytes(tool.get("inputSchema", {}))
    return description, schema, wire_bytes(tool)


def check(root: pathlib.Path) -> int:
    tools = load_tools(root)
    # Two figures, and the distinction is not cosmetic.
    #
    # `declarations` is the sum per tool — the unit that is comparable across
    # servers, and what `mcp_catalogue_census.py` reports.
    #
    # `catalogue` is the whole `tools/list` result, wrapper and separators
    # included. Those 181 B are really transmitted, so this is the figure the
    # ceiling gates on: an agent pays for the envelope too.
    declarations = sum(wire_bytes(tool) for tool in tools)
    catalogue = wire_bytes({"tools": tools})
    failures: list[str] = []

    print(f"MCP catalogue: {len(tools)} tools, {catalogue} B "
          f"(~{catalogue // BYTES_PER_TOKEN:.0f} tokens, estimate)")
    print(f"  declarations {declarations} B + {catalogue - declarations} B envelope")
    print(f"ceiling: {CATALOGUE_MAX_BYTES} B\n")

    if catalogue > CATALOGUE_MAX_BYTES:
        failures.append(
            f"catalogue is {catalogue} B, ceiling {CATALOGUE_MAX_BYTES} B "
            f"(+{catalogue - CATALOGUE_MAX_BYTES}). Move reference material to an "
            "on-demand tool or document; do not raise the ceiling."
        )

    heaviest = sorted(tools, key=lambda t: -declaration_bytes(t)[2])
    print(f"{'tool':<28}{'desc':>7}{'schema':>8}{'wire':>8}")
    for tool in heaviest[:8]:
        description, schema, total = declaration_bytes(tool)
        print(f"{tool['name']:<28}{description:>7}{schema:>8}{total:>8}")

    for tool in tools:
        description, schema, total = declaration_bytes(tool)
        if total <= DECLARATION_MAX_BYTES:
            continue
        waiver = DECLARATION_WAIVERS.get(tool["name"])
        if waiver:
            print(f"\nwaived: {tool['name']} at {total} B — {waiver}")
            continue
        failures.append(
            f"{tool['name']} declares {total} B (desc {description} + schema {schema}), "
            f"ceiling {DECLARATION_MAX_BYTES} B. A description this long is a manual: "
            "keep the contract and the run-breaking traps, move the rest behind a tool "
            "the caller invokes only when it needs them."
        )

    if failures:
        print("\nMCP surface budget exceeded:", file=sys.stderr)
        for failure in failures:
            print(f"  ✗ {failure}", file=sys.stderr)
        print(
            "\nThe ceilings in this file may be LOWERED when a reduction lands. "
            "Raising one to go green is the failure mode it exists to prevent.",
            file=sys.stderr,
        )
        return 1

    print(f"\n✓ catalogue within {CATALOGUE_MAX_BYTES} B; every declaration within budget")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", default=None)
    args = parser.parse_args()
    root = (
        pathlib.Path(args.repo_root)
        if args.repo_root
        else pathlib.Path(__file__).resolve().parents[3]
    )
    return check(root)


if __name__ == "__main__":
    sys.exit(main())
