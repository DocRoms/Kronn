-- stab-3 follow-up (PR 118, Codex review) — pacing anchors need a RECEPTION
-- clock. `timestamp` is the AUTHOR's clock (kept for display and federation
-- fidelity): a federated message can arrive stamped in the past, and the
-- pacing contract ("a new message resets the ramp / renews the lease") is
-- about when THIS instance received it, not when it was written.
-- Backfill: for local history reception ≈ authorship.
ALTER TABLE messages ADD COLUMN received_at TEXT;
UPDATE messages SET received_at = timestamp WHERE received_at IS NULL;
