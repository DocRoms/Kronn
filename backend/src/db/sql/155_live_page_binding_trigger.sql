-- v0.12.0 — Live Page ↔ workflow trigger authorization (Phase 4).
--
-- Phase 3 let a Page decide the gates of a bound run (write path). Phase 4 adds
-- the other write: triggering the bound workflow from the Page (e.g. a button
-- that spawns a run). Authorization is bounded exactly like `allowed_gate_steps`:
-- the Page may only trigger the workflow it is bound to, and may only pass launch
-- variables listed here.
--
-- NULL          → triggering is NOT allowed from the Page (mirror/gate only).
-- '[]'          → triggering allowed, but no launch variable may be passed.
-- '["a","b"]'   → triggering allowed; provided variables must be a subset.
ALTER TABLE live_page_workflow_bindings
    ADD COLUMN trigger_variable_allowlist_json TEXT;
