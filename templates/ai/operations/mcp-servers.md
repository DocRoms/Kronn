# MCP Servers — Agent tools

> **TEMPLATE FILE.** If servers below contain `{{...}}`, they are not configured yet.

## Available servers

| Server | Package | Purpose | Credentials |
|--------|---------|---------|-------------|
| {{SERVER_1}} | {{PACKAGE}} | {{PURPOSE}} | {{CREDENTIALS}} |
| {{SERVER_2}} | {{PACKAGE}} | {{PURPOSE}} | {{CREDENTIALS}} |

## Per-MCP context files

Each MCP has a context file at `ai/operations/mcp-servers/<slug>.md` with project-specific rules.
If a context file exists for an MCP, agents should read it before calling that MCP's tools.

## Files

| File | Committed | Purpose |
|------|-----------|---------|
| `.mcp.json` | No (gitignored) | Agent MCP config (managed by Kronn) |
