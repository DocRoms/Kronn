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
     [src: file: backend/src/core/config.rs:77-97]
  2. **Locked or missing key at boot.** The store is not armed and
     `config.toml` is left as it is, so credentials an install had in clear
     before the migration stay there until the key is restored. No new
     credential is written in clear: a save that would add one is refused.
     [src: file: backend/src/core/credential_store.rs:487-505]
     [src: file: backend/src/core/credential_store.rs:349-367]
  3. **Set-aside `config.toml`.** An unreadable `config.toml` is moved to
     `config.toml.corrupt.<timestamp>` (`0600`) and kept; it may hold
     credentials in clear. Its credentials join the boot, and the notice tells
     the operator to delete the file once checked; Kronn does not delete it.
     [src: file: backend/src/core/config.rs:122-160]
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
- **Suggested direction (non-binding)**: make the keychain-less case visible
  and actionable (Settings → Recovery already reports `config_holds_key`;
  push the operator to set a recovery passphrase or `KRONN_ENCRYPTION_KEK`
  from a secret manager), and offer to delete a set-aside file once its
  credentials are stored.
- **Next step**: create ticket
