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

`config.toml::server.encryption_secret` is the AES-GCM key for the
MCP env-var encryption. Back it up **separately** from the DB — if
you lose the secret, the encrypted env values in the DB are
unrecoverable. A single git-crypt'd file or a password manager entry
is enough.

## What's NOT covered

- `~/.config/kronn/config.toml` — re-derivable from the backend's
  defaults + the user's API keys (which they should also back up).
- `~/.kronn/user-context/*.md` — uploaded markdown, lives outside
  the DB. Snapshot the whole `~/.kronn/` if you want belt+suspenders.
- Disc workspace dirs (`/path/to/repo/.kronn-worktrees/<disc>/`) —
  ephemeral, recreated from git on demand.

## Future work

- Auto-rotation policy (keep N hourly + M daily + K weekly).
- Optional restic / borg / etc. integration for external storage.
