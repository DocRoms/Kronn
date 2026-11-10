-- KT-1643 — a campaign names who reviews its deliveries when it is created.
-- NULL: a campaign created before this column, or by a caller that named no
-- reviewer through a path that predates it. The JSON is a `CampaignReviewer`.
ALTER TABLE orchestration_runs ADD COLUMN reviewer_json TEXT;
