# Contact invite code: an address that authenticates, not a secret (KT-1597)

KT-1597 asked whether `contacts.invite_code` (stored in clear,
`backend/src/db/sql/022_contacts.sql:8`) should be hashed like the discussion
invite tokens (`backend/src/api/disc_invite.rs:9-10`, SHA-256, shown once).

## Verdict

- **It does authenticate a peer on its own.** `authenticate_invite_code` looks the
  presented string up and, for an `accepted` contact, returns it with no other
  proof. [src: file: backend/src/db/contacts.rs:79] [src: file: backend/src/db/contacts.rs:90]
  Callers: the WS presence admission, `claim-by-token`, `fetch-file`, the
  file-attachment host resolution.
  [src: file: backend/src/api/ws.rs:821] [src: file: backend/src/api/disc_invite.rs:2390]
  [src: file: backend/src/api/disc_invite.rs:2541] [src: file: backend/src/api/federation.rs:427]
- **It is not a secret.** It is `kronn:pseudo@host:port` (or `kronns:` behind a TLS
  proxy), built from the instance pseudo and advertised address, with no random
  part. [src: file: backend/src/api/contacts.rs:210] It is handed out by
  `GET /api/contacts/invite-code` [src: file: backend/src/api/contacts.rs:252],
  pasted between users, and carried in clear in every Presence / federation message
  (`from_invite_code`).

## Why it is not hashed

- The `pseudo` and `kronn_url` columns sit in clear beside it, and the code is
  exactly rebuilt from them. A hash column would hide nothing from anyone able to
  read the table. Pinned by the test
  `invite_code_is_derivable_from_the_stored_columns_so_hashing_it_protects_nothing`.
- The hash of such a low-entropy string is also trivially brute-forced.
- The clear value is still needed: it is returned to the UI by `list_contacts`,
  re-sent as `from_invite_code` by the outbound client, and used as the host
  identity of file announcements.
  [src: file: backend/src/core/ws_client.rs:174] [src: file: backend/src/api/federation.rs:530]
- Comparing hashes in constant time would defend nothing: there is no secret to
  recover through timing.

## The real weakness

The weakness is not storage, it is that knowing a contact's public address and
pseudo is enough to impersonate it on the auth-exempt P2P routes. Fixing it needs
a per-contact random secret exchanged at pairing (stored hashed, compared in
constant time). That is a protocol change (pairing handshake, migration of existing
contacts, wire format), a product decision, and is out of scope of KT-1597.
