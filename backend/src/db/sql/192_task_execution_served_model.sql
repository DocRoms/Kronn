-- The model a worker's runtime reported serving, apart from the requested
-- `worker_model`. NULL until one is observed.
ALTER TABLE task_executions ADD COLUMN worker_served_model TEXT;
