# Agent secret boundary (0.14.3 design note, v3)

Status: **proposal v3, awaiting validation by Romu and Codex** (KT-1006,
KT-1007, KT-990, KT-969). v1 `7e721fc6`, v2 `9261bfa7`.
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

## 2. Guarantees per platform (after the four layers ship)

| Platform | Key | Admin token / decrypted secrets over the API | Secret files | Code the agent influences (4.6) | Residual exposure |
|---|---|---|---|---|---|
| macOS native | OS Keychain only, after a verified restore test; no sidecar, nothing in `config.toml`. The Keychain item's ACL should make macOS prompt when another program reads it (to verify by probe, including `security find-generic-password` and Kronn's own binary run by an agent) | Agent env carries neither; secret-class routes need a per-action human proof | `kronn.db` `0600`, readable by the same user | Runs as the user: **no OS boundary** | A same-user process can read the DB and drive the live UI session; it cannot read the key without a Keychain prompt; values of the agent's own project MCPs |
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
be revealed or exported (one may be the recovery passphrase).

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

### Layer C — storage consolidation (KT-1007, before any file removal)

DoD items, each with a test:
1. One registry of every encrypted column (MCP configs, connections,
   execution-variable snapshots, the new credentials table). `collect_encrypted_rows`
   reads all of it; a test compares the registry with the schema so a new
   encrypted column cannot be forgotten.
2. `KeyVault::retrieve` distinguishes `Empty` from `Denied`/`Unavailable`.
   Reconcile never mints a key while any registered column holds ciphertext and
   never `mirror()`s into a vault that errored; startup stops with an
   actionable message instead.
3. `encryption_secret` is no longer serialized and reconcile stops setting it,
   in the same change as any file deletion.
4. `tokens.keys[].value` and `server.auth_token` move to an encrypted table.
   The migration decrypts every moved value back before rewriting
   `config.toml`, is interrupt-safe and idempotent (rerun, partial, already
   migrated), and keeps a backup protected by the recovery passphrase.
5. `recovery/set` refuses to replace an existing `recovery.key` without the
   current recovery passphrase or the human factor.
6. Dev builds test the Keychain path with the existing `KRONN_USE_KEYCHAIN=1`
   (`keyvault.rs:172-177`).

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

- A project shows its GitHub connection: source (`gh` login or stored token),
  the **scope actually established** (repositories and permissions read from the
  API), or "scope not verified" when the token type cannot be introspected.
- Off by default for new projects. Turning it off stops giving the token to new
  launches; agents already running keep what they received until they stop, and
  the UI says so.
- An agent can still reach GitHub through the user's local `gh` login (its
  config is in the user's home). Natively this is not closable by Kronn and is
  stated; under Docker the `gh` config is not mounted unless enabled.

## 5. Order and rollout

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
- **D2** (updated): GitHub connection per project, off by default, scope shown
  or marked unverified (4.5).
- **D3:** KT-968 in 0.15.2 with the residual risk in the advisory (already
  tagged by the 0143-scope decision).
