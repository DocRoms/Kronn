-- Contacts for multi-user collaboration.
-- Each contact represents a remote Kronn instance (peer).
CREATE TABLE IF NOT EXISTS contacts (
  id TEXT PRIMARY KEY,
  pseudo TEXT NOT NULL,
  avatar_email TEXT,
  kronn_url TEXT NOT NULL,
  invite_code TEXT NOT NULL UNIQUE,
  status TEXT NOT NULL DEFAULT 'pending',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
