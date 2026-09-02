# `agent_speaks_mcp` says how tools are presented, not whether an agent has any

`agent_speaks_mcp` lists only Claude Code, Codex, Gemini CLI, Kiro and Copilot.
Reading it as "these are the agents with tools" is wrong, and leads to
conclusions that do not hold.
[src: file: backend/src/api/disc_prompts.rs:391]

The flag decides whether the prompt DESCRIBES the MCP surface in prose. HTTP
agents get their tools DECLARED on the request's `tools` field instead —
deliberately, because describing them in prose taught the model to hallucinate
calls it did not have. [src: file: backend/src/agents/runner.rs:2767]

So an HTTP agent that the flag excludes still has a catalogue, `disc_read`
included, and can read a discussion back on its own.
[src: file: backend/src/api/agent_tools.rs:598]

Vibe likewise has MCP: Kronn synchronises its servers into `.vibe/config.toml`.
[src: file: backend/src/core/mcp_scanner.rs:1350]

## Why it matters

The distinction decides whether an agent whose history was trimmed can recover
what it is missing, or has to ask the human. It can recover — every runtime can
— which is why the truncation notice sends it to `disc_read` first and to the
user only as a fallback, and why 0.13.0 could drop automatic summaries without
leaving any agent stranded. Capability and mode of presentation are separate
questions; check the catalogue, not this flag.
[src: user: 2026-09-02: correction on the 0.13.0 room]
