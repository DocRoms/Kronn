-- Add author identity columns to messages (for multi-user display).
ALTER TABLE messages ADD COLUMN author_pseudo TEXT;
ALTER TABLE messages ADD COLUMN author_avatar_email TEXT;
