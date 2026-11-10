-- KT-1656: a task worker printed ~/.gemini/settings.json, so API tokens reached
-- the transcript of its sub-discussion (dcabb259…). The masking of what is
-- already stored runs in Rust (migrations.rs, `scrub_exposed_credentials`),
-- because it needs Kronn's redaction patterns; this row only records that it
-- ran, with the rollback snapshot the migration runner takes before it.
SELECT 1;
