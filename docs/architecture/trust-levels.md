# Trust levels (KT-1492)

A human says "I trust my agents here" at the scope they choose. Approvals that
only protect the user from their own agents are then granted automatically,
journalled, confined by an OS sandbox, and taken back when the level is lowered.

## Model

- **Levels** (`TrustLevel`, `backend/src/models/trust.rs`): `standard` (default,
  every sensitive action asks), `trusted`, `full`. Each includes the one before.
- **Scopes** (`TrustScope`): instance (Settings › Trust levels), workflow, room. The
  narrowest explicit setting wins: room, then workflow, then instance, then
  Standard. An explicit Standard on a room beats a Full instance. When one
  launch belongs to several rooms that disagree, the lowest level wins.
  [src: file: backend/src/db/trust.rs:198-240#effective]
- **The workflow's level is for its own runs.** A workflow's level applies to the
  agents its steps launch (the run is on the bridge token). A room that merely
  edits a workflow never takes that workflow's level: an agent in a room with no
  level, editing a Full workflow, gets no approval from it.
  [src: file: backend/src/api/workflows.rs:2380-2395#approve_exec_steps_by_trust]
- **Storage**: `trust_settings` (one row per scope, no row = inherit), the journal
  `trust_audit_log` (migrations 278 to 282), and two marks on the objects a
  level approved: `quick_execs.approved_by_trust` and the step field
  `exec_approved_by_trust`. Deleting a room or a workflow deletes its setting
  and takes back what its level approved at once, on every path that deletes
  one: `delete_discussion`, `delete_workflow`, a project's rooms and a batch
  run's rooms each delete and withdraw **in one transaction**
  (`trust::delete_and_withdraw`; the batch run does it between its own `BEGIN`
  and `COMMIT`, an import inside its transaction). A withdrawal that fails rolls
  the deletion back, so the scope keeps its setting and its grants; a deletion
  that fails or matches nothing withdraws nothing; inside a transaction the
  caller already holds, both join it. One test per path:
  `deleting_a_room_withdraws_what_its_level_approved` and its siblings,
  `a_failed_withdrawal_keeps_the_room_with_its_level_and_its_grants` and its
  siblings. The
  trigger (migration 282) is the net under any other raw delete: it only drops
  the setting, because SQL cannot take an approval back from its object, and a
  flag flipped there would leave an approval no revoke could find (and would miss
  a grant carrying several rooms). A grant such a delete left behind stays
  active and is judged, then withdrawn, by the next `withdraw` against the level
  then in force for its origin (`deleting_a_room_takes_its_setting_with_it_whatever_deleted_it`,
  `deleting_one_of_several_rooms_keeps_the_grant_the_others_still_justify`). The
  marks are the server's even though the models (de)serialize them (exports
  carry and seal them): an unsealed import and a bundle clear them, create and
  update overwrite them from the stored object
  (`only_a_sealed_backup_keeps_the_trust_marks`). A
  human activating a waiting Quick Exec line replaces the level's mark with their
  own approval, which a later revoke leaves alone
  (`a_human_activation_clears_a_stale_trust_mark`). Trust is not part of a
  workflow or room export.
- **Policy table**: one function (`core::trust::minimum_level`) says which
  level grants which approval. The gates and the risk panel (`GET /api/trust`
  returns `effects`) read the same table, so the panel lists what the gates do.
  [src: file: backend/src/core/trust.rs:13-24#minimum_level]

| Approval (`TrustApproval`) | From | Gate |
|---|---|---|
| `quick_exec_line` | Trusted | `api::quick_execs::approve_by_trust` (create, update, import by an agent) |
| `exec_step_line` | Trusted | `api::workflows::approve_exec_steps_by_trust` (agent create/update, bridge and native tools) |
| `room_scope_read`, `room_scope_plan` | Trusted | `db::discussion_scope::file_request_with_trust` (a request for a registered project; a request already pending for it is decided the same way, as asked, when the owner raised the level after the card was filed) |
| `validations_docker` | Trusted | `api::orchestration::ValidationReach::of_run` (`widened_by_trust`) |
| `validations_unconfined` | Full | same |
| `code_write_other_project` | Full | **no gate yet**: KT-1491 asks `core::trust::grants` |
| `host_access` | Full | **no gate yet** |

`enforced: false` on the last two is returned to the panel, which says so
instead of promising an effect that does not exist yet.

## What a level grants runs in a sandbox

A line a level approved runs as the user on the host. Without a sandbox it could
read the Kronn data directory (`kronn.db`, `encryption_key`, `recovery.key`,
`human-admin-secret`, `config.toml`, the TOTP seed, the backups). So everything a
level grants is confined (`core::trust_sandbox`) to the owner's model: **under
Full the agent does everything the owner could do in a terminal, except read or
change Kronn itself.**

| | Closed | Open |
|---|---|---|
| **Data** | the data directory and `KRONN_BACKUP_DIR` (read and write) | project databases, other repositories, the credentials of other CLIs (`~/.aws`, `gh`) |
| **Network** | nothing | the whole network, **including the Kronn API port**; Docker (host root) |

The sandbox hides **only the data directory**. It does not close the Kronn API
port and does not cut the network: the agents' MCP bridge and the owner's Quick
Exec scripts go through that port, and a level must not make anything that works
today stop working. The human-only routes are protected by the TOTP proof, not by
the network: a command that sends `PUT 127.0.0.1:<port>/api/trust/instance`
reaches the route and is refused for want of a code (see below).

- macOS: `sandbox-exec` with an allow-by-default profile and one deny, the data
  directory (`seatbelt_profile`). Linux: bubblewrap, the data directory replaced by
  an empty tmpfs, the host network kept with `--share-net` (`bubblewrap_args`).
  The command also gets its own PID namespace and a fresh `/proc`
  (`--unshare-pid --proc /proc`, with `--die-with-parent`): the host's `/proc`
  would otherwise let `/proc/<backend pid>/root/<data dir>/encryption_key` walk
  around the tmpfs, same uid. The worker sandbox's `hidden` directories (a
  validation a level opened to Docker) are mounted by the same builder
  (`bubblewrap_args_hiding`), after every bind and before the namespace flags and
  the `--` sentinel: nothing is spliced into a finished argument list, so an
  option never splits and a command named like a flag cannot move the mounts
  (`the_worker_sandbox_hides_the_data_dir_inside_the_options_before_the_command`).
  The tmpfs goes over each protected directory
  **resolved** (`bubblewrap_mount_points`): `/` is bind-mounted, so a tmpfs over
  a symlinked alias (a linked `$HOME`, a `KRONN_DATA_DIR` through a link) would
  leave the real directory readable. Landlock was not used: it cannot hide one
  directory and keep the rest.
  [src: file: backend/src/core/trust_sandbox.rs:86-90#seatbelt_profile]
- What is confined: an Exec step or a Quick Exec a level approved (the
  `approved_by_trust` mark, `workflows::exec_step::confined_for_level`), a
  validation a level opened to the host (`ValidationReachLaunch::TrustHost`),
  and a validation a level opened to Docker (`TrustDocker`: the lane sandbox
  stays, the data directory is closed to it, and an argument that names the data
  directory or a parent as a volume is refused, `argv_reaches_protected`; every
  source is compared as written **and** with its symlinks resolved, so a worktree
  link to the data directory is caught).
  [src: file: backend/src/api/orchestration.rs:5198-5216#launch_for_reach]
- A human's approval is not marked and runs as before. A mark survives only on the
  very lines a level approved: an edit of the line, or a request that forges the
  mark, drops it (`mark_line_writers`, `update_as_in`).

### Protect the Kronn vault from agents

A setting, `server.protect_vault_from_agents` in `config.toml`, **on by default**.
It has three states (`trust_sandbox::VaultProtection`):

| State | When | What a level-granted command does |
|---|---|---|
| **Active** | the setting is on and the host has `sandbox-exec` or `bwrap` | runs under the sandbox above |
| **Disabled** | the owner set `protect_vault_from_agents = false` (restart Kronn) | runs unconfined, as the owner chose |
| **Unavailable** | the setting is on but the host has no sandbox (Windows, no `bwrap`, a container that forbids user namespaces) | runs unconfined |

A missing sandbox **refuses nothing**: Trusted and Full access can be chosen, and
every gate, launch and validation works as it does under Standard, minus the
vault protection. What changes is what the human is told. `GET /api/trust`
returns `sandbox` (`available`, `reason`, `protect_vault`, `protected`,
`missing`). The level control shows "Kronn vault protection is unavailable on this
machine (no sandbox-exec or bubblewrap)" (or "turned off in the config"), the risk
panel repeats it, and a raise while the vault is unprotected needs its own
acknowledgement: the PUT answers 400 unless `acknowledged_vault_unprotected` is
true, and the panel adds a checkbox for it. The raise still needs the TOTP proof.
[src: file: backend/src/core/trust_sandbox.rs:376-383#vault_protection]

## Raising a level needs a proof an agent cannot produce

Raising a level (to Trusted or Full, at any scope) and changing or disabling the
confirmation itself need a TOTP code (RFC 6238: SHA-1, 6 digits, 30 s, ±1 step)
from any authenticator app, or one of ten single-use recovery codes
(`core::totp`, `core::trust_proof`). It is enforced in the handler, not the
screen, and loopback does not lift it: a command an agent approved that sends
`PUT 127.0.0.1:3140/api/trust/instance` is refused for want of a code.
[src: file: backend/src/core/trust_proof.rs:511-561#check_inner]

- **The code is the proof, whatever the channel.** It is accepted from:
  - the UI (the risk panel's code field, the `proof` body field);
  - plain HTTP: the `X-Kronn-Confirm: <code>` header, or the `proof` body field
    (the body wins when both are present);
  - a bridge-token call, so the owner may give a code to an agent for that one
    action. A bridge token reaches a trust write only through
    `PUT /api/trust/instance`, `PUT|DELETE /api/trust/{rooms|workflows}/{id}` and
    `POST /api/trust/proof/disable` (`bridge_token::TRUST_CONFIRM_ROUTES`), and
    only when the request carries a code (`lib.rs`, `trust_confirm_gate`).
    There is **no new MCP tool and no `BRIDGE_ROUTES` entry**: the agent calls the
    HTTP route with the code. Without a valid code, a bridge token is refused. The
    routes that hand back the seed or the recovery codes (`enroll`, `confirm`,
    `recovery-codes`) and every read stay closed to it: the reply would reach a
    model.
  - On the bridge channel the code is required for every write, lowering and
    dropping an override included.
- **Guards, the same on every channel**: a code is **single use** (a code of a step
  already accepted is a replay, a recovery code is spent); it is **bound to the one
  route and object** of the call that presented it (`Binding`: channel, route
  pattern, object such as `room:<id>`; recorded with the record as `last_use`), so
  once spent on one call it is refused on every other; every use and every refusal
  is **journalled with the channel** (`proof` / `proof_refused` lines in
  `trust_audit_log`, migration 280: channel, route, object, never the code); **5
  failures lock the confirmation, for all channels together** (see "Lockout and
  brute force"; a refusal for a lockout is not journalled, so a flood cannot fill
  the journal, but the failure that starts a lockout writes an `ALERT` line).
- **Verify-and-consume is one critical section.** `core::trust_proof` holds a
  process-wide mutex across read, verify and write of the record, and replaces the
  file by an atomic rename from a private temporary file. Two simultaneous
  requests with the same code are serialised: the second sees the step (or the
  recovery code) spent and is refused. A code given for room R therefore cannot
  also raise the instance. Tests:
  `two_simultaneous_requests_with_one_code_cannot_both_pass`,
  `a_recovery_code_is_spent_once_even_when_presented_four_times_at_once`.
- The channel `ui` / `http` / `bridge` is decided by the server: `bridge` is a live
  bridge token seen by the gate, or the agent marker of the MCP bridge; `ui` is a request carrying a browser's `Origin`
  (a label for the journal, not a privilege); anything else is `http`.
- **Enrolment** (Settings › Trust levels): the admin secret, a QR code and the manual
  key, then a first code. The record is `trust-proof.json` in the data directory:
  the seed is encrypted with the installation key, the recovery codes are keyed
  digests, the file is mode 0600. The sandbox hides that directory from what a
  level approves.
- **The first enrolment is a trust root, so it needs the admin secret.** While
  nothing is enrolled a code proves nothing, and whoever enrols owns the proof. A
  token-less loopback `curl` from an agent's shell would otherwise start an
  enrolment, confirm it and read the recovery codes (or hijack the owner's pending
  one). So `POST /api/trust/proof/enroll` and `/confirm` require the human-admin
  secret (`admin_secret` in the body: the content of the `human-admin-secret` file,
  `core::operator_secret`), which sits in the data directory the sandbox hides. The
  owner's effort is one paste. A replacement of a confirmed enrolment needs the
  admin secret **and** a proof of the current one (Settings shows the admin secret
  field only for the first enrolment, for cancelling a pending one and once the
  owner starts a replacement). A pending enrolment (a seed
  issued, no first code yet) is **not replaced** for 15 minutes (`409`). The owner
  does not have to wait: `POST /api/trust/proof/disable` with the admin secret
  cancels a pending enrolment at once (a button in Settings › Trust levels); an agent
  without the secret, or a bridge token, cannot. The admin secret is compared in
  constant time (`admin_secret_matches`). Each accepted
  step is journalled (`enrolment step authorised by the admin secret`); the seed
  and the recovery codes are never sent to a bridge token. Tests:
  `the_first_enrolment_needs_the_admin_secret_and_an_agent_without_it_gets_nothing`,
  `a_pending_enrolment_is_not_replaced_until_it_expires`,
  `the_owner_cancels_a_pending_enrolment_with_the_admin_secret`.
  [src: file: backend/src/core/trust_proof.rs:411-444#begin_enrolment]

### Lockout and brute force

The failure state is **persisted** in the data directory (`trust-proof-lock.json`,
mode 0600, hidden from level-granted commands), so a restart does not clear it.

- **Backoff**: 5 wrong codes in a row lock the confirmation for 60 s, the next
  lockout for 2 min, then 4 min, 8 min, … capped at 1 h. The step resets on a
  success.
- **Daily cap**: 30 wrong codes in a 24 h window lock it until the window ends.
  An agent that retries the moment each lockout ends tries at most 30 codes a day
  (the 7 200 a day of the earlier 60 s reset are gone).
- **The owner is never locked out for good**: a **recovery code is accepted while
  locked**, is spent, lifts the lockout, and never counts as a failure. A TOTP
  code is refused while locked without being verified, and without counting. A
  **wrong code of the recovery shape** (12 characters) is a guess at a recovery
  code: it is refused, **counts toward the daily cap** (not the streak, so no extra
  backoff step), and is journalled (`wrong recovery code while locked`) for the
  first 30 of a window, so a flood cannot fill the journal. A valid recovery code
  still unlocks, whatever the cap. The UI keeps the code field and the actions
  enabled during a lockout (the risk panel and Settings › Trust levels), so a
  recovery code can be typed; it re-reads the lock after a refused code and when
  the lockout ends (`useLockExpiry`).
- **Alert**: the wrong code that starts a lockout writes a `proof_refused` line
  containing `ALERT: repeated failures, the confirmation is locked for N s` to the
  journal. The other refusals of a lockout are not journalled.

Tests: `each_lockout_lasts_twice_as_long_up_to_an_hour`,
`a_daily_cap_bounds_the_guesses_whatever_the_backoff`,
`the_lockout_is_persisted_in_the_data_dir`,
`a_recovery_code_unlocks_and_never_counts_as_a_failure`,
`a_wrong_recovery_code_during_a_lockout_counts_toward_the_daily_cap`,
`a_wrong_recovery_code_during_a_lockout_is_journalled`,
`a_lockout_writes_an_alert_to_the_journal`.
- **No enrolment, no raise**: the PUT answers 403 and the panel links to the
  enrolment. **Lowering never needs it** (off the bridge channel): setting a lower
  level, an explicit Standard (the badge's revoke), and dropping an override that
  does not raise the level. Dropping an override that would fall back to a higher
  instance level is refused with a 403: that is a raise, made with a PUT and a code.
- `db::trust::set_level` / `clear_level` are called by `api/trust.rs` alone (a test
  walks the source).
- **Emergency reset** (nothing inside Kronn can do it for an agent): with host
  access outside Kronn, stop Kronn and delete `trust-proof.json` and
  `trust-proof-lock.json` from the data directory. Raising a level then needs a
  new enrolment (with the admin secret). Levels already set stay.
- **Agent marker (KT-1490)**: a request the MCP bridge marked as an agent's, with no
  bridge token (a host-launched CLI), is the `bridge` channel on the trust routes:
  a code is required on every write, even a lowering, and no secret is handed
  back. The marker is read before `Origin`, which a script can set. Test:
  `an_agent_marked_request_is_the_agent_channel_on_the_trust_routes`.
  [src: file: backend/src/api/trust.rs:84-95#AgentMarker]
- The passkey (WebAuthn) factor of the owner's decision B is **not delivered
  yet**: TOTP and recovery codes only.

## Lowering a level takes back what it granted

Every grant is journalled with the kind and id of the object it approved
(`quick_exec`, `workflow_step` with the step name, `room_scope_grant` with the
room) and stays `active` while that object holds the approval. `set_level`,
`clear_level` and `delete_and_withdraw` call `withdraw`: each active grant is judged by
the level now in force for the room or workflow it came from (`effective` on the
origin the journal stores), and when that level no longer grants it the approval
is removed from the object (a Quick Exec's, a step's, a scope grant is revoked) and
a `withdrawn` line names it in the journal, shown in Settings › Trust levels.
[src: file: backend/src/db/trust.rs:397-492#withdraw]

- A grant is judged by its **origin**, not by the scope that first gave it. The
  room badge writes an explicit Standard: a grant the room got from the instance
  is withdrawn with it, an unrelated room keeps its own, and a room with its own
  level keeps its grants when the instance is lowered
  (`lowering_a_room_withdraws_the_grants_it_inherited_from_the_instance`,
  `a_room_with_its_own_level_keeps_its_grants_when_the_instance_goes_down`,
  `revoking_a_room_withdraws_what_it_inherited_from_the_instance`).
- The journal keeps every room of the launch behind a grant (`discussion_ids`, a
  JSON array, migration `281_trust_grant_rooms`). A launch that owns several rooms
  is judged by the lowest of their levels, as at grant time, so lowering any one
  of them withdraws the grant (`lowering_the_non_first_room_of_a_multi_room_grant_withdraws_it`,
  `lowering_the_first_room_of_a_multi_room_grant_withdraws_it`). A row written
  before that column falls back to its single `discussion_id`. The workflow of a
  run is not stored twice: the journal resolves it from the run when it writes the
  grant, and `workflow_id` is what `withdraw` replays.
- An object a human has since re-approved (no `approved_by_trust` mark) is left
  alone. Validations' reach has no object: it ends with the level, per run.
- The journal keeps 10 000 rows and 180 days (`AUDIT_MAX_ROWS`, `AUDIT_MAX_DAYS`).
  A grant that can still be withdrawn is never pruned.
- A development database that ran this branch before its migrations were renumbered
  (`277_trust_levels`, `278_trust_withdraw`, `279_trust_proof_events`) keeps its
  receipts: `RENAMED_MIGRATIONS` maps them to 278, 279 and 280, so nothing replays
  (`trust_migrations_renumbered_after_a_dev_run_are_not_replayed`). `281` and `282`
  were added after the renumbering and never carried another number: they need no
  entry.
- Lines approved by the unreleased build that preceded migration 279 carry no mark
  and no object: they stay approved, unconfined. That build never shipped.

### Granting, saving and revoking are one unit

A gate that approves an object does not journal first and write later.
`db::trust::grant_and_write` consults the level, journals the grant and runs the
object's write in **one transaction on the one database connection** (every
`with_conn` closure holds its mutex), and `set_level` / `clear_level` take the
same connection. A lowering therefore comes either first (the level no longer
grants: nothing is approved, the save is refused with "the trust level changed
while the Quick Exec was being saved") or after (the grant is active, so it is
found and withdrawn). Quick Exec create, update and import
(`quick_execs::store_quick_exec`) read the level once to validate the line as
approved, then grant and write together. The workflow path
(`approve_exec_steps_by_trust`) grants and writes on the workflow **as stored
now**, so an edit made since the response was built is not overwritten by an older
copy. Room scope grants already run inside one closure.

**Retaining an approval is part of the same unit.** An update of an unchanged
line (Quick Exec) or step (workflow) copies the level's approval and `by trust`
mark from the row it read before the save. That copy can be stale: the owner may
have lowered the level, withdrawing the approval, in between. So the write
re-reads the row on the write connection first
(`quick_execs::keep_only_a_live_trust_approval`,
`workflows::keep_only_live_trust_marks`): a mark the stored row no longer carries
(or whose line changed) is dropped, never written back, and a human's own approval
of that very line, given since, is kept. The response shows what was stored. Tests:
`an_unchanged_line_save_interrupted_by_a_revoke_does_not_restore_the_approval`,
`an_unchanged_line_save_keeps_a_live_level_approval_and_its_grant`,
`a_stale_level_mark_does_not_erase_a_humans_approval_given_since`,
`a_workflow_save_interrupted_by_a_revoke_does_not_restore_the_approval`.

Other tests:
`a_level_lowered_between_the_check_and_the_save_stores_no_approval`,
`a_level_lowered_right_after_the_save_withdraws_the_approval_it_gave`,
`racing_saves_and_a_lowering_never_leave_an_approval_behind`,
`a_level_approval_of_exec_steps_does_not_overwrite_a_concurrent_edit`,
`a_grant_is_refused_and_nothing_written_when_the_level_no_longer_grants_it`,
`a_failed_write_rolls_the_grant_back_with_it`.

## Gates fail closed

A gate calls `db::trust::grant`, which resolves the level, checks the policy and
writes the journal line in one call. (A host with no sandbox does not stop a
level: see "Protect the Kronn vault from agents".) An unreadable setting is an
error and the gate asks as before. A hard refusal (a value in program text, an
option or a program name) is never an approval, so no level lifts it.

## Never covered

`NeverCovered` lists what no level touches. None of these gates reads a trust
level, and a test (`the_gates_that_hold_the_exclusions_never_read_a_trust_level`)
fails if one starts to:

- Kronn's own secrets (the bridge strips `token_override`; secret stores) and the
  data directory itself, which the sandbox closes to every command a level
  approves (`every_secret_kronn_keeps_is_inside_what_the_sandbox_hides`). Secrets
  are never sent to a model. The credentials of other tools (`~/.aws`, `gh`) are
  **not** covered: the panel says so;
- changing a trust setting without the owner's code: no trust route is on the
  bridge list (`trust_levels_are_human_only`), a bridge token reaches a write only
  with a valid code, a change is a destructive request (`is_destructive`) and a
  raise needs the proof on every channel;
- delete confirmations (`DELETE` stays destructive);
- a P2P peer: federation and discussion portability never read trust.

## UI

`TrustLevelControl` (Settings › Trust levels, the room's settings panel, the
workflow page), `TrustRiskPanel` (any raise above Standard: the effects, what
stays reachable, what the sandbox closes, the acknowledgement and the code, the
missing vault protection with its own acknowledgement, and the enrolment link when
no authenticator is set up), `TrustProofSection`
(enrolment, recovery codes, replace, disable), `TrustBadge` (room header,
workflow, settings; revoke is an explicit Standard at that scope). Every card
whose key starts with `kronn-elevation-` (Kronn's or an agent's) renders a line
naming the global setting with a link to `/config#settings-trust`.

## Limits

- **Agent processes are not confined by this branch (KT-1495).** The agent CLIs
  Kronn launches (Claude's Bash, Codex) are not under the sandbox yet: they can
  read `encryption_key` and `trust-proof.json`, and so mint codes. The proof
  protects against **what a level approves** (Quick Exec and Exec lines,
  validations); the agent processes themselves are confined from 0.15.2 by
  KT-1495, **which ships in the same release as this branch**. The risk panel says
  so (`trust.panel.limit`).
- The Docker daemon is root and cannot be confined: Kronn closes its own data to
  the command and refuses a volume that names it. Each source is expanded the way
  a shell would (`~`, `$HOME`, `${VAR}`), a relative one (`./link-to-data`) is
  joined to the validation's own directory, and each is compared as written, with
  `.`/`..` folded and with its symlinks resolved. The short flag glued to its
  value is read too (`-v/Users/x/.kronn:/c`, `-v~/.kronn:/c`), and so is a group of
  short flags (`-itv/Users/x/.kronn:/c`, `-dv~/.kronn:/c`: every suffix after the
  leading letters is a source), as are
  `--volume=…` and `--mount type=bind,source=…` (`unglue_short_flag`). That cannot cover a compose file
  or an image that mounts the directory, **nor a script a level approved that calls
  `docker` itself** (the guard reads the argv of the validation command, not what
  a script it runs does). Tests:
  `a_docker_volume_written_with_a_tilde_a_variable_or_a_relative_link_is_caught`,
  `a_docker_volume_that_reaches_the_data_dir_through_a_symlink_is_caught`,
  `a_docker_argument_that_names_the_data_dir_or_a_parent_is_caught` (glued forms),
  `a_docker_volume_glued_to_a_group_of_short_flags_is_caught`.
- On macOS the encryption key also has a keychain copy; the sandbox does not
  block the keychain (it would break `gh`), the OS asks for approval instead.
- With the vault protection unavailable or off, a level-granted command can read
  the data directory. The human-only routes still need the TOTP code, but the
  encryption key and `trust-proof.json` are then readable by that command: the
  panel says so and the raise asks to acknowledge it.
- A code given to an agent is good for one action; the agent (or anyone watching
  the CLI) sees it, which is why it is single use and the 30 s window is short.
- The Seatbelt profile is compiled with the OS in tests but applied for real only
  where a sandbox can be nested (a local macOS shell, Linux CI). To run the
  real-sandbox tests on a host:
  `cd backend && cargo nextest run --lib -E 'test(/trust_sandbox|exec_step_trust/)'`
