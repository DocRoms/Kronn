-- KT-1046: LLM calls of a root run's whole sub-workflow tree, raised with MAX()
-- by every run of the tree as it spends, apart from the state blob a parent's
-- older progress snapshot could overwrite.
ALTER TABLE workflow_runs ADD COLUMN tree_llm_calls INTEGER NOT NULL DEFAULT 0;
