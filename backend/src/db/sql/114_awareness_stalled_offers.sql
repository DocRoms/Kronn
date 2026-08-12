-- KT-249 — count consecutive awareness batches offered without the session
-- acknowledging any of them.
--
-- The offered cursor saturates one batch ahead of the ack cursor, so the GAP
-- alone cannot tell a single discarded delivery from a client that never
-- acknowledges at all. A counter can: it resets the moment an ack advances the
-- cursor, and only climbs while the identical batch is re-offered.
--
-- A client that never acknowledges makes every wake re-deliver the same
-- backlog indefinitely. That is silent — no error, just an unbounded cost —
-- which is why it needs an explicit signal.

ALTER TABLE discussion_sessions
    ADD COLUMN awareness_stalled_offers INTEGER NOT NULL DEFAULT 0;
