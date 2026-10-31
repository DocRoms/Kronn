# Routes that return a secret or destroy data (KT-1496)

An agent process on the same machine can call the Kronn API on `127.0.0.1`
without its bridge token, and loopback is trusted as the human
(`auth_allows`, `local_trusted` in `backend/src/lib.rs`). Once the data directory
is closed to agents (`core::trust_sandbox`), the API is the easiest way out for
secrets and the easiest way to destroy data. When the owner has enrolled the
strong confirmation of KT-1492 (a TOTP authenticator, see
[trust-levels](trust-levels.md)), the routes below ask for it.

**Owner rule: never bridle more than before.** Without an enrolment nothing
changes for anyone, a request carrying a code anyway is untouched, and the UI
only suggests enrolling (Settings › Security). Agents keep everything else.

## What is protected

`core::strong_confirm::protection` decides, from the method and the matched route
pattern. [src: file: backend/src/core/strong_confirm.rs:117-150#protection]

| Class | Routes |
| --- | --- |
| Always | `POST /api/config/recovery/set` (the passphrase that wraps the encryption key), `GET /api/config/export`, the three `…/reveal` routes (plugin env, provider API key, run variables), `POST /api/config/auth-token/regenerate`, `POST /api/config/network-exposure`, `POST /api/config/sync-agent-tokens`, `POST /api/config/agent-access`, `POST /api/config/server`, `PUT /api/projects/{id}/github`, `POST /api/agents/install`, `POST /api/human-credentials/revoke`, the instance operations (`POST /api/setup/reset`, `POST /api/config/import`, `POST /api/config/recovery/reencrypt`, `PUT /api/projects/{id}/validations-access`, which opens the host, `POST /api/agents/uninstall`, `POST /api/pages/import`, which can bring code) and **the sensitive deletions** below |
| Conditional (the handler decides) | `POST /api/mcps/bundles/export` with `include_values` (a passphrase the caller chooses is the same as plaintext); `PUT /api/mcps/custom/{server_id}` when `base_url` changes and `PUT /api/external-api/connections/{id}` when `endpoint` or `media_endpoint` changes (the stored credentials are then sent to the new place); `POST /api/quick-execs` and `PUT /api/quick-execs/{id}` when the request sets the human approval flag (`unmodelled_args_approved`) on a line a human has not already approved |
| Open on purpose | the non-secret maintenance routes `is_destructive` gates (debug log clear, API call log purge, provider quota rearm, audit-runs cleanup, rtk deactivate, `embed-origins`, context-audit baseline, audit attestation, `cleanup-orphan-env`) and every everyday `DELETE` (below); `/api/trust/*` (the trust routes ask for the code themselves; asking twice would spend two codes), `POST /api/config/recovery/restore` (it carries the recovery passphrase, its own proof), `POST /api/config/recovery/start-new-key` (the key is lost, so is the seed that would be checked) |

**What asks (owner decision, 28/10).** The code is for secrets and instance
operations, not for everyday maintenance. `ALWAYS` is an explicit list: being in
`is_destructive` (the local-or-token gate of `auth_allows`, unchanged) does not
make a route ask. A test scans the route table of `lib.rs` and fails for any
`is_destructive` route that is neither protected nor listed as open on purpose, so
a new route has to be classified. [src: file: backend/src/core/strong_confirm_tests.rs:308-335#every_destructive_route_is_classified_on_purpose]

**Which deletions ask.** Only an irreversible loss of
value, by an explicit list (`SENSITIVE_DELETES`), not "every `DELETE`":
`DELETE /api/projects/{id}`, `DELETE /api/mcps/configs/{id}` (a plugin with its
credentials), `DELETE /api/external-api/connections/{id}` (a connection with its
credentials), `DELETE /api/config/api-keys/{id}` (a stored provider key) and
`POST /api/human-credentials/revoke`. Everyday deletions go through without the
code: a message, a discussion, a task link, a workflow and its runs, a page, a
Quick Prompt, a Quick API, a Quick Exec, a skill, a profile, a directive, a
contact. They stay behind the destructive gate of `auth_allows` (local or token),
which is unchanged. A new `DELETE` route is open until it is added to the list on
purpose. [src: file: backend/src/core/strong_confirm.rs:87-92#SENSITIVE_DELETES]

Checked safe and left alone: `GET /api/config/tokens` (masked),
`GET /api/config/server`, host discovery (env names), the custom export file
(spec only), `human-credentials/*` (they need the admin secret).

**Refused outright, whatever the enrolment.** `POST /api/projects/add-folder` and
`POST /api/projects/{id}/remap-path` refuse a path inside the Kronn data
directory, equal to it, or containing it (symlinks resolved), because a project's
commands (`projects/git.rs`, `disc_git.rs`) would otherwise read the vault from
there. [src: file: backend/src/core/strong_confirm.rs:274-277#overlaps_data_dir]

Out of scope on purpose: plugin sync writes decrypted MCP env into
`<project>/.mcp.json`. Agents need those values to run their MCP servers; a
placeholder resolved at launch is a later option (KT-1494).

## The proof, on every channel

The code is the proof (the owner may give it to an agent in a CLI or a room).
A protected request carries one of:

1. `X-Kronn-Confirm: <code>` (or a `kronn_confirm` field of a JSON body): a
   current TOTP code or a recovery code, spent on this call.
2. `X-Kronn-Grant: <grant>`: a short-lived single-use grant bound to one method
   and one path, minted by `POST /api/trust/grants` from a code
   (`{ "method", "path", "code" }`). This is what the screen does after the code is
   typed, and what a download that cannot carry a code header needs. A grant
   expires after two minutes, lives in memory only (a restart ends it) and is
   burnt by the first attempt that names it, even on another call.
   [src: file: backend/src/core/strong_confirm.rs:200-240#mint]
3. A **bridge token** carrying either: accepted for that one action. This is the
   only exception to "refused to agents" on these routes
   (`lib.rs`, `strong_code_gate`), and only when a strong confirmation is
   enrolled. A bridge request on a route of its own list (`BRIDGE_ROUTES`) keeps
   that list's scoping, as before. [src: file: backend/src/lib.rs:987-1028#strong_code_gate]

A passkey stays UI-only by nature; the TOTP code works everywhere.

Guards, the same on every channel (`core::trust_proof` does the work; this
feature adds none of its own to the code):

- **single use**: a code of a step already accepted is a replay; a recovery code
  is spent;
- **bound at use**: the code is consumed by the one route and object of the
  call (`Binding { channel, route, object }`, the object being the values of the
  route's `{…}` segments); a grant is bound to the method and the concrete path;
- **journalled**: every use and refusal is written to the trust audit log with
  its route, object and channel (`ui`, `http`, `bridge`). A request with no code
  at all is the first request of a dialog, not a guess: it is neither journalled
  nor counted;
- **rate limited**: 5 failures lock the confirmation for 60 s, for every channel
  (HTTP 429, same `strong_confirmation_required` code), with the persistent
  backoff of `core::trust_proof`. The failure that starts a lockout already
  answers 429 and is journalled as an alert; a recovery code lifts the lockout.

An unreadable enrolment record is an error (HTTP 500), never "not enrolled": a
protected route does not open on a failure.

## Where it is enforced

`api::strong_confirm::strong_gate` is a route layer inside the auth layer, so it
sees the matched pattern. For an *Always* route it verifies and spends the proof
before the handler runs. For a *Conditional* route it stores what the request
carried and the handler asks with `Strong::require` once it knows the call is
sensitive. The refusal is `403` with `error_code: "strong_confirmation_required"`
(`429` on a lockout).
[src: file: backend/src/api/strong_confirm.rs:302-363#strong_gate]

The production-router tests are in `backend/tests/strong_confirm_gate.rs`: one
request per route family with no proof, a wrong code and a wrong grant; the same
families unenrolled; replay and cross-route use of a code; the grant (single use,
one call, no mint without the code); the bridge channel; the lockout; the
conditional routes; the data-directory guard.

## UI

Every `api()` call that gets `strong_confirmation_required` opens
`StrongConfirmDialog`: why the code is asked, the action (`METHOD /path`), a code
field. The code buys a grant for that call (`POST /api/trust/grants`) and the very
same request is sent again with it (`lib/strongConfirm.ts`: `withStrongConfirm`,
`strongFetch` for the downloads and uploads that read the raw response). A refused
code keeps the dialog open with the reason. Settings › Security tells an owner
with no authenticator what it would protect.

## Limits

- A code or a grant given to an agent is good for one action; the agent (and
  anyone watching its terminal) sees it. That is why it is single use.
- The maintenance routes of `is_destructive` (clearing logs, purging the API
  call log, rearming a provider quota, …) do not ask: they are not a secret and
  not an operation on the instance. Their local-or-token gate is unchanged.
- The body field is read only from a JSON body that declares a size of at most
  1 MiB; larger bodies (and a chunked upload) use the header.
- A bridge-token request with a code reaches the protected route with the
  rights of the route, not the token's project scope: the owner who hands out
  the code decides.
- The per-request enrolment check reads `trust-proof.json` from the data
  directory.
