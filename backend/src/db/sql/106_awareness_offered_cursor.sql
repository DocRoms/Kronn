-- KT-189 — third awareness cursor: what was actually OFFERED to a session.
--
-- The delivery contract is scan / offer / ack. `user_catchup_cursor` is the
-- acked side (advanced only on a confirmed model delivery). This column is
-- the offered side, written when a wake response attaches an awareness
-- batch. The ack is clamped to it: a client can never acknowledge — and
-- therefore skip — turns that no response ever carried.
ALTER TABLE discussion_sessions ADD COLUMN awareness_offered_upto INTEGER NOT NULL DEFAULT 0;

-- Existing sessions have nothing outstanding: offered starts at acked.
UPDATE discussion_sessions SET awareness_offered_upto = user_catchup_cursor;
