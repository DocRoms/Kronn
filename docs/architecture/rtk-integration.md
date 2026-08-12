# RTK integration

How Kronn detects, activates and reports RTK — product internals.

Moved out of `docs/AGENTS.md` (KT-191). The agent-facing rule (prefix commands
with `rtk`) lives in `CLAUDE.md` and is unaffected. Content verbatim.


RTK (Rust Token Killer, `github.com/rtk-docs/rtk`) is a Rust shell-output compressor that intercepts commands like `git`, `cargo`, `ls`, test runners and rewrites their output before it reaches the LLM. Measured ~89% compression on the author's local fleet. Kronn ships a first-class integration covering detection, activation, and savings readout.

**Scope — what Kronn does NOT do**: we do not wrap or intercept agent shell calls ourselves. The agent CLI (Claude Code, Codex, Gemini CLI) is the process that executes Bash tool calls, and it runs its *own* `Command::new("bash")`. RTK installs per-agent hooks that those CLIs invoke; Kronn detects + activates + observes, never proxies. This is the correct separation of concerns: RTK owns the format of each hook, Kronn owns the UX.

### What's wired

- **`backend/src/core/rtk_detect.rs`** — read-only scan of the host.
  - `rtk_binary_available()` → `which rtk`
  - `rtk_hook_configured_for(agent_type)` → scans the per-agent config file:
    - Claude Code → `~/.claude/settings.json`
    - Codex → `~/.codex/AGENTS.md` (NOT `config.toml` — RTK injects into the AGENTS.md preamble)
    - Gemini CLI → shell rc (bash/zsh/fish/profile) — `gemini_shell_rc_mentions_rtk()`
    - Kiro, Copilot CLI, Vibe, Ollama → `None` (not in RTK's supported list, or no shell to hook)
  - HOME resolution uses `$HOME` (NOT `KRONN_HOST_HOME`). In Docker `/home/kronn/.claude` etc. are bind-mounted rw from the host, so reading through the container HOME lands on the real host file. Overriding with `KRONN_HOST_HOME` (the host path, e.g. `/home/priol`) points at a path that doesn't exist inside the container and silently returns false everywhere.

- **`backend/src/models/mod.rs`** — `AgentDetection` gets `rtk_available: bool` and `rtk_hook_configured: bool` alongside `runtime_available`. Both have `#[serde(default)]` so older configs deserialize cleanly.

- **`backend/src/api/rtk.rs`** — two endpoints:
  - `POST /api/rtk/activate` — body `{ agents: AgentType[] }`. Filters to RTK-supported agents (Claude Code / Codex / Gemini CLI), spawns one `rtk init` per agent with the right flag matrix:
    - Claude Code: `rtk init -g --auto-patch --hook-only`
    - Codex: `rtk init -g --codex --auto-patch` (no `--hook-only` — incompatible with `--codex`, same for `--gemini`)
    - Gemini: `rtk init -g --gemini --auto-patch`
    `--auto-patch` is mandatory for non-interactive. Returns `{ success, stdout, stderr, per_agent: RtkAgentActivation[] }` with stdout/stderr concatenated by agent for toast display.
  - `GET /api/rtk/savings` — reads `rtk gain --all --format json`, navigates to `summary.{total_saved, avg_savings_pct, total_commands}` (RTK 0.37 shape, validated by a test). Returns `{ available, total_tokens_saved, ratio_percent, sample_count }`. `available: false` when anything fails so the UI hides cleanly.

- **Pre-flight** — `POST /api/rtk/activate` calls `std::fs::create_dir_all($HOME/.config/rtk)` before spawning, because in Docker the chain `~/.config/rtk/` may not exist and RTK errors out with "Permission denied" when mkdir crosses a uid boundary. The Dockerfile also pre-creates `/home/kronn/.config` and `chown`s it to the app user.

- **`backend/Dockerfile`** — RTK 0.37.1 pinned, curl-installed, `dpkg --print-architecture` pattern shared with `glab`, `bun`, `uv`. arm64 target (`aarch64-unknown-linux-musl`) already wired even though compose publishes x86_64 today.

- **`docker-compose.yml`** — bind mounts `~/.config/rtk` and `~/.local/share/rtk` rw. Without these the `rtk gain` call inside the container reads an empty SQLite while the user's real stats live on the host.

- **Frontend** (`frontend/src/components/settings/CompressionSection.tsx`) — single card at the top of AgentsSection. States by `configured/applicable` count (0/N amber, partial neutral, N/N green). Savings counter shows only when RTK reports `available: true` with `total > 0`. Details expand renders 3 stat cards. Install modal (when `!rtk_available`) shows the curl command + GitHub link for the user to pass to their tech colleague. Attribution always visible: "Propulsé par RTK (open source)".

- **Per-agent badge** (`frontend/src/components/settings/AgentsSection.tsx`) — rendered inline next to each agent version. 3 states for RTK-applicable agents, italic "Non pris en charge par RTK" for the rest.

- **Sobriety tooltip** — (?) button next to the "Mode économique" title reveals a paragraph: *"L'usage le plus sobre reste de ne pas utiliser d'IA. Si vous en utilisez, RTK compresse..."*. Acknowledges the eco-mode wording could oversell.

### Bug history worth remembering

Documented so future iterations don't rediscover them:

1. `rtk init -g` alone → only wires Claude Code. Need per-agent flags (`--codex`, `--gemini`).
2. No `--auto-patch` → prompt waits forever, exits 0, nothing happened ("RTK activated" lying toast).
3. `HOME=$KRONN_HOST_HOME` override → tries `mkdir /home/priol/.claude` inside the container, "failed to create directory". Leave HOME alone.
4. `--hook-only` with `--codex`/`--gemini` → "cannot be combined". The hook IS the flow for non-default agents.
5. Codex detection path `.codex/config.toml` → wrong, RTK writes to `.codex/AGENTS.md`.
6. `rtk gain` parser looking at top-level keys → wrong, RTK 0.37 nests under `summary.*`.
7. Docker container's `rtk` reads `/home/kronn/.config/rtk/` → without bind mount, disjoint from host SQLite, savings counter always zero.

Regression tests in `backend/src/core/rtk_detect.rs::tests` and `backend/src/api/rtk.rs::tests` (incl. embedded real-payload JSON). Don't remove them without a very good reason.

### Out of scope (intentionally)

- **RTK timeseries / sparkline** — `daily[]`/`weekly[]`/`monthly[]` arrays are available in `rtk gain` JSON but the UI only surfaces `summary.*`. Reason: RTK is 0.x (breaking changes unannounced), and the counter covers 80% of the perceived value. Revisit when RTK hits 1.0.
- **Scoping RTK savings by discussion / agent** — RTK's SQLite is global, Kronn doesn't instrument per-message. "Compression par discussion Kronn" requires a `ContextCompressor` trait that intercepts at the MCP output / workflow-step output level (the "désagentification" vision). See `docs/decisions.md` for the longer note.

---
