-- Who a portable discussion came from, and by which route.
--
-- The ledger only recorded that an import happened. The sidebar therefore had
-- nothing to show but the CLI session binding, which is a different thing
-- entirely — being bound to a live Codex session is not being imported.
--
-- `provenance_kind` also reserves the route that does not exist yet: importing
-- an agent transcript. Keeping it in the model now means the future feature
-- adds a value instead of reinterpreting existing rows.

ALTER TABLE discussion_imports
    ADD COLUMN provenance_kind TEXT NOT NULL DEFAULT 'portable_bundle';

-- Identity of the person who exported the bundle, as carried by the envelope.
-- Both stay NULL for bundles exported before this column existed and for
-- instances that never configured a pseudo — the UI must render the import
-- without an author rather than invent one.
ALTER TABLE discussion_imports ADD COLUMN imported_by_pseudo TEXT;
ALTER TABLE discussion_imports ADD COLUMN imported_by_avatar_email TEXT;
