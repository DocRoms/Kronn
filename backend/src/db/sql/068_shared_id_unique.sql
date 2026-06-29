-- 068 — enforce ONE local discussion per shared_id.
--
-- `find_discussion_by_shared_id` uses `query_row` (first match) and the whole
-- federation layer assumes a shared_id maps to exactly one local disc. The
-- original index (migration 023) was NON-unique, so a race (a WS invite
-- creating a mirror while a local share assigned the same id, or duplicate
-- invites) could attach messages to an arbitrary copy. Make the invariant
-- enforced rather than assumed.
--
-- Defensive de-dup first so this migration succeeds on any existing DB: keep
-- the OLDEST row per shared_id and demote the rest to local discs by NULLing
-- their shared_id. Non-destructive — no disc or message is deleted, the extra
-- copies simply stop being treated as the shared mirror.
UPDATE discussions
   SET shared_id = NULL
 WHERE shared_id IS NOT NULL
   AND rowid NOT IN (
       SELECT MIN(rowid)
         FROM discussions
        WHERE shared_id IS NOT NULL
        GROUP BY shared_id
   );

-- Replace the non-unique index with a unique partial one (NULLs are exempt,
-- so any number of purely-local discs coexist).
DROP INDEX IF EXISTS idx_discussions_shared_id;
CREATE UNIQUE INDEX idx_discussions_shared_id
    ON discussions(shared_id)
    WHERE shared_id IS NOT NULL;
