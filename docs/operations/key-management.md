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
[src: file: backend/src/core/keystore.rs:44-61]

The running process still reads `config.tokens.keys` and
`config.server.auth_token`: the boot fills them from the table, and every
`config::save` writes them back to the table (encrypted, read back) before it
writes `config.toml` without them. A failure there fails the save and leaves
the previous file untouched.
[src: file: backend/src/core/credential_store.rs:332-387]
[src: file: backend/src/core/config.rs:234-258]

## Boot order

1. The backend takes the data-directory lock, then `config::load` reads
   `config.toml`. It never mints a key or an auth token. A missing
   `config.toml` starts from defaults **without** a key, so no random key is
   ever offered to the reconciler. Values that are not a 32-byte hex key are
   ignored as candidates. A `config.toml` that cannot be parsed is moved to
   `config.toml.corrupt.<timestamp>` (`0600`, kept: it may hold the key), a
   top-level `encryption_secret` line is salvaged, and the start continues
   as a first run. The same applies to valid TOML that is not a Kronn
   configuration (a field of another type, a value from a newer Kronn); the
   log names the cause, and the setup wizard (`config_set_aside` in
   `GET /api/setup/status`), a dismissible app-wide notice and Settings →
   Recovery say where the file went and whether a valid key was recovered.
   Credentials of a parseable file (`[server].auth_token` with
   `auth_enabled`, `[tokens]` including the legacy single keys) join the
   credential boot; the notice says the kept file holds credentials in
   clear and can be deleted once checked.
2. The database opens, then the reconciler picks the key by decrypt
   self-test across every registered column:
   - a vault that cannot be read (denied keychain prompt, locked keychain,
     unreadable sidecar; a non-text sidecar is reported as corrupted, with
     "move it aside") **stops the boot** with a message naming the vault
     and what to do; nothing is minted or written. The desktop app shows the
     same message on its "Kronn could not start" screen (startup error →
     `wait_for_backend` → bootstrap failure panel, with Retry); for the
     keychain it says to choose Allow, or to start with
     `KRONN_USE_KEYCHAIN=0` (`open --env KRONN_USE_KEYCHAIN=0 -a Kronn`); a
     damaged keychain item (bad encoding, duplicates) is reported as such,
     naming `com.kronn.kronn` / `encryption_secret_v1` and when it may be
     deleted in Keychain Access;
   - the `encryption_secret` of `config.toml.backup`, its rotated copies,
     every `config.toml.retired-key.<timestamp>` and every
     `config.toml.corrupt.<timestamp>` (salvaged line) are lowest-priority,
     read-only candidates of every decision (sources `config-backup`,
     `retired-key`, `corrupt-config`). When one live key store decrypts data
     and the other decrypting keys survive only in such files, their rows
     are re-encrypted under the live key at boot (checked by read-back, one
     transaction per key), the files are kept, and the log and
     `rows_moved_from_files` in `recovery/status` name each file. With no
     live key, the file key decrypting the most rows becomes the key in use
     and the others move under it the same way. A move waits until the
     target key has a durable copy (a vault or `config.toml` reads it back),
     and a failed move changes nothing: the boot continues and
     `file_key_moves_pending` says it is retried at the next start;
   - a key is minted only when no registered column holds ciphertext;
   - two distinct keys from live key stores that each decrypt some rows (say
     the keychain holds K1 and the sidecar K2) **stop the boot** with nothing
     written; the
     message names each source, its key fingerprint and how many rows it
     decrypts. Kronn never picks one, and never overwrites a vault that holds
     a different key than the one in use. Every row is scanned, so a second
     key whose rows are all old is still found. To keep both halves, start
     Kronn with `KRONN_REENCRYPT_FROM=<fingerprint of the key to retire>` (macOS
     app: `open --env KRONN_REENCRYPT_FROM=<fingerprint> -a Kronn`): its
     rows (and the encrypted `config.toml` backup) are re-encrypted under the
     first other key listed, in one transaction checked by read-back and
     rolled back on any failure, then the boot continues; no key copy is
     deleted. If moving the backup fails (disk full) the boot still
     continues and the next start moves it. With three or more keys, repeat once per key to retire;
   - rows exist and no key decrypts them: locked state, nothing overwritten,
     and no key in memory (fail closed: nothing new is encrypted under a key
     no vault holds). Kronn keeps running so the key can be restored from
     Settings → Recovery.
   [src: file: backend/src/core/keystore.rs:440-682]
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
   it. Another value in `config.toml` (a retired key, or not a key at all)
   decrypts nothing at that point, so it is moved to
   `config.toml.retired-key.<timestamp>` (`0600`) and the key in use gets the
   file copy it needs. Before anything is encrypted, the key in use must
   have a durable copy: a vault reads it back, `config.toml` already holds
   it, or `config.toml` is written with it and read back; otherwise the boot
   stops ("No durable copy of the encryption key could be written") with
   nothing encrypted. A value config.toml still holds for itself (its
   set-aside failed) is never overwritten: the boot stops the same way.
   Every key, recovery, config and config-backup file is written through one
   helper: owner-only temp, fsync, rename, then fsync of the directory.
   `GET /api/config/recovery/status` reports `matches_key`, `key_locked`,
   `key_copies_kept` (the retention rule keeps the file copy),
   `config_holds_key`, `copies`, `stale_sources` (key stores holding another
   key, never overwritten; also refreshed after a restore),
   `invalid_sources`, `locked_credentials`, `kept_recovery_blobs`,
   `credentials_unavailable`, `recovery_other_key`, `recovery_unverified`,
   `recovery_damaged`, `config_set_aside`, `rows_moved_from_files`,
   `file_key_moves_pending`, `file_key_rows_pending`, `undecryptable_rows`
   and `locked_file_rows`; pending file-key moves are shown as "nothing to
   do"; Settings → Recovery shows the warnings, and the
   app-wide banner shows a locked key, `credentials_unavailable` and
   `config_set_aside`.
   [src: file: backend/src/core/keystore.rs:284-332]
4. `credential_store::boot` loads the stored credentials, moves any still in
   `config.toml` into the table, and generates an auth token only when none
   exists anywhere. Locked key: the store stays off and `config.toml` is left
   as it is. If this step fails before loading the token while a token is
   stored, auth is locked (below), never open. The stored token row also
   records whether auth is enabled, so a lost `config.toml` does not turn
   auth off. [src: file: backend/src/core/credential_store.rs:452-587]

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
the migration). Token, provider-key, connection-key and LiteLLM changes, and discovered keys
(route and both start-ups), are applied to the live config only once saved;
a failed save is reported, not answered as success.

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
   `config.toml.backup.<timestamp>` (`0600`) instead of being overwritten,
   with its credentials removed and its key kept. Every credential boot also
   scrubs `config.toml.backup` and every rotated copy (copies rotated by an
   earlier 0.14.3 start included), under the same copies rule for the key;
   a copy that cannot be scrubbed does not stop the others, and the errors
   are reported together.

`credential_store::read_backup(path, key)` decrypts the backup.
[src: file: backend/src/core/credential_store.rs:699-702]

Not migrated in this release: rows that do not decrypt with the current key
are kept untouched (logged as locked) and never deleted by later saves.

## Recovery passphrase, restore and imports

`POST /api/config/recovery/set` wraps the active key under a new passphrase.
When `recovery.key` already exists, the request must carry
`current_passphrase`, which must unwrap it; an unreadable `recovery.key` is
never replaced from the API (move it out of the data directory by hand).
The exceptions are a blob whose checksummed fingerprint names **another** key
(`recovery_other_key`, after a reset or a key change) and one whose payload
is damaged (`recovery_damaged`): neither can protect the key in use, so it is
replaced without a passphrase and Settings asks for none. A blob from before
0.14.3 (`recovery_unverified`), or one that matches but whose passphrase is
forgotten, needs its passphrase or an explicit confirmation
(`replace_unverified`, alias `replace_confirmed`). Every
replaced blob is kept as `recovery.previous-<timestamp>.key`, and a restore
without a pasted code tries `recovery.key` then those kept blobs, so an old
passphrase still opens its file. Concurrent sets are serialized and each
write uses its own temporary file.

`POST /api/config/recovery/restore` is for a **locked** instance. On a running
instance it is refused unless the recovered key is the one in use: restoring
another key would split the data between two keys. A successful restore
clears a credential failure recorded at start; one whose credential load
fails records it, so the status and the locked screen ask for a restart.
Another value `config.toml` held is set aside as
`config.toml.retired-key.<timestamp>` before the restored key takes the file
copy; if that fails, nothing changes.

An import keeps the source machine's encrypted rows and its recovery blob (as
`recovery.imported-<timestamp>.key`; `recovery.key` is untouched). To read
those secrets, `POST /api/config/recovery/reencrypt` (Plugins → "Re-encrypt
imported secrets") unwraps the source key with that machine's passphrase and
re-encrypts the rows it decrypts under this instance's key, in one
transaction with read-back; rows already under the instance key are never
written, and the instance key never changes. Stored credentials among the
rewritten rows are reloaded at once (a failed reload is recorded as above).
It also reads back every `locked-secrets-*.json` (see below) with the keys it
tries plus the key stores and kept files: each row one of them decrypts goes
back under the key in use (an MCP config only while its secret is still
empty, any other row only when its primary key is absent), one transaction
per file with read-back. A file whose rows are all back is renamed
`.restored` only once those rows are loaded; until then (or when the reload
fails) the restored credentials are kept out of every save's deletes. A row
whose parent is gone stays in its file, any other database error stops the
restore, a file that cannot be read is reported and skipped, and an
unreadable key store is reported, never taken as "no key". The toast counts
the rows put back and says how many still wait. Settings offers it whenever rows remain that a passphrase or code
can bring back (undecryptable rows the next start does not move by itself, or
`locked_file_rows`), with or without a kept blob (a pasted code works). Without a pasted code it tries
every kept blob: imported ones, replaced ones and the local `recovery.key`
(rows may sit under an older local key). The UI picks the flow itself:
restore when the key is locked, re-encrypt otherwise, and submits nothing
before it knows.

An import runs in one transaction: a failing project or MCP insert rolls it
all back, so local MCP secrets are never lost to a half import. GitHub
connections are not exported; those of projects that come back by id are
kept (the `projects` cascade no longer drops their tokens), and the report
names the ones whose project is gone.
[src: file: backend/src/core/keystore.rs:1642-1703]

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
older key). A reset while the key is locked also deletes the token row, which
no key can read, then resolves the key and arms the credential store at once
(adopting a key a store holds, or minting one), as a fresh start would. This
runs on a copy of the live config in restore mode (auth is never turned on or
off), adopted only on success and only when a key results: otherwise no key
stays in memory and the previous auth (an operator session token) is kept.
Only a session token is replaced; a readable `config.toml` token is stored
like any other credential. A start never turns off an auth already on.
`config.toml` is then left for a first run, so the wizard opens.

When the key is lost for good (no passphrase, no code),
`POST /api/config/recovery/start-new-key` (locked screen and banner, behind a
confirmation; key locked; a local caller, or one with the API token, and only a
local caller while auth is locked) first reads every key store strictly (an
unreadable one stops it before anything is written), copies every encrypted
row no known key decrypts into an owner-only `locked-secrets-<timestamp>.json`
in the data directory (read back), removes those rows in one transaction,
each matched by primary key and the copied ciphertext (a row changed
meanwhile rolls everything back; an MCP config keeps its settings and loses
only its secret values), then resolves a new key as above. Discussions,
projects and workflows stay; "Re-encrypt" puts the kept rows back once the
old passphrase or code is offered. If the removal rolls back, every row is
still in the database and the file is renamed `.unused`. The new key is
adopted only once the credentials are stored (or auth is off, or a token
serves); a credential boot that fails with auth on and no token locks auth.
Keep `locked-secrets-*.json`: an export does not carry it, and says so
(`X-Kronn-Export-Warning: locked-secrets-not-exported`). The response names the file and, with auth on,
the new API token.

When the key is in use but the stored credentials fail to load at start
(disk full, unreadable `config.toml`), auth is locked and enabled (fail
closed), `recovery/status` reports `credentials_unavailable`, the locked
screen says to fix the cause and restart (with auth off, the app-wide banner
and Settings say it), and credential changes are refused with that reason.
The locked screen reads only a 423 `auth_locked` answer as a remote caller;
any other failure is retried. Settings → Recovery shows every computed warning
(another or an invalid value in a key store, a single copy without a
passphrase, stored credentials no key reads). An export bundles
`recovery.key` only when it is verified for the key in use. When it carries
encrypted MCP secrets without a verified blob, the `X-Kronn-Export-Warning`
header says why (`no-recovery-passphrase`, `key-locked`,
`recovery-not-bundled`) and Settings shows it.

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
[src: file: backend/src/core/keyvault.rs:298-307]
