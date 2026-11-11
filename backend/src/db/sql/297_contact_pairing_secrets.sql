-- Per-contact pairing secrets (KT-1650). A contact's invite code is the public
-- address `kronn:pseudo@host:port`, so it cannot authenticate anyone: each paired
-- contact now carries two random secrets exchanged at pairing.
--   inbound_secret_hash : SHA-256 of the secret the peer must present to us
--                         (we never keep it in clear, like a password).
--   outbound_secret     : the secret we present to the peer (it must be sent, so
--                         it is stored in clear, owner-only DB; never listed).
ALTER TABLE contacts ADD COLUMN inbound_secret_hash TEXT;
ALTER TABLE contacts ADD COLUMN outbound_secret TEXT;

-- Pairing codes this instance handed out, kept hashed until a peer spends one.
CREATE TABLE IF NOT EXISTS contact_pairing_offers (
  id TEXT PRIMARY KEY,
  secret_hash TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL
);

-- Owner decision (KT-1650): existing contacts RE-PAIR, no compatibility with
-- peers that do not present a secret. They keep their row (discussion sharing is
-- by contact id) but are neither dialled nor admitted until paired again.
UPDATE contacts
   SET status = 'needs_repair'
 WHERE inbound_secret_hash IS NULL
   AND status IN ('accepted', 'pending');
