-- KT-1021 / KT-997 — what an audit run ran against, and what its steps cost.
--
-- `head_sha`, `branch`, `source_fingerprint`: the sources at the run's start, so
-- a resume can tell whether they moved since the run it continues.
-- `model`: the model the run's agent used, when Kronn knows it.
-- `cost_usd_micros`: a step's cost in millionths of a dollar, when the agent
-- reported one. `carried_from_run_id`: the run that actually spent a step a
-- resume inherited. Every column is NULL when unknown — never 0.
ALTER TABLE audit_runs ADD COLUMN head_sha TEXT;
ALTER TABLE audit_runs ADD COLUMN branch TEXT;
ALTER TABLE audit_runs ADD COLUMN source_fingerprint TEXT;
ALTER TABLE audit_runs ADD COLUMN model TEXT;
ALTER TABLE audit_run_steps ADD COLUMN cost_usd_micros INTEGER;
ALTER TABLE audit_run_steps ADD COLUMN carried_from_run_id TEXT;
