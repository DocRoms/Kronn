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
- **Exception (KT-1656): the credential files of the home.** The login of one
  agent CLI is not another's to read, so every confined launch also loses the
  stores of the *other* agents, and a *sandboxed* launch the shared ones. See
  [Credential files of the home](#credential-files-of-the-home-kt-1656).
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
  [src: file: backend/src/core/vault_guard.rs:416-426#claude_vault_permission_deny]
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

### Credential files of the home (KT-1656)

A task worker that probed Gemini CLI printed `~/.gemini/settings.json`, tokens
included, into its transcript (stored, and sent to the provider on every later
turn). `core::home_credentials` lists the credential stores of the home that
exist on the host, as given and with their links resolved, in two tiers:

| Tier | Stores | Hidden from |
|---|---|---|
| Other agents' | `~/.gemini`, `~/.codex`, `~/.claude.json`, `~/.claude/.credentials.json`, `~/.claude/settings.json`, `~/.claude/settings.local.json` (their `env` block, `apiKeyHelper` and `mcpServers.*.env` have the shape that leaked from `~/.gemini/settings.json`), `~/.copilot`, `~/.config/github-copilot`, `~/.kiro`, `~/.local/share/kiro-cli`, `~/.vibe`, `~/.local/share/opencode`, `~/.config/opencode` | every confined launch, except the store of the agent that is running |
| Shared | `~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.azure`, `~/.config/gcloud`, `~/.config/gh`, `~/.config/glab-cli`, `~/.config/hub`, `~/.git-credentials`, `~/.netrc`, `~/.npmrc`, `~/.yarnrc.yml`, `~/.pypirc`, `~/.cargo/credentials*`, `~/.docker/config.json`, `~/.kube/config`, `~/.m2/settings.xml`, `~/.gradle/gradle.properties`, `~/.vault-token`, … | a **sandboxed** launch only |

[src: file: backend/src/core/home_credentials.rs:178-194#hidden_in]

| Launch | Tiers hidden | Mechanism |
|---|---|---|
| Claude task worker, read-only review (its own sandbox) | both, **including Claude's own** (the CLI reads its login from outside the sandbox it gives its shell) | `sandbox.filesystem.denyRead` and a `Read(…)` rule per store; independent of the vault setting |
| **Any restricted launch**, whatever the agent (Claude, Codex, Gemini, Kiro, Copilot, Vibe, OpenCode, an ACP adapter), and a Codex `read-only` / `workspace-write` one | both, but the running agent's own store | the Kronn profile / `bwrap` mounts |
| A launch the owner opened in **full access** (CLI, ACP runtime) | other agents' only | the Kronn profile / `bwrap` mounts |
| The validations Kronn runs in a worker's tree | other agents' | the lane's sandbox |

- **The tier follows the launch's real trust level, not the kind of its
  sandbox.** `vault_guard::launch_reach` gives `Reach::FullAccess` only to a
  launch with no sandbox of its own that the owner opened in full access
  (`try_spawn`'s `full_access`, which is `false` for a task worker and for a
  launch under a read-only repository policy; the `full_access` of the ACP
  adapter's broker; the one `spawn_native` verified). Every other launch,
  including a restricted Gemini, Kiro, Copilot or Vibe CLI launch and a
  restricted ACP adapter, is `Reach::Sandboxed`. A native ACP runtime starts only
  with its full-access setting on, so it is always a full-access launch, unless it
  carries read-only grants (`NativeLaunchEnv::read_only_grants`): every route asks
  `runner::reach_is_full_access(launch_full_access, has_read_only_grants)`. No
  native agent can hold such grants today (`read_only_repos` is refused for any
  agent but Claude Code and Codex), so on that route it is defense in depth.
  [src: file: backend/src/core/vault_guard.rs:548-554#launch_reach]
- Owner decision (2026-11-10): the owner's rule above holds for an agent in full
  access: `~/.ssh`, `~/.aws`, `gh` stay reachable, as in the owner's terminal.
  Only the restricted launches lose them. The validations keep the shared files
  readable: a package manager or the Docker CLI fails on a file it is refused.
- **Independent of "Protect the Kronn vault from agents".** With the setting off,
  `trust_sandbox::credential_confinement` still builds a confinement, which hides
  the stores and no Kronn data; it is `None` only when the host has no sandbox, or
  when the setting is off and there is no store to hide.
  [src: file: backend/src/core/vault_guard.rs:566-581#launch_confinement_for]
- A path that does not exist is not listed (bubblewrap cannot mount over it);
  a store created after the launch is not covered until the next one. A `HOME`
  other than the user's is not followed.
- **A store is hidden where the tools would put it on this host**, not only under
  the home. Each store is also resolved through the environment of the Kronn
  process (read through `core::child_env::var_os`): `CLAUDE_CONFIG_DIR`,
  `CODEX_HOME`, `GEMINI_CLI_HOME` (the Gemini CLI's home, which holds `.gemini`),
  `XDG_CONFIG_HOME` (everything of `~/.config/…`: `gh`, `gcloud`, `glab-cli`,
  `opencode`, `github-copilot`, `git/credentials`), `XDG_DATA_HOME`
  (`~/.local/share/…`: `opencode`, `kiro-cli`), `GNUPGHOME`,
  `AWS_SHARED_CREDENTIALS_FILE` and `AWS_CONFIG_FILE`, `KUBECONFIG` (each entry
  of the list), `DOCKER_CONFIG`, `CARGO_HOME`, `NPM_CONFIG_USERCONFIG`,
  `GH_CONFIG_DIR`, `CLOUDSDK_CONFIG`. The default place stays hidden too: a tool
  run without the variable still finds its store there. A relative value is
  ignored (it is relative to the working directory of the tool, which Kronn does
  not know), and so is one that names the home or a parent of it
  (`GNUPGHOME=$HOME`), which would hide everything. `GEMINI_CLI_HOME` was
  verified against the installed Gemini CLI 0.66.0 (its bundled `homedir()` returns
  `process.env["GEMINI_CLI_HOME"]` when set, else the OS home, and the global
  directory is `<that home>/.gemini`, `oauth_creds.json` in it); it is not a
  documented contract, so re-check it when the CLI is upgraded.
  `CLAUDE_CONFIG_DIR` moves Claude's `.credentials.json`, `.claude.json`,
  `settings.json` and `settings.local.json` alike.
  [src: file: backend/src/core/home_credentials.rs:214-252#locations]
- Linux masks a directory with an empty tmpfs and a file with `/dev/null`
  bound over it.
- On a host with no sandbox the wrapped launches (full access, Codex, the other
  CLIs, ACP) are not confined, so these stores are open to them; a Claude worker's
  settings carry the denial either way. With "Protect the Kronn vault from
  agents" off they are hidden all the same (see above).
- **Stored output is masked, with a measured false-positive rate.** An Agent
  message is redacted when it is inserted (`db::discussions::mask_agent_output`
  → `core::redact::redact_agent_output`), irreversibly, and what a human typed is
  left alone. It masks the vendor token shapes, and a secret-named assignment or
  JSON key (`"GITHUB_PERSONAL_ACCESS_TOKEN": "…"`, `GEMINI_API_KEY=…`, the `env`
  block of a CLI's `settings.json`) **only when its value is a credential**: the
  audit redaction used for audit artifacts also rewrote code and prose that merely
  names a token (`let token = lexer.next_token();`, `Authorization: Bearer $TOKEN`),
  which an agent's reply is full of. A value is not a credential when it is a
  placeholder (`$TOKEN`, `<your-token>`, `your-api-key`, `changeme`, `xxxx`,
  `kr-join-...`, `…`), the name of an environment variable (`VOTRE_API_KEY`),
  prose (whitespace, or a script with no spaces), a dotted or snake_case
  identifier, a camelCase / PascalCase word (`initGeneration`, `TooLarge`), a
  lowercase route or path (`session/cancel`), the name it is given to repeated
  (`CHANNEL_TOKEN = 'channeltoken'`), a number of fewer than 8 digits (`PIN=1234`),
  a word in backticks right after `token:` / `secret:` in prose, a CSS variable, a
  reference (`secret://NAME`), or, for a bare value,
  shorter than 12 characters or without letters and digits or shaped like code (a
  call, a path, a URL). A quoted value counts from 8 characters, 6 under a
  `password`-like name; a `pin` name needs 8 to 12 digits. The price is the
  inverse risk: a short bare secret under a harmless name is stored as written.
  A secret word followed by a qualifier (`key`, `value`, `string`, `str`), after
  any run of `_` / `-` (`SECRET__KEY`, `secret--value`), still
  names a secret (`SECRET_KEY`, `TOKEN_VALUE`, `passwordString`): the value is then
  judged by the same rules as under the bare word.
- **A room invitation is stored as written.** An agent posts
  `disc_join({token: "kr-join-<32 hex>"})` on purpose, so that a peer can join
  within the invitation's ten minutes (`INVITE_TTL_SECS`; the database keeps only
  its hash). Masking it at insert would break the invite, so the insert path
  (`is_invite_token`: `kr-join-` and 16 characters or more) leaves it. The audit
  passes (`redact_for_audit_artifact`, `redact_stored_text`, so the owner-run
  `redact-discussion`) still mask it: they run on history, long after the expiry,
  and a `kr-join-` token found in a public place means a compromised room.
- The login files of the other agents are masked by shape too: Google OAuth
  access (`ya29.…`) and refresh (`1//0…`) tokens (`~/.gemini/oauth_creds.json`),
  GitHub fine-grained `github_pat_…` and AWS STS `ASIA…` keys.
- **Masked once, then fed everywhere.** `insert_message_inner` masks the content
  one time and hands that text to the row, `ingest_message_proposals`,
  `ingest_message_actions`, `ingest_message_questions` and
  `audit_validation::on_agent_message`: a `kronn-*` fence that carries a token is
  still ingested as a card, with the token masked in the card's own tables. The
  streaming checkpoints (`set_partial_response`,
  `set_partial_response_for_dispatch`) mask their text the same way, so
  `discussions.partial_response` and the message a boot recovery makes of it hold
  no clear secret. **A peer receives the stored text too**: `federate_message`
  sends an Agent message as its stored (masked) row, or masks the in-memory text
  when no row is found, never what the agent wrote; every caller of
  `federate_message` goes through it (the runner's reply, a source append, a
  refused start). The `validation_documents` of a validation reply are not reply
  text: they are the fingerprints of the documents on disk, captured after the
  validation child stopped.
  The regression samples are `AS_WRITTEN` in `db::insert_redaction_measure`.
  [src: file: backend/src/core/redact.rs:702-707#redact_agent_output]
- **Measured on a corpus, not yet on a database.** `db::insert_redaction_measure`
  (ignored) runs the insert path over every Agent message of a copy of a database
  and/or over text files cut into message-sized chunks. On this repository (prose,
  code, hashes, git history: 21,926 chunks) the audit redaction changed 1,508 and
  the insert path 227, each one reviewed. After the widening of round 3
  (`ya29.` / `1//0` / `github_pat_` / `ASIA` shapes, secret-word names with a
  qualifier), the same run over the merged tree (20,412 chunks, a corpus rebuilt
  from the repository, so not the same chunks) changes 206 (1.0%; the audit
  redaction 1,430); the changes the widening adds are all the fixtures of its own
  tests. Each change is: all are credential-shaped literals in
  test fixtures and demo scripts (fake `sk-…` / `ghp_…` keys, `password=hunter2`,
  `postgres://user:secret@…`), none is prose, code that only names a token, a
  commit SHA, a UUID or a digest. The false positives the review found
  (placeholders, ellipses, env-var names, an i18n key, a CJK sentence under a
  `*_token` key, the word "pin", a CSS variable, `user:pass@host`) were fixed and
  are regression samples. The run over a copy of a real database is the owner's:
  `docs/operations/db-backup.md#redaction-of-stored-credentials-kt-1656`.
  The principal ran it on the 16,004 Agent messages of the owner's database
  (read-only extract): 42 would be masked (0.26%), each reviewed. True positives:
  a real `KRONN_AUTH_TOKEN` and credential-shaped URL examples. False positives,
  fixed and now samples: 12 room invitations (`kr-join-`) and 5 identifiers or
  prose (`"token":"initGeneration"`, `` cancel token: `session/cancel` ``,
  `` un secret : `TooLarge` ``, `CHANNEL_TOKEN = 'channeltoken'`, `PIN=1234`). After
  those fixes the repository corpus run (20,421 chunks) changes 209, the three
  more than before being fake credentials in the new tests.

#### What is scrubbed after the fact, and where

A printed credential file is copied by more than the message it was quoted in.
`db::credential_scrub::TARGETS` lists every column that can carry an agent
transcript or tool output, and two passes read it:

- **Migration 293, on every install** (`redact_known_shapes`): a value with a
  vendor-shaped token (`ghp_…`, `AIza…`, `sk-…`, a JWT, a `Bearer` value, a
  `"password": "…"` field) is masked, a placeholder and every other value stay as
  written, and no secret-named JSON key masks by itself. Agent messages only for
  `messages`; every listed column elsewhere.
- **The owner-run audit of one discussion**, `POST /api/maintenance/redact-discussion`
  with `{"discussion_id": "<id or its first 8+ characters>"}` (the full audit
  redaction, whatever the role, over every table below that belongs to the
  discussion). It names no discussion in the code, is gated like the other
  destructive POSTs and is not reachable with an agent's bridge token.

| Table.column | Carries | Pass |
|---|---|---|
| `messages.content` | the transcript | 293 (Agent), audit |
| `message_tombstones.content`, `message_revision_events.content` | the copy a deletion or an edit keeps | 293, audit |
| `discussions.partial_response`, `discussions.summary_cache`, `disc_summary_ranges.summary` | a turn in flight, text written from the transcript | 293, audit |
| `discussion_important_messages.payload_json`, `discussion_questions.{payload_json,answer_json,resume_error}`, `discussion_actions.{values_json,diagnostic}`, `planning_proposal_items.{payload_json,rejected_reason}` | excerpts, answers, and the cards ingested from the `kronn-*` fences of a message | 293, audit |
| `agent_dispatch_jobs.{last_error,progress_detail}`, `agent_resume_jobs.{result_json,last_error,reason}`, `shared_runs.{result_json,exec_details_json,diagnostic}` | job results and errors | 293, audit |
| `task_execution_deliveries.manifest_json`, `task_execution_reviews.decision_json`, `task_execution_validation_runs.output`, `task_executions.{blocked_reason,outcome_reason}`, `task_execution_events.changes_json`, `delivery_summaries.canonical_json` | what a worker delivered, a principal decided, a command printed | 293, audit (by the execution's `sub_discussion_id`) |
| `planning_task_dod_items.evidence`, `planning_task_events.changes_json`, `planning_tasks.{description,blocked_reason}`, `review_findings.evidence`, `learnings.evidence_json` | evidence and notes agents attach to tasks | 293 (audit for `learnings` by discussion) |
| `workflow_runs.step_results_json`, `workflow_run_pins.content_json`, `workflow_watch_state.{last_result,last_error}`, `quick_exec_runs.{summary,diagnostics}`, `tasks.last_status_json` | step and run outputs | 293 |
| `api_call_logs.{request_excerpt,response_excerpt,error_message}`, `media_jobs.last_error`, `mcp_probe_results.summary`, `mcp_auth_failures.message`, `lite_llm_model_failures.error_message`, `trust_audit_log.detail` | what a call, a probe or a job reported | 293 (audit by discussion where the row has one) |
| `audit_runs.{documentary_notes,recommendations_json,step_outcomes_json}`, `audit_cross_findings.record_json` | what an audit agent wrote | 293 |

Not listed, with the reason: **no FTS or other virtual table exists** (nothing
creates one, and a test asserts the schema holds none), so there is no shadow copy
to rebuild after a rewrite; `context_files.extracted_text` is a file a human
uploaded, not agent output; configuration tables (`workflows.steps_json`,
`quick_prompts`, `projects`, …) hold what a human wrote; hashes (`*_hash`) hold no
text. A file on disk (`quick_exec_runs.artifact_path`, a run's artifacts
directory, a backup) is outside the database and not rewritten; the pre-migration
backup keeps the clear text (see `docs/operations/db-backup.md`). A test checks
that every column of the list exists in the schema, so a renamed column cannot
make a pass skip it silently.

### Run artifacts sit outside the vault (KT-1585)

A workflow run's artifacts directory (`{{run.artifacts_dir}}`) used to live in
the data directory, so the deny above hid it from the very agents it is for.
It now lives in `<base>/kronn-run-artifacts/<key>`: the base is
`KRONN_RUN_ARTIFACTS_DIR` when set (Docker: `/run-artifacts`, its own volume),
else the platform cache directory. A base inside a protected directory, or one
that contains it, is refused and the default is used. Details:
[workflow-read-only-repos.md](../operations/workflow-read-only-repos.md#run-artifacts-directory).

For now an Agent step can read any run's artifacts, not only its own, when its
prompt names the path; per-run confinement is planned (KT-1587, 0.15.3). Exec
outputs kept there are scrubbed of known secrets; reports and screenshots a
step writes are not.

## Loopback is not the owner

A request from `127.0.0.1` is trusted as the owner's (browser, desktop shell,
scripts). An agent process is on that same loopback, so the routes that take over
or destroy the vault need a code of the strong confirmation (TOTP,
`core::trust_proof`) once one is enrolled. KT-1496's `strong_gate` asks for it on
every channel, loopback included (see `strong-confirmation-routes.md`):

`POST` on `/api/setup/reset`, `/api/config/import`,
`/api/config/auth-token/regenerate`, `/api/config/recovery/{set,reencrypt}`.
[src: file: backend/src/core/strong_confirm.rs:43-85#ALWAYS]

- The code goes in the `X-Kronn-Confirm` header, a `kronn_confirm` body field or a
  single-use grant; the UI asks for it in `StrongConfirmDialog`.
- One gate, one code: a second loopback-only gate (KT-1495's first version) spent
  the code before `strong_gate` saw it, which then refused it as a replay. It was
  removed in favour of `strong_gate` (KT-1496 review, 03/11).
- **With no enrolment, nothing changes**: the route works as it did, and an
  unreadable record counts as no enrolment. Enrolling is what closes the door.
- The recovery routes carry their own proof and ask for no code
  (`OWN_PROOF`, owner decision 03/11): a restore needs the recovery passphrase, and
  `start-new-key` only acts while the key is absent (it refuses while the key is in
  use), when the seed that would check a code is unreadable too.
  [src: file: backend/src/core/strong_confirm.rs:126-137#OWN_PROOF]
- `POST /api/config/auth-token/regenerate` is also in `DESTRUCTIVE_POSTS`
  (`lib.rs`): a remote peer is refused even while auth is off, like a reset.
  [src: file: backend/src/lib.rs:1271-1293#DESTRUCTIVE_POSTS]

## Files

| Where | What |
|---|---|
| `core/vault_guard.rs` | which launches are wrapped, the Claude rules |
| `core/home_credentials.rs` | the credential stores of the home an agent must not read, and where the environment moves them (KT-1656) |
| `db/credential_scrub.rs` | every column that can hold agent output, and the two passes that mask it after the fact (KT-1656) |
| `agents/runner.rs` `try_spawn` | the one spawn point of every native agent |
| `agents/runner.rs` `claude_task_worker_settings` | `denyRead` and `permissions.deny` of the worker |
| `api/strong_confirm.rs` `strong_gate` | the code of the vault routes, on every channel |
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
  `cd backend && cargo nextest run --lib -E 'test(/vault_guard/)'`.
