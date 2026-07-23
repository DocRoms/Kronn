UPDATE projects
SET created_at = substr(created_at, 1, 10) || 'T' || substr(created_at, 12) || 'Z'
WHERE length(created_at) = 19 AND substr(created_at, 11, 1) = ' ';

UPDATE projects
SET updated_at = substr(updated_at, 1, 10) || 'T' || substr(updated_at, 12) || 'Z'
WHERE length(updated_at) = 19 AND substr(updated_at, 11, 1) = ' ';

UPDATE discussions
SET created_at = substr(created_at, 1, 10) || 'T' || substr(created_at, 12) || 'Z'
WHERE length(created_at) = 19 AND substr(created_at, 11, 1) = ' ';

UPDATE discussions
SET updated_at = substr(updated_at, 1, 10) || 'T' || substr(updated_at, 12) || 'Z'
WHERE length(updated_at) = 19 AND substr(updated_at, 11, 1) = ' ';

UPDATE messages
SET timestamp = substr(timestamp, 1, 10) || 'T' || substr(timestamp, 12) || 'Z'
WHERE length(timestamp) = 19 AND substr(timestamp, 11, 1) = ' ';

UPDATE workflows
SET created_at = substr(created_at, 1, 10) || 'T' || substr(created_at, 12) || 'Z'
WHERE length(created_at) = 19 AND substr(created_at, 11, 1) = ' ';

UPDATE workflows
SET updated_at = substr(updated_at, 1, 10) || 'T' || substr(updated_at, 12) || 'Z'
WHERE length(updated_at) = 19 AND substr(updated_at, 11, 1) = ' ';

UPDATE workflow_runs
SET started_at = substr(started_at, 1, 10) || 'T' || substr(started_at, 12) || 'Z'
WHERE length(started_at) = 19 AND substr(started_at, 11, 1) = ' ';

UPDATE workflow_runs
SET finished_at = substr(finished_at, 1, 10) || 'T' || substr(finished_at, 12) || 'Z'
WHERE finished_at IS NOT NULL
  AND length(finished_at) = 19
  AND substr(finished_at, 11, 1) = ' ';
