# Orchestrator grant — identity, delivery, resume

> **Superseded by [`docs/architecture/important-message-publication-authority.md`](../architecture/important-message-publication-authority.md).**
> Kept for the reasoning that led there — three proposals were refused,
> and why each failed is worth not rediscovering. Where this note and the
> contract disagree, the contract is right.

Short design for review, 2026-09-09. **Nothing implemented.** Answers the three
open points on converging the card authorities: what the grant is bound to, how
its secret reaches a bridge, and what resume and rotation do to it.

## The decision that makes the rest simple

**A grant is not attached to a session.** It is its own row, with its own id and
its own secret, and a caller *presents* it alongside whatever else it is.

Every remaining question dissolves against that:

| Attack the review raised | Why it stops |
|---|---|
| `join_via_token` replaces an existing session's `resume_token_hash` | It replaces a session. A grant is not a session, so nothing moves. |
| A grant fixed to a public session PK survives that replacement | There is no such binding to survive it. |
| `invite` + `join` with a known public id | Yields a session and a resume credential. Neither is a grant. |
| `transfer` / `rebind` changing a session's home | Home stops deciding anything. |
| Rotation without the old secret | Rotation requires presenting the current secret; there is no path that mints a replacement from an id. |

The earlier hole came from deriving a role from a property of the room. This
derives nothing: the grant is held, or it is not.

## What a grant is

- its own id, a secret stored only as a hash, a **closed role** (`human` or
  `orchestrator`) fixed at enrolment and never derived afterwards;
- an epoch, bumped by rotation and revocation, which invalidates publication
  proofs already in flight;
- a revocation with a reason, and no un-revoke.

**Who may enrol, recover, or assign a role:** the admin secret, or a live
credential whose role is `human`. **An `orchestrator` grant can do neither** —
it publishes, and that is all it does. `authorise_enrolment` must therefore take
the required role, not merely "any live credential" as B1 wrote it. That is a
correction to B1, not a refinement of it.

There is no first caller and no backfill: with no admin secret and no human
credential, an install authorises nobody.

## Delivery to a bridge

The secret is shown **once**, in the response to an enrolment the admin or a
human credential authenticated, and never again.

To reach a bridge it is placed by the operator into the bridge's own private
file, 0600, next to the binding file it already keeps — the same mechanism, the
same permissions, the same "never logged, never shown to the model" rule that
file already carries.

**Never**: an MCP tool result, a tool schema, a log line, a worker environment
variable, or any API that returns it a second time. The bridge reads the file;
nothing hands it the secret through the model's context.

**Who is granted is named explicitly.** The admin or human authorising the
enrolment supplies the label; nothing enrols itself, and no identity is inferred
from a request that happens to arrive.

## Resume, rotation, revocation

- **Resume**: nothing happens. The session reconnects and re-presents the grant
  it holds. It does not recover a grant by claiming to be the same session,
  because the grant was never tied to the session.
- **Rotation**: the row keeps its id and role, takes a new secret against
  presentation of the current one, and bumps its epoch. Proofs issued under the
  old epoch die with it.
- **Revocation**: immediate, and the epoch bump kills proofs in flight.
- **Restart, write failure, anything unexpected**: fail closed. A card that
  cannot be authorised is refused explicitly and the ordinary message survives.

Grant checks and the mutation they authorise run in **one transaction**. A
capability object carried across an `await` and reused afterwards would let a
revocation land in between — which is the B1 flaw the review named, and the
reason the check cannot be a value that outlives its own unit of work.

## Compatibility

**Bridge.** A bridge with no grant file publishes no card and is told so, with
the reason and the remedy. Ordinary messages are unaffected — the same shape as
the missing-session-credential refusal already shipped.

**UI.** The human enrols through the existing settings surface: present the
admin secret or a live human credential, name the grant, receive the secret
once. Revoke and rotate from the same list. Keyboard-operable and translated in
the four locales, like the rest of this lot.

**Existing installs.** No grant exists anywhere today, so no card publishes
until an operator bootstraps. That is fail-closed and deliberate, and it is a
real behaviour change rather than a silent tightening.

**Untouched:** global authentication, ordinary invitations, the MCP catalogue,
and message delivery of any kind.

## What this still does not prove

API identity, not physical presence. By the decided scope, neither OS-level
theft of a secret nor a direct write to the database is defended against. An
operator who leaves the grant file readable has given it away, and nothing here
detects that.
