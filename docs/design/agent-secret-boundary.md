# Agent secret boundary (0.14.3 design note)

Status: **proposal, awaiting validation** (KT-1006, KT-1007, KT-990, KT-969).
Source: the 0.14.3 audit, room `c3a5311c`, security report SEC-1 to SEC-15.

## 1. Threat model

The actor is **an agent that Kronn launched and that a prompt injection now
drives** (ticket text, web page, repository file). It runs:
- natively, as the user's OS account, with full read access to the user's files;
- under Docker, in the backend's container, with the backend's UID.

Out of scope: an attacker who already controls the user's account outside
Kronn, and malware running on the host before Kronn starts.

The promise to keep: **an agent never obtains Kronn's encryption key, its admin
API token, or a decrypted secret it was not explicitly given for its task**
(values injected for its own project's MCPs stay readable to it until the MCP
gateway, KT-968).

## 2. Where the promise breaks today (verified in code at d4d1ff69)

| # | Path | Proof |
|---|------|-------|
| 1 | The agent inherits `KRONN_AUTH_TOKEN`, `KRONN_ENCRYPTION_KEK` (raw key) and, under Docker, every provider key | `main.rs:165-169` exports the token into the backend's own env; `agents/runner.rs` `try_spawn` and `acp.rs` `native_command` inherit it all |
| 2 | Any loopback caller is "local trusted" | `lib.rs` `auth_allows`: `local_trusted \|\| has_valid_token`, and no token configured means open |
| 3 | Secret-returning routes are open to that trust | `/api/config/auth-token`, `/api/mcps/configs/{id}/reveal`, `/api/external-api/connections/{id}/reveal`, `/api/execution-context/.../reveal`, `/api/mcps/bundles/export?include_values`, `/api/config/recovery/set` (wraps the key under a caller-chosen passphrase and overwrites `recovery.key`) |
| 4 | Files next to the database hold the key | `encryption_key` sidecar (`core/keyvault.rs` tier 3), `encryption_secret` still serialized in `config.toml` (`models/setup.rs:39-41`, rewritten at each boot by `core/keystore.rs:135-147`) |
| 5 | Credentials in plaintext `config.toml` | `tokens.keys[].value` (`api/external_api_connections.rs:1690-1701`), `server.auth_token` |
| 6 | Publication admin secret in a plaintext file | `core/operator_secret.rs:31` (`human-admin-secret`) |
| 7 | Docker on macOS exposes the native install and writable CLI configs | masks miss `~/Library/Application Support/com.kronn.kronn`; `~/.claude`, `~/.codex`… mounted read-write (host code execution through hooks) |

Rows 1-3 make every file-level measure (KT-990, KT-969) insufficient on its own:
an agent that can `curl 127.0.0.1:3140/api/mcps/configs/<id>/reveal` never needs
to read `kronn.db`.

## 3. Design

Four layers, each closing a row of the table. A layer is only worth shipping
with the ones above it.

### Layer A — the agent's environment is built, not inherited (KT-1006, KT-1013)

One function builds the environment of every agent child process, for all three
routes (native ACP, adapters, direct):
- start from an **allow-list** of the user's environment: `PATH`, `HOME`,
  `LANG`/`LC_*`, `TERM`, `TMPDIR`, `SHELL`, proxy variables, and the agent's
  own documented variables;
- add what the launch needs: its provider key (only its own), room or workflow
  context, `KRONN_DISCUSSION_ID`, per-launch MCP references (KT-964, KT-1003),
  and a **scoped bridge token** (layer B);
- never: `KRONN_AUTH_TOKEN`, `KRONN_ENCRYPTION_KEK`, other providers' keys,
  `GH_TOKEN` unless the project enables it.

`main.rs` stops exporting `KRONN_AUTH_TOKEN` into its own environment.

### Layer B — two caller classes on the API (KT-1006)

- **Bridge token**: minted per agent launch, kept in memory, bound to one
  discussion (or one task execution / workflow run), expiring with the launch,
  revocable. It only opens the routes the `kronn-internal` bridge calls. The
  agent can read it (it is in the bridge's environment), so its scope **is**
  what the agent may do through room tools — that is acceptable by design.
- **Human session**: what the UI uses.
- **Secret-class routes** (table row 3, plus key import/export, recovery,
  human credentials admin, auth token rotation) **refuse the bridge token and
  stop accepting loopback trust alone**.

Open decision D1: how a human session proves it is human on loopback, since
anything the browser can send from 127.0.0.1, a same-user process can send too.

| Option | How | Cost |
|---|---|---|
| D1-a (recommended) | Secret-class routes need a short **elevation** (5 min) obtained by typing a passphrase the agent never sees: the recovery passphrase when set, else a new "secrets passphrase" created on first use | One prompt before reveal/export/recovery |
| D1-b | Desktop: Tauri IPC only; browser: disabled | Browser users lose reveal/export |
| D1-c | Keep loopback trust, rely on layers C-D only | Leaves table row 3 open |

### Layer C — storage consolidation (KT-1007, prerequisite of KT-990)

- `encryption_secret` no longer serialized; existing values migrated into the
  vault chain, then removed, only after a successful decrypt check of every
  encrypted column.
- `tokens.keys[].value` and `server.auth_token` move to an encrypted table;
  `config.toml` keeps ids and labels only.
- Reconcile enumerates **every** encrypted column (MCP configs, connections,
  execution-variable snapshots…), from one registry the tests check against the
  schema.
- A vault **read error** is an error: startup stops with a clear message; it is
  never treated as "empty" and never followed by `mirror()` overwriting the item.
- Debug builds get an opt-in to the Keychain (`KRONN_DEV_KEYCHAIN=1`) so the
  macOS path can be tested from start-dev.

### Layer D — files out of the agent's reach (KT-990, KT-969)

- macOS / Windows: once layer C holds, the key lives only in the OS store; the
  sidecar is removed after a verified restore test from the recovery
  passphrase. `kronn.db` stays `0600` (done in 0.14.2).
- Native agents: deny reads of the data directory in each CLI's own sandbox
  where it exists (Claude `permissions.deny` on the data dir, Codex sandbox
  config). This is defense in depth; the OS user boundary is not crossed.
- Linux / WSL without a keyring: documented limit; the key stays in the
  sidecar, protected by layers A-C only.
- Docker: agents run under a second UID without access to `/data`
  (`0700` backend), repositories shared through a group; the masks also cover
  the native data directory on macOS; CLI config mounts become read-only or are
  copied in at start.

## 4. Order and rollout

1. KT-1007 (layer C), behind a migration that refuses to remove anything until
   every column decrypts. Test with real 0.14.2 config and database copies.
2. KT-1006 + KT-1013 (layers A-B). The Python bridge moves to the bridge token in
   the same commit, or rooms go mute; a two-turn Claude discussion, a Codex
   discussion and one native ACP agent are the real-run proof.
3. KT-990 (layer D native), then KT-969 (layer D Docker).
4. KT-968 (gateway) can follow in 0.15 once the advisory states the remaining
   exposure: values of the agent's own project MCPs.

## 5. Tests that prove the boundary

- An agent process environment contains none of the forbidden variables
  (unit test on the builder, per route).
- With a bridge token, every secret-class route returns 401/403; room routes
  work (API tests).
- Loopback without elevation cannot reveal, export values, read the auth token
  or set recovery (API tests).
- Config and database copies from 0.14.2 migrate without losing a secret, and a
  simulated vault read error stops startup (tests).
- Real probe: a Claude discussion asked to `env`, `curl` the reveal route and
  `cat` the data directory gets nothing usable.

## 6. Decisions requested

- **D1** human-session proof (table above).
- **D2** whether `GH_TOKEN` stays global by default (today every Claude/Codex
  launch receives `gh auth token`) or becomes a per-project opt-in.
- **D3** whether KT-968 (MCP gateway) moves to 0.15 with the residual risk in
  the advisory.
