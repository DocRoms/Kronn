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
# tree's 110-tool payload. `task_unlink_discussion` was added under it rather
# than over it: the four heaviest descriptions it pushed past the line were
# tightened by nearly what the new tool costs, so one tool was added for six
# bytes rather than for its own weight.
# It carries no slack: lower it whenever the catalogue shrinks, never raise it
# to make a build pass.
# KT-546: compact media guidance retains the billing/slot/wait contract and
# removes stale numeric examples. Pin the smaller measured payload, no slack.
# KT-663 — the one deliberate raise, and it is +7 B over the 86 824 it started
# from. `media_generate` now says where its values come from (`agent_list`
# capabilities) and that a reference picture may be an earlier clip's last
# image. Nearly all of it was paid for inside the same declaration, and the
# envelope, reference-ordering and cost detail moved to `tool_manual({tool:
# "media_generate"})`, which this budget does not measure. The remaining seven
# bytes buy a capability agents had no way to learn about; the operator
# approved them explicitly. This does not reopen the rule above — the next
# addition pays for itself or moves to a manual, as this one almost entirely
# did.
# KT-670: `audit_launch` dropped a top-level `allOf` the OpenAI wire refuses.
# Ceiling lowered by what it weighed.
# KT-678: `page_update_html` now points at its own manual for Page buttons, the
# tool that adds them having said nothing about them. The description was
# rewritten to carry the pointer and still weighs 9 B less; ceiling lowered.
# `disc_wait_for_peer` now says a later Kronn call ends a backgrounded wait, in
# 12 B less than the text it corrects; ceiling lowered.
# KT-790/KT-791: the principal-notice and compact-status rewrites net 10 B less;
# ceiling lowered.
# KT-796: `workflow_create_draft` names TriggerWorkflow and separates the step
# types with commas (a `·` costs 6 B on the wire); ceiling lowered by the 49 B.
# KT-837: worker-offer room semantics moved to the on-demand manual; ceiling
# lowered by the resulting 25 B.
# KT-839: replacing an execution's validations is a `validations` change on
# `task_exec_reassign`, not a tool of its own (a declaration of its own cost
# 666 B), and `task_exec_prepare` gained a `validations` input. The remainder was
# paid for inside the same family: `task_exec_deliver` lost the sentences its
# parameters and manual already carry. Ceiling lowered by 1 B, not raised.
# KT-1025: `workflow_trigger` gained `step_agents`, paid for inside its own
# declaration (the agent detail lives in its manual); ceiling lowered by 6 B.
# KT-1037: enabling became human-only, so `workflow_set_enabled` lost its
# `force` input and its enable guidance; ceiling lowered by 400 B.
# KT-1098: `page_update_html` gained `slug` (rename), paid for inside its own
# declaration; ceiling unchanged.
# KT-1128: `skill_create` and `skill_update` gained `project_id`, paid for by
# dropping the false claim that an update changes the skill's id; ceiling
# lowered by 9 B.
# KT-1100: retention became human-only on a stored workflow, so
# `workflow_update` lost its `retention` input (its shortened description
# stays). That removal took back the 109 B the input had added after
# KT-1138's 85_354; net zero, ceiling unchanged.
# Keep the declaration budget at the measured payload size. Extended contracts
# are loaded through tool_manual and are excluded from this wire-size budget.
# KT-1138: `workflow_validate` (a workflow's ready/blockers verdict) was paid
# for by tightening the workflow_list/get/clone/update descriptions; the
# verdict guidance itself travels in tool results and the skill. Ceiling
# lowered by 372 B (85_726 → 85_354 after the rebase).
# KT-1139: `workflow_update_step` and a one-step `workflow_get` (large
# workflows are edited step by step) were paid for by tightening
# workflow_step_schema/active_runs/runs/run_discussions; ceiling lowered by
# 6 B (85_354 -> 85_348). Re-measured on the 0.15.0 integration head
# (KT-1138 + KT-1100 + KT-1139 + KT-800): exactly 85_348 B.
# KT-697: `agent_list` says reachability is probed, in 5 B less; re-measured
# on the 0.15.0 integration head: exactly 85_343 B.
# KT-1104: `page_delete_dataset` was paid for by tightening the page family
# and `convention_get` descriptions; re-measured on the 0.15.0 integration
# head: exactly 85_341 B.
# KT-1210: audit_launch gains `fresh` and `retry_steps`, paid for by tightening
# both audit descriptions; ceiling lowered by 73 B (85_341 -> 85_268).
# KT-1205: the template update/fill flow lives in the audit_prepare manual;
# compact audit descriptions lower the catalogue by 165 B.
# KT-1205 + KT-1218: preserve both template and Verify docs guidance, measured
# after integration and shortening repeated descriptions.
# KT-154: `task_assign` (assign yourself or another agent, `unassign:true` to
# remove; the contract lives in its tool_manual) was paid for by tightening the
# qa_list, disc_link, disc_transfer_session, disc_find_by_session,
# disc_workspace_set and task_exec_review descriptions; ceiling lowered by 6 B
# (85_095 -> 85_089).
# KT-1254: `task_update_dod` takes `evidence` and `task_exec_review` names the
# post-integration live check (a DoD the principal verifies after integration,
# never an approve blocker; `task_create` cannot declare one, a new task has no
# execution); paid for by dropping what `task_exec_review`'s `decision` schema
# already said and tightening its identity sentence, ceiling lowered by 86 B.
# KT-1262: `task_exec_deliver` and `task_exec_review` name the fields the
# backend requires (`tests_executed`, `reviewed_head_sha`, `dod_verifications`,
# the string version "1"). The spawned worker's `tests_executed` schema is now
# declared inside the TOOLS literal (the projection in `_visible_tools` reuses
# it), so this gate measures it; the disc_link, disc_find_by_session,
# disc_transfer_session and disc_workspace_set descriptions were tightened to
# pay for it. Pinned to the payload measured on the merged tree, 84_967 B
# (85_003 -> 84_967).
# KT-1267: `task_exec_cancel` says the task status is left alone and how to
# relaunch, inside the bytes of the text it replaces; ceiling lowered by 1 B
# (84_967 -> 84_966).
# KT-1329: `disc_append` names the `kronn-question` card for a human decision
# (the contract stays in `tool_manual disc_question_list`); paid for by
# tightening its alias sentence and four schema descriptions, ceiling lowered
# by 11 B (84_966 -> 84_955) and the heaviest declaration by 11 B.
# KT-1270: `room_scope` (a room's project reach, and the human-decided request
# for more) was paid for by moving the retry-key detail of `task_create` and
# the `time.now` grammar of `qa_create_draft` into their manuals, which already
# carried or now carry them; the catalogue nets 37 B less (84_966 -> 84_929).
# Pinned to the payload measured on the tree merging both (84_918 B).
CATALOGUE_MAX_BYTES = 84_918

# Per-declaration ceiling. The five heaviest tools were 29% of the catalogue for
# 6% of the tools; their descriptions had grown into manuals. A per-tool cap is
# what stops one tool from quietly becoming a book.
#
# Also corrected: this used to add raw `description` bytes to a separately
# serialised schema, which counted neither the JSON envelope nor the escaping. It
# now measures the whole declaration exactly as sent.
DECLARATION_MAX_BYTES = 2_746

# Tools allowed above DECLARATION_MAX_BYTES, with the reason. An entry here is a
# debt acknowledged in writing, not an exemption to forget: shrink the tool and
# delete the line. Empty on purpose — the ceiling is pinned to today's heaviest
# declaration, so no tool needs one yet, and a waiver that never fires would only
# imply a debt that does not exist.
DECLARATION_WAIVERS: dict[str, str] = {}

BYTES_PER_TOKEN = 3.7


CATALOGUE_MUTATORS = {"append", "extend", "insert", "__iadd__"}


def reject_catalogue_mutations(tree: ast.Module) -> None:
    """The gate measures the TOOLS literal, so the literal must be the catalogue.

    A tool appended anywhere else (`TOOLS.extend([...])` after `main()`) reaches
    `tools/list` while the measured bytes stay green.
    """
    assignments = 0
    for node in ast.walk(tree):
        if isinstance(node, (ast.Assign, ast.AnnAssign, ast.AugAssign)):
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            for target in targets:
                if isinstance(target, ast.Name) and target.id == "TOOLS":
                    if isinstance(node, ast.AugAssign):
                        raise SystemExit(
                            f"TOOLS in {BRIDGE} is extended with `+=` at line {node.lineno}: "
                            "declare every tool inside the TOOLS literal"
                        )
                    assignments += 1
                elif (
                    isinstance(target, ast.Subscript)
                    and isinstance(target.value, ast.Name)
                    and target.value.id == "TOOLS"
                ):
                    raise SystemExit(
                        f"TOOLS in {BRIDGE} is modified by index at line {node.lineno}: "
                        "declare every tool inside the TOOLS literal"
                    )
        elif (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id == "TOOLS"
            and node.func.attr in CATALOGUE_MUTATORS
        ):
            raise SystemExit(
                f"TOOLS in {BRIDGE} is changed by `TOOLS.{node.func.attr}` at line "
                f"{node.lineno}: declare every tool inside the TOOLS literal so the "
                "budget measures what tools/list serves"
            )
    if assignments > 1:
        raise SystemExit(f"TOOLS in {BRIDGE} is assigned {assignments} times: keep one literal")


def load_tools(root: pathlib.Path) -> list[dict]:
    """Read TOOLS without importing the bridge.

    Importing would run module-level setup and need the whole environment; the
    catalogue is literal data, so parsing is both cheaper and safer. JSON Schema
    bounds may reference scalar constants declared before ``TOOLS``; resolve
    those names without executing the module.
    """
    source = (root / BRIDGE).read_text()
    tree = ast.parse(source)
    reject_catalogue_mutations(tree)

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
