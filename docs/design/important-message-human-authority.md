# Publication authority for important messages — threat model and options

Design note for arbitration. **Nothing here is implemented**, no migration is
reserved, and no table, endpoint or key exists.

Rewritten after review found that the first draft assumed `Orchestrator` was
already authenticated, and left the enrolment bootstrap, recovery and
authenticator guarantees unaddressed. Each of those alone was enough to sink it.

## What we are protecting

The claim that a card was published by a given authority. This started as a
question about `Human`. It is not: `Orchestrator` rests on the same weakness.

## Threat model

**The adversary is a process running as the same OS user, on the same machine.**
Concretely: a CLI worker Kronn itself launched. It can read the whole user
filesystem, make loopback HTTP calls, and read anything the server or the
browser writes down.

Out of scope: remote callers (`auth_middleware` already requires a bearer for
any non-local address), other OS users, and root.

**The consequence:** any secret readable by the same OS user is not a boundary.

| Candidate | Why it fails |
|---|---|
| `auth_strict_localhost` bearer | One shared secret; a worker holding it is indistinguishable from the person. |
| A capability ticket from an issuing endpoint | The endpoint has no identity to check either, so a worker omits its session and asks for one. |
| A pairing code on the server's terminal | Terminal output, supervisor logs and browser storage are all reachable by the same OS user. "Never logged" is not a property the server can guarantee about its own stdout. |

## Step 0 — proof of possession, before anything else

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

### A.1 — Who authorises the FIRST enrolment

The gap that sank the first draft. If any local caller can enrol the first
passkey, the adversary enrols its own and *becomes* the human.

There is no way to close this from inside the machine: every bootstrap secret
lands somewhere the same OS user can read. The honest construction is **trust on
first use, made explicit and narrow** — the same shape as an SSH host key:

- enrolment is possible **only while the credential set is empty**;
- the window is opened deliberately during setup, and is expected to happen
  **before any agent has ever run on that install**;
- once a credential exists the set is **sealed**: adding, replacing or removing
  one requires an assertion from an existing credential;
- the UI states plainly which install enrolled and when, so a surprise enrolment
  is visible rather than silent.

The guarantee therefore *begins* at enrolment. If an adversary enrols first it
wins, and no later mechanism recovers from that. That belongs in the product's
own words, not in a user's eventual discovery.

### A.2 — Recovery

A lost passkey must not mean a lost install, and recovery must not reopen A.1
for whoever asks.

A recovery code, shown **once** at enrolment, stored **off the machine** —
password manager, paper. It is the only artefact in this design deliberately not
on disk, because it is the only one that survives the adversary owning the disk.
Presenting it reopens the enrolment window once and invalidates itself.

If that code is lost too, the honest answer is a reset that clears the credential
set and is recorded as such: an audited break-glass, not a silent recovery.

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

## Option B — accept the limitation, and shrink what depends on it

`Human` is not publishable through the API. The only authority is
`Orchestrator`, which — once Step 0 lands — means a caller that holds its session
credential. A person who wants a card asks the orchestrator to publish it;
author, date and source event are recorded either way.

**Gives up:** "the human can also create an important message" is not met, and
becomes a documented limitation until the deployment model changes —
authentication enabled, and workers holding credentials they cannot escalate
from (a separate OS user, or a token distinct from the human's).

**Worth:** no new machinery, and it claims nothing it cannot prove.

## What each covers

| Threat | Step 0 alone | + A | B |
|---|---|---|---|
| A local caller presents someone else's session id | **covered** | covered | covered |
| A local caller reads another process's credential | no | covered for `Human` | n/a |
| An adversary that enrolled first | n/a | **not covered** (A.1) | n/a |
| An adversary driving the real browser | n/a | not covered (A.3) | n/a |
| A remote caller | already covered by `auth_middleware` | same | same |
| A person approving without reading | no | no | no |

## Recommendation

**Step 0 regardless, and it is the release blocker** — a defect, not a missing
feature: today a worker can publish as the orchestrator.

Then **B**, with A available if human publication is wanted later. A is a real
feature — enrolment, sealing, recovery, several origins in Docker — and should
be decided as one rather than slipped in under a card-rendering lot.

Whichever is chosen, the criterion is not ticked on half a rights contract.
