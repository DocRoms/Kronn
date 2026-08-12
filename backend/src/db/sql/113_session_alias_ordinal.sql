-- KT-247 — stable per-provider ordinal for joined CLI sessions.
--
-- Until now the `@claude-cli-2` ordinal existed only in the composer, recomputed
-- from the participants list on every render. That list filters `status != 'left'`,
-- so a session leaving SHIFTED the ordinals of the ones after it: a `@claude-cli-2`
-- written yesterday could designate a different session today, and a message
-- header would rewrite its own past. Persisting the ordinal makes the identity
-- stable for the lifetime of the session row.
--
-- 0 means NOT ASSIGNED — never render it as an alias. The ordinal is allocated at
-- join time; a row still at 0 signals a write path that bypassed that allocation,
-- which must surface as unknown rather than as a fabricated `-cli-0`.
ALTER TABLE discussion_sessions ADD COLUMN alias_ordinal INTEGER NOT NULL DEFAULT 0;

-- Backfill over ALL rows, including `left` ones: excluding them would reproduce
-- the very shifting this column removes. `id` breaks ties because same-second
-- joins are common when two CLIs are launched together.
WITH ranked AS (
    SELECT id,
           ROW_NUMBER() OVER (
               PARTITION BY disc_id, agent_type
               ORDER BY joined_at, id
           ) AS ord
      FROM discussion_sessions
)
UPDATE discussion_sessions
   SET alias_ordinal = (SELECT ord FROM ranked WHERE ranked.id = discussion_sessions.id);
