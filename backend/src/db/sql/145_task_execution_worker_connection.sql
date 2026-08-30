ALTER TABLE task_executions
    ADD COLUMN worker_connection_id TEXT
        REFERENCES external_api_connections(id);

