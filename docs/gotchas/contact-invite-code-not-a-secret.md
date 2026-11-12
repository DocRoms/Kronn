# Contact invite code: an address that authenticates, not a secret (KT-1597, fixed by KT-1650)

KT-1597 asked whether `contacts.invite_code` (stored in clear,
`backend/src/db/sql/022_contacts.sql:8`) should be hashed like the discussion
invite tokens (`backend/src/api/disc_invite.rs:9-10`, SHA-256, shown once).

## Verdict (KT-1597)

- **It authenticated a peer on its own.** The old `authenticate_invite_code`
  looked the presented string up and, for an `accepted` contact, returned it with
  no other proof, on the WS presence admission, `claim-by-token`, `fetch-file` and
  the file-attachment host resolution.
- **It is not a secret.** It is `kronn:pseudo@host:port` (or `kronns:` behind a TLS
  proxy), built from the instance pseudo and advertised address, with no random
  part. [src: file: backend/src/api/contacts.rs:398] It is handed out by
  `GET /api/contacts/invite-code` [src: file: backend/src/api/contacts.rs:440],
  pasted between users, and carried in clear in every Presence / federation message
  (`from_invite_code`).

## Why it is not hashed

- The `pseudo` and `kronn_url` columns sit in clear beside it, and the code is
  exactly rebuilt from them. A hash column would hide nothing from anyone able to
  read the table. Pinned by the test
  `the_public_invite_code_alone_authenticates_nobody`.
- The hash of such a low-entropy string is also trivially brute-forced.
- The clear value is still needed: it is returned to the UI by `list_contacts`,
  re-sent as `from_invite_code` by the outbound client, and used as the host
  identity of file announcements.
  [src: file: backend/src/core/ws_client.rs:174] [src: file: backend/src/api/federation.rs:601]

## The fix (KT-1650)

The code now only **names** a contact. What authenticates it is a per-contact
random secret exchanged at pairing, stored hashed and compared in constant time,
required on every route that authenticates a peer. Existing contacts re-pair
(owner decision, no compatibility with older peers). Design, flow and the route
table: [P2P contact pairing](../architecture/p2p-contact-pairing.md).

Rules that follow, to keep when touching P2P:

- Never add a route, a frame or a lookup that turns an invite code into an
  authenticated contact. Go through `authenticate_peer`, or through the contact id
  an admitted connection already holds.
  [src: file: backend/src/db/contacts.rs:143]
- Never trust the `from_invite_code` of a relayed frame to pick a contact (the
  FileAttached host is the authenticated peer).
- Never return `outbound_secret` or `inbound_secret_hash` through an API, an
  export or the local bus.
