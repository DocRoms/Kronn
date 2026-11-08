-- KT-1564 — the providers that refused an execution during one quota fallback
-- chain. A refusal that announced no reset leaves nothing a global clock can
-- test, so without this memory a chain of fallbacks among three providers
-- (Codex refuses, then Gemini, then Codex again...) spins forever, one dispatch
-- and one handoff per refusal. `reset_at` NULL means "unavailable until the
-- execution is back on its original worker"; the rows go with the origin row
-- (a human reassignment, or the return itself).
CREATE TABLE IF NOT EXISTS task_execution_quota_refusals (
    task_execution_id TEXT NOT NULL
                        REFERENCES task_executions(id) ON DELETE CASCADE,
    provider          TEXT NOT NULL,
    reset_at          TEXT,
    PRIMARY KEY (task_execution_id, provider)
);
