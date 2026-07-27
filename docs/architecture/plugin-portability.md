# Plugin portability

Kronn exports a selected set of plugin configurations as a versioned
`kronn.plugins` JSON bundle. Configuration-only export is the default and
contains no environment values.

## Sensitive-value boundary

Including values is an explicit danger-zone action:

1. Kronn previews every outgoing key and identifies sensitive values and
   CLI-backed values.
2. The operator must type `EXPORTER LES SECRETS` and provide a passphrase of at
   least 12 characters.
3. Kronn encrypts the complete payload with a fresh AES-256-GCM key and wraps
   that key with the existing Argon2id recovery framing.
4. Only ciphertext, the wrapped key and a value-free manifest are written to
   the downloaded bundle.

Credentials supplied live by a trusted local CLI provider are never
exportable. Export audit records contain bundle/config identifiers, counts and
the with/without-values flag, but no value, passphrase or payload.

## Import trust rules

- A registry plugin is reconstructed from the receiving Kronn instance's
  current trusted registry definition; commands from the bundle are ignored.
- Unknown executable and MCP server definitions are refused. Only a manual
  API-only definition without a CLI credential command may be materialized.
- Imported configurations are unscoped, non-global and have host sync disabled.
  The operator must explicitly choose their eventual exposure.
- A configuration with the same plugin/label or semantic configuration hash is
  skipped and reported rather than overwritten.
- Exact replay is idempotent. Reusing a bundle id with changed content is an
  explicit conflict.

The audit and import ledger are introduced by migration 094.

[src: file: backend/src/api/plugin_portability.rs:351-530]
[src: file: backend/src/api/plugin_portability.rs:572-797]
[src: file: backend/src/api/plugin_portability.rs:826-955]
[src: file: backend/src/db/sql/094_plugin_bundle_audit.sql:1-24]
[src: file: frontend/src/components/PluginPortabilityModal.tsx:28-337]
