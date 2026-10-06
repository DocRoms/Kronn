# Agent secret boundary (0.14.3 design note, v3.1)

Status: **v3.1. Layer A and the bridge token (layer B, first half) are
implemented in 0.14.3; the per-action human proof is deferred to 0.15 by Romu
(card `0143-secu-d1-v31`).** See §9 for what shipped and the residual path.
(KT-1006, KT-1013, KT-1007, KT-990, KT-969). v1 `7e721fc6`, v2 `9261bfa7`.
Sources: the 0.14.3 security audit (room `c3a5311c`, SEC-1 to SEC-15), Codex's
reviews (room messages `060744b2`, `8b2bd770`, `e0163229`), an independent
adversarial review (findings A1-A5, B1-B3 below), and Romu's question on an
authenticator factor (`0293365d`).

v3 changes: no privilege-preserving reset (4.3), an execution boundary for any
code an agent can influence (4.6), Windows downgraded to the Linux line of the
matrix, a browser bootstrap path (4.2), passkeys as the primary human factor.

## 1. Threat model

The actor is **an agent that Kronn launched and that a prompt injection now
drives** (ticket text, web page, repository file). It runs:
- natively, as the user's OS account;
- under Docker, in the backend's container, with the backend's UID.

It can run any command its CLI allows, read what its OS identity can read, and
call any loopback service. Out of scope: an attacker who controls the user's
account outside Kronn, malware present before Kronn starts.

There is **no single promise across platforms**. Section 2 states, per
platform, what this design guarantees and what remains exposed. A CLI's own
sandbox (Claude permissions, Codex sandbox) is listed as defense in depth only,
never as the mechanism behind a guarantee.

## 2. Guarantees per platform (target, once all four layers ship)

**Not the 0.14.3 state.** 0.14.3 ships layer A, the bridge token (first half
of layer B) and layer C. The per-action human proof (D1) is deferred to 0.15,
so the loopback path to secret-class routes stays open; KT-990 and KT-969
(layer D) are **not** fixed. The table below is the target. What actually holds
in 0.14.3 is in §9.

| Platform | Key | Admin token / decrypted secrets over the API | Secret files | Code the agent influences (4.6) | Residual exposure |
|---|---|---|---|---|---|
| macOS native | OS Keychain only, after a verified restore test; no sidecar, nothing in `config.toml`. The Keychain item's ACL is meant to make macOS prompt when another program reads it: **not demonstrated yet**, to be proven by probe (`security find-generic-password`, Kronn's own binary run by an agent) before any cell relies on it | Agent env carries neither; secret-class routes need a per-action human proof | `kronn.db` `0600`, readable by the same user | Runs as the user: **no OS boundary** | A same-user process can read the DB and drive the live UI session; it cannot read the key without a Keychain prompt, if the ACL probe confirms it; values of the agent's own project MCPs |
| Windows native | Credential Manager: **not a boundary** between processes of the same account (`CredRead` serves any process of the logon session) | Same as macOS | Same | No OS boundary | Same as Linux native: an agent can obtain the key and decrypt offline |
| Linux / WSL native | **Sidecar file stays** (no keyring assumed) | Same as macOS | Sidecar + DB readable by the same user | No OS boundary | An agent can read the sidecar and the DB and decrypt offline: layers A-B only close the API path |
| Docker (Linux host) | Sidecar in `/data`, `0700` to the backend UID | Same; the container spawn path uses the same env builder | Agents run under a second UID without access to `/data` (KT-969) | Second UID for every agent-influenced execution | Repositories writable by agents; SSH agent socket usable while an agent runs |
| Docker on macOS | Same as Docker | Same | Same, plus masks over the native data directory | Same | Same; the native install is no longer readable from the container |

Only the Docker lines give a boundary against an agent that reads files or runs
code. The native lines close the API and environment paths and, on macOS, the
key itself; the advisory states the rest plainly, and the UI shows the current
platform's line (4.7).

The evidence for every cell is a test or a real probe listed in section 7.

## 3. Where the promise breaks today (verified at `7e721fc6`)

| # | Path | Proof |
|---|------|-------|
| 1 | Agents inherit `KRONN_AUTH_TOKEN`, `KRONN_ENCRYPTION_KEK` (raw key when set) and, under Docker, every provider key | `main.rs:165-169` exports the token into the backend env; `agents/runner.rs` `try_spawn` and `acp.rs` `native_command` inherit everything; `docker-compose.yml:154-169` |
| 2 | Loopback is "local trusted"; no configured token means open | `lib.rs` `auth_allows` |
| 3 | Secret-returning or secret-moving routes open to that trust | `/api/mcps/configs/{id}/reveal`, `/api/external-api/connections/{id}/reveal`, `/api/execution-context/.../reveal`, `/api/mcps/bundles/export?include_values`, `GET /api/config/export` (bundles `recovery.key`, `api/setup.rs:1655-1662`), `POST /api/config/import`, `POST /api/config/recovery/set` (wraps the key under a caller-chosen passphrase **and** overwrites `recovery.key`, `setup.rs:2310`), `POST /api/config/sync-agent-tokens` (writes provider keys to `~/.codex/auth.json`, `~/.gemini`, `setup.rs:1126-1154`), `POST /api/config/discover-keys` (`setup.rs:1158-1210`), `POST /api/config/auth-token/regenerate` (`lib.rs:891`). `GET /api/config/auth-token` exists (`setup.rs:1018`) but is not routed |
| 4 | **Exec routes hand out the backend environment and any file** | `POST /api/projects/{id}/exec` (`lib.rs:1268`) runs `sh -c` with no env scrub (`api/git_ops.rs:1571-1588`); the allow-list includes `env`, `cat`, `find`, `stat` with absolute paths (`git_ops.rs:1520-1524`). Workflow Exec steps also inherit the backend env (`workflows/exec_step.rs:377`) |
| 5 | Key next to the database | sidecar `encryption_key` (`core/keyvault.rs` tier 3); `encryption_secret` still serialized in `config.toml` (`models/setup.rs:39-41`) and set again at each boot (`core/keystore.rs:135-147`) |
| 6 | Credentials in plaintext `config.toml` | `tokens.keys[].value` (`api/external_api_connections.rs:1690-1701`), `server.auth_token` |
| 7 | Key-loss traps | reconcile reads only `mcp_configs.env_encrypted` (`keystore.rs:72-83`) and mints a new key when it is empty even if other columns hold ciphertext; `OsKeychain::retrieve` returns `Ok(None)` on any keychain error (`keyvault.rs:72-88`) and `mirror()` then overwrites the item (`keyvault.rs:238-246`) |
| 8 | Publication admin secret in a plaintext file | `core/operator_secret.rs:31` |
| 9 | Trusted files the agent can write | natively, the bridge script path and the project `.mcp.json` (used as the canonical registry by the ACP broker) sit where the agent writes |
| 10 | GitHub token to every agent | `gh auth token` copied into `GH_TOKEN`, `GITHUB_TOKEN`, `COPILOT_GITHUB_TOKEN` (`agents/runner.rs:12525-12549`) |

## 4. Design

Four layers. Order: C, then A+B together, then D. Each layer's tests are
release gates, not prose.

### Layer A — every child process gets a built environment (KT-1006, KT-1013)

One builder produces the environment of **every** process Kronn starts on a
caller's behalf: agent CLIs on all three routes (native ACP, adapters, direct),
the container spawn path, **the exec routes** and **workflow Exec steps**.
- Start from an allow-list of the user's environment: `PATH`, `HOME`, `USER`,
  `LANG`/`LC_*`, `TERM`, `TMPDIR`, `SHELL`, proxy variables, the agent's own
  documented variables.
- Add what the launch needs: its own provider key, room or workflow context,
  `KRONN_DISCUSSION_ID`, per-launch MCP references (KT-964, KT-1003), the bridge
  token (layer B), the GitHub token only when the project's connection allows
  it (section 4.5).
- Never: `KRONN_AUTH_TOKEN`, `KRONN_ENCRYPTION_KEK`, another provider's key.
- `main.rs` stops exporting `KRONN_AUTH_TOKEN` into its own environment.
- Exec allow-list: drop `env`; `cat`/`head`/`tail`/`find`/`stat`/`grep` refuse
  paths outside the project root (resolved, symlinks followed). Exec routes are
  also gated by layer B (they are not bridge routes).

### Layer B — caller classes and per-action human proof (KT-1006)

**Bridge token.** Minted per agent launch, held in memory, bound to one
discussion (or one task execution, or one workflow run) and to its project.
- Authorization is a **positive list of operations**, each with the resources
  it may touch. The check covers identifiers in the path **and** the body
  (discussion, task, project, workflow, QP, API connection ids). The list is
  derived from what `backend/scripts/disc-introspection-mcp.py` calls; a test
  parses the script's API paths and fails if one is missing from the list or
  the list grants a route the script never calls.
- Effectful operations (`workflow_trigger`, `qp_run`, `qe_run`, `qa_run`,
  `api_call`, `task_exec_launch`, `media_generate`, `audit_launch`) are allowed
  only on resources of the bound project and are logged with the token id.
- Lifetime: revoked when the launch ends, on backend restart (memory only) and
  when the discussion or run is deleted. A subprocess that outlives its launch
  holds a dead token. Tests cover cross-room and cross-project refusals for
  path and body ids.
- The agent can read this token. Its scope is by design what the agent may do
  through room tools.

**Human proof per action.** Every secret-class route (section 3 rows 3 and 4,
plus human-credential administration) requires a **proof bound to one action**:
- the server issues a nonce for `(UI session, verb, route, resource id,
  effect parameters)`: effect parameters are what changes the outcome (for
  example the export options, or the content of a recovery change);
- the human authorises it with a factor the agent does not hold (section 4.2);
- the proof is single-use, consumed atomically (two concurrent requests with
  the same proof: exactly one succeeds), short-lived (60 s), and valid for that
  exact action only. There is no session-wide elevation window.

Tests: an agent's request during a valid human proof is refused; a proof
replayed for another verb, resource, id or parameter set is refused; two
concurrent uses of one proof yield one success.

### 4.2 The human factor

Factors, strongest first. A user enrols at least **two** before any secret can
be revealed or exported (one may be the recovery passphrase). Two factors on
the same phone (a phone passkey and a TOTP app on it) do **not** give
independent recovery: losing the phone loses both, so enrolment must say so
and ask for a factor that does not live on that device.

1. **Passkey (WebAuthn)**: Touch ID, Windows Hello, a security key or a phone
   passkey. Kronn stores only the public key, so reading the whole data
   directory yields nothing usable. This is the only factor that stays sound
   on Linux and Windows natively. To verify before committing (spike): WebAuthn
   needs a domain as relying party, not an IP, so the UI must be served on
   `http://localhost:<port>`. In the macOS desktop app, WKWebView only handles
   WebAuthn with the restricted `com.apple.developer.web-browser.public-key-credential`
   entitlement or with Associated Domains served from a real domain, neither of
   which fits a local app; the desktop build therefore uses factor 4 instead
   (sources: Apple developer forums thread 774904, public Tauri/WKWebView
   passkey PRs, found 2026-10-04, not reproduced here).
2. **TOTP authenticator app** (RFC 6238), as a fallback. Kronn must keep the
   seed to verify codes; an agent that reads the seed computes every future
   code. The seed is therefore stored like the key (Keychain on macOS) and the
   matrix applies: on Linux and Windows natively it is no stronger than the
   files it sits in.
3. **Secrets passphrase**, last resort: an Argon2id hash, never the encryption
   key. Reading the hash gives an offline guessing target, not the passphrase.
4. **OS user presence on desktop** (LocalAuthentication, Windows Hello through
   the Tauri IPC) can stand in for 1 when WebAuthn is unavailable in the
   webview.

Honest limit: in a browser on loopback, a same-user process that can read and
drive the live UI session can still act through it while a human approves an
action. Per-action binding (layer B) limits what it gains to that one action.

**Enrolment, replacement and recovery follow the same model.**
- *First enrolment (bootstrap).* A fresh install has no factor. The first
  enrolment needs a **one-time bootstrap code** that the backend prints at
  start-up on its terminal (`make start`, `kronn` CLI, desktop window) and never
  serves over the API; it expires after 15 minutes or one use. On desktop, OS
  user presence replaces it. Until a factor exists, secret-class routes are
  closed (nothing to reveal is better than revealing to the first caller).
  Limit: natively, an agent that reads the backend's terminal output or log
  file can see the code; the log line is therefore written to the terminal
  only, never to `kronn.log`.
- *Existing install without any factor (upgrade).* Secret-class routes stay
  closed until the user enrols through the bootstrap code; secrets keep
  working server-side meanwhile.
- *Replacement* of a factor requires another enrolled factor.

### 4.3 Lost device or forgotten access

No reset ever keeps the privileges of the factors it replaces. What can be
recovered and what must be reconfigured:

| Situation | Outcome |
|---|---|
| New machine or Keychain reset, recovery passphrase known | Key restored from `recovery.key`; every secret works again |
| One factor lost, another enrolled | Replace the lost one with the remaining one |
| Keychain lost, no recovery passphrase | Encrypted values cannot be recovered. Kronn **never mints a new key over existing ciphertext**: it lists each locked secret (provider keys, MCP tokens, connections) for re-entry; discussions, workflows, tasks and settings stay intact (not encrypted) |
| Every factor lost, key still available | **Reconfiguration mode**: the user enrols new factors through the bootstrap code. Old secrets keep being used by their **current** consumers only. Forbidden in this mode, enforced server-side: revealing or exporting old values, re-wrapping or exporting the old key (`recovery/set`, config export), and changing where an old secret goes (connection URL, MCP command or args, workflow API binding). To change any of those, the user re-enters the secret, which then becomes a new, normally protected value |
| Recovery passphrase forgotten | Set a new one with another factor; the old `recovery.key` is replaced only after that proof (B3) |

Chain tests, not route tests: reset, then try `recovery/set`, config export,
bundle export, and a connection URL change to an attacker host; each must fail
for old secrets.

### 4.6 Code an agent can influence runs behind the same boundary as the agent

Removing variables does not stop a script from reading files. Any execution
whose content an agent can influence runs with the agent's identity, never the
backend's:
- workflow Exec steps, QE runs, project setup and test/build hooks, the
  project exec route, and their children;
- under Docker, they run under the agents' UID (KT-969), without `/data`;
- natively, there is no second identity: these executions are listed in the
  matrix as unbounded, and an agent-triggered run (bridge token) of an Exec step
  or QE requires the human proof of layer B unless the workflow was approved
  with its current content (KT-918 fingerprint, 0.15).

Test (Docker): an agent triggers a workflow of its own project whose script
reads a fake secret outside the repository; the read fails with an empty
environment and the agents' UID.

### 4.7 Transparency in the UI

Security only helps if the user can see it:
- Settings show the current platform's line of the matrix: what is protected,
  what is not, and why.
- Every human proof dialog names the exact action, resource and parameters it
  approves.
- Locked secrets (4.3) and reconfiguration mode are shown on each affected
  item, with the action that unlocks it.
- The GitHub connection shows its real scope or "scope not verified" (4.5).

### Layer C — storage consolidation (KT-1007) — implemented in 0.14.3

Operator view: [`operations/key-management.md`](../operations/key-management.md).

1. **Done.** `keystore::ENCRYPTED_COLUMNS` lists every encrypted column
   (`mcp_configs.env_encrypted`, `execution_variable_snapshots.values_encrypted`,
   `stored_credentials.value_encrypted`,
   `project_github_connections.token_encrypted`); `collect_encrypted_rows` samples all
   of them, and `encrypted_column_registry_matches_the_schema` fails when a
   column named `*encrypted*`/`*cipher*` is missing from the registry.
2. **Done.** `KeyVault::retrieve` returns `Ok(None)` only for an empty vault;
   denied or unreadable vaults return a `VaultError`. `KeyStore::snapshot`
   fails on the first unreadable vault and the reconciler stops the boot with
   `KeyBootError::VaultUnreadable` (both mains exit); `mirror()` never writes
   a vault whose read failed. Mint/adopt only when no registered column holds
   ciphertext.
3. **Done.** `encryption_secret` is `serde(skip_serializing)`. It stays the
   in-process key every consumer reads; the only copy written to
   `config.toml` is the one `config::retain_disk_key` keeps until the key
   decrypts every non-empty column and two independent copies remain without
   it (two persisted vaults, keychain and sidecar, or one plus a
   `recovery.key` whose fingerprint matches); a different legacy value is moved to
   `config.toml.retired-key.<ts>` and the key in use gets the file copy.
   Config backups, retired-key and corrupt-config files are read-only
   candidates of every decision; rows under such a file key are moved under
   the live key at boot, the file kept. `mirror()` never writes a vault
   holding another key; two live keys that each decrypt data stop the boot
   (resolved by `KRONN_REENCRYPT_FROM`, one key per start).
   Keys compare in one canonical spelling; the env variable is not counted as
   a persisted copy; recovery blobs carry a checksummed fingerprint. A locked boot keeps no
   key in memory (fail closed); a stored auth token it cannot read locks the
   API (423 `auth_locked`, recovery screen).
4. **Done.** `tokens.keys[]` and `server.auth_token` live in
   `stored_credentials` (migration 217). The boot migration merges, writes,
   reads back, writes an encrypted backup of the old file
   (`config.toml.pre-credential-store.enc`, `0600`, under the instance key,
   hence recoverable through the recovery passphrase), then rewrites
   `config.toml`; it also scrubs the DB migration runner's plaintext
   `config.toml.backup`. Readers keep using the live config, filled from the
   table at boot and written back by every `config::save`. Reveal routes are
   unchanged.
5. **Done (passphrase proof).** `recovery/set` refuses to replace an existing
   `recovery.key` without `current_passphrase`, except one verified for
   another key or with a damaged payload (kept as `recovery.previous-<ts>`),
   and one from before 0.14.3 after an explicit confirmation; imports never
   replace it;
   `recovery/restore` never swaps the key of a running instance (imported
   secrets are re-encrypted instead). The human factor (D1) is not part of
   0.14.3.
6. **Done.** Dev builds exercise the keychain with `KRONN_USE_KEYCHAIN=1`
   (`keyvault.rs` `use_os_keychain`); outside macOS/Windows the default is off.

### Layer D — files out of reach (KT-990, KT-969)

- macOS / Windows: the sidecar is removed only after layer C and a restore
  test from the recovery passphrase on that machine.
- Linux / WSL: the sidecar stays (section 2 limit).
- Docker: agents under a second UID without access to `/data` (`0700`);
  repositories shared through a group; masks cover the native data directory on
  macOS hosts; CLI config directories mounted read-only or copied in at start
  (no host code execution through hooks).
- Trusted files (row 9): the bridge is launched from Kronn's install
  directory, never from a repository path; the ACP broker's canonical MCP list
  comes from the database, not from the agent-writable `.mcp.json`.

### 4.5 GitHub connection per project (D2)

**Status: implemented** (0.14.3, approved by Romu on card `0143-secu-d2-v31`).
User guide: [`guides/github-connection.md`](../guides/github-connection.md).
Code: `core/github_connection.rs` (`env_for_launch`, `apply_launch_env`, scope
check), `db/github_connections.rs` (migration 216, upgrade seeding),
`api/github_connection.rs`, `ProjectGithubRow.tsx`. Decisions taken while
implementing: workflow Exec steps receive the project's token when it is
connected and none otherwise; summaries and audits receive none; Copilot's own
configured token is unchanged; quick execs and the project exec route keep the
backend environment until the environment builder (layer A) covers them.

Goal (Romu, card `0143-secu-d2-gh-token`): turning GitHub on for a project is
one click, and the UI says what it gives and what it risks.

- **Where:** a "GitHub" row on the project card (Overview), and the same state
  as a chip in the discussion header of that project.
- **States, always shown with their meaning:**
  - *Not connected*: agents of this project get no GitHub token. Button
    "Connect GitHub".
  - *Connected via your gh login* or *via a stored token*: the scope actually
    established is listed (repositories, permissions, read from the GitHub API
    when the token type allows it), or "scope not verified" with the reason.
  - *Available but off*: a token exists on this machine, the project does not
    use it.
- **One click:** "Connect GitHub" reuses the existing `gh` login (no copy-paste)
  after a confirmation dialog stating the risk in plain words: "Agents working on
  this project will be able to act on GitHub with this token, within the scope
  shown." A fine-grained token restricted to the project's repositories can be
  pasted instead, and the dialog recommends it when the `gh` token is broad.
- **Existing projects on upgrade:** projects whose remote is on GitHub keep
  receiving the token (nothing breaks silently), and show a one-time notice with
  the state and a "Turn off" button. New projects start *Not connected*.
- **Turning it off** stops giving the token to new launches; agents already
  running keep what they received until they stop, and the UI says so.
- **Limits stated in the dialog and the matrix:** natively an agent can still
  reach GitHub through the user's local `gh` login in their home directory;
  Kronn cannot close that. Under Docker the `gh` configuration is not mounted
  unless the project is connected.

### 4.2b Verifiable human proofs (Codex review `17b0883b`)

The backend never trusts a boolean from a client:
- **OS user presence (desktop):** the backend issues the action challenge; the
  Tauri native side asks LocalAuthentication / Windows Hello and signs the
  challenge with a per-install key generated in the OS secure store at
  enrolment (Secure Enclave on macOS when available). The backend holds the
  public key and verifies the signature over `(challenge, action digest)`.
- **WebAuthn:** the challenge embeds the action digest; the server checks
  origin, RP ID, `userVerification` and the signature counter.
- **TOTP:** the accepted time step is consumed atomically with the action
  nonce; a code is never accepted twice, even for another action.
- **Passkeys:** the guarantee is that Kronn never receives the private key
  (synced passkeys exist; Kronn does not rely on device-bound keys).
- An agent able to modify Kronn's code or authentication records defeats any
  of these; that is the residual risk of the native lines of section 2.

## 5. Order and rollout

0.14.3 delivers steps 1 and 2 without the human factor: layer C, layer A and
the bridge token. The human proof, the rest of step 2, moves to 0.15; step 3
is not started.

1. KT-1007 (layer C), tested on copies of real 0.14.2 config and databases.
2. KT-1006 + KT-1013 (layers A-B), in one change with the bridge moving to the
   bridge token; the human factor UI ships with it.
3. KT-990 (layer D native), then KT-969 (layer D Docker).
4. KT-968 (MCP gateway) is in 0.15.2; the advisory states the remaining
   exposure (values of the agent's own project MCPs).

## 6. What breaks, and how it is kept working

- Rooms: the bridge reads the bridge token from its environment; no route it
  calls disappears (parse test, layer B).
- `lib/*.sh` and the Docker UI rely on loopback trust (`api-client.sh:23`):
  read-only routes keep working; secret-class routes return an explicit
  "human proof required" error that the CLI prints.
- The user's own CLI sessions outside Kronn keep their `.mcp.json` (KT-1008
  decides the native file policy).

## 7. Tests and probes that back section 2

- Environment builder: forbidden variables absent for every route, including
  exec routes and workflow Exec steps (unit tests).
- Bridge token: positive list complete against the bridge script; cross-room
  and cross-project refusals on path and body ids; dead after launch end and
  after restart (API tests).
- Human proof: refusal without proof, replay refusal, concurrent single success,
  parameter binding (API tests).
- Exec: `env` refused, `cat <absolute path outside project>` refused (tests).
- Layer C: registry versus schema; denied Keychain at boot stops startup;
  migration from real 0.14.2 copies loses nothing (tests).
- Factors: two required before any reveal; bootstrap code single-use, 15 min,
  absent from the API and from `kronn.log`; replacement needs another factor.
- Reconfiguration mode chain tests (4.3): reset, then `recovery/set`, config
  export, bundle export and a connection URL change all fail for old secrets.
- Execution boundary (Docker): an agent-triggered Exec step cannot read a fake
  secret outside the repository (4.6).
- Real probe per CLI (Claude two turns, Codex, one native ACP agent): asked to
  `env`, `curl` the reveal and exec routes and read the data directory, the
  agent gets nothing usable beyond what section 2 lists as residual.

## 8. Decisions requested

- **D1** (v3): passkey first, TOTP fallback, passphrase last, at least two
  factors enrolled, bootstrap code at first enrolment, no privilege-preserving
  reset (4.2, 4.3). A WebAuthn spike on `localhost` and in the Tauri webview
  comes first.
- **D2** (v3.1): one-click GitHub connection per project, risk dialog, scope
  shown or marked unverified, existing GitHub projects kept connected with a
  one-time notice, new projects not connected (4.5).
- **D3:** KT-968 in 0.15.2 with the residual risk in the advisory (already
  tagged by the 0143-scope decision).

## 9. Implementation status (0.14.3)

**Layer A — shipped (KT-1006, KT-1013).** `backend/src/core/child_env.rs` builds
the environment of every process Kronn starts on a caller's behalf: agent CLIs
on the direct route and the Claude/Codex adapters (`try_spawn`), native ACP
agents (`native_command`), the task-worker auth probes, the project and
discussion exec routes, workflow Exec steps (setup included), workspace
lifecycle hooks, Quick Exec (task validations included) and the API-call
credential CLIs, and every `git` process (`core::cmd` gives git its own policy:
the base allow-list plus git's identity, config and prompt variables, never a
repository selector), since a repository's hooks, filters and drivers run
inside git. Each path has a test that inspects the final environment;
the seal runs after every other `.env()` of the launch. Agents
under Docker run in the backend container through the same `try_spawn`; there
is no separate container spawn path. Each child starts from `env_clear()`, gets
the reviewed allow-list (process essentials, temp dirs, proxies, TLS stores, XDG
dirs, SSH agent socket, toolchains, Windows essentials), its own agent's
documented variables, then the launch's values (provider key for this agent
only, discussion/room/workflow/task contexts, `KRONN_BACKEND_URL`, MCP
references, the bridge token). A final seal drops `KRONN_AUTH_TOKEN`,
`KRONN_ENCRYPTION_KEK`, `KRONN_KEK`, other agents' provider keys and any
secret-looking name nobody granted. `main.rs` no longer exports
`KRONN_AUTH_TOKEN`; an operator-set value is mirrored into the config and
removed from the process environment. GitHub variables are never inherited:
`core::github_connection::env_for_launch` (§4.5) adds them for a connected
project only, on every route (native ACP, Exec steps, Quick Exec runs, exec
routes); workspace hooks and Kronn's internal Quick Exec callers (task
validations, probes) carry no project and get none.
Native ACP agents now receive their configured key and their room and workflow
contexts (KT-1013).

*Spawn inventory.* The compiler enforces it. `core::cmd::{async_cmd,sync_cmd}`
take a `ChildRoute` and build the environment before returning the command
(`git_cmd`, `async_git_cmd`, `tool_cmd`, `sync_tool_cmd` are shorthands); a
spawn without a route does not compile. `backend/clippy.toml` refuses
`std::process::Command::new`, `tokio::process::Command::new`, the `open`
crate's functions (`open::commands`/`with_command` included) and libc's
`fork`/`clone`/`exec*`/`fexecve`/`posix_spawn*`/`popen`/`system`/`syscall`
outside `core/cmd.rs` (test code excepted), which also covers aliases and
function pointers (`forkpty`, `rfork`, `execveat` and `execvP` included,
platform-specific ones marked `allow-invalid`); `desktop/src-tauri/clippy.toml` holds the desktop crate to
the same rule, plus `tauri_plugin_shell::Shell::{command,sidecar,open}`,
`AppHandle::restart` and `tauri::process::restart` (its `caffeinate` and
login-shell PATH probe use the Tool route), and CI runs clippy on both
crates. The test `clippy_spawn_ban_bypasses_are_exactly_these` lists the
`#[allow]`/`#[expect]` sites of both crates (the lint, its old name
`disallowed_method`, the `style` and `all` groups, and `warnings`) and
refuses a crate-wide override in either `Cargo.toml` `[lints]` table or in
`.cargo/config` (rustflags `-A`, `--allow`, `--warn`, `--force-warn`,
`--cap-lints`, an `[alias]` named `clippy`, an `[env]` key starting with
`CLIPPY_`), with `-` read as `_` in lint names. The system opener goes through `cmd::open_in_system`
(Tool route). A caller that adds values after construction seals again.
Routes beyond the agent and exec ones:

| Route | Inherits beyond the base allow-list | Used by |
|---|---|---|
| `Git` | git identity, config location, prompts | every `git`, git inside WSL (`scanner.rs`) |
| `GitHost` | the above plus `GH_CONFIG_DIR`, `GH_HOST`, `GLAB_CONFIG_DIR`, `GITLAB_HOST`, `GL_HOST`; `gh` also gets the connected project's GitHub variables (`env_for_launch`), nothing else | PR creation and lookup (`api/git_ops.rs`), `gh auth token`, GitLab discovery |
| `DependencyCheck` | Go, Bundler/rbenv, .NET/NuGet, Poetry and Composer locations (proxy, cache, home) | `core/dependency_updates.rs` package managers and Renovate |
| `Docker` | `DOCKER_*`, `COMPOSE_*` that do not look like a credential | project `docker compose`, Composer through Docker |
| `QuickExec` | nothing | Quick Exec, task validations and their `cargo metadata` |
| `Tool` | nothing (ccusage also gets `CLAUDE_CONFIG_DIR`) | agent install/uninstall and the Kiro installer, RTK, ccusage, `wsl.exe` lookups, Tailscale and network probes, `caffeinate`, `launchctl`/`sysctl`, `hostname`, `kill`/`taskkill`, `id`/`chown`, stream lifelines |

No registry token is inherited by a dependency check, nor `GITLAB_TOKEN` by
`glab`: a repository's config could print it. Both read their own config file
or login instead, which the base allow-list keeps reachable (`HOME`, XDG).

*Authenticated push.* `git push` takes its token from the project's §4.5
connection (`env_for_launch`), never from an MCP config, and only for a
GitHub remote, pushed to its credential-free HTTPS URL. The token travels as
an `http.https://github.com/.extraHeader` through `GIT_CONFIG_*`, never in an
argument or a variable named after it, and that push runs with hooks off
(`core.hooksPath=/dev/null`, `--no-verify`), no credential helper (a
repository's could receive it) and TLS verification forced. Other remotes,
or a project not connected, push with the user's own credentials.

Probes are built from an allow-list too. Version and model discovery and the
`npx --yes <pkg> --version` runtime probe use `cmd::discovery_cmd(program,
family)`: the agent family's route (so a CLI still reads its own login: home,
XDG and its config directories such as `CLAUDE_CONFIG_DIR` or `CODEX_HOME`)
minus every credential it would inherit (provider keys, GitHub variables,
secret-looking names), started in the temporary directory. A CLI
authenticated only by a provider key exported to Kronn therefore reports no
models; its configured key reaches it on a real launch. The document sidecar
uses the Tool route plus the operator's own `KRONN_DOCS_*` and `PYTHON*`
settings. A credential under a name no list knows (`MYSQL_PWD`,
`DATABASE_URL` with userinfo, `*_PASSPHRASE`, `SENTRY_DSN`) reaches none of
them.

*Desktop webview helpers.* On Linux (WebKitGTK) and Windows (WebView2) the
system webview starts its own helper processes with the desktop's process
environment, outside `core::cmd`. The desktop therefore keeps only a reviewed
allow-list in its process environment and withholds everything else at start,
before any thread or window (`child_env::withhold_process_environment`, after
the admin token and the key override). The allow-list is the base list
(PATH, HOME, locale, temp dirs, proxies, TLS stores, XDG directories,
toolchains, Windows essentials), display and session plumbing (`DISPLAY`,
`WAYLAND_*`, `XAUTHORITY`, `DBUS_*`, `DESKTOP_SESSION`, AppImage's `APPDIR`
and `LD_LIBRARY_PATH`), toolkit and runtime prefixes (`XDG_`, `GDK_`, `GTK_`,
`GIO_`, `GST_`, `WEBKIT_`, `WEBVIEW2_`, `LIBGL_`, `MESA_`, `QT_`, `SNAP_` and
`SNAP`, `TAURI_`, `RUST_`, `DYLD_`…), never a credential-looking name. No
`KRONN_*` setting stays live (a webhook URL is a secret whose name is not):
Kronn reads them, `KRONN_USE_KEYCHAIN` included, from the withheld set. A
credential under any name, and any name that is not Unicode, is withheld.
Every environment read in Kronn goes through `child_env::var` / `var_os` /
`vars_os` (live value, else the withheld one) and every write through
`child_env::set_var` / `remove_var`: once the desktop withholds, a name off
the allow-list is set into the withheld set, never live, and a removal drops
it from both. Both clippy files refuse `std::env::var`, `var_os`, `vars`,
`vars_os`, `set_var` and `remove_var` outside `child_env`, and libc's
`getenv`, `secure_getenv` and `_NSGetEnviron`, so no read can miss a withheld
variable. Child routes see
withheld variables through the same overlay; names compare case-insensitively
on Windows. The relaunch below gets them all back.

One declared exception remains, and only on the desktop: the app relaunching
itself (`desktop/src-tauri/src/main.rs::self_restart_command`, through
`cmd::full_env_sync_cmd(program, FullEnvReason::SelfRestart)`). It is Kronn
itself: it keeps its environment and working directory (withheld credentials
included), loses the forbidden names, and gets the operator's key override
handed back. The test
`full_env_cmd_sites_are_exactly_the_declared_exceptions` checks it is the only
call site in either crate, and `both_clippy_files_ban_the_same_spawn_entry_points`
that every Tauri restart entry point (`AppHandle::restart`,
`AppHandle::request_restart`, `tauri::process::restart`) is banned.

The MCP probe is no longer an exception: a server's command may come from a
repository's `.mcp.json`, so the probe starts it with a built environment (base
allow-list plus that server's configured values, `api/mcps.rs::mcp_probe_command`).
An operator-set `KRONN_ENCRYPTION_KEK` (and the legacy `KRONN_KEK`) leaves the
process environment at start, like `KRONN_AUTH_TOKEN`; the key is kept in
memory (`keyvault::take_env_kek`).

The exec routes run without a shell (no variable, `~` or glob expansion), drop
`env`, refuse `find -exec/-delete/…` and `git --no-index/--output`, and refuse
any path argument of `cat`, `head`, `tail`, `find`, `stat`, `grep`, `rg`, `wc`,
`du`, `file`, `tree` and `ls` that resolves outside the project, symlinks
followed (`core/fs_guard.rs::resolve_contained_read`).

**Layer B, bridge token — shipped (KT-1006).** `backend/src/core/bridge_token.rs`.

*Lifecycle.* Every agent launch gets a random 256-bit token (`kbt_…`), held only
in memory, including a launch that owns no discussion, execution or run (that
token reads the global catalogues and nothing else); a launch whose token cannot
be minted does not start. The token is bound to the launch's discussions, task
execution and workflow run, else to its declared project, and its project is
frozen on first use. On every call, every discussion it owns (its launch's and
the ones it created through `disc/create` or `media/generate`), its execution
and its run are re-read: all must still exist and sit in the frozen project.
One of them deleted, or moved to another project, kills the token. It also dies
when the launch's process handle is dropped, when the launch is cancelled, after
12 hours whatever happens, and on restart. The operator token is compared in constant time on
the HTTP and WebSocket paths. The bridge reads `KRONN_BRIDGE_TOKEN` first and
falls back to `KRONN_AUTH_TOKEN` for host sessions.

*Requests.* A token is accepted only on `BRIDGE_ROUTES` (derived from the bridge
script; `backend/scripts/test_bridge_routes.py` fails on drift either way), and
the agent library's write routes are listed but never granted. Every id is
read wherever the request names it: declared path parameters (undecodable ones
refused), the query, any depth of the JSON body, and the workflow an import
carries in `content`. *Default deny on id fields*: a key that looks like an id
(`id`, `ref`, or a name ending in `_id`, `_ids`, `_ref`, `_refs`,
`_reference`) is refused unless it is in `ID_KEYS` with a resolver, in the
reviewed `PLAIN_ID_KEYS` (a caller's own session, idempotency keys, model and
library ids, git refs), the own `id` of a step, option or DoD item, or inside a
caller's own data (`OPAQUE_KEYS`: an external API body, a dataset, a schema);
the keys of a caller's own maps (`USER_KEYED_KEYS`: template variables, an
external API's path, query and headers, per-step choices) are names, and only
their values are walked; the query follows the same rule. A value that reaches
Kronn through a template is checked where it is rendered: a PublishPageData
page or an Agent step's room named through a template must, once rendered,
belong to the run's project (or be project-less for a project-less run), on
every run whoever triggered it. A literal page or room id is the workflow
author's choice and is used whatever its project. On a token's workflow save,
update or import, a step's `page_publish.page_id` and `room_id` are what the
workflow will write into: each must belong to the token's project and to no
other, so a token never saves a literal shared or foreign page or room. A
page slug never looks like an id, and a page is resolved by id before slug. In an import, only the resources the bundle
lists are internal, per kind (`workflow.id` and `referenced_workflows` for
workflows, `referenced_quick_prompts`, `_quick_apis`, `_quick_execs`,
`_pages`); a room, config or connection is never internal. A non-JSON body is
refused, and so is a write with a body and no Content-Type (only the
context-file upload is multipart). Ids are resolved the way the handler
resolves them (`KT-12` references, an execution named by its task, an offer
through its execution, an invite or resume credential to its room; a message,
file, dispatch, CLI session, workspace, orchestration run or proposal as its
discussion; a media job as its discussion or project; an audit run as its
project) and an id that resolves to nothing is refused. A refusal names the
kind, never the other project's id. Then:
- *shared resources* (project-less, serving every project, or several projects)
  may be read, never written; a write needs a resource that belongs to the
  token's project and to no other; a project-less discussion or workflow run is
  private to its own launch (a run's project is its own, never its workflow's
  current home); run lists, their state filter and pages, and a workflow's
  `last_run` only consider runs of the token's project or its own run, in SQL
  before any limit, and so do the duration estimates of workflow trigger, run
  status and wait; a Quick Prompt's estimate counts only launches in the
  caller's project (a bridge token's, or an in-process agent's); an in-process agent's workflow list takes its `last_run`
  from its own project's runs; an
  MCP config linked to projects and opted into General serves those projects
  and project-less tokens, as the plugin overview shows;
- *effects* need the token's project, or a shared resource on a route whose
  handler runs it for that project (workflow, Quick Prompt and batch triggers
  get the project added; Quick API, Quick Exec and `agent-api/call` read the
  token's project in the handler and refuse a config it cannot see); a
  project-less token runs only project-less resources; effects are logged with
  the token id;
- *projects*: a created resource is forced into the token's project, an update
  cannot clear, change or widen its project, and a workflow may be scoped to the
  token's project only (`null` or `Projects:[bound]`); an agent's workflow save
  stays in that project and never approves script content (KT-918);
- *sessions and invites do not cross projects* (deliberate rule): every route
  in `SESSION_KEYED_ROUTES` (peer-join, peer-resume, orchestrator-return-resume,
  peer-leave, the workspace and its history lease, link, unlink,
  transfer-session, accept-offer, find_by_session, session-status) resolves the
  room its credential or session names first and refuses one outside the
  token's scope (peer-join also checks every room where the named session is
  active, since joining ends it there); a write whose session resolves to no
  room is refused, and a
  token never forces a session reassignment. Any other route naming a joined
  session needs that session's room visible. A test lists every request type
  carrying a caller-supplied session; a new one fails until reviewed. An
  invite for another instance's room is refused;
- *a saved Quick Exec* (`quick_exec_id`) is a resource: an import validates the
  ones it does not bundle like a create does, and a run refuses one of another
  project; a token's learning proposal is forced into its project and never a
  preference; its media generation without a discussion creates one in its
  project, which the token then owns;
- *planning*: a token's planning write is recorded as an agent's, whatever
  actor it names; a `kronn-plan-action` fence posted in a room may only touch
  tasks of the room's project and of no other (an item naming another
  project's task, or a task shared with one, is refused on arrival and again
  at apply), a created task lands in the room's project, and the card
  names that project.

*Responses.* Every response is scoped, whatever the verb and its shape (an
`ApiResponse` through its `data`, any other JSON body as a whole): an object
naming any resource outside the scope, at any depth, is dropped from its list
(only the count paired with that list follows) or refused when it is the
response itself; an object naming an id that resolves to nothing is hidden. A
workflow export is all or nothing: one bundled dependency outside the scope
refuses it. The plugin overview shows a token only the configs its project
may use, that project's customised contexts, and no server detected in
another project; a page's feeding workflows are scoped like a workflow list. The discussion list, the task
list and the discussion search filter in the query, so pages stay full.

The WebSocket bus refuses a bridge token (403) and any credential other than the
operator token (401); without a credential it keeps loopback trust. The bearer
scheme is matched case-insensitively, a bridge token in any other
Authorization form is refused, and the routes answered before the gate for
remote peers (claim-by-token, fetch-file) refuse a bridge token. Any bearer
that matches neither the operator token nor a live bridge token is refused
outright, never downgraded to loopback trust. Not covered by an agent's
environment any more but still inheriting the desktop's own: the self-restart,
the one declared exception of the spawn inventory above.

**Deferred to 0.15 — the residual path, stated plainly.** Loopback requests
*without* a token keep today's trust. An agent on the same machine can drop its
bridge token and call any route from `127.0.0.1`, secret-class routes included
(reveal, export, recovery, exec). What 0.14.3 removes is the admin token and the
key from the agent's environment, the exec routes' environment and file dump,
and every route outside the bridge's list for a token-bearing caller. On an
instance with strict localhost or LAN exposure, the agent no longer holds a
credential that opens the API. The per-action human proof (layer B, second
half) closes the loopback path in 0.15.

**Real probe for the human.**
1. Start a Claude discussion on a project, with Kronn running natively.
2. Ask Claude to run `env | grep -E 'KRONN|_API_KEY|_TOKEN'`.
3. Expected: `KRONN_DISCUSSION_ID`, `KRONN_BACKEND_URL`, `KRONN_BRIDGE_TOKEN`
   (a `kbt_…` value), Claude's own key only if one is configured in Kronn, the
   the GitHub token only if the project is connected; never `KRONN_AUTH_TOKEN`,
   `KRONN_ENCRYPTION_KEK` nor another provider's key.
4. Ask Claude to call a room tool (`disc_meta`, then `disc_append` a short
   line): both work.
5. Ask Claude to `curl -H "Authorization: Bearer $KRONN_BRIDGE_TOKEN"
   http://127.0.0.1:3140/api/config/export`: expect HTTP 403.
6. After the turn ends, the same `curl` with that token value on
   `/api/discussions` answers 401.
