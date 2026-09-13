# Publication authority for important messages — threat model and options

> **Superseded by [`docs/architecture/important-message-publication-authority.md`](../architecture/important-message-publication-authority.md).**
> Kept for the reasoning that led there — three proposals were refused,
> and why each failed is worth not rediscovering. Where this note and the
> contract disagree, the contract is right.

Design note for arbitration. **Nothing here is implemented**, no migration is
reserved, and no table, endpoint or key exists.

Rewritten after review found that the first draft assumed `Orchestrator` was
already authenticated, and left the enrolment bootstrap, recovery and
authenticator guarantees unaddressed. Each of those alone was enough to sink it.

## What we are protecting

The claim that a card was published by a given authority. This started as a
question about `Human`. It is not: `Orchestrator` rests on the same weakness.

## Threat model, as the human decision bounded it

`kt619-authenticated-api-or-os-isolation` was answered **`authenticated-api`**:
distinct API identities with proof of possession, and **credential theft or
direct DB writes by the same OS account are explicitly out of scope**.

That settles what earlier drafts kept re-litigating. The adversary here is a
caller that **presents** an identity it does not hold — a worker naming an
orchestrator's session, a payload claiming a role, a request omitting what it
cannot produce. It is not a process rifling through another's files.

Also out of scope: remote callers (`auth_middleware` already requires a bearer
for any non-local address), other OS users, and root.

**Both authorities are required.** Deferring `Human` is not an available answer,
and no waiver was granted.

**Historical note.** Three proposals were refused before the decision narrowed
the model, and the reason is worth keeping: under an adversary that reads the
same user's files, any secret it can read is not a boundary.

| Candidate | Why it was refused |
|---|---|
| `auth_strict_localhost` bearer | One shared secret; every holder looks the same. Still true, and still not an identity. |
| A capability ticket from an issuing endpoint | The issuing endpoint had no identity to check either — the hole moved one step, it did not close. |
| A pairing code on the server's terminal | Rested on stdout never being captured, which the server cannot promise about itself. |

The first two fail on their own terms and stay refused. The third fails only
against file-reading, which the decision now excludes — but it is still weaker
than a real credential, so it is not revived.

## Step 0 — proof of possession — **DONE** (`4b95e5df`)

**A prerequisite, not an option.** The first draft called it orthogonal; that was
wrong. Without it neither option below means anything, because the authority is
chosen by a string the caller types.

`DiscAppendRequest.session_id` arrives in the JSON body and the handler extracts
no authenticated identity. Resolving it proves **the row exists**, never that
the caller holds it — and session ids are visible in room metadata, so a worker
only has to read one and present it.
`disc_append_refuses_a_worker_presenting_an_orchestrators_session_id` records
this; run with `--ignored` it fails on `left: Some(1)`.

The credential already exists: `discussion_sessions.resume_token_hash`, minted
by `new_resume_credential`, used by the join and resume paths. `disc_append`
does not use it. Requiring it there changes one decision point, not the auth
model — but the bridge must send the token it already holds, which is a
transport contract change.

Residual under this threat model: the token sits where the same OS user can read
it. It still moves the bar from *anyone local who read a session id* to *a
process that took another process's credential* — the ordinary bar everywhere
else, and the difference between a mistake and an intrusion.

## Option A — platform-verified user presence (WebAuthn)

A passkey with `userVerification: "required"`; publishing a `Human` card needs
an assertion whose challenge is bound to discussion id, content hash, single-use
nonce and short expiry. The private key lives in the authenticator or platform
keystore, never in browser storage or a file. No home-made cryptography: a W3C
standard with a maintained Rust server implementation.

### A.1 — Who authorises the first enrolment — **superseded**

This note proposed trust on first use: enrolment while the credential set is
empty, sealed afterwards. That was written under the earlier threat model, where
every bootstrap secret was assumed readable by the adversary, so a ceremony was
the only thing left.

**The decided model removes that assumption**, and with it the reason for TOFU.
Credential theft by the same OS account is out of scope, so a secret CAN be the
bootstrap — provided it never travels where a worker can pick it up.

The pending arbitration (`kt619-human-credential-bootstrap`) is therefore the
live proposal, not this section: a dedicated admin secret delivered out of band
through operator private storage, never through an API, MCP, log or worker
environment. TOFU is explicitly rejected there, and rightly — a window anyone
can walk through first is not an authorisation, it is a race.

Recorded rather than deleted, because the reasoning shows why the answer changed
with the model rather than with anyone's preference.

### A.2 — Recovery — **superseded**

Same fate. This note proposed a recovery code held off the machine, which was
the only artefact that survived an adversary reading the disk. Under the decided
model recovery can rest on the same out-of-band admin secret, and the pending
card requires it to be protected with **no anonymous reset** — closing the
break-glass this section left open.

### A.3 — What the authenticator actually guarantees

`userVerification: "required"` is a *request*. The relying party must verify the
**UV flag in the assertion** and refuse when it is absent; asking without
checking proves nothing.

Even then, virtual and software authenticators exist, and a process able to
drive the user's browser through devtools can register one. Attestation could
restrict enrolment to known platform authenticators, at the cost of breaking
legitimate setups — probably not worth it here, but it is the lever if the
threat model tightens.

**Residual:** an adversary that can drive the real browser session defeats this,
as it defeats any browser-based authentication. WebAuthn proves presence and
possession, not attention: someone who approves without reading approves
whatever was put in front of them.

### A.4 — Compatibility

| Surface | Works? | Condition |
|---|---|---|
| Browser to `http://localhost` | Yes | Browsers treat `localhost` as a secure context, so no TLS needed. |
| Browser over LAN / Tailscale, plain HTTP | **No** | Not a secure context. Needs TLS, or this surface keeps Option B. |
| Desktop app | Depends | Only if its web view exposes WebAuthn. To verify before committing. |
| Docker | Yes, with care | The ceremony runs in the user's browser; the RP ID must match the origin actually visited, so several entry origins mean several enrolments. |

**Surfaces touched:** one dependency, an enrolment endpoint pair, a credentials
table, a recovery code, and a verification at the single publication point.
`auth_middleware` is not modified.

## Option B — defer `Human` — **withdrawn**

Earlier drafts proposed shipping `Orchestrator` alone and documenting the rest
as a limitation. The human decision requires **both** authorities and granted no
waiver, so this is no longer an available answer. It is recorded here only so
the option is not re-proposed a fourth time.

## What each covers, under the decided model

| Threat | Step 0 (done) | Human authority (A) |
|---|---|---|
| A caller presents a session id it does not hold | **covered** | covered |
| A caller claims a role in the payload | **covered** | covered |
| A caller omits what it cannot produce | **covered** (refused, and told to reload) | covered |
| An authenticated worker publishing as orchestrator | **covered** | covered |
| A worker publishing as the human | not applicable | **the remaining gap** |
| Credential theft / direct DB write, same OS account | out of scope by decision | out of scope by decision |
| A person approving without reading | no | no |

## Where this leaves `Human`

**The live proposal is the pending card**, not the options below: a dedicated
admin secret out of band, a distinct and revocable human credential, single-use
publication proofs bound to discussion, content and expiry, protected recovery
with no anonymous reset, and a stated Web/Desktop/Docker contract. What follows
is the reasoning that led there.

Step 0 gives every CLI caller a real identity. It does **not** give the browser
one: `send_message` takes no caller identity at all, and there is no artefact a
browser holds that a local process could not also present.

So `Human` needs its own credential, enrolled and proven per publication —
bound to discussion id, content hash, single-use nonce and expiry, the four
links already agreed. The open questions are A.1 to A.3 above: who authorises
the first enrolment, how recovery works, and what the authenticator actually
guarantees. Under the decided model those get easier, because an enrolment
artefact no longer has to survive an adversary reading it off the disk — but
they still have to be answered before anything is built.

Migration 174 is reserved for that table and remains unopened.
