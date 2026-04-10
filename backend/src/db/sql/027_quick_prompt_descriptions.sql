-- Quick Prompts v2: optional description for the prompt template
-- (per-variable descriptions live inside variables_json, no schema change needed)
-- Added 2026-04-10 to support batch workflows: the batch runner UI needs to
-- show the user what a Quick Prompt does and what each variable means before
-- firing it off against N tickets.
ALTER TABLE quick_prompts ADD COLUMN description TEXT NOT NULL DEFAULT '';
