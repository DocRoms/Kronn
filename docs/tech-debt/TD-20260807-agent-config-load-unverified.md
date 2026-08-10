# TD-20260807-agent-config-load-unverified

- **ID**: TD-20260807-agent-config-load-unverified
- **Area**: Backend / Agents / MCP

- **Problem (fact)**: Kronn writes MCP configuration into each agent CLI's own config file and then assumes the agent loaded it. Nothing verifies that assumption, and every supported agent can reject a syntactically valid file for reasons Kronn never sees. The proven instance is Mistral Vibe:

  Kronn writes `<project>/.vibe/config.toml` [src: file: backend/src/core/mcp_scanner.rs:1594] and appends the `kronn-internal` bridge to it [src: file: backend/src/core/mcp_scanner.rs:1658]. Vibe discovers that file, then checks its workspace-trust store against the file's **parent directory** — the `.vibe/` folder, not the repository root. (Verified against the installed `mistral-vibe` 2.24.0 package: `ProjectConfigLayer._check_trust` returns `self._trust_store.is_trusted(self._config_file_path.parent)`, and `TrustedFoldersManager._closest_decision` walks ancestors. Vibe is a third-party dependency, so this is not citable to a repo path.) The **nearest** decision wins, so an untrusted `.vibe/` overrides a trusted repository root. When the verdict is untrusted, Vibe drops the entire config layer and loads **zero** MCP servers, while the file on disk plainly lists them.

  Observed on this machine 2026-08-08: `/Users/priol/Repositories/Kronn` was trusted, `/Users/priol/Repositories/Kronn/.vibe` was in the `untrusted` list, and Vibe connected to none of the six configured MCP servers. `front_euronews` was in the same state. Neither Kronn nor Vibe emitted any diagnostic.

  A single declined trust prompt causes this permanently: Vibe's `ProjectConfigLayer._on_trust_changed(new=False)` writes the `.vibe` directory into `untrusted`, and no amount of re-syncing by Kronn changes the outcome.

- **What is fixed (2026-08-08)**: **Detection only.** Kronn now reads Vibe's trust store and reports when it has written a config Vibe will ignore:
  - `core::vibe_trust` mirrors Vibe's nearest-ancestor trust resolution [src: file: backend/src/core/vibe_trust.rs:105].
  - The project sync warns at write time on `kronn::host_sync` [src: file: backend/src/core/mcp_scanner.rs:1709].
  - Agent detection surfaces `vibe.project_config_untrusted` to the UI [src: file: backend/src/agents/mod.rs:464].
  - `kronn doctor` lists every blocked Kronn-managed config with the fix [src: file: kronn:875].

- **Why we can't fix now (constraint)**:
  - **Kronn must not repair this itself.** Re-trusting a folder is a security decision that belongs to the user; silently rewriting `~/.vibe/trusted_folders.toml` would let Kronn re-grant a permission the user explicitly declined. Detection is the correct boundary — the remaining gap is deliberate.
  - **The general case is unsolved.** Vibe's trust store is one of several ways an agent can ignore a written config. Codex, Gemini CLI, Kiro and Copilot CLI each have their own precedence rules, schema expectations and (for some) trust prompts. Kronn has no feedback channel from any of them, so the same class of silent failure can recur per agent, and each needs its own probe.
  - **No general readback exists.** Verifying "the agent actually loaded these servers" requires either an agent-specific status command or spawning the agent, both of which cost real time in a sync path that currently runs on every config save.

- **Impact**: correctness | dev friction | user experience — an agent silently loses every MCP capability while the config file, the UI and the DB all show it as configured.

- **Where (pointers)**:
  - `backend/src/core/vibe_trust.rs` — trust-store reader and nearest-ancestor resolution
  - `backend/src/core/mcp_scanner.rs:1594` — `sync_vibe_project_config`, the write path
  - `backend/src/core/mcp_scanner.rs:1709` — `warn_if_vibe_trust_blocks`, the sync-time probe
  - `backend/src/agents/mod.rs:464` — `detect_runtime_warning`, the UI surface
  - `backend/src/core/mcp_scanner.rs:2162` — `sync_affected_projects`, where a general per-agent probe would hook in, alongside the existing `warn_missing_host_binaries` precedent

- **Suggested direction (non-binding)**:
  1. **Generalise the probe.** Introduce a small per-agent `verify_config_loadable(project)` alongside each host-sync writer, returning `Loadable | Blocked(reason) | Unknown`. Vibe's trust check is the first implementation; `Unknown` is an honest default for agents with no cheap probe.
  2. **Report, never repair.** Keep every probe read-only. Offer the user a one-click *"open the trust prompt"* action rather than editing another tool's security state.
  3. **Fold into `kronn doctor` as the single operator surface**, mirroring how `warn_missing_host_binaries` [src: file: backend/src/core/mcp_scanner.rs:2203] already routes there.
  4. Consider a post-write readback for the cheap cases (config re-parsed through the agent's own schema) before reaching for anything that spawns an agent.

- **Next step**: create a ticket for item 1 (per-agent `verify_config_loadable` contract). Items 2–4 are design constraints on that ticket, not separate work.
