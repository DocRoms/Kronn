-- 264 (KT-1211): what the step's agent was last seen writing.
--   observed_hash  fingerprint of the target's content the last time the audit
--                  saw it change while the agent ran. A boot after a crash puts
--                  the target back only when what is on disk is this content;
--                  anything else (a hand edit made after the crash, a write the
--                  orphaned agent made unseen) is left in place under a
--                  partial marker, never overwritten.
ALTER TABLE audit_step_snapshots ADD COLUMN observed_hash TEXT;
