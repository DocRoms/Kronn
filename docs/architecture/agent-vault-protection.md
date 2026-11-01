# Agent processes and the Kronn vault (KT-1495)

An agent CLI runs as the user who runs Kronn. Its own tools (a shell, a file
reader) could read the Kronn data directory — `encryption_key`, `recovery.key`,
`human-admin-secret`, `config.toml`, `trust-proof.json`, `kronn.db` — whatever
Kronn's gates say, and an agent on the loopback could call the API as the owner by
leaving its bridge token out. This note states what is closed, what is not, and
why. It builds on the OS confinement of the trust levels
([trust-levels.md](trust-levels.md)) and on the design note
[agent-secret-boundary.md](../design/agent-secret-boundary.md).

## Owner rule: never bridle more than before

An agent in full access may reach everything it could before. Kronn closes **its
own vault** and nothing else:

- **Only the Kronn data directory** (and `KRONN_BACKUP_DIR` when it lives
  elsewhere) is denied, read and write. Project databases, other repositories,
  the credentials of other tools (`~/.aws`, `gh`), Docker and the network stay as
  they were.
- **The Kronn API port stays open.** The agents' MCP bridge and the owner's Quick
  Exec scripts go through it. It is not denied by the sandbox.
- **`auth_strict_localhost` is unchanged** (off by default): it would break every
  local script and the browser, and a human is not recognised by an address.
- **The setting "Protect the Kronn vault from agents"**
  (`server.protect_vault_from_agents`, on by default) turns all of it off.
- **Without an available sandbox** (Windows, no `bwrap`, a container that forbids
  user namespaces) nothing is wrapped and **nothing that works today is refused**:
  Settings › Security says the protection is unavailable, with the reason
  (`trust.vault.unavailable`).

## What an agent can and cannot read, per runtime

"Closed" means a shell or a file tool of that agent gets "permission denied" (or
an empty directory on Linux) for the data directory.

| Runtime / launch | Data directory | Mechanism |
|---|---|---|
| Claude, full access, any audit (`--dangerously-skip-permissions`) | **closed** | the process is started under the OS sandbox; plus `permissions.deny` for `Read`, `Edit`, `Write` |
| Claude, a plain discussion or a room | **closed** | same |
| Claude, task worker | **closed** | Claude's own sandbox, told `sandbox.filesystem.denyRead` (its `allowRead` is only additive) plus the same `permissions.deny`; writes stay confined to the lane |
| Codex, full access and audits (`--sandbox=danger-full-access`) | **closed** | the process is started under the OS sandbox |
| Codex, default `read-only` sandbox, and the task worker's `workspace-write` | **closed** | Kronn **replaces** Codex's sandbox (a Seatbelt profile cannot be applied inside another: `sandbox_apply: Operation not permitted`): Codex runs `--sandbox=danger-full-access` inside a Kronn-generated profile equivalent to the mode, with the data directory denied on top (see below). The model's commands get the host network, which Codex's own modes withheld |
| ACP runtimes (OpenCode, Gemini, Copilot, Kiro, Vibe) | **closed** | the process is started under the OS sandbox (they exist only with full access) |
| ACP adapters of Claude and Codex | as the CLI above | same detection of the CLI's own sandbox |
| HTTP / Ollama agents | not applicable | no shell; confined by `resolve_in_workspace` |
| Any of the above on a host with no sandbox, or with the setting off | **open** | reported in Settings › Security; nothing is refused |

How the choice is made, in one place: every native agent process goes through
`try_spawn`, which asks `vault_guard::launch_sandbox` whether the CLI brings its
own OS sandbox. A Claude launch whose `--settings` enable it keeps it (and tells
it the data directory); a Codex launch that did not turn its sandbox off has it
replaced; everything else is wrapped as it is.
[src: file: backend/src/core/vault_guard.rs:132-142#launch_sandbox]
[src: file: backend/src/core/vault_guard.rs:319-338#wrap_launch]

- macOS wraps with `sandbox-exec` (allow by default, one deny on the data
  directory). Linux wraps with `bwrap`: the data directory is replaced by an empty
  tmpfs, the host network is kept. `--new-session` is dropped for an agent: it is
  its own process-group leader (cancellation kills the group) and `setsid` refuses
  a group leader. Every `bwrap` form (the generic wrap and the Codex replacement)
  gives the agent its own PID namespace and a fresh `/proc`, and mounts the tmpfs
  on the resolved path: the host `/proc` would let `/proc/<backend pid>/root/<data dir>`
  walk around the tmpfs.
- Claude's rules name the directory as `Read(//abs/path/**)` (two slashes: a path
  from the filesystem root), for `Read`, `Edit` and `Write`. A deny rule is checked
  before the permission mode, so the bypass flag does not lift it.
  [src: file: backend/src/core/vault_guard.rs:416-426#claude_permission_deny]
  `sandbox.filesystem.denyRead` is a documented field of Claude Code's sandbox
  settings ("paths to deny reading within the sandbox, merged with paths from
  `Read(...)` deny permission rules", read from the installed 2.1.285 binary's
  settings schema); the `//` spelling is Claude's documented form for a path from
  the filesystem root and is proven by the live probe below, not by a unit test.

### Codex: the sandbox is replaced, not nested

Codex's `read-only` and `workspace-write` modes are enforced by a Seatbelt profile
(macOS) or bubblewrap (Linux) that Codex applies to the commands the model runs.
Wrapping Codex in Kronn's profile would nest the two, which macOS refuses. Kronn
therefore starts Codex with `--sandbox=danger-full-access` (dropping any
`--sandbox` it carried) inside a profile of its own that reproduces the mode:

| Mode it replaces | Writes allowed | Read | Data directory |
|---|---|---|---|
| `read-only` (Codex's default for `exec`) | the temp directories, `CODEX_HOME` (else `~/.codex`), `/dev`; `~/.npm` for an `npx` launch | everything | denied, read and write |
| `workspace-write` (the task worker) | the same, plus the worktree; the worktree's `.git` stays read-only | everything | denied, read and write |

[src: file: backend/src/core/vault_guard.rs:242-275#codex_seatbelt_profile]

macOS is allow-by-default with a broad `file-write*` deny, the allowed roots, the
`.git` deny, and the data-directory deny last (the last matching rule wins). Linux
binds `/` read-only, binds the same roots read-write, `--ro-bind`s the `.git`, then
puts an empty tmpfs over the data directory.

**What the replacement cannot keep: Codex's per-command network block.** Codex's
own sandbox withholds the network from the commands the model runs. Kronn's
profile wraps the whole Codex process, which needs the network for its own model
calls, and neither Seatbelt nor bubblewrap can tell Codex from a command it
starts. The profile therefore leaves the network as it is (the owner rule: deny
only the data directory, never the network), and a Codex shell command can reach
the host network. Codex's own write protection of other files (an `apply_patch`
refused by policy) becomes a permission error from the OS instead.

When the setting is off, or the host has no sandbox, Codex keeps its own sandbox
exactly as before.
- The Claude rules apply whenever the setting is on, even where no OS sandbox can
  be applied: they are the CLI's own and bridle nothing.

### What stays reachable, on purpose

- The Kronn API on loopback, with the agent's bridge token.
- The macOS keychain copy of the encryption key (blocking the keychain breaks `gh`).
- On Linux native, the MCP bridge sidecar keeps its presence binding and onboarding
  marker in `~/.config/kronn`, which is the data directory there: under the
  sandbox they last for one agent launch (a reload within the launch keeps them).
- A file the agent is *given* elsewhere (a copy of the database, a backup in
  another directory) is not covered; neither is a secret in the agent's own
  environment (the child environment is built, not inherited: `core::child_env`).

## Loopback is not the owner

A request from `127.0.0.1` is trusted as the owner's (browser, desktop shell,
scripts). An agent process is on that same loopback, so the routes that take over
or destroy the vault ask a caller admitted **by its address alone** for a code of
the strong confirmation (TOTP, `core::trust_proof`):

`POST` on `/api/setup/reset`, `/api/config/import`,
`/api/config/auth-token/regenerate`, `/api/config/recovery/{set,restore,reencrypt,start-new-key}`.
[src: file: backend/src/api/human_gate.rs:36-44#HUMAN_PROOF_ROUTES]

- The code goes in the `X-Kronn-Confirm` header (the same header as the trust
  routes). The UI prompts for it when the server answers 403 `confirm_required`
  (`sendWithConfirmation` in `frontend/src/lib/api.ts`) and sends the request again.
- The same guards as KT-1492: single use, bound to the route (the object is
  `vault`), journalled with the channel (`proof` / `proof_refused` in
  `trust_audit_log`), one lockout shared by every channel.
- **With no enrolment, nothing changes**: the route works as it did, and an
  unreadable record counts as no enrolment. Enrolling is what closes the door.
- A caller with the **operator token** already holds a credential and is not asked
  again; a wrong bearer is refused outright (it never falls back to loopback). A
  bridge token reaches none of these routes (they are not on its list).
- The gate sits in `auth_middleware` after the bridge gate, for a request that is
  `local_trusted` and has no valid operator token.
  [src: file: backend/src/lib.rs:710-815#auth_middleware]
- Not covered by this gate: the locked-auth recovery routes, which exist for a
  lost key and cannot verify a code encrypted under it.
- `POST /api/config/auth-token/regenerate` is also in `DESTRUCTIVE_POSTS`
  (`lib.rs`): a remote peer is refused even while auth is off, like a reset.
  [src: file: backend/src/lib.rs:1176-1192#DESTRUCTIVE_POSTS]

## Files

| Where | What |
|---|---|
| `core/vault_guard.rs` | which launches are wrapped, the Claude rules |
| `agents/runner.rs` `try_spawn` | the one spawn point of every native agent |
| `agents/runner.rs` `claude_task_worker_settings` | `denyRead` and `permissions.deny` of the worker |
| `api/human_gate.rs` | the loopback proof of the vault routes |
| `acp.rs` `native_command` / `vault_confined` | a native ACP runtime is started behind the same sandbox as `try_spawn` |
| `acp/secret_files.rs` `KRONN_SECRET_NAMES` | `recovery.key` and `trust-proof.json`; the broker refuses them for an ACP read |

## Live probe, per runtime

`agents/vault_probe_test.rs` launches each installed runtime through `try_spawn`
(the ACP one through `start_agent_with_config`) with a prompt that makes its shell
and its file tool read `<data dir>/encryption_key` and `kronn.db`, plus a control
file. It fails if a canary of the vault shows up, and if the control canary does
not (an agent that did nothing proves nothing). It calls real models, so it is
opt-in, and each runtime skips when its CLI is not on `PATH` or when this process
cannot start a sandbox:

```text
cd backend
KRONN_LIVE_VAULT_PROBE=1 cargo nextest run --lib --no-capture -E 'test(/live_vault_probe/)'
# one runtime: KRONN_LIVE_VAULT_PROBE_ONLY=codex_read_only (claude_chat,
# claude_task_worker, codex_read_only, codex_workspace_write, codex_full_access, acp)
```

The probe's work dir is spelled as the OS does (`/private/var/…` on macOS, where
the temp root is a link), so a runtime that compares the paths it is given with
its own cwd does not take its work dir for an outside one (the ACP broker refuses
a question that names no location inside the project).

## Limits

- A shell started by an agent that Kronn did not start is not covered.
- A Claude task worker relies on Claude's own `denyRead`; if a Claude release
  stopped honouring it the live probe `claude_task_worker` fails.
- The Docker image runs the backend and the agents in one container: the data
  directory is the container's. The vault sandbox needs user namespaces there;
  where they are blocked the protection is reported unavailable.
- Real-sandbox tests need a host that can start a sandbox of its own: on a CI
  runner or a local macOS shell,
  `cd backend && cargo nextest run --lib -E 'test(/vault_guard|human_gate/)'`.
