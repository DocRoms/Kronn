# GitHub runners export `XDG_CONFIG_HOME`

The `ubuntu-latest` runners set `XDG_CONFIG_HOME` in every job's environment;
a macOS shell and a plain `docker run ubuntu` usually do not. `kronn service
install` writes the shell's `XDG_CONFIG_HOME` into the unit
(`_service_env_pairs`), so a bats test that installs a fake service and then
asserts the unit has no `XDG_CONFIG_HOME` passes locally and fails on CI.
[src: file: lib/service.sh:211-216]

On CI the failing test did not show as `not ok`: bats 1.13 printed
"Executed 558 instead of expected 559 tests" and the test's number was simply
missing (same symptom as [stat-gnu-bsd-portability](stat-gnu-bsd-portability.md)).
Re-running the job failed identically; reproducing needs the variable:
`docker run -e XDG_CONFIG_HOME=/home/t/.config ubuntu:24.04 …`.

Fix: a test that reads the service environment back unsets the variable in
`setup`, like it isolates `HOME` and `KRONN_DATA_DIR`.
[src: file: tests/bats/service_update.bats:17]
