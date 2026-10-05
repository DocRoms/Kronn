# Key management (encryption key, credentials, recovery)

Since 0.14.3 (KT-1007), `config.toml` holds no secret once the boot migration
has run. This page says where each secret lives, what the boot does with it,
and how to recover.

## Where secrets live

| Secret | Location |
|---|---|
| Encryption key (AES-256-GCM, hex) | Vault ladder: `KRONN_ENCRYPTION_KEK` env → OS keychain (macOS / Windows release builds) → `encryption_key` sidecar (`0600`) in the data directory. `config.toml` keeps a copy only while no vault reads it back. |
| Provider keys, External API connection keys | `stored_credentials` table (`kind = provider_key`), encrypted with the instance key |
| API auth token (`server.auth_token`) | `stored_credentials` table (`kind = auth_token`) |
| MCP env values, execution-variable snapshots, per-project GitHub tokens | `mcp_configs.env_encrypted`, `execution_variable_snapshots.values_encrypted`, `project_github_connections.token_encrypted` |
| Recovery blob | `recovery.key` (`0600`): the key wrapped under the recovery passphrase (Argon2id) |

Every encrypted column is listed in one registry, `keystore::ENCRYPTED_COLUMNS`;
a test compares it with the schema, so a new `*_encrypted` / cipher column
cannot be added without the reconciler seeing it.
[src: file: backend/src/core/keystore.rs:42-59]

The running process still reads `config.tokens.keys` and
`config.server.auth_token`: the boot fills them from the table, and every
`config::save` writes them back to the table (encrypted, read back) before it
writes `config.toml` without them. A failure there fails the save and leaves
the previous file untouched.
[src: file: backend/src/core/credential_store.rs:229-246]
[src: file: backend/src/core/config.rs:166-175]

## Boot order

1. `config::load` reads `config.toml`. It never mints a key or an auth token.
2. The database opens, then the reconciler picks the key by decrypt
   self-test across every registered column:
   - a vault that cannot be read (denied keychain prompt, locked keychain,
     unreadable sidecar) **stops the boot** with a message naming the vault
     and what to do; nothing is minted or written. The desktop app shows the
     same message on its "Kronn could not start" screen (startup error →
     `wait_for_backend` → bootstrap failure panel, with Retry); for the
     keychain it says to choose Allow, or to start with
     `KRONN_USE_KEYCHAIN=0` (`open --env KRONN_USE_KEYCHAIN=0 -a Kronn`);
   - a key is minted only when no registered column holds ciphertext;
   - two distinct keys that each decrypt some rows (say the keychain holds
     K1 and the sidecar K2) **stop the boot** with nothing written; the
     message names each source, its key fingerprint and how many rows it
     decrypts. Kronn never picks one, and never overwrites a vault that holds
     a different key than the one in use;
   - rows exist and no key decrypts them: locked state, nothing overwritten,
     and no key in memory (fail closed: nothing new is encrypted under a key
     no vault holds). Kronn keeps running so the key can be restored from
     Settings → Recovery.
   [src: file: backend/src/core/keystore.rs:298-381]
3. The resolved key is mirrored into the writable vaults. A vault whose read
   failed is never written. `config.toml` drops its copy only when a vault
   reads the key back, the key decrypts at least one row of every non-empty
   column, **and** either a recovery passphrase is set or the `encryption_key`
   sidecar holds the key too. A different legacy key in `config.toml` is
   kept. Without a recovery passphrase, nothing deletes a local copy of the
   key (vault, sidecar); `GET /api/config/recovery/status` reports it
   (`key_copies_kept`, `config_holds_key`) and the boot log warns.
   [src: file: backend/src/core/keystore.rs:231-271]
4. `credential_store::boot` loads the stored credentials, moves any still in
   `config.toml` into the table, and generates an auth token only when none
   exists anywhere. Locked key: the store stays off and `config.toml` is left
   as it is. [src: file: backend/src/core/credential_store.rs:272-391]

An operator-set `KRONN_AUTH_TOKEN` is read and removed from the process
environment before the database opens (`config::take_env_auth_token`). Step 4
stores it as the auth token when none is stored yet; a different stored token
wins, with a warning, as the `config.toml` one did before. Either way auth is
enabled and the value never reaches `config.toml` or a child process. The
LAN guard and the auth middleware read the resulting token.

## Auth locked

When a stored auth token exists but cannot be decrypted (key locked, or the
row is under another key) and auth is enabled, the API is **auth locked**,
not open: every route answers `423 Locked` except `/api/health` and, from a
local caller, `GET /api/config/recovery/status` and
`POST /api/config/recovery/restore`. A successful restore loads the token and
lifts the lock. The WebSocket gives no connection the local-frontend trust in
that state. The static UI still loads; there is no dedicated recovery screen
yet, so restore through Settings → Recovery when it renders, or with the
restore route. An operator-set `KRONN_AUTH_TOKEN` replaces the locked token
for the session.

## The one-time migration from 0.14.2

Safe to interrupt at any point; a rerun converges:

1. upsert table ∪ `config.toml` (the file wins on the same id);
2. read every row back and compare the decrypted values;
3. write `config.toml.pre-credential-store.enc` (`0600`): the old file,
   encrypted with the instance key. An existing backup is kept (it is the
   oldest original). The recovery passphrase restores that key, so it also
   opens this backup; no new plaintext file is written;
4. save `config.toml` without credentials;
5. drop the key from `config.toml.backup` when it is the key in use and no
   longer needed in `config.toml`. That copy, which the DB migration runner
   writes before pending migrations, never holds credentials: it is written
   owner-only with the auth token and provider keys already removed, so a
   failed credential boot cannot leave them in plaintext there.

`credential_store::read_backup(path, key)` decrypts the backup.
[src: file: backend/src/core/credential_store.rs:453-456]

Not migrated in this release: rows that do not decrypt with the current key
are kept untouched (logged as locked) and never deleted by later saves.

## Recovery passphrase

`POST /api/config/recovery/set` wraps the active key under a new passphrase.
When `recovery.key` already exists, the request must carry
`current_passphrase`, which must unwrap it; an unreadable `recovery.key` is
never replaced from the API (move it out of the data directory by hand).
[src: file: backend/src/core/keystore.rs:418-453]

## Testing the Keychain path from a dev build

Debug builds skip the OS keychain (an unsigned binary changes identity at each
rebuild, so macOS would prompt on every restart). To exercise the keychain
path, including a denied prompt, start the dev backend with
`KRONN_USE_KEYCHAIN=1`; `KRONN_USE_KEYCHAIN=0` forces the sidecar in a release
build. Outside macOS and Windows the default is off (no keychain backend).
[src: file: backend/src/core/keyvault.rs:211-217]
