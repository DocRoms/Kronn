-- KT-1202: whether an agent turn resumed its CLI/ACP conversation, started a
-- new one or fell back to the full history, and why (JSON, NULL = unknown).
ALTER TABLE messages ADD COLUMN session_continuity TEXT;
