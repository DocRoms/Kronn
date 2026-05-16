-- 0.8.5 — Backfill v1 snapshot for QPs created before migration 058.
--
-- Migration 058 added the snapshot table + the lineage columns but
-- didn't seed historical QPs (only NEW inserts via
-- `insert_quick_prompt` get auto-seeded). Without this backfill, every
-- pre-0.8.5 QP shows "0 versions" forever in the history drawer.
--
-- The INSERT is gated by `NOT EXISTS` against `quick_prompt_versions`
-- so the migration stays idempotent — running it on a fresh DB (where
-- 058 already created the snapshots via new inserts) writes zero rows.
--
-- `lower(hex(randomblob(16)))` mints a 32-char hex id (UUID-like, no
-- dashes). That's fine for the snapshot row — the user-facing identity
-- is `(quick_prompt_id, version_index)`, which is already UNIQUE; this
-- column is just a synthetic PK.

INSERT INTO quick_prompt_versions (
    id, quick_prompt_id, version_index, name, icon, prompt_template,
    variables_json, agent, project_id,
    skill_ids_json, profile_ids_json, directive_ids_json,
    tier, description, created_at
)
SELECT
    lower(hex(randomblob(16))) AS id,
    qp.id,
    1 AS version_index,
    qp.name, qp.icon, qp.prompt_template,
    qp.variables_json, qp.agent, qp.project_id,
    qp.skill_ids_json, qp.profile_ids_json, qp.directive_ids_json,
    qp.tier, COALESCE(qp.description, ''), qp.created_at
FROM quick_prompts qp
WHERE NOT EXISTS (
    SELECT 1 FROM quick_prompt_versions v WHERE v.quick_prompt_id = qp.id
);
