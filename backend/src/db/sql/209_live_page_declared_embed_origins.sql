-- Third-party sites a Live Page declares it embeds from (`data-kronn-embed`),
-- beyond what its HTML spells out: content its scripts build at runtime. Set
-- from an imported Artifact's `embed_origins` and exported again merged with
-- the origins read from the HTML, so a chain of imports keeps them. A JSON
-- array of normalized origins; information for the next importer, never a
-- permission (those live in config.toml, `embed_allowed_origins`).
ALTER TABLE live_pages ADD COLUMN declared_embed_origins TEXT NOT NULL DEFAULT '[]';
