#!/usr/bin/env python3
"""Controlled A/B/C/D Token Economics benchmark (KT-192, KT-198).

The benchmark gives Claude Code and Codex the same small engineering case with
four context-delivery strategies.  It records the CLIs' native usage counters,
wall-clock duration and a deterministic answer score; prompt and answer bodies
never enter the JSON report.

This is deliberately a replay, not an agent that edits the checkout.  It makes
the variants comparable and safe to repeat while still charging every provider
for its real first-turn bootstrap and configured runtime.
"""

from __future__ import annotations

import argparse
import json
import math
import statistics
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


VARIANTS = ("A", "B", "C", "D")
# Calibrated against the reference room on 2026-08-12 (452,876 B of main
# messages). 180 KiB is intentionally conservative: it represents a long-lived
# session without embedding or exporting any real conversation content.
LONG_SESSION_CONTEXT_BYTES = 180_000
EXPECTED = {
    "cause": "timeout_returned_to_model",
    "file": "backend/scripts/disc-introspection-mcp.py",
    "test": "silent_room_produces_zero_model_turns",
    "decision": "keep_wait_bridge_side",
}
OUTPUT_SCHEMA = {
    "type": "object",
    "additionalProperties": False,
    "properties": {key: {"type": "string"} for key in EXPECTED},
    "required": list(EXPECTED),
}


@dataclass(frozen=True)
class Run:
    provider: str
    variant: str
    duration_ms: int
    raw_traffic_tokens: int
    non_cached_input_tokens: int
    cache_read_tokens: int
    cache_write_tokens: int | None
    output_tokens: int
    quality_score: int
    success: bool

    def export(self) -> dict[str, Any]:
        return self.__dict__.copy()


def _case() -> str:
    return (
        "Case TE-01. A quiet cross-agent room repeatedly wakes a model. "
        "Canonical evidence fields: "
        "cause=timeout_returned_to_model; "
        "file=backend/scripts/disc-introspection-mcp.py; "
        "test=silent_room_produces_zero_model_turns; "
        "decision=keep_wait_bridge_side. "
        "In words: disc_wait_for_peer returns its normal timeout to the model, "
        "which then calls the same tool again; the accepted decision keeps the "
        "wait bridge-side until a real message arrives."
    )


def build_prompt(variant: str) -> str:
    """Build equivalent evidence using the four release-gate strategies."""
    if variant not in VARIANTS:
        raise ValueError(f"unknown variant: {variant}")

    case = _case()
    historical_turn = (
        "Historical turn: the team discussed unrelated UI polish, packaging, "
        "localisation and release notes. No decision in this turn supersedes "
        "the TE-01 evidence."
    )
    rows: list[str] = []
    size = 0
    while size < LONG_SESSION_CONTEXT_BYTES:
        row = f"{historical_turn} Sequence {len(rows)}."
        rows.append(row)
        size += len(row.encode()) + 1
    history = "\n".join(rows)
    polls = "\n".join(
        f"Poll {i}: disc_wait_for_peer timed out; model was told to poll again."
        for i in range(24)
    )
    resume = (
        "Bounded resume bundle:\n"
        "objective=eliminate model turns in a quiet room\n"
        "task=TE-01\n"
        f"evidence={case}\n"
        "blockers=none\n"
    )
    quick_exec = (
        "Quick Exec proof: targeted regression test currently fails 1/1 before "
        "the decision and passes 1/1 when the timeout stays bridge-side.\n"
        "Review ledger delta: one open cause TE-01; no unrelated files changed.\n"
    )

    if variant == "A":
        context = f"Long session with model repolling:\n{history}\n{polls}\n{case}"
    elif variant == "B":
        context = f"Long session, wait removed from the model loop:\n{history}\n{case}"
    elif variant == "C":
        context = resume
    else:
        context = resume + quick_exec

    return (
        f"{context}\n\n"
        "Return only the requested JSON. Identify the canonical cause, source "
        "file, regression test and accepted decision. Do not use tools."
    )


def score(answer: dict[str, Any]) -> int:
    return sum(answer.get(key) == value for key, value in EXPECTED.items())


def _run_claude(prompt: str, schema_path: Path, timeout: int) -> tuple[dict, dict, int]:
    started = time.monotonic()
    completed = subprocess.run(
        [
            "claude", "-p", prompt, "--output-format", "json", "--tools", "",
            "--no-session-persistence", "--json-schema", schema_path.read_text(),
        ],
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
    )
    duration_ms = round((time.monotonic() - started) * 1000)
    if completed.returncode != 0:
        raise RuntimeError(f"claude exited {completed.returncode}: {completed.stderr[-500:]}")
    envelope = json.loads(completed.stdout)
    answer = envelope.get("structured_output")
    if not isinstance(answer, dict):
        answer = json.loads(envelope["result"])
    return answer, envelope["usage"], duration_ms


def _run_codex(prompt: str, schema_path: Path, timeout: int) -> tuple[dict, dict, int]:
    started = time.monotonic()
    completed = subprocess.run(
        [
            "codex", "exec", "--json", "--ephemeral", "--ignore-user-config",
            "--ignore-rules", "--sandbox", "read-only", "--output-schema",
            str(schema_path), prompt,
        ],
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
    )
    duration_ms = round((time.monotonic() - started) * 1000)
    if completed.returncode != 0:
        raise RuntimeError(f"codex exited {completed.returncode}: {completed.stderr[-500:]}")
    answer: dict[str, Any] | None = None
    usage: dict[str, Any] | None = None
    for line in completed.stdout.splitlines():
        event = json.loads(line)
        if event.get("type") == "item.completed":
            item = event.get("item") or {}
            if item.get("type") == "agent_message":
                answer = json.loads(item["text"])
        elif event.get("type") == "turn.completed":
            usage = event.get("usage")
    if answer is None or usage is None:
        raise RuntimeError("codex output omitted the final answer or usage")
    return answer, usage, duration_ms


def execute(provider: str, variant: str, schema_path: Path, timeout: int) -> Run:
    prompt = build_prompt(variant)
    if provider == "claude":
        answer, usage, duration = _run_claude(prompt, schema_path, timeout)
        non_cached = int(usage.get("input_tokens", 0))
        cache_read = int(usage.get("cache_read_input_tokens", 0))
        cache_write: int | None = int(usage.get("cache_creation_input_tokens", 0))
        output = int(usage.get("output_tokens", 0))
    elif provider == "codex":
        answer, usage, duration = _run_codex(prompt, schema_path, timeout)
        total_input = int(usage.get("input_tokens", 0))
        cache_read = int(usage.get("cached_input_tokens", 0))
        non_cached = max(0, total_input - cache_read)
        cache_write_value = usage.get("cache_write_input_tokens")
        cache_write = int(cache_write_value) if cache_write_value is not None else None
        output = int(usage.get("output_tokens", 0))
    else:
        raise ValueError(f"unsupported provider: {provider}")
    raw = non_cached + cache_read + (cache_write or 0) + output
    quality = score(answer)
    return Run(
        provider=provider,
        variant=variant,
        duration_ms=duration,
        raw_traffic_tokens=raw,
        non_cached_input_tokens=non_cached,
        cache_read_tokens=cache_read,
        cache_write_tokens=cache_write,
        output_tokens=output,
        quality_score=quality,
        success=quality == len(EXPECTED),
    )


def percentile(values: list[int], pct: float) -> int:
    """Nearest-rank percentile, deterministic for the small benchmark sample."""
    if not values:
        raise ValueError("cannot calculate a percentile of no values")
    ordered = sorted(values)
    return ordered[max(0, math.ceil(pct * len(ordered)) - 1)]


def summarise(runs: list[Run]) -> dict[str, Any]:
    groups: dict[str, dict[str, Any]] = {}
    for provider in sorted({run.provider for run in runs}):
        groups[provider] = {}
        for variant in VARIANTS:
            selected = [run for run in runs if run.provider == provider and run.variant == variant]
            if not selected:
                continue
            groups[provider][variant] = {
                "runs": len(selected),
                "success_rate_pct": round(100 * sum(run.success for run in selected) / len(selected), 2),
                "quality_median": statistics.median(run.quality_score for run in selected),
                "raw_traffic_tokens_median": statistics.median(run.raw_traffic_tokens for run in selected),
                "raw_traffic_tokens_p90": percentile([run.raw_traffic_tokens for run in selected], 0.9),
                "duration_ms_median": statistics.median(run.duration_ms for run in selected),
                "duration_ms_p90": percentile([run.duration_ms for run in selected], 0.9),
            }
        a = groups[provider].get("A")
        d = groups[provider].get("D")
        if a and d:
            groups[provider]["comparison"] = {
                "median_raw_traffic_reduction_pct": round(
                    100 * (a["raw_traffic_tokens_median"] - d["raw_traffic_tokens_median"])
                    / a["raw_traffic_tokens_median"],
                    2,
                ),
                "p90_duration_change_pct": round(
                    100 * (d["duration_ms_p90"] - a["duration_ms_p90"])
                    / a["duration_ms_p90"],
                    2,
                ),
                "quality_not_degraded": d["quality_median"] >= a["quality_median"],
            }
    return groups


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--provider", choices=("claude", "codex", "all"), default="all")
    parser.add_argument("--repetitions", type=int, default=5)
    parser.add_argument("--timeout", type=int, default=180)
    parser.add_argument("--json", dest="json_path", type=Path)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    if args.repetitions < 1:
        parser.error("--repetitions must be positive")

    if args.dry_run:
        print(json.dumps({variant: len(build_prompt(variant).encode()) for variant in VARIANTS}))
        return 0

    providers = ("claude", "codex") if args.provider == "all" else (args.provider,)
    runs: list[Run] = []
    with tempfile.TemporaryDirectory(prefix="kronn-token-economics-") as tmp:
        schema_path = Path(tmp) / "answer.schema.json"
        schema_path.write_text(json.dumps(OUTPUT_SCHEMA), encoding="utf-8")
        for repetition in range(args.repetitions):
            for provider in providers:
                for variant in VARIANTS:
                    run = execute(provider, variant, schema_path, args.timeout)
                    runs.append(run)
                    print(
                        f"{provider} {variant} run {repetition + 1}: "
                        f"raw={run.raw_traffic_tokens} duration={run.duration_ms}ms "
                        f"quality={run.quality_score}/4"
                    )

    report = {
        "schema_version": "1.0.0",
        "method": "controlled-replay",
        "repetitions": args.repetitions,
        "privacy": "aggregate counters only; prompts and answers are not exported",
        "variants": {
            "A": "long session plus model-visible polling",
            "B": "long session without model repolling",
            "C": "fresh session hydrated from a bounded resume bundle",
            "D": "C plus bounded Quick Exec proof and review-ledger delta",
        },
        "summary": summarise(runs),
        "runs": [run.export() for run in runs],
    }
    rendered = json.dumps(report, indent=2, ensure_ascii=False) + "\n"
    if args.json_path:
        args.json_path.write_text(rendered, encoding="utf-8")
    else:
        print(rendered)
    return 0 if all(run.success for run in runs) else 1


if __name__ == "__main__":
    raise SystemExit(main())
