-- KT-920: the repository profile `kronn/project.toml`.
-- The starter profile a Full audit drafts for a repository whose default
-- branch has none. Kept with the run, never written into the checkout; a
-- human publishes it through a pull request.
ALTER TABLE audit_runs ADD COLUMN project_profile_draft TEXT;
-- The profile snapshots a trusted Page action was approved with (KT-1029):
-- revalidations that cannot read git compare against them.
ALTER TABLE live_page_action_trusts ADD COLUMN profile_snapshots_json TEXT;
