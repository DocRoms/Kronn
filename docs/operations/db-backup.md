# DB backup runbook

## What's at risk

`~/.config/kronn/kronn.db` (or `$KRONN_DATA_DIR/kronn.db`) holds
**every** Kronn artefact:

- Discussions + messages
- Workflows + runs + step results
- MCP configs + encrypted env vars
- Skills, profiles, directives
- Quick prompts, quick APIs
- Token usage history

A disk corruption, accidental `rm`, or partial write during a power
loss = **lost forever** without a copy.

## Built-in backups

Kronn writes three kinds of copies. `[src: file: backend/src/core/backup.rs:1]`

| Copy | When | Where |
|------|------|-------|
| Scheduled | every `KRONN_BACKUP_INTERVAL_HOURS` (default 24, `0` disables), keeps `KRONN_BACKUP_KEEP` (default 7) | `KRONN_BACKUP_DIR`, else `<data_dir>/backups/kronn-auto-*.db` |
| Manual | Settings → Database → Backup (`POST /api/db/backup`) | `<data_dir>/backups/kronn-*.db` |
| Pre-migration | at boot, only when a migration is pending | `<data_dir>/kronn.db.backup` |

Scheduled and manual copies use `VACUUM INTO` on their own read-only
connection (`snapshot_database`). It reads one WAL snapshot, so the write
connection keeps committing during the copy, and it skips free pages, so a
copy is the size of the live data rather than of the file. While it runs the
WAL cannot be checkpointed past that snapshot and grows with the writes made
meanwhile. A paged `backup.step(N)` was rejected: from another connection it
restarts whenever the writer commits between two steps, so a large copy on a
busy instance may never finish, and from the write connection it freezes every
writer. The copy is staged in an owner-only `.<name>.partial` directory,
synced, then renamed, and it is refused with a log line naming the free and
needed space when the target disk lacks room (live data + 10 % + 512 MB).

The pre-migration copy is a file copy of the checkpointed database to
`kronn.db.backup.tmp`, synced, then renamed over `kronn.db.backup`. If the
disk lacks room or the copy fails, boot stops before any migration with a
message saying so. `KRONN_MIGRATION_BACKUP=0` upgrades without a backup; use
it only after taking a copy by hand. `[src: file: backend/src/db/migrations.rs:1]`

## Run retention and compaction

Step outputs are almost all of a workflow run's weight, and `workflow_runs`
was 9.2 GB of a 10 GB base. Two mechanisms keep it bounded.
`[src: file: backend/src/db/run_retention.rs:1]`

- **Payload retention, opt-in on every install.** Once enabled, every 6 h (first pass 10 min after
  boot, never during boot), Kronn replaces the step outputs of finished runs
  older than `server.run_payload_retention_days` (default `0`, which keeps
  them; Settings → Database suggests 30 days, one click) with `[Output removed by run retention]`. The run row,
  its steps, statuses, timings, tokens, branches and links stay, so no foreign
  key cascade fires. It works 25 runs per transaction and releases the write
  connection between chunks. `payload_compacted_at` marks a trimmed run.
  While it is off, the Automations page and Settings → Database show a banner
  with the database size; dismissing it is synced (`kronn:runRetentionBannerDismissed`)
  and it returns once the file passes 2 GB.
- **Row deletion, opt-in.** `server.run_retention_days > 0` deletes old runs
  with the same rules and chunking. `0` (default) never deletes history.
- **Runs without changes (KT-1100), on by default.** A top-level run that
  succeeded without any effect is stored with `outcome = 'no_op'` and deleted
  24 h after it finished (`DEFAULT_NO_OP_RETENTION_HOURS`), with the same
  rules and chunking; its unchanged Page publication does not hold it, the
  publication stays with its run link cleared. Its step outputs are cut to
  2 000 characters as soon as it is classified (every step succeeded, so no
  error is cut). See [Runs without changes](#runs-without-changes-kt-1100).
  `[src: file: backend/src/workflows/run_effect.rs:1]`
- **Per-workflow retention (KT-1100).** `Workflow.retention`
  (`{no_op_hours, success_days, failure_days}`, editor card "Run retention",
  `workflow_update`) overrides each global window for its class: no-op runs,
  other successful runs, and `Partial`/`Failed`/`Cancelled`/`StoppedByGuard`
  runs. An absent window inherits, `0` keeps forever, and an unreadable
  setting keeps every run of that workflow. One purge serves both: the pass
  deletes per class, first each overriding workflow with its own window, then
  every other workflow with the global one. The purge applies it whether the
  workflow is enabled or not, so only a human sets it on a stored workflow:
  an agent's update that changes it is refused, an agent's write keeps the
  stored value in the UPDATE itself (no stale read), a `kronn/` re-import keeps
  the stored value, and an agent may set it only on a new draft, which runs
  nothing until a human enables it.
  `[src: file: backend/src/core/run_retention.rs:1]`

Never touched, whatever their age: runs that are not `Success`, `Partial`,
`Failed`, `Cancelled` or `StoppedByGuard` (so not `Running`, `Pending`,
`WaitingApproval` or `Interrupted`); batch and compare runs; runs that still
own a worktree (`workspace_path` set; cleared by the runner as soon as the
checkout is removed, by the boot janitor otherwise); children of a parent that can still resume; and every run named by a
column of `REFERENCING_COLUMNS`: `workflow_runs.parent_run_id`,
`workflow_runs.triggered_by_run_id`, `discussions.workflow_run_id`,
`compare_run_scopes`, `batch_compare_judge_runs`,
`batch_compare_evaluations`, `live_page_dataset_points`,
`live_page_publications`, `discussion_questions.resume_run_id` and
`workflow_step_room_activities`. A test fails when a new foreign key to
`workflow_runs` is not listed. `shared_runs.id` is deliberately absent: every
run has that card, which links to the run instead of reading its outputs. Deleting a workflow is refused while one of its
runs is live, paused or interrupted with a worktree.

Trimming only grows SQLite's free list: the file keeps its size. Settings →
Database → *Compact the database* (`POST /api/db/compact`) runs `VACUUM` then a
truncating checkpoint and reports the size before and after. It holds the
write connection for the whole rewrite, so it is never automatic, it is
refused while a workflow run is in progress, and it needs free space for the
rebuilt copy both in the temporary directory and beside the database.

### Runs without changes (KT-1100)

A run is classified once, when a top-level linear run ends in `Success`; every
other run keeps `outcome = NULL`, which retention treats as a run with an
effect. The classification is fail-closed: `no_op` needs every step to be
neutral and the database to hold no trace of an effect.

| Step | Neutral when |
|------|--------------|
| Exec | exit 0 and a line that is exactly `KRONN_NOOP` on stdout or stderr |
| Agent | its `---STEP_OUTPUT---` envelope has `"no_change": true` (tokens spent do not matter then) |
| ApiCall | the method sent was `GET` or `HEAD` (read from the step summary) |
| CollectApiData | every source is a Quick API read with no error; a CLI source never is |
| PublishPageData | `content_changed` false, no changed dataset, no point added or removed |
| JsonData, TransformData | always |
| any other (Notify, Gate, batch, sub-workflow, trigger…) | never |

A run that owns a worktree or preserved a branch is never `no_op`, and neither
is a run with a dataset point, a publication that changed content, an
`api_call_logs` row that is not `GET`/`HEAD`, or any other row of
`REFERENCING_COLUMNS` (a discussion, a child run, a question…). A step's own
declaration is trusted for what Kronn cannot observe: an Exec or Agent that
prints the marker while writing files in the main checkout is the workflow
author's error. The run list hides no-op runs by default
(`GET /api/workflows/{id}/runs?hide_no_op=true`, same flag on `/runs/count`)
and folds a streak of them into one row when shown.

### Measured on a generated base

`backend/src/db/large_db_measure.rs` (ignored test, `KRONN_MEASURE_DIR`)
builds a 0.14.2-schema base of 9 950 runs, 4.1 GB of step outputs (3-10 steps,
2-60 KB outputs, one in ten 200-600 KB), runs the real code and prints the
times. Apple SSD, warm page cache, 2026-10-05:

| Step | 4 GB base |
|------|-----------|
| 0.14.2 token stats (`SUM(tokens_used)`) | 0.64 s |
| 0.14.3 token stats, workflow list | < 0.01 s |
| Pre-migration backup (APFS clones the file) | 0.02 s |
| Byte-for-byte copy (ext4 in Docker does this) | 1.2 s |
| Migrations 212 + 213 (two index builds) | 0.65 + 0.76 s |
| `VACUUM INTO` backup | 5.5 s, 4 117 MB |
| Payload retention, 7 523 runs in 301 chunks | 6.8 s, slowest chunk 0.13 s |
| `VACUUM INTO` backup after retention | 1.5 s, 833 MB |
| Compaction (`VACUUM` + checkpoint) | 4.4 s, 4 118 MB → 833 MB |

For a 10 GB base, scale by 2.5: the boot copy is instant on APFS and about
3 s warm (5-10 s cold on an SSD, minutes on a spinning disk) elsewhere; the
two index builds read the run table once each, about 3.5 s warm. The size
left after retention depends on how much history is younger than the window.

## Extra insurance: hourly snapshot via cron

SQLite's `.backup` command produces a consistent file even while the
backend has the DB open (it uses the SQLite online-backup API). One
line in cron is enough:

```bash
# crontab -e
# Hourly Kronn DB snapshot — keeps last 24 (truncated by find -mmin)
0 * * * * sqlite3 ~/.config/kronn/kronn.db ".backup '/var/backups/kronn/kronn-$(date +\%Y\%m\%d-\%H).db'" && find /var/backups/kronn -name 'kronn-*.db' -mmin +1500 -delete
```

Steps:

```bash
# 1. Pick a backup destination on a different disk if possible
sudo mkdir -p /var/backups/kronn
sudo chown $USER:$USER /var/backups/kronn

# 2. Test the command manually first
sqlite3 ~/.config/kronn/kronn.db ".backup '/var/backups/kronn/kronn-test.db'"
ls -la /var/backups/kronn/kronn-test.db   # should match size of source within a few KB

# 3. Wire the crontab
crontab -e
# (paste the line from above)
```

For **paranoid** setups, mirror to a second host nightly:

```bash
0 3 * * * rsync -a --delete /var/backups/kronn/ user@backup-host:/var/backups/kronn-$(hostname)/
```

## Restore

```bash
kronn stop
cp /var/backups/kronn/kronn-2026-05-09-15.db ~/.config/kronn/kronn.db
kronn start
```

The restored DB is consistent; Kronn's `db::migrations::run` is
idempotent so the schema converges even if the backup is from a
slightly older release.

## Don't trust filesystem snapshots alone

ZFS / Btrfs snapshots capture WAL pages mid-flight unless you
explicitly `sqlite3 .backup` first. A FS-level snapshot of an active
SQLite database may be unrecoverable without WAL replay. The
`sqlite3 .backup` API is the only way to get a guaranteed-consistent
copy without stopping the backend.

## Encrypted secrets

The AES-GCM key encrypts MCP env values, execution-variable snapshots,
provider keys and the API auth token (all in the DB since 0.14.3). It
lives in the OS keychain or the `encryption_key` sidecar, not in the DB:
a DB backup alone is unreadable without it. Set a recovery passphrase
(Settings → Recovery) and keep the recovery code off the machine — see
[key-management.md](key-management.md).

## What's NOT covered

- `~/.config/kronn/config.toml` — settings only; credentials are in
  the DB (encrypted) since 0.14.3.
- `~/.kronn/user-context/*.md` — uploaded markdown, lives outside
  the DB. Snapshot the whole `~/.kronn/` if you want belt+suspenders.
- Disc workspace dirs (`/path/to/repo/.kronn-worktrees/<disc>/`) —
  ephemeral, recreated from git on demand.

## Future work

- Auto-rotation policy (keep N hourly + M daily + K weekly).
- Optional restic / borg / etc. integration for external storage.
