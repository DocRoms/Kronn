-- KT-1103 — a cron or watch trigger without a timezone now follows Kronn's
-- global zone. Existing ones were written in UTC: pin them to UTC explicitly
-- so their firing times do not move on upgrade.
UPDATE workflows
SET trigger_json = json_set(trigger_json, '$.timezone', 'UTC')
WHERE json_valid(trigger_json)
  AND json_extract(trigger_json, '$.type') IN ('Cron', 'Watch')
  AND json_extract(trigger_json, '$.timezone') IS NULL;
