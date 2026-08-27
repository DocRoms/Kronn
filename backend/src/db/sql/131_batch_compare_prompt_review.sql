-- Prompt-level synthesis closes the Compare feedback loop: the judge grades
-- candidate answers independently, then records whether the evaluated QP
-- itself can be improved and why.

ALTER TABLE batch_compare_judge_runs ADD COLUMN prompt_review_json TEXT;
