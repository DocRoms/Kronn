-- Favorite / pinned workflows — same affordance as discussions.pinned:
-- pinned entries surface first in the Workflows page list.
ALTER TABLE workflows ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
