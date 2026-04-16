- **ID**: TD-20260417-audit-monolith
- **Area**: Backend
- **Severity**: Medium (risky to touch, 10-step SSE pipeline with state machine)

## Problem (fact)
`backend/src/api/audit.rs` is ~1966 lines containing the full AI-audit lifecycle: 10-step SSE pipeline, drift detection, validation dialog, briefing, template install, cancel/status endpoints. Mixing handler glue with complex synchronization (cancel tokens, per-project locks, partial progress writes) in one file makes changes risky — any tweak to one sub-feature risks breaking another.

## Impact
- Test friction: mocking the full file for a targeted test pulls in briefing, drift, and validation code paths.
- Review cost: every audit-pipeline PR touches the same 2000-line file.
- Cognitive load: `run_audit`, `full_audit`, `partial_audit`, `validate_audit`, `start_briefing` live side-by-side with helpers and state logic; hard to see which feature uses what.

## Why we can't fix now (constraint)
Unlike models, audit.rs has **logic coupling**: cancel tokens and SSE senders are shared state between handlers and helpers. A clean split requires moving some of that state into a shared `AuditEngine` struct first, which is a non-trivial design decision. Better done in a focused session after the design is agreed.

## Where (pointers)
- `backend/src/api/audit.rs` (~1966L)
- Handler fns: `run_audit` (239), `audit_info` (403), `check_drift` (426), `partial_audit` (462), `validate_audit` (646), `mark_bootstrapped` (694), `full_audit` (959), `cancel_audit` (1423), `get_briefing` (1534), `set_briefing` (1548), `start_briefing` (1565), `audit_status` (1811)
- Pure helpers: `compute_audit_info_sync` (742), `detect_issue_tracker_mcp` (1403), `remove_bootstrap_block` (1671)
- State dependencies: `AppState.cancel_registry`, per-project audit locks, SSE broadcast channel

## Suggested direction (non-binding)
Split into a sub-directory:

```
backend/src/api/audit/
├── mod.rs              # route dispatcher + shared helpers
├── pipeline.rs         # run_audit, full_audit (the 10-step engine)
├── drift.rs            # check_drift, partial_audit, drift-related helpers
├── validation.rs       # validate_audit, mark_bootstrapped
├── briefing.rs         # get/set/start briefing
├── status.rs           # audit_info, audit_status, cancel_audit
└── helpers.rs          # compute_audit_info_sync, detect_issue_tracker_mcp,
                        # remove_bootstrap_block, any pure utilities
```

### Prerequisite refactor
Before splitting, extract an `AuditEngine` (or similar) that holds:
- Cancel token registry (keyed by project_id)
- Per-project audit lock to prevent concurrent runs
- SSE broadcast channel

Handlers then take `&AuditEngine` through `AppState`, and each sub-module is a thin wrapper over engine methods — no shared mutable state leaking across files.

### Pure-helper quick win
Even without the full split, the 3 pure helpers (`compute_audit_info_sync`, `detect_issue_tracker_mcp`, `remove_bootstrap_block`) can be extracted to `audit_helpers.rs` with unit tests — same pattern as `disc_helpers.rs`. ~30 min, low risk.

## Next step
1. **Quick win (safe, ~30 min)**: extract the 3 pure helpers to `audit_helpers.rs` with unit tests. Mirrors the disc_helpers pattern proven on 2026-04-17.
2. **Full split (~2-3h)**: design `AuditEngine` abstraction first, then move handlers into sub-modules. Requires user sign-off on the engine API.
