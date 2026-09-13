# Keep application helpers out of Bats' assertion namespace

`lib/ui.sh` intentionally defines `fail()` as an output helper: it prints an
error and returns success. Bats assertions use a different function named
`fail()` whose nonzero status is the assertion failure. Loading the UI library
after bats-support silently replaced that boundary, so assertions could print an
error while the test still passed. The principal reproduced this with an
intentionally broken low-disk supervisor before accepting KT-638.
[src: file: lib/ui.sh:66]
[src: file: tests/bats/test_helper.bash:17-41]

The shared library loader now restores Bats' failure helper after sourcing the
application library. Tests of the colliding application function run it in a
separate shell, preserving its production return value and formatting. Three
harness regressions exercise incorrect success, failure, and output assertions;
their final checks use shell status directly, so the same collision cannot hide
those failures. All three failed before the helper correction.
[src: file: tests/bats/ui.bats:10-26]

Do not use Bats' `output` variable to retain a fixture path across `run` calls:
each call replaces it with captured stdout. Do not convert arbitrary command
failure into an "unavailable dependency" skip; check the dependency explicitly,
then require the command to succeed. The secret-substitution regressions retain
literal HOME/PATH placeholders without changing those process variables.
[src: file: tests/bats/bugfixes.bats:182-256]

The effective suite also exposed stale fixed-version input, an empty explicit
OS argument falling back to the host, an omitted MCP-template status, and an
array join using only the first character of a multi-character IFS separator.
The banner now gets a controlled VERSION fixture; the CLI corrections preserve
omitted-argument host detection, configured MCP precedence, and the full middle
dot separator. These are bounded corrections, not relaxed assertions.
[src: file: tests/bats/ui.bats:107-115]
[src: file: lib/ui.sh:326-328]
[src: file: lib/repos.sh:53-71]
