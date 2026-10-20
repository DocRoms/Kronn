-- KT-1330: durable origin of a message ('orchestration' = posted by the
-- orchestrator). NULL = ordinary message OR a row stored before this column
-- existed; readers fall back to the legacy id prefix for those.
ALTER TABLE messages ADD COLUMN kind TEXT;
