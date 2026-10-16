# `stat -f` is not a format flag on GNU coreutils

`kronn service update` records the pre-migration backup's mtime before and
after a restart to decide whether the database was migrated. The helper tried
the BSD/macOS spelling `stat -f %m FILE` first and fell back to GNU's
`stat -c %Y FILE`. On GNU coreutils `-f` means "file system status", so the
first call prints a multi-line file-system block to stdout and exits 1; the
fallback then appends the real mtime. The arithmetic comparison received that
block, failed with a syntax error, and the migration flag stayed at 0.
[src: file: lib/service-update.sh:31-38]

On the Ubuntu CI runner this made four `service_update.bats` rollback tests
fail (every test where `kronn.db.backup` exists), while the macOS suite was
green. Bats 1.13 reported the run as "Executed 427 instead of expected 431"
with no `not ok` lines for them; run with `--filter`, the same tests report
the real assertion failure.
[src: url: https://github.com/DocRoms/Kronn/actions/runs/38716955660/job/116407733631]
[src: file: tests/bats/service_update.bats:277-372]

Try the GNU form first: `stat -c` on BSD/macOS fails without printing anything,
so `stat -c %Y FILE 2>/dev/null || stat -f %m FILE 2>/dev/null` is clean on both.
A portable alternative is `date -r FILE +%s`, which both implementations accept.
A GNU-shaped fake `stat` on `PATH` reproduces the Linux behaviour on macOS
without a container.
