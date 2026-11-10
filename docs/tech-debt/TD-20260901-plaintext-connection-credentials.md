# TD-20260901-plaintext-connection-credentials

- **ID**: TD-20260901-plaintext-connection-credentials
- **Area**: Backend / Security
- **Problem (fact)**: Since 0.14.3 (KT-1007), connection credentials are no
  longer in plaintext `config.toml`. `ExternalApiConnection.credential_slug`
  names an `ApiKey` in `TokensConfig.keys`; `credential_store::boot` fills
  those keys from the `stored_credentials` table (AES-256-GCM, instance key),
  and every `config::save` writes them back to the table before writing
  `config.toml` without them. Three residual cases still leave a credential,
  or the key that decrypts it, in clear on disk:
  1. **Key kept in `config.toml` on keychain-less hosts.** `config.toml` keeps
     `encryption_secret` until two independent durable copies exist (two
     vaults, or one vault plus a verified recovery passphrase). A sidecar-only
     install (Linux, WSL, Docker, dev builds) without a verified passphrase
     keeps it there, next to the database it decrypts: the encryption then
     protects no more than the file permissions (`0600`).
     [src: file: backend/src/core/keystore.rs:305-325]
     [src: file: backend/src/core/config.rs:164-212#DISK_KEYS]
  2. **Locked or missing key at boot.** The store is not armed and
     `config.toml` is left as it is, so credentials an install had in clear
     before the migration stay there until the key is restored. No new
     credential is written in clear: a save that would add one is refused.
     [src: file: backend/src/core/credential_store.rs:484-506]
     [src: file: backend/src/core/credential_store.rs:348-366]
  3. **Set-aside `config.toml`.** An unreadable `config.toml` is moved to
     `config.toml.corrupt.<timestamp>.enc` (`0600`), encrypted with the
     instance key (KT-1528); with no key to encrypt with, only its key line is
     kept; when the key stores cannot be read, it is moved unchanged (`0600`) and
     encrypted at the next start. Its credentials join the boot, and the notice tells the operator to
     delete the file once checked; Kronn does not delete it.
     [src: file: backend/src/core/config.rs:412-467#set_aside_unreadable_config]
     [src: file: backend/src/core/config.rs:1109-1121#set_aside_unreadable_config]
- **Why we can't fix now (constraint)**: All three are deliberate KT-1007
  trade-offs against losing the key or a credential: dropping the file copy
  of the key without two durable copies, rewriting `config.toml` under a
  locked key, or deleting a file that may be the only copy of a key would
  each risk unrecoverable loss. They are documented in
  `docs/operations/key-management.md`: "Where secrets live" (key row), "Boot
  order" steps 1, 3 and 4, and the paragraph "While the store cannot be armed"
  that ends "Boot order".
- **Impact**: security
- **Where (pointers)**: `backend/src/core/keystore.rs` (copy retention),
  `backend/src/core/config.rs` (`disk_toml`, `load` set-aside),
  `backend/src/core/credential_store.rs` (`boot`, `sync_for_save`).
- **Suggested direction (non-binding)**: done by KT-1594 for the visibility
  and the set-aside file (see Next step). What remains is the design choice of
  keeping the key in `config.toml` on a keychain-less host without a verified
  recovery passphrase.
- **Status**: partly closed by KT-1594. Case 1 is flagged (banner and Settings →
  Recovery, `config_holds_key`) and leads to a recovery passphrase or
  `KRONN_ENCRYPTION_KEK`; case 2 lists the credentials still in clear while the
  key is locked; case 3 is deletable by the owner only when nothing in the
  file can be lost (every leaf value is a credential verified in
  `stored_credentials` or sits at a place of a closed list of non-secret
  settings; an unknown field, a webhook URL, a line that does not parse, a
  sealed copy, or a key some file still needs blocks it), behind the strong
  confirmation (KT-1496). [src: file: backend/src/core/set_aside.rs:1-25]
  The key itself is still kept in `config.toml` in case 1 until two durable
  copies exist.
- **Next step**: close when case 1 no longer needs the file copy (a keychain
  or `KRONN_ENCRYPTION_KEK` is the norm), or accept the residual risk.
