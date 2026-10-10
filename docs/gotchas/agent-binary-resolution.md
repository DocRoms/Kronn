# Agent binary resolution and npx fallback

The Darwin host-binary exclusion is conditional on container execution. A
native macOS backend must keep a CLI even when its directory is listed in
`KRONN_HOST_BIN`. In a container, resolution continues through PATH after a
skipped host binary so a later Linux installation can still win.
[src: file: backend/src/agents/mod.rs:178]
[src: file: backend/src/agents/mod.rs:1110]

Agent detection exposes the installed `path`, or a `fallback_command` containing
the resolved npx executable and arguments. The version comes from that command's
successful `--version` output; failed or empty output leaves it unknown. Settings
shows the command and explicitly warns about npx fallback. Refreshing detection
also clears cached fallback versions.
[src: file: backend/src/models/setup.rs:811]
[src: file: backend/src/agents/mod.rs:488]
[src: file: backend/src/agents/mod.rs:900]
[src: file: frontend/src/components/settings/AgentsSection.tsx:885]

A direct CLI spawn failure can also trigger npx. The launch records the fallback
command and measured version in runtime provenance and emits a warning log;
it never inserts a notice into the agent's output. The version probe reuses the
five-minute cache keyed by command and working directory. Workflow attempts retain those
facts separately from the output, including through serialization and templates.
Settings detection is a cached backend-level probe, not a record of a particular
project's run. A missing version remains unknown.
[src: file: backend/src/agents/runner.rs:4401]
[src: file: backend/src/agents/provenance.rs:8]
[src: file: backend/src/workflows/steps.rs:1208]
[src: file: backend/src/workflows/template.rs:114]
[src: file: backend/src/agents/mod.rs:880]
[src: file: backend/src/agents/mod.rs:900]
[src: file: backend/src/agents/mod.rs:56]

When the backend's current directory cannot be read, only the PATH scan is
skipped; absolute host-bin directories are still scanned with the same native
versus container rules.
[src: file: backend/src/agents/mod.rs:1110]

For a version-mismatch investigation, retain the **same backend process's**
`find_binary('codex')` debug lines: PATH, host directories, container/host flags,
and the selected or skipped path. Record its `KRONN_HOST_BIN` value and compare
the reported installed version with the command that was actually selected.
The debug entry reads the backend environment; a terminal's PATH alone is not
that evidence.
[src: file: backend/src/agents/mod.rs:1111]

The native/container regression and fake npx tests establish resolver behavior;
they do not establish the cause of the colleague's reported KT-665 version
mismatch. That incident still needs the reproducing backend's logs and
environment before a causal conclusion can be recorded.
[src: file: backend/src/agents/mod.rs:1837]
[src: file: backend/src/agents/mod.rs:1930]
[src: file: backend/src/agents/mod.rs:1976]
