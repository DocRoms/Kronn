# Agent binary resolution and npx fallback

The Darwin host-binary exclusion is conditional on container execution. A
native macOS backend must keep a CLI even when its directory is listed in
`KRONN_HOST_BIN`. In a container, resolution continues through PATH after a
skipped host binary so a later Linux installation can still win.
[src: file: backend/src/agents/mod.rs:172-188#should_skip_darwin_host_binary]
[src: file: backend/src/agents/mod.rs:1305-1387#find_binary_in_paths]

Agent detection exposes the installed `path`, or a `fallback_command` containing
the resolved npx executable and arguments. The version comes from that command's
successful `--version` output; failed or empty output leaves it unknown. Settings
shows the command and explicitly warns about npx fallback. Refreshing detection
also clears cached fallback versions.
[src: file: backend/src/models/setup.rs:888-891#fallback_command]
[src: file: backend/src/agents/mod.rs:626-653#fallback_command]
[src: file: backend/src/agents/mod.rs:1055-1125#probe_npx_invocation]
[src: file: backend/src/agents/mod.rs:1420-1447#version_from_output]
[src: file: backend/src/agents/mod.rs:367-374#detect_all_cached]
[src: file: backend/src/agents/mod.rs:501-514#invalidate_detect_cache]
[src: file: frontend/src/components/settings/AgentsSection.tsx:899-912#fallback_command]

A direct CLI spawn failure can also trigger npx. The launch records the fallback
command and measured version in runtime provenance and emits a warning log;
it never inserts a notice into the agent's output. The version probe reuses the
five-minute cache keyed by command and working directory. Workflow attempts retain those
facts separately from the output, including through serialization and templates.
Settings detection is a cached backend-level probe, not a record of a particular
project's run. A missing version remains unknown.
[src: file: backend/src/agents/runner.rs:4583-4616#record_npx_fallback]
[src: file: backend/src/agents/provenance.rs:7-19#AgentRuntimeProvenance]
[src: file: backend/src/agents/provenance.rs:23-34#record_npx_fallback]
[src: file: backend/src/models/workflows.rs:1952-1986#WorkflowAgentAttempt]
[src: file: backend/src/workflows/steps.rs:1425-1446#npx_fallback_command]
[src: file: backend/src/workflows/template.rs:153-194#set_step_provenance]
[src: file: backend/src/agents/mod.rs:54-59#RUNTIME_CACHE]
[src: file: backend/src/agents/mod.rs:1084-1094#cache_key]
[src: file: backend/src/agents/mod.rs:1014-1036#probe_runtime]
[src: file: backend/src/agents/mod.rs:367-374#detect_all_cached]

When the backend's current directory cannot be read, only the PATH scan is
skipped; absolute host-bin directories are still scanned with the same native
versus container rules.
[src: file: backend/src/agents/mod.rs:1174-1181#current_dir]
[src: file: backend/src/agents/mod.rs:1313-1318#which_in_all]
[src: file: backend/src/agents/mod.rs:1347-1384#host_dirs]

For a version-mismatch investigation, retain the **same backend process's**
`find_binary('codex')` debug lines: PATH, host directories, container/host flags,
and the selected or skipped path. Record its `KRONN_HOST_BIN` value and compare
the reported installed version with the command that was actually selected.
The debug entry reads the backend environment; a terminal's PATH alone is not
that evidence.
[src: file: backend/src/agents/mod.rs:1154-1172#find_binary_on_machine]
[src: file: backend/src/agents/mod.rs:1327-1338#should_skip_darwin_host_binary]

The native/container regression and fake npx tests establish resolver behavior;
they do not establish the cause of the colleague's reported KT-665 version
mismatch. That incident still needs the reproducing backend's logs and
environment before a causal conclusion can be recorded.
[src: file: backend/src/agents/mod.rs:2159-2256]
[src: file: backend/src/agents/mod.rs:2258-2416]
