-- Version and enforce the discussion ↔ external CLI session binding contract.
--
-- Historical databases could contain the same (CLI, session id) bound to
-- several discussions because the old "last link wins" implementation only
-- closed the previous binding on the target discussion. Keep the most recent
-- open row, close the others, then enforce the invariant at the database edge.

ALTER TABLE discussions ADD COLUMN source_binding_version INTEGER;
ALTER TABLE disc_source_history
    ADD COLUMN binding_version INTEGER NOT NULL DEFAULT 1;

UPDATE disc_source_history
   SET unlinked_at = COALESCE(unlinked_at, linked_at)
 WHERE unlinked_at IS NULL
   AND rowid NOT IN (
       SELECT MAX(rowid)
         FROM disc_source_history
        WHERE unlinked_at IS NULL
        GROUP BY source_agent, source_session_id
   );

UPDATE discussions
   SET source_agent = NULL,
       source_session_id = NULL,
       source_binding_version = NULL
 WHERE source_agent IS NOT NULL
   AND source_session_id IS NOT NULL
   AND NOT EXISTS (
       SELECT 1
         FROM disc_source_history history
        WHERE history.disc_id = discussions.id
          AND history.source_agent = discussions.source_agent
          AND history.source_session_id = discussions.source_session_id
          AND history.unlinked_at IS NULL
   );

UPDATE discussions
   SET source_binding_version = 1
 WHERE source_agent IS NOT NULL
   AND source_session_id IS NOT NULL;

CREATE UNIQUE INDEX idx_disc_source_session_one_open
    ON disc_source_history(source_agent, source_session_id)
 WHERE unlinked_at IS NULL;
