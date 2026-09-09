-- KT-619 volet B — authenticated authority for a human-published card.
--
-- Decided 2026-09-09 (`kt619-human-credential-bootstrap` → dedicated-human-credential).
-- API identity is proven; physical presence is NOT claimed, and OS-level secret
-- theft or direct writes to this database are outside the guarantee. Only
-- hashes live here: every plaintext is shown once and never stored.

-- The bootstrap. One row, ever: `singleton` is CHECK-pinned so a second admin
-- secret cannot be minted alongside the first and quietly authorise enrolments.
-- The plaintext is written to operator private storage and returned by no API.
CREATE TABLE human_admin_secret (
    singleton    INTEGER NOT NULL PRIMARY KEY CHECK (singleton = 1),
    secret_hash  TEXT    NOT NULL,
    -- Where the operator can find it. A path, never the secret.
    delivered_to TEXT    NOT NULL,
    created_at   TEXT    NOT NULL,
    rotated_at   TEXT
);

-- Credentials that may publish as `Human`. Distinct from the admin secret, so
-- revoking one does not cost the other, and revocable without a reset.
CREATE TABLE human_credentials (
    id           TEXT NOT NULL PRIMARY KEY,
    label        TEXT NOT NULL,
    secret_hash  TEXT NOT NULL UNIQUE,
    -- 'admin' for the first, 'human' when an existing credential enrolled it.
    -- There is no 'anonymous': enrolment always names who authorised it.
    enrolled_by  TEXT NOT NULL CHECK (enrolled_by IN ('admin', 'human')),
    created_at   TEXT NOT NULL,
    revoked_at   TEXT,
    revoked_reason TEXT
);

CREATE INDEX idx_human_credentials_live
    ON human_credentials(revoked_at, created_at);

-- One publication proof: single use, and bound to what it was issued for.
--
-- `content_hash` is why a captured proof cannot be moved onto another card, and
-- `discussion_id` why it cannot be moved to another room. Single use is claimed
-- by `UPDATE … SET consumed_at = ? WHERE id = ? AND consumed_at IS NULL` and
-- believed only when it reports one changed row: a read-then-write would let two
-- concurrent callers both see NULL and both publish.
CREATE TABLE human_publication_proofs (
    id            TEXT NOT NULL PRIMARY KEY,
    credential_id TEXT NOT NULL REFERENCES human_credentials(id) ON DELETE CASCADE,
    discussion_id TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    content_hash  TEXT NOT NULL,
    issued_at     TEXT NOT NULL,
    expires_at    TEXT NOT NULL,
    consumed_at   TEXT,
    -- Rotation and revocation invalidate proofs already in flight: a proof is
    -- only usable while its credential's epoch still matches.
    credential_epoch INTEGER NOT NULL
);

CREATE INDEX idx_human_proofs_lookup
    ON human_publication_proofs(credential_id, discussion_id, content_hash);

-- Epoch bumps on rotation/revocation; proofs carry the epoch they were issued
-- under, so invalidation is a single write rather than a sweep.
ALTER TABLE human_credentials ADD COLUMN epoch INTEGER NOT NULL DEFAULT 1;
