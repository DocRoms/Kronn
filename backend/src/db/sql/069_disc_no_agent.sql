-- 069 — F9: "human-only" discussions (no agent ever responds).
--
-- A 1:1 (or group) human↔human chat must NEVER trigger Kronn's local agent
-- runner, even on an instance that HAS an agent installed. Without this, a
-- human posting in a shared "people chat" on such an instance would get an
-- unsolicited agent reply. `send_message` checks this flag and skips the runner
-- entirely when set.
--
-- Default 0 keeps every existing discussion agent-capable (current behaviour).
ALTER TABLE discussions ADD COLUMN no_agent INTEGER NOT NULL DEFAULT 0;
