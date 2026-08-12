# Stack

Dependency versions per layer.

Moved out of `docs/AGENTS.md` (KT-191): a reference table with no rule in it —
checked, zero imperative lines — so no session needs it to start a task.

Not a duplicate of `architecture/overview.md`, which documents services, ports
and request flow but carries no version table. Keep it that way: versions here,
architecture there.

Content verbatim.


| Layer | Technology |
|-------|------------|
| Backend | Rust (axum 0.8, tokio 1.x, serde 1, anyhow 1, rusqlite 0.39, ts-rs 12, reqwest 0.13, calamine 0.34, pdf-extract 0.10) |
| Frontend | React 19 + stable TypeScript 7 native compiler; TypeScript 6 alias retained for API-dependent lint tooling (Vite 8 / rolldown, Lucide icons 1.x, Node >= 24 LTS) |
| Styling | CSS tokens + utility classes + component classes (`src/styles/`). Inline `style={{}}` only for dynamic values. No CSS framework |
| i18n | Custom lightweight system (fr/en/es), localStorage, no external lib |
| Type bridge | ts-rs (Rust → TypeScript) |
| Database | SQLite (`kronn.db`, WAL mode, foreign keys) |
| Streaming | SSE (Server-Sent Events) for agent responses and workflow run updates |
| Container | Docker Compose (backend + frontend + nginx gateway) |
| Agents | Claude Code CLI, OpenAI Codex CLI, Vibe (Mistral), Gemini CLI (Google), Kiro (Amazon), **Ollama (local, v0.4.0)** — HTTP API streaming `/api/chat` with system/user role separation, zero cost. Planned: OpenCode, DeepSeek |
| MCP sync | 7 formats: `.mcp.json` (Claude), `.kiro/settings/mcp.json` (Kiro), `.docs/mcp/mcp.json` (Kiro new), `.gemini/settings.json` (Gemini CLI), `.vibe/config.toml` (Vibe), `~/.codex/config.toml` (Codex), `~/.copilot/mcp-config.json` (Copilot CLI). Also syncs Claude Code's `.claude/settings.local.json` `enabledMcpjsonServers` whitelist |
| Skills sync | Native SKILL.md files written to `.claude/skills/`, `.agents/skills/` (Codex), `.gemini/skills/` for progressive agent discovery. Profiles synced as agent files (`.claude/agents/`, `.gemini/agents/`, `.codex/agents/`, `.copilot/agents/`). Vibe/Kiro: prompt injection fallback |
| API keys | Multi-key per provider (named keys, active selection), stored in `config.toml` as `[[tokens.keys]]` array. Agent auth files synced (e.g. `~/.codex/auth.json`). Override toggle per provider without deleting keys. |
| Token tracking | Per-message `tokens_used` + `auth_mode` (override/local). Codex: parsed from stderr. Claude Code: `--output-format stream-json --verbose --include-partial-messages` (tokens from `result` event and `message_delta`). Ollama: `prompt_eval_count` + `eval_count` from streaming JSON `done` chunk (cost: $0). Gemini/Vibe: TODO. |

---
