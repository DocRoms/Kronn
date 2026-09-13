# Who may publish an important card

**Contract as decided, 2026-09-09.** Arbitration
`kt619-authenticated-api-or-os-isolation` → `authenticated-api`, then
`kt619-human-credential-bootstrap` → `dedicated-human-credential`.

This replaces the earlier design notes, which argued a threat model the
decision narrowed. Where they disagree with this page, this page is right.

## The short version

An important card needs two things the caller must **hold**, not name:

1. an **enrolled grant** — its own row, its own secret, a closed role;
2. a **single-use proof** issued for that room, that exact body, with an expiry.

Neither can be reached by inviting, joining, transferring or rebinding, because
a grant is attached to no session.

## What this proves, and what it does not

**Proves:** the caller holds a credential enrolled by an authority the operator
established, and the publication it is making is the one the server issued a
proof for.

**Does not prove:** physical presence. And by the decided scope, neither
OS-level theft of a secret nor a direct write to the database is defended
against — an operator whose grant file is readable has given it away, and
nothing here detects that.

None of this should ever be described as more than it is.

## The bootstrap

Kronn mints one admin secret, writes it to the operator's private directory
(the host config directory, or the Docker volume — the same place the database
lives), and keeps only its hash.

- mode `0600` at creation, written to a temporary file, fsynced, renamed, and
  the directory fsynced too;
- **the file is written before the row is committed.** A crash between the two
  must never leave a secret in the database the operator has no copy of: that
  install could never enrol, with a hash claiming otherwise. The reverse — a
  file whose secret authenticates nothing — is cleaned up;
- read back only if it is a regular file, owned by this process, readable by
  nobody else. A symlink is refused rather than followed;
- the path comes from server configuration. **No API supplies it**, there is no
  fallback and no search.

There is exactly one. A second would silently authorise enrolments nobody
sanctioned, so minting a second is refused; rotate instead.

**There is no trust-on-first-use.** An install with no admin secret and no
credential authorises nobody, including whoever asks first. A window the first
caller wins is not an authorisation.

## Rotating and recovering it

Two doors, and they answer different questions.

**Still holding it.** `POST /api/human-credentials/admin/rotate` takes the
current admin secret and mints its replacement. The response carries a **path**,
never a secret: the new plaintext goes to the operator's private file and over
no wire at all. Settings offers the same thing as a button, and locks the screen
back afterwards — the authority the operator just typed is dead, and leaving it
on screen would say otherwise.

Authenticated as the **admin and nothing else**. A `human` grant administers
credentials — it enrols, lists and revokes — but rotating the bootstrap is a
different power: allowing it would let a laptop the operator enrolled lock the
operator out of their own install.

**Lost it.** No route can help, and that is the point: an API that re-delivers
the admin secret to whoever asks is the hole this whole contract closes. The way
back is a file named `recover-admin-secret` in the private directory, honoured
at the next boot. Creating it proves write access to the directory that already
holds the database and the secret — available precisely to the person who can no
longer authenticate, and to nobody who cannot already take everything.

The request is consumed **after** the new secret is committed and on disk, so a
failed attempt is retried on the next boot rather than swallowed. The cost of
that ordering is a crash between the commit and the removal rotating a second
time, which delivers another usable secret rather than losing one.

Either way, **every credential already enrolled keeps working.** Rotation moves
the power to enrol NEW ones; it is not a revocation sweep, and an operator
rotating a secret they still hold must not discover they have logged out every
device they enrolled.

A rotation that cannot write its row returns before it touches the file, and one
that cannot commit puts the previous file back — unless that file was readable
by others, in which case it is already given away and restoring it would restore
exactly what was being rotated away from.

## Grants

| | |
|---|---|
| **role** | `human` or `orchestrator`, fixed at enrolment, derived from nothing afterwards |
| **who may enrol** | the admin secret, or a live `human` grant |
| **who may not** | an `orchestrator`, ever — it publishes, and that is all |
| **rotation** | possession of the current secret, or an administering authority; the role is absent from the write |
| **revocation** | administering authority only; immediate, and no un-revoke |

An `orchestrator` cannot enrol, list or revoke anything. It **can** rotate its
own secret, by presenting that secret — and only its own: a credential aimed at
another's id is refused, which the route tests check both ways.

That exception is deliberate and narrow. Rotating one's own secret changes
nothing about what one may do, so it is not a step towards privilege; refusing
it would mean a compromised orchestrator could not replace its own secret
without an administrator, which costs availability and buys nothing.

Rotation and revocation bump an epoch, and proofs carry the epoch they were
issued under — so everything in flight dies at once rather than being hunted
down.

## Proofs

Issued to a live grant, over the exact body about to be posted, for one room,
with a short expiry. Spent inside the same transaction as the message, the card
and its targets: consuming a proof in one unit and writing the card in another
would burn it on a publication that never landed.

Refused, and none of the refusals says which: expired, already spent, issued for
another room, issued over a different body, or minted under an epoch its
credential has since left.

**The worker refusal runs first**, before the proof is touched. A principal that
holds a grant and is later delegated must not publish steering cards while
working on someone else's task — and because the check comes first, its proof
survives to be spent once it is no longer working. That is read from the exact
active assignment, never from the room: a room is transferable, and reading it
as a role is what an earlier version got wrong.

## Delivery to a bridge

The secret is shown **once**, in the response to the enrolment that created it,
and no route returns it again.

To reach a bridge, the operator places it in that bridge's private directory
beside the binding file it already keeps: `0600`, one fixed name, no search. The
bridge refuses anything that is not a private regular file it owns.

**Never** an MCP tool result, a tool schema, a log line, or a worker environment
variable.

## What happens without any of it

Nothing breaks, and nothing publishes. A missing grant, an unreadable one or a
refused proof loses the **card** and posts the **message**, with the reason and
the remedy. An ordinary message asks for nothing at all.

**Every existing install starts here.** No grant exists anywhere, so no
important card publishes until an operator bootstraps. That is deliberate and it
is a real behaviour change, not a silent tightening.

## Queued messages and interrupted publication

The UI obtains a fresh proof immediately before each queued send, including a
retry after a lost receipt. The grant, proof and abort controller stay in memory;
the durable outbox stores only the message, its targets and its stable UUID.
The server returns the existing receipt for that UUID rather than creating a
second message/card, even when the retry supplies a new proof. A previously
refused card is not added retroactively on replay.

Preparation and an HTTP write already started are different states. Removing
an entry, clearing the queue, leaving the room or unmounting the page cancels
pending preparation; a late proof cannot send the abandoned message. Navigation
preserves unsent text. Stop and changing the credential pause unsent entries
until an explicit retry. A write already started stays tracked until its receipt
or a retryable error: cancellation cannot undo a server commit.

Reload restores text and the same UUID, not publication authority. Without a
newly entered grant the restored message is ordinary text. The refusal notice
does not claim the message was delivered before an acceptance receipt arrives.
[src: file: frontend/src/hooks/useMessageQueue.ts:1]
[src: file: frontend/src/lib/importantPublication.ts:1]
[src: file: frontend/src/pages/DiscussionsPage.tsx:1]
[src: file: backend/src/api/discussions/messaging.rs:3345]

## Reading the code

| | |
|---|---|
| grants, proofs, bootstrap | `backend/src/db/human_credentials.rs` |
| delivery to the operator | `backend/src/core/operator_secret.rs` |
| enrolment routes | `backend/src/api/human_credentials.rs` |
| authority at publication | `backend/src/db/discussion_important.rs` |
| the append boundary | `backend/src/api/disc_source.rs` |
| schema | `backend/src/db/sql/174_human_publication_credentials.sql` |
