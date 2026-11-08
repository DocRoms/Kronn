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

A configuration-only export also withholds what the server definition itself
may hold (KT-1040, `api/plugin_portability/redaction.rs`). It is an
allow-list over every string field of `McpServer`: a value leaves the machine
only when it is provably not a secret.

- Kept: a value equal to the built-in catalogue's at the same place; a server
  id in Kronn's generated format `custom-<slug>-<8 lowercase hex>` whose slug
  is the slug of the plugin's name when that name is kept (the slug is the
  lowercased name, so it would bring a withheld one back), or a plain slug that
  passes the name check; any other id is replaced by `custom-export-<8 hex>`,
  the same for every config of that server within one export, never a
  placeholder; env-key names `[A-Za-z0-9_]+` with no token-shaped word (the
  bundle's env-key list too, so a key derived from a withheld field label
  cannot bring it back), unless the catalogue entry declares the key or the
  definition keeps it (a catalogue plugin round-trips with no placeholder); enum tags and HTTP methods (letters only); names
  (server name, config label, header names, scope, token JSONPath, body keys)
  with no token-shaped word; a header or request-body value made only of
  `${ENV.KEY}` references (an OAuth2 extra header: `{KEY}`), whose keys pass
  the same check, optionally after an auth scheme word; a URL that is only
  `scheme://host[:port]` with a numeric port and a host that is a real IPv6
  literal, has no token-shaped label, or is the catalogue's; a lone reference;
  a relative endpoint path whose every segment is a reference, `vN` or a plain
  lowercase word; a flag argument `-x` (one letter) or `--lower-case`; an
  empty string; the declared TTL and `public` flag. A word is token-shaped
  when it mixes letters and digits over 8 characters (`OAuth2`, `S3`, `GPT4`,
  `K8s` are names), holds a run of six digits or more, or is longer than 20
  characters; a value over 8 characters where two segments or more hold a
  digit, whatever separates them (`4B7D-9F1C-0A2B`, `4B7D.9F1C`,
  `4111 1111 1111 1111`), is token-shaped as a whole. That also withholds real
  keys such as `S3_BUCKET_V2` or `K8S_NODE_1`, which the import asks back: the
  safe side. A URL host label keeps a stricter rule, where any letter-digit mix
  is withheld.
- Withheld: everything else, including descriptions, `docs_url`, URL paths,
  queries, user info and token-shaped hosts, stdio commands and arguments,
  CLI-token commands, TokenExchange endpoints, body literals, numbers and
  booleans, OAuth2 token URLs, a withheld `test_endpoint` (as the same
  placeholder as its endpoint), and any leaf no rule names, whatever its type
  (the log names its path). The fixtures destructure every struct and variant
  without `..`, set every `Option` and fill every list, and a test checks that
  the leaves they reach are exactly the decided paths.

**Residual risk.** A host label of 20 letters or fewer, and a lowercase
endpoint path segment, are kept in clear (the host also in the URL hint): the
shape test cannot tell `secretword.example.com` from a real subdomain.
The same shape test decides every name-like value: server names and config
labels, header names, scopes and the token JSONPath, env-key names and the
keys inside `${ENV.KEY}` and `{KEY}` references, request-body keys, and
plain-slug server ids (and Kronn's `custom-<slug>-<8 hex>` ids through the
name their slug comes from); in all of them an all-letter word of 20
characters or fewer, a letter-digit word of 8 characters or fewer, and a run
where only one segment holds a digit (`abcdefgh_ijklmnop_q1`, roughly 5% of
random 4×8 base62 tokens) are kept in clear. Keep secrets out of hosts, paths
and names, or put them in a field and reference it with `${ENV.KEY}`.

A withheld value becomes a named placeholder `${ENV.KRONN_EXPORT_<FIELD>}`
(the same literal gets the same name); the bundle lists each one with its
field, whether the plugin needs it, its JSON type (a number or a boolean goes
back as one), and for a URL its origin when the host is kept. The
import review (bundle or old per-plugin file) asks for each placeholder of a
custom definition and the import puts the typed values back
(`placeholder_values`). One left empty stays a reference: the import goes
through with a warning naming it, and the plugin fails its first call naming
it. A registry plugin is rebuilt from the catalogue, so it asks nothing. The
encrypted export with values keeps the definition whole. The per-plugin export
(`/api/mcps/custom/{id}/export-file`) applies the same redaction, its
validation accepts `KRONN_EXPORT_` references; its file import route, with no
review, answers with a notice naming those left to fill.

Credentials supplied live by a trusted local CLI provider are never
exportable. Export audit records contain bundle/config identifiers, counts and
the with/without-values flag, but no value, passphrase or payload.

## Import trust rules

- A registry plugin is reconstructed from the receiving Kronn instance's
  current trusted registry definition; commands from the bundle are ignored.
- Unknown executable and MCP server definitions are refused. Only a manual
  API-only definition without a CLI credential command may be materialized.
- Imported configurations are unscoped, non-global and have host sync disabled
  at the database level. The UI lists every created configuration with
  **all projects** selected by default. The shared scope editor also exposes
  project/General visibility and, for trusted registry MCP/hybrid plugins,
  optional local-CLI sync with the concrete host-file preview. The dialog
  applies the displayed choices through either the explicit apply button or
  X/backdrop close; a failed write keeps the dialog open. A closed import
  therefore never leaves a configuration silently unscoped (KT-352/KT-831).
- A configuration with the same plugin/label or semantic configuration hash is
  skipped and reported rather than overwritten.
- Exact replay is idempotent. Reusing a bundle id with changed content is an
  explicit conflict.

The audit and import ledger are introduced by migration 094.

[src: file: backend/src/api/plugin_portability.rs:253-293#plugin_value_descriptors]
[src: file: backend/src/api/plugin_portability.rs:336-371#build_preview]
[src: file: backend/src/api/plugin_portability.rs:396-532#record_event]
[src: file: backend/src/api/plugin_portability.rs:660-792#current_registry_server]
[src: file: backend/src/api/plugin_portability.rs:870-1053#import_payload]
[src: file: backend/src/db/sql/094_plugin_bundle_audit.sql:1-24#plugin_bundle_events]
[src: file: frontend/src/components/PluginPortabilityModal.tsx:72-80#scopeFor]
[src: file: frontend/src/components/PluginPortabilityModal.tsx:83-86#supportsHostSync]
[src: file: frontend/src/components/PluginPortabilityModal.tsx:232-254#saveImportScopes]
[src: file: frontend/src/components/PluginPortabilityModal.tsx:256-293#handleClose]
[src: file: frontend/src/components/plugins/PluginScopeEditor.tsx:38-176#PluginScopeEditor]
