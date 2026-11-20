-- A pairing code may be issued for one existing contact (KT-1650): the owner
-- re-pairs a `needs_repair` row from its entry. Such an offer pairs that exact
-- contact and nobody else; an offer without a contact pairs a NEW contact only,
-- so a live offer can never take over another contact's row (its id, shared
-- discussions, context files).
ALTER TABLE contact_pairing_offers ADD COLUMN contact_id TEXT;
