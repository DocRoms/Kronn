-- Durable health memory for proxy-declared LiteLLM models. `/v1/models` can
-- advertise aliases that the upstream project/region is not allowed to call;
-- remembering invocation failures prevents users from repeatedly selecting a
-- known-bad entry while still allowing an explicit retry.
CREATE TABLE IF NOT EXISTS lite_llm_model_failures (
    endpoint TEXT NOT NULL,
    model TEXT NOT NULL,
    status_code INTEGER NOT NULL,
    error_message TEXT NOT NULL,
    first_failed_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_failed_at TEXT NOT NULL DEFAULT (datetime('now')),
    failure_count INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (endpoint, model)
);

CREATE INDEX IF NOT EXISTS idx_lite_llm_model_failures_recent
    ON lite_llm_model_failures(endpoint, last_failed_at DESC);
