# Key management (encryption key, credentials, recovery)

Since 0.14.3 (KT-1007), `config.toml` holds no secret once the boot migration
has run. This page says where each secret lives, what the boot does with it,
and how to recover.

## Where secrets live

| Secret | Location |
|---|---|
| Encryption key (AES-256-GCM, hex) | Vault ladder: `KRONN_ENCRYPTION_KEK` env → OS keychain (macOS / Windows release builds) → `encryption_key` sidecar (`0600`) in the data directory. `config.toml` keeps a copy until two independent copies exist (see step 3). |
| Provider keys, External API connection keys | `stored_credentials` table (`kind = provider_key`), encrypted with the instance key |
| API auth token (`server.auth_token`) | `stored_credentials` table (`kind = auth_token`) |
| MCP env values, execution-variable snapshots, per-project GitHub tokens | `mcp_configs.env_encrypted`, `execution_variable_snapshots.values_encrypted`, `project_github_connections.token_encrypted` |
| Recovery blob | `recovery.key` (`0600`): the key wrapped under the recovery passphrase (Argon2id), plus the key's fingerprint in clear so Kronn can tell which key it recovers without the passphrase |
| Imported recovery blobs | `recovery.imported-<timestamp>.key` (`0600`): another machine's blob carried by an import, never replacing `recovery.key` |
| Replaced recovery blobs | `recovery.previous-<timestamp>.key` (`0600`): a `recovery.key` that wrapped another key than the one in use, kept when it is replaced |

Keys are compared, counted and fingerprinted in one spelling (lower-case hex,
no surrounding whitespace): `ABCD…` in the env and `abcd…` in the sidecar are
one key. A value with non-hex or multi-byte characters is rejected without
ever panicking the boot. The recovery blob stores the fingerprint plus a
checksum binding it to the wrapped payload, so a damaged payload never counts
as a verified copy.

Every encrypted column is listed in one registry, `keystore::ENCRYPTED_COLUMNS`;
a test compares it with the schema, so a new `*_encrypted` / cipher column
cannot be added without the reconciler seeing it.
[src: file: backend/src/core/keystore.rs:42-59]

The running process still reads `config.tokens.keys` and
`config.server.auth_token`: the boot fills them from the table, and every
`config::save` writes them back to the table (encrypted, read back) before it
writes `config.toml` without them. A failure there fails the save and leaves
the previous file untouched.
[src: file: backend/src/core/credential_store.rs:295-349]
[src: file: backend/src/core/config.rs:177-201]

## Boot order

1. The backend takes the data-directory lock, then `config::load` reads
   `config.toml`. It never mints a key or an auth token. A missing
   `config.toml` starts from defaults **without** a key, so no random key is
   ever offered to the reconciler. Values that are not a 32-byte hex key are
   ignored as candidates.
2. The database opens, then the reconciler picks the key by decrypt
   self-test across every registered column:
   - a vault that cannot be read (denied keychain prompt, locked keychain,
     unreadable sidecar; a non-text sidecar is reported as corrupted, with
     "move it aside") **stops the boot** with a message naming the vault
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
     a different key than the one in use. Every row is scanned, so a second
     key whose rows are all old is still found. To keep both halves, start
     Kronn with `KRONN_REENCRYPT_FROM=<fingerprint of the key to retire>`: its
     rows (and the encrypted `config.toml` backup) are re-encrypted under the
     first other key listed, in one transaction checked by read-back and
     rolled back on any failure, then the boot continues; no key copy is
     deleted. With three or more keys, repeat once per key to retire;
   - rows exist and no key decrypts them: locked state, nothing overwritten,
     and no key in memory (fail closed: nothing new is encrypted under a key
     no vault holds). Kronn keeps running so the key can be restored from
     Settings → Recovery.
   [src: file: backend/src/core/keystore.rs:328-452]
3. The resolved key is mirrored into the **empty** writable vaults; a vault
   holding another key, or whose read failed, is never written. `config.toml`
   drops its copy only when the key decrypts at least one row of every
   non-empty column **and** two independent copies remain without it: two
   persisted vaults reading the key back (keychain, sidecar), or one plus a
   `recovery.key` whose fingerprint matches the key. `KRONN_ENCRYPTION_KEK`
   is not counted: a one-off variable is not durable. A restore applies the
   same rule. A sidecar-only
   install (Linux, WSL, Docker, dev builds) without a verified passphrase
   therefore keeps the key in `config.toml`. A `recovery.key` written before
   0.14.3 has no fingerprint and does not count until the passphrase is used
   once (a restore, or replacing it in Settings → Recovery), which records
   it. A different legacy key in `config.toml` is kept.
   `GET /api/config/recovery/status` reports `matches_key`, `key_locked`,
   `key_copies_kept` (the retention rule keeps the file copy),
   `config_holds_key`, `copies`, `stale_sources` (key stores holding another
   key, never overwritten), `invalid_sources`, `locked_credentials` and
   `kept_recovery_blobs`; Settings → Recovery shows the warnings.
   [src: file: backend/src/core/keystore.rs:253-293]
4. `credential_store::boot` loads the stored credentials, moves any still in
   `config.toml` into the table, and generates an auth token only when none
   exists anywhere. Locked key: the store stays off and `config.toml` is left
   as it is. If this step fails before loading the token while a token is
   stored, auth is locked (below), never open. The stored token row also
   records whether auth is enabled, so a lost `config.toml` does not turn
   auth off. [src: file: backend/src/core/credential_store.rs:414-547]

An operator-set `KRONN_AUTH_TOKEN` is read and removed from the process
environment before the database opens (`config::take_env_auth_token`). Step 4
stores it as the auth token when none is stored yet; a different stored token
wins, with a warning, as the `config.toml` one did before. While the store is
not armed (key locked), or the stored token row cannot be read, the env
token serves **this session only**: it is never written to `config.toml` nor
to the store. A later restore keeps it when no token is stored, and never
changes whether auth is enabled. Either way auth is enabled and the value
never reaches a child process: both the backend and the desktop app remove it
from their environment before any thread starts.

While the store cannot be armed, `config.toml` keeps only the credentials it
held at start: adding or changing a provider key, a connection key or the
token is refused with "the encryption key is locked", the boot key
auto-discovery is skipped, and no save writes a credential in clear.

## Auth locked

When a stored auth token exists but cannot be decrypted (key locked, or the
row is under another key) and auth is enabled, the API is **auth locked**,
not open: every route answers `423 Locked` except `/api/health` and, from a
caller on this machine (whatever `auth_strict_localhost` says: no token can be
checked), `GET /api/config/recovery/status`,
`POST /api/config/recovery/restore` and, when the key is in use and only the
token row is unreadable, `POST /api/config/auth-token/regenerate`. The JSON body carries
`error_code: "auth_locked"`, and the UI then shows a dedicated "Kronn is
locked" screen: on another machine it says to open Kronn where it runs; with
the key lost, the passphrase (and optional recovery code) form; with only the
token row unreadable, a "Set a new API token" button. A successful restore
loads the token, lifts the lock and reloads the app. A failed credential load
after a restore is reported, not answered as success. The
WebSocket gives no connection the local-frontend trust in that state. The LAN
boot guard counts the locked state as enforced auth, so an exposed install
still starts and can be recovered.

A stored row the current key cannot decrypt is never rewritten or deleted by
ordinary saves, and never blocks them. Two things replace it: setting a new
auth token (regenerate, network exposure), and a credential still present in
`config.toml` with the same id at start (the file is newer, so it wins, as in
the migration). Token changes are applied to the live config only once saved.

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
   failed credential boot cannot leave them in plaintext there. An existing
   `config.toml.backup` with other content (an older key, say) is moved to
   `config.toml.backup.<timestamp>` (`0600`) instead of being overwritten.

`credential_store::read_backup(path, key)` decrypts the backup.
[src: file: backend/src/core/credential_store.rs:625-628]

Not migrated in this release: rows that do not decrypt with the current key
are kept untouched (logged as locked) and never deleted by later saves.

## Recovery passphrase, restore and imports

`POST /api/config/recovery/set` wraps the active key under a new passphrase.
When `recovery.key` already exists, the request must carry
`current_passphrase`, which must unwrap it; an unreadable `recovery.key` is
never replaced from the API (move it out of the data directory by hand). A
replaced blob that wrapped another key is kept as
`recovery.previous-<timestamp>.key`. Concurrent sets are serialized and each
write uses its own temporary file.

`POST /api/config/recovery/restore` is for a **locked** instance. On a running
instance it is refused unless the recovered key is the one in use: restoring
another key would split the data between two keys.

An import keeps the source machine's encrypted rows and its recovery blob (as
`recovery.imported-<timestamp>.key`; `recovery.key` is untouched). To read
those secrets, `POST /api/config/recovery/reencrypt` (Plugins → "Re-encrypt
imported secrets") unwraps the source key with that machine's passphrase and
re-encrypts the rows it decrypts under this instance's key, in one
transaction with read-back; rows already under the instance key are never
written, and the instance key never changes. Without a pasted code it tries
every kept blob: imported ones, replaced ones and the local `recovery.key`
(rows may sit under an older local key). The UI picks the flow itself:
restore when the key is locked, re-encrypt otherwise, and submits nothing
before it knows.

An import runs in one transaction: a failing project or MCP insert rolls it
all back, so local MCP secrets are never lost to a half import. GitHub
connections are not exported; those of projects that come back by id are
kept (the `projects` cascade no longer drops their tokens), and the report
names the ones whose project is gone.
[src: file: backend/src/core/keystore.rs:761-811]

## Reset

`POST /api/setup/reset` empties every table holding ciphertext
(`ENCRYPTED_COLUMNS`) along with the other data, except the API auth token,
which a reset keeps (in memory and in the store) so a LAN-bound instance never
answers unauthenticated; a failure clears nothing and is reported. When
`config.toml` carries a needed copy of the key, it is replaced by a file
holding only that key (the next start is a first run with the same key);
otherwise it is removed. A file counts as key-only only when it holds nothing
but `encryption_secret`. The encrypted credential backup is deleted and
`config.toml.backup` is set aside under a timestamped name (it may hold an
older key).

## Downgrading to 0.14.2

**Set a recovery passphrase before downgrading**, and keep the code. 0.14.2
does not know the `stored_credentials` table: provider and connection keys
look absent, and it mints a new auth token in `config.toml`; on returning to
0.14.3 that file token wins over the stored one (the file is newer).

One case can lose the key in use: a key store holding another key (0.14.3
never overwrites it; Settings → Recovery and the boot log name it in
`stale_sources`) with no MCP configuration rows. 0.14.2 then sees no data to
check, adopts that other key, overwrites the sidecar with it and writes it to
`config.toml`. Only `recovery.key` (or your saved code) then holds the key in
use. A reset's key-only `config.toml` cannot be parsed by 0.14.2 either: run
the setup once in 0.14.3 before downgrading. Keep the
`config.toml.pre-credential-store.enc` backup.

## Testing the Keychain path from a dev build

Debug builds skip the OS keychain (an unsigned binary changes identity at each
rebuild, so macOS would prompt on every restart). To exercise the keychain
path, including a denied prompt, start the dev backend with
`KRONN_USE_KEYCHAIN=1`; `KRONN_USE_KEYCHAIN=0` forces the sidecar in a release
build. Outside macOS and Windows the default is off (no keychain backend).
[src: file: backend/src/core/keyvault.rs:231-237]
