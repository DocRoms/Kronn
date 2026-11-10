-- KT-1656: a task worker printed ~/.gemini/settings.json, so API tokens reached
-- the transcript of its sub-discussion. The masking of what is already stored
-- runs in Rust (migrations.rs -> db/credential_scrub.rs `scrub_known_shapes`),
-- because it needs Kronn's redaction patterns: vendor-shaped tokens only, in every
-- column that can carry agent output, and no discussion is named. The full audit
-- redaction of one discussion is an explicit owner action
-- (POST /api/maintenance/redact-discussion), not part of this migration. This
-- file only gives the runner a receipt and the rollback snapshot it takes first.
SELECT 1;
