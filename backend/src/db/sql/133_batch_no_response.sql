-- A child that terminates without any durable Agent reply is observably
-- different from a child that returned an explicit error. It remains part of
-- batch_failed, while this subset counter lets summaries explain the silence.
ALTER TABLE workflow_runs
ADD COLUMN batch_no_response INTEGER NOT NULL DEFAULT 0;
