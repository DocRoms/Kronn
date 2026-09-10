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

## Grants

| | |
|---|---|
| **role** | `human` or `orchestrator`, fixed at enrolment, derived from nothing afterwards |
| **who may enrol** | the admin secret, or a live `human` grant |
| **who may not** | an `orchestrator`, ever — it publishes, and that is all |
| **rotation** | possession of the current secret, or an administering authority; the role is absent from the write |
| **revocation** | administering authority only; immediate, and no un-revoke |

An `orchestrator` cannot enrol, list, revoke or rotate anything, including
itself. Letting it would have let the role that administers nothing reach the
role that administers everything, in one step.

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

## Reading the code

| | |
|---|---|
| grants, proofs, bootstrap | `backend/src/db/human_credentials.rs` |
| delivery to the operator | `backend/src/core/operator_secret.rs` |
| enrolment routes | `backend/src/api/human_credentials.rs` |
| authority at publication | `backend/src/db/discussion_important.rs` |
| the append boundary | `backend/src/api/disc_source.rs` |
| schema | `backend/src/db/sql/174_human_publication_credentials.sql` |
