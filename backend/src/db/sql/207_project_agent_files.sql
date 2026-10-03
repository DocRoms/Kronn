-- KT-971 — where Kronn writes a project's agent files (.mcp.json, .kiro/,
-- .gemini/, .vibe/, .ai/, its .kronn/ ownership records and .gitignore lines).
--
-- `repo`: in the repository, as before. `outside`: in Kronn's data directory;
-- the repository is left untouched and only Claude Code, which takes its MCP
-- config by path, keeps its MCP servers.
ALTER TABLE projects ADD COLUMN agent_files TEXT NOT NULL DEFAULT 'repo';
