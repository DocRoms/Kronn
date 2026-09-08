# Human authority for important messages — threat model and options

Design note for arbitration. **Nothing here is implemented**, no migration is
reserved, and no table, endpoint or key exists. Written for KT-619 after two
proposals were refused, both for the same reason.

## What we are protecting

The `Human` authority on an important card: the claim that a specific card was
published by the person, not by an agent. `Orchestrator` is out of scope — its
lineage resolves from the caller's own durable session, which Kronn writes at
offer acceptance and the caller does not author.

## Threat model

**The adversary is a process running as the same OS user, on the same machine.**
Concretely: a CLI worker Kronn itself launched. It can read the whole user
filesystem, make loopback HTTP calls, and read anything the server or the
browser writes down.

Out of scope: remote callers (`auth_middleware` already requires a bearer for
any non-local address), other OS users, and root.

**The consequence that killed both earlier proposals:** any secret readable by
the same OS user is not a boundary. That rules out, in order of how tempting
they were —

| Candidate | Why it fails |
|---|---|
| `auth_strict_localhost` bearer | One shared secret; a worker holding it is indistinguishable from the person. |
| A capability ticket from an issuing endpoint | The endpoint has no identity to check either, so a worker simply omits its session and asks for one. |
| A pairing code shown on the server's terminal | Terminal output, supervisor logs and browser storage are all reachable by the same OS user. "Never logged" is not a property the server can guarantee about its own stdout. |

What survives the model is narrow: **something the platform attests, that a
process cannot fabricate** — or an honest refusal to claim the distinction.

## Option A — platform-verified user presence (WebAuthn)

Enrol a passkey with `userVerification: "required"`. Publishing a `Human` card
requires an assertion whose challenge is bound to the four links already named
in review: discussion id, content hash, single-use nonce, short expiry.

Why it holds where the others did not: the private key lives in the
authenticator or the platform keystore (Secure Enclave, TPM), never in browser
storage or a file, and the assertion needs a biometric or PIN gesture. A
same-user process can read every byte Kronn writes and still cannot produce one.

No home-made cryptography: this is a W3C standard with a maintained Rust
server-side implementation.

**Compatibility**

| Surface | Works? | Condition |
|---|---|---|
| Browser → `http://localhost` | Yes | Browsers treat `localhost` as a secure context, so WebAuthn is available without TLS. |
| Browser → LAN / Tailscale over plain HTTP | **No** | Not a secure context. Needs TLS, or this surface keeps Option B's limitation. |
| Desktop app | Depends | Only if its web view exposes WebAuthn. To verify before committing to A. |
| Docker | Yes, with care | The server may run in a container; the ceremony runs in the user's browser. The RP ID must match the origin actually visited, so several entry origins mean several enrolments. |

**Surfaces touched:** one dependency, an enrolment endpoint pair, a credentials
table, and a verification at the single publication point. `auth_middleware` is
not modified.

**Limits, stated rather than discovered later:** an adversary who can drive the
real browser session (a malicious extension, or automating the user's browser)
can still raise the prompt; and a person who approves without reading approves
whatever was put in front of them. WebAuthn proves presence and possession, not
attention.

## Option B — accept the limitation, and shrink what depends on it

Do not attempt the distinction. `Human` is simply not publishable through the
API; the only authority is `Orchestrator`, whose lineage is genuinely verified.
A person who wants a card asks the orchestrator to publish it, which stays
auditable — author, date and source event are recorded either way.

This is the current state of the branch, and it costs nothing to keep.

**What it gives up:** the criterion's "the human can also create an important
message" is not met. It becomes a documented product limitation until the
deployment model itself changes — authentication enabled, and workers holding
credentials they cannot escalate from (a separate OS user, or a token distinct
from the human's).

**What it is worth:** it is the only option that adds no machinery and claims
nothing it cannot prove.

## Recommendation

**B now, A as the real fix if human publication is wanted.**

A is the only proposal that does not rest on a secret the adversary can read.
But it is a genuine feature — enrolment, recovery when a passkey is lost,
several origins in Docker — and it should be decided as one, not slipped in
under a card-rendering lot.

Whichever is chosen, the criterion should not be ticked on the
`Orchestrator` half alone: half of a rights contract is not a rights contract.
